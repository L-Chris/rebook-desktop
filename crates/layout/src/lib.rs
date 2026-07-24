//! Renderer-independent pagination for normalized reading IR.

use std::ops::Range;
use std::sync::Arc;

use image::ImageError;
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontStyle, FontWeight, Layout,
    LayoutContext, LineHeight, StyleProperty,
};
use rebook_publication::{
    Block, BookSource, ImageStyle, Inline, PublicationError, Rgba, Section, SourceRange,
    TextAlignment, TextBlock, TextBlockKind, TextStyle,
};
use thiserror::Error;

const COLUMN_GAP: f32 = 36.0;
const MIN_COLUMN_WIDTH: f32 = 360.0;
const MAX_COLUMN_WIDTH: f32 = 960.0;

/// Logical viewport in device-independent pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutViewport {
    pub width: u32,
    pub height: u32,
}

impl LayoutViewport {
    pub fn new(width: u32, height: u32) -> Result<Self, LayoutError> {
        if width == 0 || height == 0 {
            return Err(LayoutError::InvalidViewport);
        }
        Ok(Self { width, height })
    }
}

/// User-controlled values that invalidate pagination.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReaderStyle {
    pub font_size: f32,
    pub font_family: ReaderFontFamily,
    pub horizontal_margin: f32,
    pub vertical_margin: f32,
    pub spread: SpreadMode,
    pub foreground: Rgba,
    pub background: Rgba,
}

/// Semantic font family selected by the reader.
///
/// Native platforms resolve these through their installed system fonts, matching
/// the generic family model used by rebook's browser renderer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ReaderFontFamily {
    #[default]
    Serif,
    SansSerif,
    Monospace,
    MicrosoftYaHei,
    SimSun,
    KaiTi,
}

impl ReaderFontFamily {
    #[must_use]
    pub const fn css_stack(self) -> &'static str {
        match self {
            Self::Serif => "serif",
            Self::SansSerif => "sans-serif",
            Self::Monospace => "monospace",
            Self::MicrosoftYaHei => {
                "'Microsoft YaHei', 'Microsoft YaHei UI', 'PingFang SC', sans-serif"
            }
            Self::SimSun => "SimSun, 'Songti SC', 'Noto Serif CJK SC', serif",
            Self::KaiTi => "KaiTi, STKaiti, 'Noto Serif CJK SC', serif",
        }
    }
}

/// Maximum number of book pages shown in one viewport.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SpreadMode {
    /// Always paginate as one page per viewport.
    #[default]
    Single,
    /// Use a two-page spread when both columns can remain comfortably readable.
    Double,
}

impl SpreadMode {
    #[must_use]
    pub fn toggled(self) -> Self {
        match self {
            Self::Single => Self::Double,
            Self::Double => Self::Single,
        }
    }
}

impl Default for ReaderStyle {
    fn default() -> Self {
        Self {
            font_size: 16.0,
            font_family: ReaderFontFamily::default(),
            horizontal_margin: 44.0,
            vertical_margin: 44.0,
            spread: SpreadMode::Double,
            foreground: Rgba::BLACK,
            background: Rgba {
                red: 250,
                green: 248,
                blue: 243,
                alpha: 255,
            },
        }
    }
}

/// Brush carried through Parley without coupling layout to a paint backend.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextBrush {
    pub color: Rgba,
    pub underline: bool,
}

/// One immutable paginated section.
pub struct SectionLayout {
    pub pages: Vec<PageLayout>,
}

/// Renderer-independent display data for one page.
pub struct PageLayout {
    pub viewport: LayoutViewport,
    pub background: Rgba,
    pub items: Vec<PageItem>,
}

/// Positioned page content.
pub enum PageItem {
    Text(TextPlacement),
    Image(ImagePlacement),
    Separator(SeparatorPlacement),
}

/// A line slice from a shaped paragraph.
pub struct TextPlacement {
    pub layout: Arc<Layout<TextBrush>>,
    pub lines: Range<usize>,
    pub origin_x: f32,
    pub origin_y: f32,
    pub source: Option<SourceRange>,
}

/// Decoded RGBA image ready for upload by the renderer.
#[derive(Clone)]
pub struct RasterImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Arc<[u8]>,
}

/// Positioned raster image.
pub struct ImagePlacement {
    pub image: RasterImage,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub source: Option<SourceRange>,
}

/// Positioned thematic break.
pub struct SeparatorPlacement {
    pub x: f32,
    pub y: f32,
    pub width: f32,
}

/// Stateful layout engine. Font discovery and shaping caches live for the reader session.
pub struct LayoutEngine {
    font_context: FontContext,
    layout_context: LayoutContext<TextBrush>,
}

impl Default for LayoutEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl LayoutEngine {
    pub fn new() -> Self {
        Self {
            font_context: FontContext::new(),
            layout_context: LayoutContext::new(),
        }
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "reader viewport dimensions are bounded far below f32's exact integer range"
    )]
    pub fn layout_section(
        &mut self,
        source: &dyn BookSource,
        section: &Section,
        viewport: LayoutViewport,
        reader_style: ReaderStyle,
    ) -> Result<SectionLayout, LayoutError> {
        let page_width = viewport.width as f32;
        let page_height = viewport.height as f32;
        let geometry = resolve_page_geometry(page_width, page_height, reader_style);
        let content_width = geometry.width;

        let mut paginator = Paginator::new(viewport, reader_style.background, geometry);

        for block in &section.blocks {
            match block {
                Block::Text(block) => {
                    let prepared = self.shape_text(block, reader_style, content_width);
                    paginator.push_text(&prepared, block)?;
                }
                Block::Image(image) => {
                    let resource = source.resource(&image.href)?;
                    let decoded = image::load_from_memory(&resource.bytes)?.to_rgba8();
                    let raster = RasterImage {
                        width: decoded.width(),
                        height: decoded.height(),
                        pixels: decoded.into_raw().into(),
                    };
                    paginator.push_image(raster, image.style, image.source.clone());
                }
                Block::Separator => paginator.push_separator(),
                Block::PageBreak => paginator.force_page(),
            }
        }

        Ok(SectionLayout {
            pages: paginator.finish(),
        })
    }

    fn shape_text(
        &mut self,
        block: &TextBlock,
        reader_style: ReaderStyle,
        content_width: f32,
    ) -> Arc<Layout<TextBrush>> {
        let (text, spans) = flatten_text(block, reader_style.foreground);
        let available_width = (content_width - block.style.indent).max(40.0);
        let mut builder =
            self.layout_context
                .ranged_builder(&mut self.font_context, &text, 1.0, false);
        builder.push_default(StyleProperty::FontFamily(FontFamily::from(
            reader_style.font_family.css_stack(),
        )));
        builder.push_default(StyleProperty::FontSize(reader_style.font_size));
        builder.push_default(StyleProperty::LineHeight(LineHeight::FontSizeRelative(
            block.style.line_height,
        )));
        builder.push_default(StyleProperty::Brush(TextBrush {
            color: reader_style.foreground,
            underline: false,
        }));

        for span in spans {
            let size = reader_style.font_size * span.style.size_scale.clamp(0.5, 3.0);
            builder.push(StyleProperty::FontSize(size), span.range.clone());
            builder.push(
                StyleProperty::Brush(TextBrush {
                    color: span.style.color,
                    underline: span.style.underline,
                }),
                span.range.clone(),
            );
            if span.style.bold {
                builder.push(
                    StyleProperty::FontWeight(FontWeight::BOLD),
                    span.range.clone(),
                );
            }
            if span.style.italic {
                builder.push(
                    StyleProperty::FontStyle(FontStyle::Italic),
                    span.range.clone(),
                );
            }
            if span.style.underline {
                builder.push(StyleProperty::Underline(true), span.range);
            }
        }

        let mut layout = builder.build(&text);
        layout.break_all_lines(Some(available_width));
        let alignment = match block.style.align {
            TextAlignment::Start => Alignment::Start,
            TextAlignment::Center => Alignment::Center,
            TextAlignment::End => Alignment::End,
            TextAlignment::Justify => Alignment::Justify,
        };
        layout.align(alignment, AlignmentOptions::default());
        Arc::new(layout)
    }
}

fn resolve_page_geometry(
    page_width: f32,
    page_height: f32,
    reader_style: ReaderStyle,
) -> PageGeometry {
    let horizontal_margin = reader_style
        .horizontal_margin
        .min(page_width.mul_add(0.2, -8.0).max(20.0));
    let vertical_margin = reader_style
        .vertical_margin
        .min(page_height.mul_add(0.2, -8.0).max(20.0));
    let double_available = page_width - horizontal_margin * 2.0 - COLUMN_GAP;
    let column_count = if reader_style.spread == SpreadMode::Double
        && double_available >= MIN_COLUMN_WIDTH * 2.0
    {
        2
    } else {
        1
    };
    let column_gap = if column_count == 2 { COLUMN_GAP } else { 0.0 };
    let column_divisor = if column_count == 2 { 2.0 } else { 1.0 };
    let content_width = ((page_width - horizontal_margin * 2.0 - column_gap) / column_divisor)
        .clamp(80.0, MAX_COLUMN_WIDTH);
    let spread_width = content_width * column_divisor + column_gap;
    let content_left = ((page_width - spread_width) / 2.0).max(horizontal_margin);
    let content_bottom = (page_height - vertical_margin).max(vertical_margin + 40.0);

    PageGeometry {
        left: content_left,
        top: vertical_margin,
        width: content_width,
        bottom: content_bottom,
        column_count,
        column_gap,
    }
}

struct StyledRange {
    range: Range<usize>,
    style: TextStyle,
}

fn flatten_text(block: &TextBlock, fallback_color: Rgba) -> (String, Vec<StyledRange>) {
    let mut text = String::new();
    let mut spans = Vec::new();
    let prefix = match block.kind {
        TextBlockKind::ListItem {
            ordered: true,
            ordinal,
        } => format!("{ordinal}. "),
        TextBlockKind::ListItem { ordered: false, .. } => "• ".to_owned(),
        _ => String::new(),
    };
    if !prefix.is_empty() {
        let start = text.len();
        text.push_str(&prefix);
        spans.push(StyledRange {
            range: start..text.len(),
            style: TextStyle {
                color: fallback_color,
                ..TextStyle::default()
            },
        });
    }

    for inline in &block.content {
        match inline {
            Inline::Text(run) => {
                let start = text.len();
                text.push_str(&run.text);
                let mut style = run.style;
                if style.color == Rgba::BLACK {
                    style.color = fallback_color;
                }
                spans.push(StyledRange {
                    range: start..text.len(),
                    style,
                });
            }
            Inline::Break => text.push('\n'),
        }
    }
    (text, spans)
}

struct Paginator {
    viewport: LayoutViewport,
    background: Rgba,
    left: f32,
    top: f32,
    width: f32,
    bottom: f32,
    column_count: usize,
    column_gap: f32,
    column_index: usize,
    column_has_content: bool,
    cursor_y: f32,
    pages: Vec<PageLayout>,
    items: Vec<PageItem>,
}

#[derive(Clone, Copy)]
struct PageGeometry {
    left: f32,
    top: f32,
    width: f32,
    bottom: f32,
    column_count: usize,
    column_gap: f32,
}

impl Paginator {
    fn new(viewport: LayoutViewport, background: Rgba, geometry: PageGeometry) -> Self {
        Self {
            viewport,
            background,
            left: geometry.left,
            top: geometry.top,
            width: geometry.width,
            bottom: geometry.bottom,
            column_count: geometry.column_count,
            column_gap: geometry.column_gap,
            column_index: 0,
            column_has_content: false,
            cursor_y: geometry.top,
            pages: Vec::new(),
            items: Vec::new(),
        }
    }

    fn push_text(
        &mut self,
        prepared: &Arc<Layout<TextBrush>>,
        block: &TextBlock,
    ) -> Result<(), LayoutError> {
        self.add_spacing(block.style.margin_before);
        let mut line_start = 0;
        while line_start < prepared.len() {
            let first = prepared.get(line_start).ok_or(LayoutError::InvalidLayout)?;
            let first_top = first.metrics().block_min_coord;
            let mut line_end = line_start;
            let mut slice_height = 0.0;
            while line_end < prepared.len() {
                let line = prepared.get(line_end).ok_or(LayoutError::InvalidLayout)?;
                let candidate_height = line.metrics().block_max_coord - first_top;
                let remaining = self.bottom - self.cursor_y;
                if candidate_height > remaining && line_end > line_start {
                    break;
                }
                if candidate_height > remaining && self.column_has_content {
                    self.advance_column();
                    break;
                }
                slice_height = candidate_height.max(line.metrics().line_height);
                line_end += 1;
            }
            if line_end == line_start {
                continue;
            }
            self.items.push(PageItem::Text(TextPlacement {
                layout: Arc::clone(prepared),
                lines: line_start..line_end,
                origin_x: self.column_left() + block.style.indent,
                origin_y: self.cursor_y - first_top,
                source: block.source.clone(),
            }));
            self.column_has_content = true;
            self.cursor_y += slice_height;
            line_start = line_end;
            if line_start < prepared.len() {
                self.advance_column();
            }
        }
        self.add_spacing(block.style.margin_after);
        Ok(())
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "decoded image dimensions are bounded by publication resource limits"
    )]
    fn push_image(&mut self, image: RasterImage, style: ImageStyle, source: Option<SourceRange>) {
        let intrinsic_width = image.width.max(1) as f32;
        let intrinsic_height = image.height.max(1) as f32;
        let aspect_ratio = intrinsic_width / intrinsic_height;
        let content_height = self.bottom - self.top;
        let requested_height = style.height.map(|height| height.resolve(content_height));
        let requested_width = style
            .width
            .map(|width| width.resolve(self.width))
            .or_else(|| requested_height.map(|height| height * aspect_ratio))
            .unwrap_or(intrinsic_width)
            .max(1.0);
        let requested_height = requested_height
            .unwrap_or(requested_width / aspect_ratio)
            .max(1.0);
        let max_width = style
            .max_width
            .map_or(self.width, |width| width.resolve(self.width))
            .clamp(1.0, self.width);
        let max_height = style
            .max_height
            .map_or(content_height, |height| height.resolve(content_height))
            .clamp(1.0, content_height);
        let scale = (max_width / requested_width)
            .min(max_height / requested_height)
            .min(1.0);
        let width = requested_width * scale;
        let height = requested_height * scale;
        if self.cursor_y + height > self.bottom && self.column_has_content {
            self.advance_column();
        }
        let x = self.column_left() + (self.width - width) / 2.0;
        self.items.push(PageItem::Image(ImagePlacement {
            image,
            x,
            y: self.cursor_y,
            width,
            height,
            source,
        }));
        self.column_has_content = true;
        self.cursor_y += height + 14.0;
    }

    fn push_separator(&mut self) {
        self.add_spacing(12.0);
        if self.cursor_y + 1.0 > self.bottom && self.column_has_content {
            self.advance_column();
        }
        self.items.push(PageItem::Separator(SeparatorPlacement {
            x: self.column_left() + self.width * 0.25,
            y: self.cursor_y,
            width: self.width * 0.5,
        }));
        self.column_has_content = true;
        self.cursor_y += 13.0;
    }

    fn add_spacing(&mut self, amount: f32) {
        let amount = amount.max(0.0);
        if self.cursor_y + amount > self.bottom && self.column_has_content {
            self.advance_column();
        } else {
            self.cursor_y += amount;
        }
    }

    fn force_page(&mut self) {
        if self.column_has_content || !self.items.is_empty() {
            self.advance_column();
        }
    }

    fn column_left(&self) -> f32 {
        let offset = if self.column_index == 0 {
            0.0
        } else {
            self.width + self.column_gap
        };
        self.left + offset
    }

    fn advance_column(&mut self) {
        if self.column_index + 1 < self.column_count {
            self.column_index += 1;
            self.column_has_content = false;
            self.cursor_y = self.top;
        } else {
            self.commit_page();
        }
    }

    fn commit_page(&mut self) {
        if self.items.is_empty() {
            self.cursor_y = self.top;
            return;
        }
        self.pages.push(PageLayout {
            viewport: self.viewport,
            background: self.background,
            items: std::mem::take(&mut self.items),
        });
        self.column_index = 0;
        self.column_has_content = false;
        self.cursor_y = self.top;
    }

    fn finish(mut self) -> Vec<PageLayout> {
        self.commit_page();
        if self.pages.is_empty() {
            self.pages.push(PageLayout {
                viewport: self.viewport,
                background: self.background,
                items: Vec::new(),
            });
        }
        self.pages
    }
}

/// Native layout errors.
#[derive(Debug, Error)]
pub enum LayoutError {
    #[error("viewport dimensions must be positive")]
    InvalidViewport,
    #[error("text layout produced inconsistent line metrics")]
    InvalidLayout,
    #[error(transparent)]
    Publication(#[from] PublicationError),
    #[error("image decode failed: {0}")]
    Image(#[from] ImageError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_font_families_expose_generic_and_named_fallback_stacks() {
        assert_eq!(ReaderStyle::default().font_family, ReaderFontFamily::Serif);
        assert_eq!(ReaderFontFamily::Serif.css_stack(), "serif");
        assert_eq!(ReaderFontFamily::SansSerif.css_stack(), "sans-serif");
        assert_eq!(ReaderFontFamily::Monospace.css_stack(), "monospace");
        assert!(
            ReaderFontFamily::MicrosoftYaHei
                .css_stack()
                .contains("Microsoft YaHei")
        );
        assert!(ReaderFontFamily::SimSun.css_stack().ends_with("serif"));
        assert!(ReaderFontFamily::KaiTi.css_stack().ends_with("serif"));
    }

    #[test]
    fn wide_viewports_cap_and_center_each_reading_column() {
        let viewport_width = 3_000.0;
        let page_height = 900.0;
        let single = resolve_page_geometry(
            viewport_width,
            page_height,
            ReaderStyle {
                spread: SpreadMode::Single,
                ..ReaderStyle::default()
            },
        );
        assert_eq!(single.column_count, 1);
        assert!((single.width - MAX_COLUMN_WIDTH).abs() < f32::EPSILON);
        assert!((single.left - (viewport_width - MAX_COLUMN_WIDTH) / 2.0).abs() < f32::EPSILON);

        let double = resolve_page_geometry(
            viewport_width,
            page_height,
            ReaderStyle {
                spread: SpreadMode::Double,
                ..ReaderStyle::default()
            },
        );
        let spread_width = MAX_COLUMN_WIDTH * 2.0 + COLUMN_GAP;
        assert_eq!(double.column_count, 2);
        assert!((double.width - MAX_COLUMN_WIDTH).abs() < f32::EPSILON);
        assert!((double.left - (viewport_width - spread_width) / 2.0).abs() < f32::EPSILON);
    }
    use rebook_publication::{
        Book, ImageLength, Metadata, PublicationId, PublicationUrl, Resource, SpineItemId, TocEntry,
    };

    struct EmptySource {
        book: Book,
    }

    impl BookSource for EmptySource {
        fn book(&self) -> &Book {
            &self.book
        }

        fn parse_section(&self, _index: usize) -> Result<Section, PublicationError> {
            unreachable!()
        }

        fn resource(&self, href: &PublicationUrl) -> Result<Resource, PublicationError> {
            Err(PublicationError::ResourceNotFound(href.to_string()))
        }
    }

    #[test]
    fn long_paragraph_is_split_into_multiple_pages() {
        let source = EmptySource {
            book: Book {
                id: PublicationId::new("test").unwrap(),
                metadata: Metadata::default(),
                cover: None,
                sections: Vec::new(),
                table_of_contents: Vec::<TocEntry>::new(),
            },
        };
        let section = Section {
            id: SpineItemId::new("chapter").unwrap(),
            href: PublicationUrl::parse("chapter.xhtml").unwrap(),
            blocks: vec![Block::Text(TextBlock {
                kind: TextBlockKind::Paragraph,
                content: vec![Inline::Text(rebook_publication::TextRun {
                    text: "这是用于验证分页的数据。".repeat(500),
                    style: TextStyle::default(),
                    link: None,
                })],
                style: rebook_publication::BlockStyle::default(),
                source: None,
            })],
        };
        let layout = LayoutEngine::new()
            .layout_section(
                &source,
                &section,
                LayoutViewport::new(600, 400).unwrap(),
                ReaderStyle::default(),
            )
            .unwrap();
        assert!(layout.pages.len() > 1);
    }

    #[test]
    fn double_spread_places_two_columns_in_one_viewport() {
        let source = EmptySource {
            book: Book {
                id: PublicationId::new("test").unwrap(),
                metadata: Metadata::default(),
                cover: None,
                sections: Vec::new(),
                table_of_contents: Vec::new(),
            },
        };
        let section = Section {
            id: SpineItemId::new("chapter").unwrap(),
            href: PublicationUrl::parse("chapter.xhtml").unwrap(),
            blocks: vec![Block::Text(TextBlock {
                kind: TextBlockKind::Paragraph,
                content: vec![Inline::Text(rebook_publication::TextRun {
                    text: "双栏分页应当把连续内容放进同一屏幕的左右页面。".repeat(500),
                    style: TextStyle::default(),
                    link: None,
                })],
                style: rebook_publication::BlockStyle::default(),
                source: None,
            })],
        };
        let viewport = LayoutViewport::new(900, 700).unwrap();
        let single = LayoutEngine::new()
            .layout_section(&source, &section, viewport, ReaderStyle::default())
            .unwrap();
        let double = LayoutEngine::new()
            .layout_section(
                &source,
                &section,
                viewport,
                ReaderStyle {
                    spread: SpreadMode::Double,
                    ..ReaderStyle::default()
                },
            )
            .unwrap();

        assert!(!single.pages.is_empty());
        let text_origins = double.pages[0]
            .items
            .iter()
            .filter_map(|item| match item {
                PageItem::Text(text) => Some(text.origin_x),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(text_origins.iter().any(|origin| *origin < 450.0));
        assert!(text_origins.iter().any(|origin| *origin > 450.0));
    }

    #[test]
    fn image_css_dimensions_are_resolved_and_aspect_ratio_is_preserved() {
        let viewport = LayoutViewport::new(400, 500).unwrap();
        let mut paginator = Paginator::new(
            viewport,
            Rgba::BLACK,
            PageGeometry {
                left: 0.0,
                top: 0.0,
                width: 400.0,
                bottom: 500.0,
                column_count: 1,
                column_gap: 0.0,
            },
        );
        paginator.push_image(
            RasterImage {
                width: 800,
                height: 600,
                pixels: Vec::new().into(),
            },
            ImageStyle {
                width: Some(ImageLength::Fraction(0.8)),
                max_width: Some(ImageLength::Pixels(250.0)),
                ..ImageStyle::default()
            },
            None,
        );

        let pages = paginator.finish();
        let PageItem::Image(image) = &pages[0].items[0] else {
            panic!("expected an image placement");
        };
        assert!((image.width - 250.0).abs() < 0.001);
        assert!((image.height - 187.5).abs() < 0.001);
        assert!((image.x - 75.0).abs() < 0.001);
    }
}
