use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_interpret::font::{FontData, FontQuery};
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{RenderCache, RenderSettings};
use lopdf::{Document, TocType, decode_text_string};
use rebook_publication::{
    Book, BookSource, Metadata, PublicationError, PublicationUrl, RenditionLayout, Resource,
    Section,
};
use sha2::{Digest, Sha256};

use crate::source::{DirectBookSource, SectionContent, SourceBook, SourceSection, SourceTocEntry};
use crate::{BookFormat, FormatError, conversion_error};

const COVER_PATH: &str = "Cover/thumbnail.png";
const PAGE_PATH_PREFIX: &str = "Pages/page-";
const PAGE_CACHE_CAPACITY: usize = 6;
const PAGE_MAX_DIMENSION: f32 = 2_048.0;
const COVER_MAX_DIMENSION: f32 = 384.0;
const MAX_RENDER_SCALE: f32 = 2.0;
const CJK_FALLBACK_FONT: &[u8] = include_bytes!("../../../assets/fonts/LXGWWenKai-Regular.ttf");

pub(crate) struct PdfPublication {
    descriptor: DirectBookSource,
    bytes: Arc<Vec<u8>>,
    page_count: usize,
    cache: Mutex<PdfResourceCache>,
}

#[derive(Default)]
struct PdfResourceCache {
    cover: Option<Arc<[u8]>>,
    pages: HashMap<usize, Arc<[u8]>>,
    page_lru: VecDeque<usize>,
}

pub(crate) fn open(bytes: &[u8], file_name: &str) -> Result<PdfPublication, FormatError> {
    let bytes = Arc::new(bytes.to_vec());
    let pdf = Pdf::new(Arc::clone(&bytes))
        .map_err(|error| conversion_error(BookFormat::Pdf, format_args!("{error:?}")))?;
    let page_count = pdf.pages().len();
    if page_count == 0 {
        return Err(conversion_error(
            BookFormat::Pdf,
            "PDF does not contain any pages",
        ));
    }

    let lopdf = Document::load_mem(bytes.as_ref()).ok();
    let title = lopdf
        .as_ref()
        .and_then(|document| document_info_text(document, b"Title"))
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| title_from_file_name(file_name));
    let authors = lopdf
        .as_ref()
        .and_then(|document| document_info_text(document, b"Author"))
        .filter(|author| !author.trim().is_empty())
        .into_iter()
        .collect();
    let table_of_contents = lopdf
        .as_ref()
        .and_then(|document| document.get_toc().ok())
        .map_or_else(Vec::new, |toc| build_outline(&toc.toc, page_count));
    let sections = (0..page_count)
        .map(|index| SourceSection {
            title: format!("Page {}", index + 1),
            content: SectionContent::Image {
                resource_path: page_path(index),
                alt: format!("PDF page {}", index + 1),
            },
            linear: true,
        })
        .collect();
    let descriptor = DirectBookSource::open(
        SourceBook {
            id: format!("{:x}", Sha256::digest(bytes.as_ref())),
            metadata: Metadata {
                title,
                authors,
                languages: Vec::new(),
                layout: RenditionLayout::PrePaginated,
            },
            sections,
            table_of_contents,
            resources: Vec::new(),
            cover_path: Some(COVER_PATH.into()),
        },
        BookFormat::Pdf,
    )?;

    Ok(PdfPublication {
        descriptor,
        bytes,
        page_count,
        cache: Mutex::new(PdfResourceCache::default()),
    })
}

impl BookSource for PdfPublication {
    fn book(&self) -> &Book {
        self.descriptor.book()
    }

    fn parse_section(&self, index: usize) -> Result<Section, PublicationError> {
        self.descriptor.parse_section(index)
    }

    fn resource(&self, href: &PublicationUrl) -> Result<Resource, PublicationError> {
        let path = href.resource_url();
        let bytes = if path.path() == COVER_PATH {
            self.cover_resource()?
        } else {
            let page_index = page_index_from_path(path.path())
                .filter(|index| *index < self.page_count)
                .ok_or_else(|| PublicationError::ResourceNotFound(href.to_string()))?;
            self.page_resource(page_index)?
        };
        Ok(Resource {
            href: path,
            media_type: "image/png".into(),
            bytes,
        })
    }
}

impl PdfPublication {
    fn cover_resource(&self) -> Result<Arc<[u8]>, PublicationError> {
        let mut cache = self.lock_cache()?;
        if let Some(cover) = &cache.cover {
            return Ok(Arc::clone(cover));
        }
        let cover: Arc<[u8]> = self.render_page(0, COVER_MAX_DIMENSION)?.into();
        cache.cover = Some(Arc::clone(&cover));
        Ok(cover)
    }

    fn page_resource(&self, page_index: usize) -> Result<Arc<[u8]>, PublicationError> {
        let mut cache = self.lock_cache()?;
        if let Some(page) = cache.pages.get(&page_index).cloned() {
            touch_page(&mut cache.page_lru, page_index);
            return Ok(page);
        }

        let page: Arc<[u8]> = self.render_page(page_index, PAGE_MAX_DIMENSION)?.into();
        cache.pages.insert(page_index, Arc::clone(&page));
        touch_page(&mut cache.page_lru, page_index);
        while cache.pages.len() > PAGE_CACHE_CAPACITY {
            let Some(evicted) = cache.page_lru.pop_front() else {
                break;
            };
            cache.pages.remove(&evicted);
        }
        Ok(page)
    }

    fn lock_cache(&self) -> Result<std::sync::MutexGuard<'_, PdfResourceCache>, PublicationError> {
        self.cache.lock().map_err(|_| {
            PublicationError::InvalidPublication("PDF raster cache is unavailable".into())
        })
    }

    fn render_page(
        &self,
        page_index: usize,
        max_dimension: f32,
    ) -> Result<Vec<u8>, PublicationError> {
        let pdf = Pdf::new(Arc::clone(&self.bytes)).map_err(pdf_render_error)?;
        let page = pdf.pages().get(page_index).ok_or_else(|| {
            PublicationError::ResourceNotFound(format!("PDF page {}", page_index + 1))
        })?;
        let (width, height) = page.render_dimensions();
        let scale = (max_dimension / width.max(height).max(1.0)).min(MAX_RENDER_SCALE);
        let cache = RenderCache::new();
        let interpreter_settings = interpreter_settings();
        let pixmap = hayro::render(
            page,
            &cache,
            &interpreter_settings,
            &RenderSettings {
                x_scale: scale,
                y_scale: scale,
                bg_color: WHITE,
                ..RenderSettings::default()
            },
        );
        pixmap.into_png().map_err(pdf_render_error)
    }
}

fn interpreter_settings() -> InterpreterSettings {
    let mut settings = InterpreterSettings::default();
    let default_resolver = Arc::clone(&settings.font_resolver);
    let cjk_fallback: FontData = Arc::new(CJK_FALLBACK_FONT);
    settings.font_resolver = Arc::new(move |query| match query {
        FontQuery::Fallback(fallback) if fallback.character_collection.is_some() => {
            Some((Arc::clone(&cjk_fallback), 0))
        }
        FontQuery::Fallback(_) | FontQuery::Standard(_) => default_resolver(query),
    });
    settings
}

fn pdf_render_error(error: impl std::fmt::Debug) -> PublicationError {
    PublicationError::InvalidPublication(format!("PDF rendering failed: {error:?}"))
}

fn page_path(index: usize) -> String {
    format!("{PAGE_PATH_PREFIX}{:05}.png", index + 1)
}

fn page_index_from_path(path: &str) -> Option<usize> {
    path.strip_prefix(PAGE_PATH_PREFIX)?
        .strip_suffix(".png")?
        .parse::<usize>()
        .ok()?
        .checked_sub(1)
}

fn touch_page(lru: &mut VecDeque<usize>, page_index: usize) {
    lru.retain(|cached| *cached != page_index);
    lru.push_back(page_index);
}

fn document_info_text(document: &Document, key: &[u8]) -> Option<String> {
    let info = document.trailer.get(b"Info").ok()?;
    let (_, info) = document.dereference(info).ok()?;
    let value = info.as_dict().ok()?.get(key).ok()?;
    let (_, value) = document.dereference(value).ok()?;
    decode_text_string(value).ok()
}

fn build_outline(items: &[TocType], page_count: usize) -> Vec<SourceTocEntry> {
    let mut cursor = 0;
    build_outline_level(items, page_count, &mut cursor, 0)
}

fn build_outline_level(
    items: &[TocType],
    page_count: usize,
    cursor: &mut usize,
    parent_level: usize,
) -> Vec<SourceTocEntry> {
    let mut entries = Vec::new();
    while let Some(item) = items.get(*cursor) {
        let level = item.level.max(1);
        if level <= parent_level {
            break;
        }
        *cursor += 1;
        let children = build_outline_level(items, page_count, cursor, level);
        let label = item.title.trim();
        if label.is_empty() || item.page == 0 || item.page > page_count {
            entries.extend(children);
            continue;
        }
        entries.push(SourceTocEntry {
            label: label.to_owned(),
            href: format!("Text/section-{}.xhtml", item.page),
            children,
        });
    }
    entries
}

fn title_from_file_name(file_name: &str) -> String {
    Path::new(file_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::trim)
        .filter(|stem| !stem.is_empty())
        .unwrap_or("Untitled PDF")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use rebook_publication::{Block, BookSource};

    use super::*;

    #[test]
    fn opens_and_renders_a_pdf_as_lazy_fixed_pages() {
        let bytes = minimal_pdf();
        let publication = open(&bytes, "fallback.pdf").unwrap();
        assert_eq!(publication.book().metadata.title, "Test PDF");
        assert_eq!(publication.book().metadata.authors, ["Rebook"]);
        assert_eq!(
            publication.book().metadata.layout,
            RenditionLayout::PrePaginated
        );
        assert_eq!(publication.book().sections.len(), 1);
        assert_eq!(publication.book().table_of_contents[0].label, "Page 1");

        let section = publication.parse_section(0).unwrap();
        let Some(Block::Image(image)) = section.blocks.first() else {
            panic!("expected a fixed PDF page image");
        };
        assert_eq!(image.href.path(), "Pages/page-00001.png");

        let page = publication.resource(&image.href).unwrap();
        assert!(page.bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        let cached = publication.resource(&image.href).unwrap();
        assert!(Arc::ptr_eq(&page.bytes, &cached.bytes));
        let cover = publication
            .resource(publication.book().cover.as_ref().unwrap())
            .unwrap();
        assert!(cover.bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        let page_dimensions = png_dimensions(&page.bytes);
        let cover_dimensions = png_dimensions(&cover.bytes);
        assert!(cover_dimensions.0 <= page_dimensions.0);
        assert!(cover_dimensions.1 <= page_dimensions.1);
        assert_ne!(cover_dimensions, (0, 0));
    }

    fn png_dimensions(bytes: &[u8]) -> (u32, u32) {
        (
            u32::from_be_bytes(bytes[16..20].try_into().unwrap()),
            u32::from_be_bytes(bytes[20..24].try_into().unwrap()),
        )
    }

    fn minimal_pdf() -> Vec<u8> {
        let content = b"BT /F1 12 Tf 20 80 Td (Hello PDF) Tj ET";
        let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 120 160] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_vec(),
            format!("<< /Length {} >>\nstream\n", content.len())
                .into_bytes()
                .into_iter()
                .chain(content.iter().copied())
                .chain(b"\nendstream".iter().copied())
                .collect(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
            b"<< /Title (Test PDF) /Author (Rebook) >>".to_vec(),
        ];
        let mut output = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
        let mut offsets = Vec::with_capacity(objects.len());
        for (index, object) in objects.iter().enumerate() {
            offsets.push(output.len());
            writeln!(&mut output, "{} 0 obj", index + 1).unwrap();
            output.write_all(object).unwrap();
            output.extend_from_slice(b"\nendobj\n");
        }
        let xref = output.len();
        write!(&mut output, "xref\n0 {}\n", objects.len() + 1).unwrap();
        output.extend_from_slice(b"0000000000 65535 f \n");
        for offset in offsets {
            writeln!(&mut output, "{offset:010} 00000 n ").unwrap();
        }
        write!(
            &mut output,
            "trailer\n<< /Size {} /Root 1 0 R /Info 6 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .unwrap();
        output
    }
}
