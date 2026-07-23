//! Narrow adapter around the pre-alpha Blitz document stack.
//!
//! No Blitz, Stylo, Taffy, or Parley type appears in the public API. This keeps the reader,
//! publication, and desktop layers insulated from backend churn.

use std::sync::{Arc, Mutex, MutexGuard};

use anyrender::render_to_buffer;
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::{
    DocGuard, DocGuardMut, Document as BlitzDocument, DocumentConfig, FontContext, StyleThreading,
};
use blitz_html::{HtmlDocument, HtmlProvider};
use blitz_paint::paint_scene;
use blitz_traits::net::{Bytes, Method, NetHandler, NetProvider, Request};
use blitz_traits::shell::{ColorScheme as BlitzColorScheme, Viewport as BlitzViewport};
use rebook_publication::{Publication, PublicationError, PublicationUrl, SpineItemId};
use thiserror::Error;
use url::Url;

const READER_UA_CSS: &str = r"
html, body { min-height: 100%; }
body { overflow-wrap: break-word; }
img, svg, video { max-width: 100%; height: auto; }
table { max-width: 100%; }
script, form { display: none !important; }
";

/// Viewport used for one continuous reflow layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutViewport {
    /// Logical CSS-pixel width.
    pub width: u32,
    /// Logical CSS-pixel height.
    pub height: u32,
    /// Device scale factor.
    pub scale_factor: f32,
    /// Color scheme used for media queries.
    pub color_scheme: ColorScheme,
}

impl LayoutViewport {
    /// Creates and validates a layout viewport.
    pub fn new(width: u32, height: u32, scale_factor: f32) -> Result<Self, RenderError> {
        if width == 0 || height == 0 || !scale_factor.is_finite() || scale_factor <= 0.0 {
            return Err(RenderError::InvalidViewport);
        }
        Ok(Self {
            width,
            height,
            scale_factor,
            color_scheme: ColorScheme::Light,
        })
    }
}

/// Color scheme used during style resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorScheme {
    /// Light background and dark text.
    Light,
    /// Dark background and light text.
    Dark,
}

/// Layout metrics that do not expose backend-specific tree types.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutMetrics {
    /// Root border-box width in CSS pixels.
    pub content_width: f32,
    /// Root border-box height in CSS pixels.
    pub content_height: f32,
    /// Number of nodes in the backend DOM, useful for diagnostics.
    pub node_count: usize,
    /// Whether a critical stylesheet or font request remains unresolved.
    pub has_pending_critical_resources: bool,
}

/// Axis-aligned backend layout rectangle in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutRect {
    /// Horizontal position relative to the laid-out document.
    pub x: f32,
    /// Vertical position relative to the laid-out document.
    pub y: f32,
    /// Border-box width.
    pub width: f32,
    /// Border-box height.
    pub height: f32,
}

/// Text hit returned by the experimental backend adapter.
///
/// `backend_node_id` is deliberately labelled unstable. Phase 0 must add a canonical DOM to
/// `SourceAnchor` mapping before this value can be persisted in a Locator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendTextHit {
    /// Ephemeral Blitz inline-root node ID.
    pub backend_node_id: usize,
    /// UTF-8 byte offset in the inline root's shaped text.
    pub utf8_byte_offset: usize,
}

/// A blocked or failed subresource request observed by the adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceFailure {
    /// Fully resolved URL requested by Blitz.
    pub url: String,
    /// Stable high-level reason.
    pub reason: ResourceFailureReason,
}

/// Why a renderer subresource could not be supplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceFailureReason {
    /// Scheme or host was outside the in-process publication namespace.
    BlockedExternalUrl,
    /// Only GET requests are accepted.
    UnsupportedMethod,
    /// URL could not be converted to a canonical publication path.
    InvalidPublicationUrl,
    /// Publication resource was absent or rejected by its safety budget.
    ResourceUnavailable,
    /// Request had already been cancelled.
    Cancelled,
}

/// Resolved, laid-out reflowable document.
pub struct ReflowDocument {
    spine_item_id: SpineItemId,
    href: PublicationUrl,
    document: HtmlDocument,
    resource_failures: Arc<Mutex<Vec<ResourceFailure>>>,
}

impl BlitzDocument for ReflowDocument {
    fn inner(&self) -> DocGuard<'_> {
        self.document.inner()
    }

    fn inner_mut(&mut self) -> DocGuardMut<'_> {
        self.document.inner_mut()
    }
}

impl ReflowDocument {
    /// Loads and lays out one spine item through the isolated Blitz adapter.
    pub fn layout(
        publication: Arc<dyn Publication>,
        spine_index: usize,
        viewport: LayoutViewport,
    ) -> Result<Self, RenderError> {
        let spine_item = publication
            .reading_order()
            .get(spine_index)
            .ok_or(RenderError::SpineIndexOutOfBounds(spine_index))?
            .clone();
        let resource = publication.resource(&spine_item.href)?;
        if !matches!(
            resource.media_type.as_str(),
            "application/xhtml+xml" | "text/html"
        ) {
            return Err(RenderError::UnsupportedContentType(resource.media_type));
        }
        let html = std::str::from_utf8(&resource.bytes)
            .map_err(|error| RenderError::InvalidDocumentEncoding(error.to_string()))?;

        let base_url = publication_base_url(&spine_item.href)?;
        let resource_failures = Arc::new(Mutex::new(Vec::new()));
        let provider = PublicationNetProvider {
            publication,
            failures: resource_failures.clone(),
        };
        let color_scheme = match viewport.color_scheme {
            ColorScheme::Light => BlitzColorScheme::Light,
            ColorScheme::Dark => BlitzColorScheme::Dark,
        };
        let mut document = HtmlDocument::from_html(
            html,
            DocumentConfig {
                viewport: Some(BlitzViewport::new(
                    viewport.width,
                    viewport.height,
                    viewport.scale_factor,
                    color_scheme,
                )),
                base_url: Some(base_url.to_string()),
                ua_stylesheets: Some(vec![READER_UA_CSS.into()]),
                net_provider: Some(Arc::new(provider)),
                html_parser_provider: Some(Arc::new(HtmlProvider)),
                font_ctx: Some(FontContext::new()),
                style_threading: StyleThreading::Sequential,
                ..DocumentConfig::default()
            },
        );
        document.resolve(0.0);

        Ok(Self {
            spine_item_id: spine_item.id,
            href: spine_item.href,
            document,
            resource_failures,
        })
    }

    /// Spine item represented by this layout.
    pub fn spine_item_id(&self) -> &SpineItemId {
        &self.spine_item_id
    }

    /// Canonical content document URL.
    pub fn href(&self) -> &PublicationUrl {
        &self.href
    }

    /// Returns current root geometry and diagnostic counts.
    pub fn metrics(&self) -> LayoutMetrics {
        let layout = self.document.root_element().final_layout;
        LayoutMetrics {
            content_width: layout.size.width,
            content_height: layout.size.height,
            node_count: self.document.tree().len(),
            has_pending_critical_resources: self.document.has_pending_critical_resources(),
        }
    }

    /// Returns a selector's border-box after layout.
    pub fn element_rect(&self, selector: &str) -> Result<Option<LayoutRect>, RenderError> {
        let Some(node_id) = self
            .document
            .query_selector(selector)
            .map_err(|error| RenderError::InvalidSelector(format!("{error:?}")))?
        else {
            return Ok(None);
        };
        let node = self
            .document
            .get_node(node_id)
            .ok_or(RenderError::BackendInvariant(
                "query result node disappeared",
            ))?;
        let layout = node.final_layout;
        Ok(Some(LayoutRect {
            x: layout.location.x,
            y: layout.location.y,
            width: layout.size.width,
            height: layout.size.height,
        }))
    }

    /// Maps a CSS-pixel point to a shaped inline-root byte offset.
    pub fn hit_text(&self, x: f32, y: f32) -> Option<BackendTextHit> {
        self.document
            .find_text_position(x, y)
            .map(|(backend_node_id, utf8_byte_offset)| BackendTextHit {
                backend_node_id,
                utf8_byte_offset,
            })
    }

    /// Returns a stable snapshot of blocked or unavailable subresources.
    pub fn resource_failures(&self) -> Vec<ResourceFailure> {
        lock_unpoisoned(&self.resource_failures).clone()
    }

    /// Paints the current first viewport into an RGBA8 buffer through the Vello CPU backend.
    /// This is intended for deterministic diagnostics and visual regression capture, and does
    /// not require a GPU, window, or display server.
    pub fn render_offscreen_rgba(
        &mut self,
        viewport: LayoutViewport,
    ) -> Result<Vec<u8>, RenderError> {
        let physical_width = physical_dimension(viewport.width, viewport.scale_factor)?;
        let physical_height = physical_dimension(viewport.height, viewport.scale_factor)?;
        Ok(render_to_buffer::<VelloCpuImageRenderer, _>(
            |scene| {
                paint_scene(
                    scene,
                    self.document.as_mut(),
                    f64::from(viewport.scale_factor),
                    physical_width,
                    physical_height,
                    0,
                    0,
                );
            },
            physical_width,
            physical_height,
        ))
    }
}

struct PublicationNetProvider {
    publication: Arc<dyn Publication>,
    failures: Arc<Mutex<Vec<ResourceFailure>>>,
}

impl NetProvider for PublicationNetProvider {
    fn fetch(&self, _doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        let resolved_url = request.url.to_string();
        let result = self.resolve_request(&request);
        match result {
            Ok(bytes) => handler.bytes(resolved_url, Bytes::copy_from_slice(&bytes)),
            Err(reason) => {
                lock_unpoisoned(&self.failures).push(ResourceFailure {
                    url: resolved_url.clone(),
                    reason,
                });
                // NetHandler has no error callback. Supplying empty bytes lets Blitz clear its
                // pending-resource bookkeeping while still producing a deterministic fallback.
                handler.bytes(resolved_url, Bytes::new());
            }
        }
    }
}

impl PublicationNetProvider {
    fn resolve_request(&self, request: &Request) -> Result<Arc<[u8]>, ResourceFailureReason> {
        if request
            .signal
            .as_ref()
            .is_some_and(blitz_traits::net::AbortSignal::aborted)
        {
            return Err(ResourceFailureReason::Cancelled);
        }
        if request.method != Method::GET {
            return Err(ResourceFailureReason::UnsupportedMethod);
        }
        if request.url.scheme() != "epub" || request.url.host_str() != Some("publication") {
            return Err(ResourceFailureReason::BlockedExternalUrl);
        }
        let raw_path = request.url.path().trim_start_matches('/');
        let href = PublicationUrl::parse(raw_path)
            .map_err(|_| ResourceFailureReason::InvalidPublicationUrl)?;
        self.publication
            .resource(&href)
            .map(|resource| resource.bytes)
            .map_err(|_| ResourceFailureReason::ResourceUnavailable)
    }
}

fn publication_base_url(href: &PublicationUrl) -> Result<Url, RenderError> {
    let root = Url::parse("epub://publication/")?;
    root.join(href.path()).map_err(RenderError::from)
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn physical_dimension(logical: u32, scale_factor: f32) -> Result<u32, RenderError> {
    let physical = f64::from(logical) * f64::from(scale_factor);
    if !physical.is_finite() || physical > f64::from(u32::MAX) {
        return Err(RenderError::RenderTargetTooLarge);
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "finite positive value is bounded to u32::MAX immediately above"
    )]
    let rounded = physical.round() as u32;
    (rounded > 0)
        .then_some(rounded)
        .ok_or(RenderError::RenderTargetTooLarge)
}

/// Errors produced by the backend-isolated reflow adapter.
#[derive(Debug, Error)]
pub enum RenderError {
    /// Viewport dimensions or scale were invalid.
    #[error("layout viewport must have positive dimensions and finite positive scale")]
    InvalidViewport,
    /// Scaled render target cannot be represented safely.
    #[error("scaled render target dimensions are invalid or too large")]
    RenderTargetTooLarge,
    /// Requested spine index does not exist.
    #[error("spine index is out of bounds: {0}")]
    SpineIndexOutOfBounds(usize),
    /// Spine content is not currently supported by the reflow engine.
    #[error("unsupported reflow content type: {0}")]
    UnsupportedContentType(String),
    /// Content document was not valid UTF-8.
    #[error("content document encoding is not supported: {0}")]
    InvalidDocumentEncoding(String),
    /// CSS selector syntax was invalid.
    #[error("invalid selector: {0}")]
    InvalidSelector(String),
    /// An expected backend node or layout structure was missing.
    #[error("renderer backend invariant failed: {0}")]
    BackendInvariant(&'static str),
    /// Internal `epub://` base URL could not be constructed.
    #[error("renderer URL construction failed: {0}")]
    Url(#[from] url::ParseError),
    /// Publication resource access failed.
    #[error(transparent)]
    Publication(#[from] PublicationError),
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use rebook_publication::{
        Metadata, PublicationId, RenditionLayout, Resource, SpineItem, TocEntry,
    };

    use super::{
        ColorScheme, LayoutViewport, Publication, PublicationError, PublicationUrl, ReflowDocument,
        ResourceFailureReason,
    };

    #[test]
    fn lays_out_external_publication_css_without_network_access() {
        let publication = fake_publication(
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><head>
                <link rel="stylesheet" href="../Styles/book.css"/>
              </head><body><p id="target">你好，Rust 原生排版。</p></body></html>"#,
            [(
                "OPS/Styles/book.css",
                "#target { width: 321px; font-size: 32px; margin: 0; }",
            )],
        );
        let document = ReflowDocument::layout(
            publication,
            0,
            LayoutViewport::new(800, 600, 1.0).expect("valid viewport"),
        )
        .expect("layout succeeds");
        let target = document
            .element_rect("#target")
            .expect("valid selector")
            .expect("target exists");

        assert!((target.width - 321.0).abs() < f32::EPSILON);
        assert!(target.height > 0.0);
        assert!(document.metrics().node_count > 4);
        assert!(!document.metrics().has_pending_critical_resources);
        assert!(document.resource_failures().is_empty());
    }

    #[test]
    fn maps_a_point_in_chinese_text_to_a_backend_text_offset() {
        let publication = fake_publication(
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><body style="margin:0">
              <p id="target" style="font-size:32px; margin:0">你好，Rust。</p>
            </body></html>"#,
            [],
        );
        let document = ReflowDocument::layout(
            publication,
            0,
            LayoutViewport::new(800, 600, 1.0).expect("valid viewport"),
        )
        .expect("layout succeeds");
        let target = document
            .element_rect("#target")
            .expect("valid selector")
            .expect("target exists");
        let hit = document.hit_text(target.x + 4.0, target.y + target.height / 2.0);

        assert!(
            hit.is_some(),
            "text hit must resolve inside laid-out paragraph"
        );
    }

    #[test]
    fn blocks_http_subresources_and_records_a_diagnostic() {
        let publication = fake_publication(
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><head>
              <link rel="stylesheet" href="https://example.com/tracker.css"/>
            </head><body><p>offline</p></body></html>"#,
            [],
        );
        let mut viewport = LayoutViewport::new(800, 600, 1.0).expect("valid viewport");
        viewport.color_scheme = ColorScheme::Dark;
        let document = ReflowDocument::layout(publication, 0, viewport).expect("layout succeeds");

        assert_eq!(
            document.resource_failures()[0].reason,
            ResourceFailureReason::BlockedExternalUrl
        );
        assert!(!document.metrics().has_pending_critical_resources);
    }

    #[test]
    fn paints_a_non_empty_first_viewport_with_vello_cpu() {
        let publication = fake_publication(
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><body style="margin:0;background:#f7f2e8">
              <h1 style="color:#7a3f2b">Rust 原生首屏</h1>
            </body></html>"#,
            [],
        );
        let viewport = LayoutViewport::new(320, 200, 1.0).expect("valid viewport");
        let mut document = ReflowDocument::layout(publication, 0, viewport).expect("layout");
        let rgba = document
            .render_offscreen_rgba(viewport)
            .expect("offscreen paint");
        let distinct_colors = rgba
            .chunks_exact(4)
            .map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]])
            .collect::<std::collections::HashSet<_>>();

        assert_eq!(rgba.len(), 320 * 200 * 4);
        assert!(distinct_colors.len() > 8, "text must produce varied pixels");
        assert!(rgba.chunks_exact(4).any(|pixel| pixel[3] != 0));
    }

    fn fake_publication<const N: usize>(
        html: &str,
        extras: [(&str, &str); N],
    ) -> Arc<dyn Publication> {
        let chapter_href = PublicationUrl::parse("OPS/Text/chapter.xhtml").expect("valid URL");
        let mut resources =
            HashMap::from([(chapter_href.path().to_owned(), html.as_bytes().into())]);
        for (href, body) in extras {
            resources.insert(href.into(), body.as_bytes().into());
        }
        Arc::new(FakePublication {
            id: PublicationId::new("test-publication").expect("valid ID"),
            metadata: Metadata {
                title: "Test".into(),
                authors: Vec::new(),
                languages: vec!["zh-CN".into()],
                layout: RenditionLayout::Reflowable,
            },
            reading_order: vec![SpineItem {
                id: rebook_publication::SpineItemId::new("chapter").expect("valid spine ID"),
                href: chapter_href,
                media_type: "application/xhtml+xml".into(),
                linear: true,
                properties: Vec::new(),
            }],
            resources,
        })
    }

    struct FakePublication {
        id: PublicationId,
        metadata: Metadata,
        reading_order: Vec<SpineItem>,
        resources: HashMap<String, Arc<[u8]>>,
    }

    impl Publication for FakePublication {
        fn id(&self) -> &PublicationId {
            &self.id
        }

        fn metadata(&self) -> &Metadata {
            &self.metadata
        }

        fn reading_order(&self) -> &[SpineItem] {
            &self.reading_order
        }

        fn table_of_contents(&self) -> &[TocEntry] {
            &[]
        }

        fn resource(&self, href: &PublicationUrl) -> Result<Resource, PublicationError> {
            let bytes = self
                .resources
                .get(href.path())
                .cloned()
                .ok_or_else(|| PublicationError::ResourceNotFound(href.to_string()))?;
            let media_type = if std::path::Path::new(href.path())
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("css"))
            {
                "text/css"
            } else {
                "application/xhtml+xml"
            };
            Ok(Resource {
                href: href.clone(),
                media_type: media_type.into(),
                bytes,
            })
        }
    }
}
