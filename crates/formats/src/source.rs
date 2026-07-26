use std::collections::HashMap;
use std::sync::Arc;

use rebook_html::parse_section;
use rebook_publication::{
    Block, Book, BookSource, ImageBlock, ImageStyle, Metadata, PublicationError, PublicationId,
    PublicationUrl, Resource, Section, SpineItem, SpineItemId, TocEntry,
};

use crate::{BookFormat, FormatError, conversion_error};

pub(crate) struct SourceBook {
    pub id: String,
    pub metadata: Metadata,
    pub sections: Vec<SourceSection>,
    pub table_of_contents: Vec<SourceTocEntry>,
    pub resources: Vec<SourceResource>,
    pub cover_path: Option<String>,
}

pub(crate) struct SourceSection {
    pub title: String,
    pub content: SectionContent,
    pub linear: bool,
}

pub(crate) enum SectionContent {
    Html(String),
    Image { resource_path: String, alt: String },
}

pub(crate) struct SourceResource {
    pub path: String,
    pub media_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone)]
pub(crate) struct SourceTocEntry {
    pub label: String,
    pub href: String,
    pub children: Vec<SourceTocEntry>,
}

pub(crate) struct DirectBookSource {
    book: Book,
    sections: Vec<SectionContent>,
    resources: HashMap<String, StoredResource>,
}

struct StoredResource {
    href: PublicationUrl,
    media_type: String,
    bytes: Arc<[u8]>,
}

impl DirectBookSource {
    pub(crate) fn open(book: SourceBook, format: BookFormat) -> Result<Self, FormatError> {
        let mut descriptors = Vec::with_capacity(book.sections.len());
        let mut sections = Vec::with_capacity(book.sections.len());
        let mut fallback_toc = Vec::new();
        for (index, section) in book.sections.into_iter().enumerate() {
            let number = index + 1;
            let id = SpineItemId::new(format!("section-{number}"))?;
            let href = PublicationUrl::parse(&format!("Text/section-{number}.xhtml"))?;
            if section.linear {
                fallback_toc.push(TocEntry {
                    label: section.title.clone(),
                    href: Some(href.clone()),
                    children: Vec::new(),
                });
            }
            descriptors.push(SpineItem {
                id,
                href,
                media_type: "application/xhtml+xml".into(),
                linear: section.linear,
                properties: Vec::new(),
            });
            sections.push(section.content);
        }
        if descriptors.is_empty() {
            return Err(conversion_error(format, "没有可阅读的正文"));
        }

        let table_of_contents = if book.table_of_contents.is_empty() {
            fallback_toc
        } else {
            book.table_of_contents
                .into_iter()
                .map(parse_toc_entry)
                .collect::<Result<Vec<_>, _>>()?
        };
        let cover = book
            .cover_path
            .as_deref()
            .map(PublicationUrl::parse)
            .transpose()?;
        let resources = book
            .resources
            .into_iter()
            .map(|resource| {
                let href = PublicationUrl::parse(&resource.path)?;
                Ok((
                    href.path().to_owned(),
                    StoredResource {
                        href,
                        media_type: resource.media_type,
                        bytes: resource.bytes.into(),
                    },
                ))
            })
            .collect::<Result<HashMap<_, _>, PublicationError>>()?;
        Ok(Self {
            book: Book {
                id: PublicationId::new(book.id)?,
                metadata: book.metadata,
                cover,
                sections: descriptors,
                table_of_contents,
            },
            sections,
            resources,
        })
    }
}

impl BookSource for DirectBookSource {
    fn book(&self) -> &Book {
        &self.book
    }

    fn parse_section(&self, index: usize) -> Result<Section, PublicationError> {
        let descriptor = self
            .book
            .sections
            .get(index)
            .ok_or_else(|| PublicationError::ResourceNotFound(format!("section {index}")))?;
        let content = self
            .sections
            .get(index)
            .ok_or_else(|| PublicationError::ResourceNotFound(format!("section {index}")))?;
        match content {
            SectionContent::Html(body) => {
                let document = format!(
                    "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title></title></head><body>{body}</body></html>"
                );
                parse_section(&document, descriptor, |_| None)
                    .map_err(|error| PublicationError::InvalidPublication(error.to_string()))
            }
            SectionContent::Image { resource_path, alt } => {
                let href = PublicationUrl::parse(resource_path)?;
                Ok(Section {
                    id: descriptor.id.clone(),
                    href: descriptor.href.clone(),
                    blocks: vec![Block::Image(ImageBlock {
                        href,
                        alt: alt.clone(),
                        style: ImageStyle::default(),
                        source: None,
                        text_layer: None,
                    })],
                    anchors: Vec::new(),
                })
            }
        }
    }

    fn resource(&self, href: &PublicationUrl) -> Result<Resource, PublicationError> {
        let resource = self
            .resources
            .get(href.resource_url().path())
            .ok_or_else(|| PublicationError::ResourceNotFound(href.to_string()))?;
        Ok(Resource {
            href: resource.href.clone(),
            media_type: resource.media_type.clone(),
            bytes: Arc::clone(&resource.bytes),
        })
    }
}

fn parse_toc_entry(entry: SourceTocEntry) -> Result<TocEntry, PublicationError> {
    Ok(TocEntry {
        label: entry.label,
        href: Some(PublicationUrl::parse(&entry.href)?),
        children: entry
            .children
            .into_iter()
            .map(parse_toc_entry)
            .collect::<Result<Vec<_>, _>>()?,
    })
}

pub(crate) fn escape_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub(crate) fn escape_attribute(value: &str) -> String {
    escape_text(value)
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use rebook_publication::{BookSource, RenditionLayout};

    use super::*;

    #[test]
    fn direct_source_parses_lazy_html_toc_fragments_and_resources() {
        let source = DirectBookSource::open(
            SourceBook {
                id: "direct-source-test".into(),
                metadata: Metadata {
                    title: "Direct".into(),
                    authors: Vec::new(),
                    languages: Vec::new(),
                    layout: RenditionLayout::Reflowable,
                },
                sections: vec![SourceSection {
                    title: "Chapter".into(),
                    content: SectionContent::Html(
                        "<h1 id=\"chapter\">Chapter</h1><img src=\"../Images/cover.png\"/>".into(),
                    ),
                    linear: true,
                }],
                table_of_contents: vec![SourceTocEntry {
                    label: "Chapter".into(),
                    href: "Text/section-1.xhtml#chapter".into(),
                    children: Vec::new(),
                }],
                resources: vec![SourceResource {
                    path: "Images/cover.png".into(),
                    media_type: "image/png".into(),
                    bytes: vec![1, 2, 3],
                }],
                cover_path: Some("Images/cover.png".into()),
            },
            BookFormat::Fb2,
        )
        .unwrap();

        assert_eq!(
            source.book().table_of_contents[0]
                .href
                .as_ref()
                .and_then(PublicationUrl::fragment),
            Some("chapter")
        );
        let section = source.parse_section(0).unwrap();
        assert_eq!(section.anchors[0].fragment, "chapter");
        let cover = source
            .resource(source.book().cover.as_ref().unwrap())
            .unwrap();
        assert_eq!(cover.bytes.as_ref(), [1, 2, 3]);
    }
}
