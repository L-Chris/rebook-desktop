//! Safe, pull-based EPUB publication parser.

mod reading;

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::Arc;

use quick_xml::Reader;
use quick_xml::events::Event;
use rebook_publication::{
    Book, BookSource, Link, Metadata, PublicationError, PublicationId, PublicationUrl,
    RenditionLayout, Resource, Section, SpineItem, SpineItemId, TocEntry,
};
use roxmltree::{Document, Node};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use zip::{CompressionMethod, ZipArchive};

/// Resource budgets applied before and during decompression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpubLimits {
    /// Maximum compressed EPUB file size.
    pub max_archive_bytes: u64,
    /// Maximum number of non-directory archive entries.
    pub max_entries: usize,
    /// Maximum uncompressed size of one entry.
    pub max_entry_bytes: u64,
    /// Maximum declared uncompressed size across all entries.
    pub max_total_uncompressed_bytes: u64,
    /// Maximum uncompressed/compressed ratio for a non-empty entry.
    pub max_compression_ratio: u64,
    /// Maximum bytes accepted for one XML document.
    pub max_xml_bytes: u64,
    /// Maximum XML element nesting depth.
    pub max_xml_depth: usize,
}

impl Default for EpubLimits {
    fn default() -> Self {
        Self {
            max_archive_bytes: 512 * 1024 * 1024,
            max_entries: 10_000,
            max_entry_bytes: 64 * 1024 * 1024,
            max_total_uncompressed_bytes: 1024 * 1024 * 1024,
            max_compression_ratio: 200,
            max_xml_bytes: 8 * 1024 * 1024,
            max_xml_depth: 128,
        }
    }
}

/// Parser behavior for spec violations commonly found in real publications.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpubOpenOptions {
    /// Resource budgets.
    pub limits: EpubLimits,
    /// When true, missing or misplaced `mimetype` is an error instead of a warning.
    pub strict_container: bool,
}

/// Machine-readable diagnostic severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticSeverity {
    /// Publication opened, but compatibility recovery was needed.
    Warning,
    /// Informational parser observation.
    Info,
}

/// Parser diagnostic retained by a successfully opened publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Stable diagnostic code.
    pub code: String,
    /// Severity.
    pub severity: DiagnosticSeverity,
    /// Human-readable explanation.
    pub message: String,
    /// Related publication resource.
    pub resource: Option<String>,
}

/// Parsed EPUB backed by an immutable in-memory archive and lazy resource reads.
#[derive(Debug)]
pub struct EpubPublication {
    book: Book,
    manifest: Vec<Link>,
    diagnostics: Vec<Diagnostic>,
    media_types: HashMap<String, String>,
    archive: EpubArchive,
}

impl EpubPublication {
    /// Opens an EPUB from a local file after enforcing the archive-size budget.
    pub fn open_file(path: impl AsRef<Path>) -> Result<Self, EpubError> {
        Self::open_file_with_options(path, EpubOpenOptions::default())
    }

    /// Opens an EPUB from a local file with explicit parser options.
    pub fn open_file_with_options(
        path: impl AsRef<Path>,
        options: EpubOpenOptions,
    ) -> Result<Self, EpubError> {
        let mut file = File::open(path.as_ref())?;
        let size = file.metadata()?.len();
        ensure_limit(
            size <= options.limits.max_archive_bytes,
            format!(
                "archive contains {size} bytes; limit is {}",
                options.limits.max_archive_bytes
            ),
        )?;
        let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
        file.by_ref()
            .take(options.limits.max_archive_bytes.saturating_add(1))
            .read_to_end(&mut bytes)?;
        ensure_limit(
            u64::try_from(bytes.len()).unwrap_or(u64::MAX) <= options.limits.max_archive_bytes,
            "archive grew beyond its configured size while being read",
        )?;
        Self::open_bytes_with_options(bytes, options)
    }

    /// Opens an EPUB from immutable bytes using default limits.
    pub fn open_bytes(bytes: impl Into<Arc<[u8]>>) -> Result<Self, EpubError> {
        Self::open_bytes_with_options(bytes, EpubOpenOptions::default())
    }

    /// Opens an EPUB from immutable bytes with explicit limits and compatibility behavior.
    pub fn open_bytes_with_options(
        bytes: impl Into<Arc<[u8]>>,
        options: EpubOpenOptions,
    ) -> Result<Self, EpubError> {
        let bytes = bytes.into();
        ensure_limit(
            u64::try_from(bytes.len()).unwrap_or(u64::MAX) <= options.limits.max_archive_bytes,
            "archive exceeds configured byte limit",
        )?;
        let archive = EpubArchive::new(bytes.clone(), options.limits)?;
        let mut diagnostics = Vec::new();
        validate_mimetype(&archive, options.strict_container, &mut diagnostics)?;

        let container_url = PublicationUrl::parse("META-INF/container.xml")?;
        let container = archive.read_xml(&container_url)?;
        let rootfile_path = parse_container(&container)?;
        let package_url = PublicationUrl::parse(&rootfile_path)?.resource_url();
        let package = archive.read_xml(&package_url)?;
        let package_model = parse_package(&package, &package_url)?;

        let mut media_types = HashMap::new();
        let mut manifest = Vec::with_capacity(package_model.manifest.len());
        for item in package_model
            .manifest_order
            .iter()
            .filter_map(|id| package_model.manifest.get(id))
        {
            media_types.insert(item.href.path().to_owned(), item.media_type.clone());
            manifest.push(Link {
                href: item.href.clone(),
                media_type: item.media_type.clone(),
                properties: item.properties.clone(),
            });
        }

        let reading_order = build_reading_order(&package_model)?;
        let table_of_contents = parse_navigation(&archive, &package_model)?;
        let digest = Sha256::digest(bytes.as_ref());
        let id = PublicationId::new(format!("sha256:{digest:x}"))?;

        if table_of_contents.is_empty() {
            diagnostics.push(Diagnostic {
                code: "epub.navigation.missing".into(),
                severity: DiagnosticSeverity::Warning,
                message: "publication has no usable EPUB navigation document or NCX".into(),
                resource: Some(package_url.to_string()),
            });
        }

        Ok(Self {
            book: Book {
                id,
                metadata: package_model.metadata,
                cover: package_model.cover,
                sections: reading_order,
                table_of_contents,
            },
            manifest,
            diagnostics,
            media_types,
            archive,
        })
    }

    /// Returns package manifest links in declaration order.
    pub fn manifest(&self) -> &[Link] {
        &self.manifest
    }

    /// Returns compatibility diagnostics emitted while opening.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

impl BookSource for EpubPublication {
    fn book(&self) -> &Book {
        &self.book
    }

    fn parse_section(&self, index: usize) -> Result<Section, PublicationError> {
        self.parse_section_ir(index)
            .map_err(EpubError::into_publication_error)
    }

    fn resource(&self, href: &PublicationUrl) -> Result<Resource, PublicationError> {
        let href = href.resource_url();
        let bytes = self
            .archive
            .read(&href)
            .map_err(EpubError::into_publication_error)?;
        let media_type = self
            .media_types
            .get(href.path())
            .cloned()
            .unwrap_or_else(|| guess_media_type(href.path()).to_owned());
        Ok(Resource {
            href,
            media_type,
            bytes: bytes.into(),
        })
    }
}

impl EpubPublication {
    fn parse_section_ir(&self, index: usize) -> Result<Section, EpubError> {
        let descriptor = self.book.sections.get(index).ok_or_else(|| {
            EpubError::InvalidArchive(format!("section index out of bounds: {index}"))
        })?;
        if descriptor.media_type != "application/xhtml+xml" && descriptor.media_type != "text/html"
        {
            return Err(EpubError::Unsupported(format!(
                "reflowable section media type: {}",
                descriptor.media_type
            )));
        }

        let xml = self.archive.read_xml(&descriptor.href)?;
        reading::parse_section(&xml, descriptor, |href| self.archive.read_stylesheet(href))
    }
}
#[derive(Debug)]
struct EpubArchive {
    bytes: Arc<[u8]>,
    entries: HashMap<String, ArchiveEntry>,
    limits: EpubLimits,
}

#[derive(Debug, Clone)]
struct ArchiveEntry {
    index: usize,
    size: u64,
    compressed_size: u64,
    stored: bool,
}

impl EpubArchive {
    fn new(bytes: Arc<[u8]>, limits: EpubLimits) -> Result<Self, EpubError> {
        let mut archive = ZipArchive::new(Cursor::new(bytes.clone()))?;
        ensure_limit(
            archive.len() <= limits.max_entries,
            format!(
                "archive contains {} entries; limit is {}",
                archive.len(),
                limits.max_entries
            ),
        )?;

        let mut entries = HashMap::with_capacity(archive.len());
        let mut total_uncompressed = 0_u64;
        for index in 0..archive.len() {
            let file = archive.by_index(index)?;
            if file.is_dir() {
                continue;
            }
            if file.encrypted() {
                return Err(EpubError::Unsupported(
                    "encrypted ZIP entries are not supported".into(),
                ));
            }
            if file
                .unix_mode()
                .is_some_and(|mode| mode & 0o170_000 == 0o120_000)
            {
                return Err(EpubError::InvalidArchive(format!(
                    "symbolic-link ZIP entry is not allowed: {}",
                    file.name()
                )));
            }

            let href = PublicationUrl::parse(file.name())?.resource_url();
            ensure_limit(
                file.size() <= limits.max_entry_bytes,
                format!(
                    "entry {} declares {} uncompressed bytes; per-entry limit is {}",
                    href,
                    file.size(),
                    limits.max_entry_bytes
                ),
            )?;
            ensure_compression_ratio(file.size(), file.compressed_size(), limits, &href)?;
            total_uncompressed = total_uncompressed
                .checked_add(file.size())
                .ok_or_else(|| EpubError::ResourceLimit("uncompressed size overflow".into()))?;
            ensure_limit(
                total_uncompressed <= limits.max_total_uncompressed_bytes,
                format!(
                    "archive declares {total_uncompressed} uncompressed bytes; total limit is {}",
                    limits.max_total_uncompressed_bytes
                ),
            )?;

            let entry = ArchiveEntry {
                index,
                size: file.size(),
                compressed_size: file.compressed_size(),
                stored: file.compression() == CompressionMethod::Stored,
            };
            if entries.insert(href.path().to_owned(), entry).is_some() {
                return Err(EpubError::InvalidArchive(format!(
                    "duplicate canonical ZIP entry: {href}"
                )));
            }
        }
        Ok(Self {
            bytes,
            entries,
            limits,
        })
    }

    fn read(&self, href: &PublicationUrl) -> Result<Vec<u8>, EpubError> {
        let entry = self
            .entries
            .get(href.path())
            .ok_or_else(|| EpubError::ResourceNotFound(href.to_string()))?;
        ensure_limit(
            entry.size <= self.limits.max_entry_bytes,
            format!("resource exceeded size budget: {href}"),
        )?;
        ensure_compression_ratio(entry.size, entry.compressed_size, self.limits, href)?;

        let mut archive = ZipArchive::new(Cursor::new(self.bytes.clone()))?;
        let mut file = archive.by_index(entry.index)?;
        let capacity = usize::try_from(entry.size).unwrap_or(0);
        let mut bytes = Vec::with_capacity(capacity);
        file.by_ref()
            .take(self.limits.max_entry_bytes.saturating_add(1))
            .read_to_end(&mut bytes)?;
        ensure_limit(
            u64::try_from(bytes.len()).unwrap_or(u64::MAX) <= self.limits.max_entry_bytes,
            format!("resource expanded beyond size budget: {href}"),
        )?;
        Ok(bytes)
    }

    fn read_xml(&self, href: &PublicationUrl) -> Result<String, EpubError> {
        let entry = self
            .entries
            .get(href.path())
            .ok_or_else(|| EpubError::ResourceNotFound(href.to_string()))?;
        ensure_limit(
            entry.size <= self.limits.max_xml_bytes,
            format!(
                "XML resource {} declares {} bytes; XML limit is {}",
                href, entry.size, self.limits.max_xml_bytes
            ),
        )?;
        let bytes = self.read(href)?;
        let text = decode_xml(&bytes, href)?;
        sanitize_and_validate_xml(&text, href, self.limits.max_xml_depth)
    }

    fn read_stylesheet(&self, href: &PublicationUrl) -> Result<String, EpubError> {
        let entry = self
            .entries
            .get(href.path())
            .ok_or_else(|| EpubError::ResourceNotFound(href.to_string()))?;
        ensure_limit(
            entry.size <= self.limits.max_xml_bytes,
            format!(
                "stylesheet {} declares {} bytes; text limit is {}",
                href, entry.size, self.limits.max_xml_bytes
            ),
        )?;
        decode_xml(&self.read(href)?, href)
    }
}

#[derive(Debug)]
struct PackageModel {
    metadata: Metadata,
    cover: Option<PublicationUrl>,
    manifest: BTreeMap<String, ManifestItem>,
    manifest_order: Vec<String>,
    spine: Vec<SpineReference>,
    ncx_id: Option<String>,
}

#[derive(Debug, Clone)]
struct ManifestItem {
    id: String,
    href: PublicationUrl,
    media_type: String,
    properties: Vec<String>,
}

#[derive(Debug)]
struct SpineReference {
    idref: String,
    linear: bool,
    properties: Vec<String>,
}

fn validate_mimetype(
    archive: &EpubArchive,
    strict: bool,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<(), EpubError> {
    let href = PublicationUrl::parse("mimetype")?;
    let Some(entry) = archive.entries.get(href.path()) else {
        return container_issue(
            strict,
            diagnostics,
            "epub.container.mimetype-missing",
            "EPUB archive is missing its root mimetype entry",
        );
    };
    let bytes = archive.read(&href)?;
    let valid_value = bytes == b"application/epub+zip";
    if !valid_value || !entry.stored || entry.index != 0 {
        return container_issue(
            strict,
            diagnostics,
            "epub.container.mimetype-invalid",
            "mimetype must be the first, stored entry and contain application/epub+zip",
        );
    }
    Ok(())
}

fn container_issue(
    strict: bool,
    diagnostics: &mut Vec<Diagnostic>,
    code: &str,
    message: &str,
) -> Result<(), EpubError> {
    if strict {
        Err(EpubError::InvalidArchive(message.into()))
    } else {
        diagnostics.push(Diagnostic {
            code: code.into(),
            severity: DiagnosticSeverity::Warning,
            message: message.into(),
            resource: Some("mimetype".into()),
        });
        Ok(())
    }
}

fn parse_container(xml: &str) -> Result<String, EpubError> {
    let document = Document::parse(xml)?;
    document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "rootfile")
        .and_then(|node| attribute_local(node, "full-path"))
        .map(str::to_owned)
        .ok_or_else(|| EpubError::InvalidXml {
            resource: "META-INF/container.xml".into(),
            message: "container has no rootfile full-path".into(),
        })
}

fn parse_package_metadata(package: Node<'_, '_>) -> (Metadata, Option<String>) {
    let metadata_node = package
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == "metadata");
    let title = metadata_node
        .and_then(|metadata| first_descendant_text(metadata, "title"))
        .unwrap_or_else(|| "Untitled publication".into());
    let authors =
        metadata_node.map_or_else(Vec::new, |metadata| descendant_texts(metadata, "creator"));
    let languages =
        metadata_node.map_or_else(Vec::new, |metadata| descendant_texts(metadata, "language"));
    let layout = metadata_node
        .and_then(|metadata| {
            metadata.descendants().find(|node| {
                node.is_element()
                    && node.tag_name().name() == "meta"
                    && attribute_local(*node, "property") == Some("rendition:layout")
            })
        })
        .and_then(normalized_node_text)
        .filter(|value| value == "pre-paginated")
        .map_or(RenditionLayout::Reflowable, |_| {
            RenditionLayout::PrePaginated
        });
    let epub2_cover_id = metadata_node.and_then(|metadata| {
        metadata
            .descendants()
            .find(|node| {
                node.is_element()
                    && node.tag_name().name() == "meta"
                    && attribute_local(*node, "name") == Some("cover")
            })
            .and_then(|node| attribute_local(node, "content"))
            .map(str::to_owned)
    });
    (
        Metadata {
            title,
            authors,
            languages,
            layout,
        },
        epub2_cover_id,
    )
}

fn parse_package(xml: &str, package_url: &PublicationUrl) -> Result<PackageModel, EpubError> {
    let document = Document::parse(xml)?;
    let package = document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "package")
        .ok_or_else(|| EpubError::InvalidXml {
            resource: package_url.to_string(),
            message: "package document has no package element".into(),
        })?;

    let (metadata, epub2_cover_id) = parse_package_metadata(package);

    let manifest_node = package
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == "manifest")
        .ok_or_else(|| EpubError::InvalidXml {
            resource: package_url.to_string(),
            message: "package document has no manifest".into(),
        })?;
    let mut manifest = BTreeMap::new();
    let mut manifest_order = Vec::new();
    for item in manifest_node
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
    {
        let id = required_attribute(item, "id", package_url)?;
        let raw_href = required_attribute(item, "href", package_url)?;
        let media_type = required_attribute(item, "media-type", package_url)?;
        let href = package_url.resolve(&raw_href)?.resource_url();
        let properties = token_attribute(item, "properties");
        let model = ManifestItem {
            id: id.clone(),
            href,
            media_type,
            properties,
        };
        if manifest.insert(id.clone(), model).is_some() {
            return Err(EpubError::InvalidXml {
                resource: package_url.to_string(),
                message: format!("duplicate manifest ID: {id}"),
            });
        }
        manifest_order.push(id);
    }
    let cover = manifest_order
        .iter()
        .filter_map(|id| manifest.get(id))
        .find(|item| {
            item.properties
                .iter()
                .any(|property| property == "cover-image")
        })
        .or_else(|| {
            epub2_cover_id
                .as_ref()
                .and_then(|cover_id| manifest.get(cover_id))
        })
        .map(|item| item.href.clone());

    let spine_node = package
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == "spine")
        .ok_or_else(|| EpubError::InvalidXml {
            resource: package_url.to_string(),
            message: "package document has no spine".into(),
        })?;
    let ncx_id = attribute_local(spine_node, "toc").map(str::to_owned);
    let mut spine = Vec::new();
    for itemref in spine_node
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "itemref")
    {
        spine.push(SpineReference {
            idref: required_attribute(itemref, "idref", package_url)?,
            linear: attribute_local(itemref, "linear") != Some("no"),
            properties: token_attribute(itemref, "properties"),
        });
    }
    if spine.is_empty() {
        return Err(EpubError::InvalidXml {
            resource: package_url.to_string(),
            message: "package spine is empty".into(),
        });
    }

    Ok(PackageModel {
        metadata,
        cover,
        manifest,
        manifest_order,
        spine,
        ncx_id,
    })
}

fn build_reading_order(package: &PackageModel) -> Result<Vec<SpineItem>, EpubError> {
    package
        .spine
        .iter()
        .map(|reference| {
            let item = package.manifest.get(&reference.idref).ok_or_else(|| {
                EpubError::InvalidArchive(format!(
                    "spine references unknown manifest ID: {}",
                    reference.idref
                ))
            })?;
            let mut properties = item.properties.clone();
            for property in &reference.properties {
                if !properties.contains(property) {
                    properties.push(property.clone());
                }
            }
            Ok(SpineItem {
                id: SpineItemId::new(item.id.clone())?,
                href: item.href.clone(),
                media_type: item.media_type.clone(),
                linear: reference.linear,
                properties,
            })
        })
        .collect()
}

fn parse_navigation(
    archive: &EpubArchive,
    package: &PackageModel,
) -> Result<Vec<TocEntry>, EpubError> {
    let nav_item = package
        .manifest_order
        .iter()
        .filter_map(|id| package.manifest.get(id))
        .find(|item| item.properties.iter().any(|property| property == "nav"));
    if let Some(nav_item) = nav_item {
        let xml = archive.read_xml(&nav_item.href)?;
        let toc = parse_epub_navigation(&xml, &nav_item.href)?;
        if !toc.is_empty() {
            return Ok(toc);
        }
    }

    let ncx_item = package
        .ncx_id
        .as_ref()
        .and_then(|id| package.manifest.get(id))
        .or_else(|| {
            package
                .manifest_order
                .iter()
                .filter_map(|id| package.manifest.get(id))
                .find(|item| item.media_type == "application/x-dtbncx+xml")
        });
    ncx_item.map_or_else(
        || Ok(Vec::new()),
        |item| {
            let xml = archive.read_xml(&item.href)?;
            parse_ncx(&xml, &item.href)
        },
    )
}

fn parse_epub_navigation(xml: &str, nav_url: &PublicationUrl) -> Result<Vec<TocEntry>, EpubError> {
    let document = Document::parse(xml)?;
    let nav = document.descendants().find(|node| {
        node.is_element()
            && node.tag_name().name() == "nav"
            && attribute_local(*node, "type")
                .is_some_and(|value| value.split_ascii_whitespace().any(|token| token == "toc"))
    });
    let Some(ordered_list) = nav.and_then(|node| direct_child(node, "ol")) else {
        return Ok(Vec::new());
    };
    parse_nav_list(ordered_list, nav_url)
}

fn parse_nav_list(
    list: Node<'_, '_>,
    nav_url: &PublicationUrl,
) -> Result<Vec<TocEntry>, EpubError> {
    list.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "li")
        .map(|item| {
            let label_node = item
                .children()
                .find(|node| node.is_element() && matches!(node.tag_name().name(), "a" | "span"));
            let label = label_node
                .and_then(normalized_node_text)
                .unwrap_or_else(|| "Untitled section".into());
            let href = label_node
                .and_then(|node| attribute_local(node, "href"))
                .map(|value| nav_url.resolve(value))
                .transpose()
                .or_else(|error| match error {
                    PublicationError::ExternalUrl(_) => Ok(None),
                    other => Err(other),
                })?;
            let children = direct_child(item, "ol")
                .map(|nested| parse_nav_list(nested, nav_url))
                .transpose()?
                .unwrap_or_default();
            Ok(TocEntry {
                label,
                href,
                children,
            })
        })
        .collect()
}

fn parse_ncx(xml: &str, ncx_url: &PublicationUrl) -> Result<Vec<TocEntry>, EpubError> {
    let document = Document::parse(xml)?;
    let Some(nav_map) = document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "navMap")
    else {
        return Ok(Vec::new());
    };
    parse_nav_points(nav_map, ncx_url)
}

fn parse_nav_points(
    parent: Node<'_, '_>,
    ncx_url: &PublicationUrl,
) -> Result<Vec<TocEntry>, EpubError> {
    parent
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "navPoint")
        .map(|point| {
            let label = point
                .descendants()
                .find(|node| node.is_element() && node.tag_name().name() == "navLabel")
                .and_then(|node| first_descendant_text(node, "text"))
                .unwrap_or_else(|| "Untitled section".into());
            let href = point
                .children()
                .find(|node| node.is_element() && node.tag_name().name() == "content")
                .and_then(|node| attribute_local(node, "src"))
                .map(|value| ncx_url.resolve(value))
                .transpose()
                .or_else(|error| match error {
                    PublicationError::ExternalUrl(_) => Ok(None),
                    other => Err(other),
                })?;
            Ok(TocEntry {
                label,
                href,
                children: parse_nav_points(point, ncx_url)?,
            })
        })
        .collect()
}

fn sanitize_and_validate_xml(
    xml: &str,
    href: &PublicationUrl,
    max_depth: usize,
) -> Result<String, EpubError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut depth = 0_usize;
    let mut plain_html_doctype = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(_)) => {
                depth = depth.saturating_add(1);
                if depth > max_depth {
                    return Err(EpubError::ResourceLimit(format!(
                        "XML depth exceeds {max_depth}: {href}"
                    )));
                }
            }
            Ok(Event::End(_)) => depth = depth.saturating_sub(1),
            Ok(Event::DocType(doctype))
                if href.path().to_ascii_lowercase().ends_with(".xhtml")
                    && doctype.trim_ascii().eq_ignore_ascii_case(b"html") =>
            {
                let end = usize::try_from(reader.buffer_position()).unwrap_or(xml.len());
                let Some(start) = xml[..end].to_ascii_lowercase().rfind("<!doctype") else {
                    return Err(EpubError::InvalidXml {
                        resource: href.to_string(),
                        message: "failed to locate validated XHTML DOCTYPE".into(),
                    });
                };
                plain_html_doctype = Some(start..end);
            }
            Ok(Event::DocType(_)) => {
                return Err(EpubError::InvalidXml {
                    resource: href.to_string(),
                    message: "DOCTYPE is disabled for untrusted EPUB XML except plain XHTML <!DOCTYPE html>"
                        .into(),
                });
            }
            Ok(Event::Eof) => {
                let Some(range) = plain_html_doctype else {
                    return Ok(xml.to_owned());
                };
                let mut sanitized = String::with_capacity(xml.len() - range.len());
                sanitized.push_str(&xml[..range.start]);
                sanitized.push_str(&xml[range.end..]);
                return Ok(sanitized);
            }
            Ok(_) => {}
            Err(error) => {
                return Err(EpubError::InvalidXml {
                    resource: href.to_string(),
                    message: error.to_string(),
                });
            }
        }
    }
}

fn decode_xml(bytes: &[u8], href: &PublicationUrl) -> Result<String, EpubError> {
    if let Some(body) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16(body, true, href);
    }
    if let Some(body) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16(body, false, href);
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8(bytes.to_vec()).map_err(|error| EpubError::InvalidXml {
        resource: href.to_string(),
        message: format!("XML must use UTF-8 or UTF-16: {error}"),
    })
}

fn decode_utf16(
    bytes: &[u8],
    little_endian: bool,
    href: &PublicationUrl,
) -> Result<String, EpubError> {
    if !bytes.len().is_multiple_of(2) {
        return Err(EpubError::InvalidXml {
            resource: href.to_string(),
            message: "UTF-16 XML has an odd byte length".into(),
        });
    }
    let words = bytes.chunks_exact(2).map(|chunk| {
        let pair = [chunk[0], chunk[1]];
        if little_endian {
            u16::from_le_bytes(pair)
        } else {
            u16::from_be_bytes(pair)
        }
    });
    char::decode_utf16(words)
        .collect::<Result<String, _>>()
        .map_err(|error| EpubError::InvalidXml {
            resource: href.to_string(),
            message: format!("invalid UTF-16 XML: {error}"),
        })
}

fn ensure_compression_ratio(
    size: u64,
    compressed_size: u64,
    limits: EpubLimits,
    href: &PublicationUrl,
) -> Result<(), EpubError> {
    if size == 0 {
        return Ok(());
    }
    ensure_limit(
        compressed_size > 0 && size / compressed_size.max(1) <= limits.max_compression_ratio,
        format!(
            "entry compression ratio exceeds {}: {href}",
            limits.max_compression_ratio
        ),
    )
}

fn ensure_limit(condition: bool, message: impl Into<String>) -> Result<(), EpubError> {
    if condition {
        Ok(())
    } else {
        Err(EpubError::ResourceLimit(message.into()))
    }
}

fn required_attribute(
    node: Node<'_, '_>,
    name: &str,
    resource: &PublicationUrl,
) -> Result<String, EpubError> {
    attribute_local(node, name)
        .map(str::to_owned)
        .ok_or_else(|| EpubError::InvalidXml {
            resource: resource.to_string(),
            message: format!("{} element is missing {name}", node.tag_name().name()),
        })
}

fn attribute_local<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|attribute| attribute.name() == name)
        .map(|attribute| attribute.value())
}

fn token_attribute(node: Node<'_, '_>, name: &str) -> Vec<String> {
    attribute_local(node, name)
        .map(|value| value.split_ascii_whitespace().map(str::to_owned).collect())
        .unwrap_or_default()
}

fn direct_child<'a>(node: Node<'a, 'a>, name: &str) -> Option<Node<'a, 'a>> {
    node.children()
        .find(|child| child.is_element() && child.tag_name().name() == name)
}

fn first_descendant_text(node: Node<'_, '_>, name: &str) -> Option<String> {
    node.descendants()
        .find(|child| child.is_element() && child.tag_name().name() == name)
        .and_then(normalized_node_text)
}

fn descendant_texts(node: Node<'_, '_>, name: &str) -> Vec<String> {
    node.descendants()
        .filter(|child| child.is_element() && child.tag_name().name() == name)
        .filter_map(normalized_node_text)
        .collect()
}

fn normalized_node_text(node: Node<'_, '_>) -> Option<String> {
    let text = node
        .descendants()
        .filter(roxmltree::Node::is_text)
        .filter_map(|descendant| descendant.text())
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ");
    (!text.is_empty()).then_some(text)
}

fn guess_media_type(path: &str) -> &'static str {
    let extension = path.rsplit_once('.').map(|(_, extension)| extension);
    match extension.map(str::to_ascii_lowercase).as_deref() {
        Some("xhtml" | "html" | "htm") => "application/xhtml+xml",
        Some("css") => "text/css",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("otf") => "font/otf",
        Some("ttf") => "font/ttf",
        _ => "application/octet-stream",
    }
}

/// EPUB open and resource errors.
#[derive(Debug, Error)]
pub enum EpubError {
    /// File-system access failed.
    #[error("EPUB I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// ZIP central directory or entry data was invalid.
    #[error("invalid EPUB ZIP: {0}")]
    Zip(#[from] zip::result::ZipError),
    /// XML tree parsing failed.
    #[error("invalid EPUB XML: {0}")]
    XmlTree(#[from] roxmltree::Error),
    /// A specific XML resource was invalid.
    #[error("invalid XML in {resource}: {message}")]
    InvalidXml {
        /// Resource being parsed.
        resource: String,
        /// Parser or validation detail.
        message: String,
    },
    /// ZIP structure or EPUB relationships were invalid.
    #[error("invalid EPUB archive: {0}")]
    InvalidArchive(String),
    /// A resource was not present in the archive.
    #[error("EPUB resource not found: {0}")]
    ResourceNotFound(String),
    /// A safety budget was exceeded.
    #[error("EPUB resource limit exceeded: {0}")]
    ResourceLimit(String),
    /// An intentionally unsupported container feature was encountered.
    #[error("unsupported EPUB feature: {0}")]
    Unsupported(String),
    /// Format-neutral publication validation failed.
    #[error(transparent)]
    Publication(#[from] PublicationError),
}

impl EpubError {
    fn into_publication_error(self) -> PublicationError {
        match self {
            Self::ResourceNotFound(resource) => PublicationError::ResourceNotFound(resource),
            Self::ResourceLimit(message) => PublicationError::ResourceLimit(message),
            Self::Publication(error) => error,
            Self::Io(error) => PublicationError::Io(error.to_string()),
            other => PublicationError::InvalidPublication(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use rebook_publication::{Block, BookSource, PublicationUrl, RenditionLayout};
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    use super::{EpubError, EpubLimits, EpubOpenOptions, EpubPublication};

    #[test]
    fn opens_epub3_navigation_and_lazy_resources() {
        let bytes = minimal_epub();
        let publication = EpubPublication::open_bytes(bytes).expect("valid EPUB");

        assert_eq!(publication.book().metadata.title, "原生阅读器");
        assert_eq!(publication.book().metadata.authors, ["Rebook"]);
        assert_eq!(
            publication.book().cover.as_ref().map(PublicationUrl::path),
            Some("OPS/Images/cover.png")
        );
        assert_eq!(
            publication.book().metadata.layout,
            RenditionLayout::Reflowable
        );
        assert_eq!(publication.manifest()[0].href.path(), "OPS/nav.xhtml");
        assert_eq!(publication.book().sections.len(), 1);
        assert_eq!(publication.book().table_of_contents[0].label, "第一章");
        assert_eq!(publication.diagnostics().len(), 0);

        let href = PublicationUrl::parse("OPS/Text/chapter.xhtml").expect("valid URL");
        let resource = publication.resource(&href).expect("chapter resource");
        assert!(String::from_utf8_lossy(&resource.bytes).contains("你好，Rust"));

        let section = publication.parse_section(0).expect("reading IR");
        assert!(matches!(section.blocks.first(), Some(Block::Text(_))));
        let cover = publication
            .resource(publication.book().cover.as_ref().expect("EPUB 3 cover"))
            .expect("cover resource");
        assert_eq!(cover.bytes.as_ref(), b"fake-png");
    }

    #[test]
    fn rejects_archive_entries_that_escape_the_root() {
        let bytes = zip_entries(&[("../evil", b"escape", CompressionMethod::Stored)]);
        let error = EpubPublication::open_bytes(bytes).expect_err("unsafe path must fail");
        assert!(matches!(error, EpubError::Publication(_)));
    }

    #[test]
    fn rejects_declared_uncompressed_size_over_budget() {
        let bytes = minimal_epub();
        let options = EpubOpenOptions {
            limits: EpubLimits {
                max_total_uncompressed_bytes: 128,
                ..EpubLimits::default()
            },
            strict_container: false,
        };
        let error = EpubPublication::open_bytes_with_options(bytes, options)
            .expect_err("budget must be enforced");
        assert!(matches!(error, EpubError::ResourceLimit(_)));
    }

    #[test]
    fn strict_mode_rejects_a_compressed_mimetype_entry() {
        let entries = minimal_entries();
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes, _) in entries {
            writer
                .start_file(
                    name,
                    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
                )
                .expect("start entry");
            writer.write_all(bytes).expect("write entry");
        }
        let bytes = writer.finish().expect("finish ZIP").into_inner();
        let error = EpubPublication::open_bytes_with_options(
            bytes,
            EpubOpenOptions {
                strict_container: true,
                ..EpubOpenOptions::default()
            },
        )
        .expect_err("strict mode must reject invalid mimetype placement");
        assert!(matches!(error, EpubError::InvalidArchive(_)));
    }

    #[test]
    fn falls_back_to_epub2_ncx_navigation() {
        let publication = EpubPublication::open_bytes(zip_entries(&epub2_entries()))
            .expect("valid EPUB 2 publication");

        assert_eq!(publication.book().table_of_contents[0].label, "NCX 第一章");
        assert_eq!(
            publication.book().cover.as_ref().map(PublicationUrl::path),
            Some("OPS/Images/cover.jpg")
        );
        assert_eq!(
            publication.book().table_of_contents[0]
                .href
                .as_ref()
                .expect("NCX href")
                .to_string(),
            "OPS/Text/chapter.xhtml#start"
        );
    }

    #[test]
    fn rejects_doctype_before_tree_parsing() {
        let mut entries = minimal_entries();
        entries[1] = (
            "META-INF/container.xml",
            br#"<?xml version="1.0"?><!DOCTYPE container><container><rootfiles><rootfile full-path="OPS/package.opf"/></rootfiles></container>"#,
            CompressionMethod::Deflated,
        );
        let error = EpubPublication::open_bytes(zip_entries(&entries))
            .expect_err("untrusted DOCTYPE must fail");

        assert!(matches!(error, EpubError::InvalidXml { .. }));
        assert!(error.to_string().contains("DOCTYPE is disabled"));
    }

    #[test]
    fn allows_plain_html_doctype_for_xhtml_navigation() {
        let mut entries = minimal_entries();
        entries[3] = (
            "OPS/nav.xhtml",
            r#"<?xml version="1.0"?><!DOCTYPE html><html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
              <body><nav epub:type="toc"><ol><li><a href="Text/chapter.xhtml#start">第一章</a></li></ol></nav></body>
            </html>"#
                .as_bytes(),
            CompressionMethod::Deflated,
        );

        let publication = EpubPublication::open_bytes(zip_entries(&entries))
            .expect("plain HTML DOCTYPE in XHTML navigation must be accepted");

        assert_eq!(publication.book().table_of_contents[0].label, "第一章");
    }

    #[test]
    fn rejects_xhtml_doctype_with_external_identifier() {
        let mut entries = minimal_entries();
        entries[3] = (
            "OPS/nav.xhtml",
            br#"<?xml version="1.0"?><!DOCTYPE html SYSTEM "https://example.com/xhtml.dtd"><html xmlns="http://www.w3.org/1999/xhtml"><body/></html>"#,
            CompressionMethod::Deflated,
        );
        let error = EpubPublication::open_bytes(zip_entries(&entries))
            .expect_err("external XHTML DOCTYPE must fail");

        assert!(matches!(error, EpubError::InvalidXml { .. }));
        assert!(error.to_string().contains("DOCTYPE is disabled"));
    }

    fn minimal_epub() -> Vec<u8> {
        zip_entries(&minimal_entries())
    }

    fn minimal_entries() -> Vec<(&'static str, &'static [u8], CompressionMethod)> {
        vec![
            ("mimetype", b"application/epub+zip", CompressionMethod::Stored),
            (
                "META-INF/container.xml",
                br#"<?xml version="1.0"?>
                <container xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
                  <rootfiles><rootfile full-path="OPS/package.opf" media-type="application/oebps-package+xml"/></rootfiles>
                </container>"#,
                CompressionMethod::Deflated,
            ),
            (
                "OPS/package.opf",
                r#"<?xml version="1.0" encoding="UTF-8"?>
                <package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="book-id">
                  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
                    <dc:identifier id="book-id">urn:uuid:test</dc:identifier>
                    <dc:title>原生阅读器</dc:title><dc:creator>Rebook</dc:creator><dc:language>zh-CN</dc:language>
                  </metadata>
                  <manifest>
                    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
                    <item id="cover" href="Images/cover.png" media-type="image/png" properties="cover-image"/>
                    <item id="chapter" href="Text/chapter.xhtml" media-type="application/xhtml+xml"/>
                  </manifest>
                  <spine><itemref idref="chapter"/></spine>
                </package>"#
                    .as_bytes(),
                CompressionMethod::Deflated,
            ),
            (
                "OPS/nav.xhtml",
                r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
                  <body><nav epub:type="toc"><ol><li><a href="Text/chapter.xhtml#start">第一章</a></li></ol></nav></body>
                </html>"#
                    .as_bytes(),
                CompressionMethod::Deflated,
            ),
            (
                "OPS/Images/cover.png",
                b"fake-png",
                CompressionMethod::Deflated,
            ),
            (
                "OPS/Text/chapter.xhtml",
                r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><body><h1 id="start">第一章</h1><p>你好，Rust。</p></body></html>"#
                    .as_bytes(),
                CompressionMethod::Deflated,
            ),
        ]
    }

    fn epub2_entries() -> Vec<(&'static str, &'static [u8], CompressionMethod)> {
        vec![
            ("mimetype", b"application/epub+zip", CompressionMethod::Stored),
            (
                "META-INF/container.xml",
                br#"<?xml version="1.0"?><container><rootfiles><rootfile full-path="OPS/package.opf"/></rootfiles></container>"#,
                CompressionMethod::Deflated,
            ),
            (
                "OPS/package.opf",
                br#"<?xml version="1.0"?>
                <package version="2.0">
                  <metadata><title>EPUB 2</title><meta name="cover" content="cover-art"/></metadata>
                  <manifest>
                    <item id="chapter" href="Text/chapter.xhtml" media-type="application/xhtml+xml"/>
                    <item id="cover-art" href="Images/cover.jpg" media-type="image/jpeg"/>
                    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
                  </manifest>
                  <spine toc="ncx"><itemref idref="chapter"/></spine>
                </package>"#,
                CompressionMethod::Deflated,
            ),
            (
                "OPS/toc.ncx",
                r#"<?xml version="1.0"?><ncx><navMap>
                  <navPoint><navLabel><text>NCX 第一章</text></navLabel>
                    <content src="Text/chapter.xhtml#start"/>
                  </navPoint>
                </navMap></ncx>"#
                    .as_bytes(),
                CompressionMethod::Deflated,
            ),
            (
                "OPS/Images/cover.jpg",
                b"fake-jpeg",
                CompressionMethod::Deflated,
            ),
            (
                "OPS/Text/chapter.xhtml",
                br#"<html><body><h1 id="start">Chapter</h1></body></html>"#,
                CompressionMethod::Deflated,
            ),
        ]
    }

    fn zip_entries(entries: &[(&str, &[u8], CompressionMethod)]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes, compression) in entries {
            writer
                .start_file(
                    *name,
                    SimpleFileOptions::default().compression_method(*compression),
                )
                .expect("start entry");
            writer.write_all(bytes).expect("write entry");
        }
        writer.finish().expect("finish ZIP").into_inner()
    }
}
