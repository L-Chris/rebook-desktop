//! Renderer-independent pagination for normalized reading IR.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;

use image::ImageError;
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontStyle, FontWeight, Layout,
    LayoutContext, LineHeight, StyleProperty,
};
use rebook_publication::{
    Block, BookSource, FixedPageTextLayer, FixedPageTextRect, ImageStyle, Inline, PublicationError,
    PublicationUrl, RenditionLayout, Rgba, Section, SourceRange, TextAlignment, TextBlock,
    TextBlockKind, TextRun, TextStyle,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const DEFAULT_COLUMN_GAP: f32 = 36.0;
const IMAGE_BLOCK_GAP: f32 = 14.0;
const MIN_COLUMN_WIDTH: f32 = 360.0;
const MAX_COLUMN_WIDTH: f32 = 960.0;
const DEFAULT_TOP_MARGIN: f32 = 0.0;
const DEFAULT_BOTTOM_MARGIN: f32 = 24.0;

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
#[derive(Debug, Clone, PartialEq)]
pub struct ReaderStyle {
    pub typography: ReaderTypography,
    pub horizontal_margin: f32,
    pub top_margin: f32,
    pub bottom_margin: f32,
    pub column_gap: f32,
    pub spread: SpreadMode,
    pub foreground: Rgba,
    pub background: Rgba,
}

/// Generic family used for ordinary reading text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReaderDefaultFont {
    #[default]
    Serif,
    SansSerif,
}

/// Readest-compatible native typography preferences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReaderTypography {
    pub default_font: ReaderDefaultFont,
    pub default_cjk_font: String,
    pub serif_font: String,
    pub sans_serif_font: String,
    pub monospace_font: String,
    pub font_size: f32,
    pub minimum_font_size: f32,
    pub font_weight: u16,
}

impl ReaderTypography {
    /// Repairs persisted or externally supplied settings before layout uses them.
    pub fn normalize(&mut self) {
        let defaults = Self::default();
        normalize_family(&mut self.default_cjk_font, &defaults.default_cjk_font);
        normalize_family(&mut self.serif_font, &defaults.serif_font);
        normalize_family(&mut self.sans_serif_font, &defaults.sans_serif_font);
        normalize_family(&mut self.monospace_font, &defaults.monospace_font);
        self.minimum_font_size = finite_clamp(self.minimum_font_size, 1.0, 120.0, 8.0);
        self.font_size = finite_clamp(self.font_size, self.minimum_font_size, 120.0, 16.0);
        self.font_weight = self.font_weight.clamp(100, 900).div_ceil(100) * 100;
    }

    #[must_use]
    pub fn default_stack(&self) -> String {
        match self.default_font {
            ReaderDefaultFont::Serif => self.serif_stack(),
            ReaderDefaultFont::SansSerif => self.sans_serif_stack(),
        }
    }

    #[must_use]
    pub fn serif_stack(&self) -> String {
        font_stack(
            [
                self.serif_font.as_str(),
                self.default_cjk_font.as_str(),
                "LXGW WenKai GB Screen",
                "LXGW WenKai",
                "Noto Serif SC",
                "Source Han Serif SC",
                "Songti SC",
                "SimSun",
                "Georgia",
                "Times New Roman",
            ],
            "serif",
        )
    }

    #[must_use]
    pub fn sans_serif_stack(&self) -> String {
        font_stack(
            [
                self.sans_serif_font.as_str(),
                self.default_cjk_font.as_str(),
                "LXGW WenKai GB Screen",
                "LXGW WenKai",
                "Noto Sans SC",
                "Source Han Sans SC",
                "PingFang SC",
                "Microsoft YaHei",
                "Roboto",
                "Arial",
            ],
            "sans-serif",
        )
    }

    #[must_use]
    pub fn monospace_stack(&self) -> String {
        font_stack(
            [
                self.monospace_font.as_str(),
                "Fira Code",
                "Consolas",
                self.default_cjk_font.as_str(),
                "LXGW WenKai GB Screen",
                "LXGW WenKai",
                "SFMono-Regular",
                "Menlo",
                "Courier New",
            ],
            "monospace",
        )
    }
}

impl Default for ReaderTypography {
    fn default() -> Self {
        Self {
            default_font: ReaderDefaultFont::Serif,
            default_cjk_font: "LXGW WenKai GB Screen".into(),
            serif_font: "Bitter".into(),
            sans_serif_font: "Roboto".into(),
            monospace_font: "Consolas".into(),
            font_size: 16.0,
            minimum_font_size: 8.0,
            font_weight: 400,
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
    /// Paginate as single pages and present the active section as a vertical flow.
    Scroll,
}

impl SpreadMode {
    #[must_use]
    pub fn toggled(self) -> Self {
        match self {
            Self::Single => Self::Double,
            Self::Double => Self::Scroll,
            Self::Scroll => Self::Single,
        }
    }
}

impl Default for ReaderStyle {
    fn default() -> Self {
        Self {
            typography: ReaderTypography::default(),
            horizontal_margin: 44.0,
            top_margin: DEFAULT_TOP_MARGIN,
            bottom_margin: DEFAULT_BOTTOM_MARGIN,
            column_gap: DEFAULT_COLUMN_GAP,
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

fn normalize_family(value: &mut String, fallback: &str) {
    *value = value.trim().to_owned();
    if value.is_empty() {
        value.push_str(fallback);
    }
}

fn finite_clamp(value: f32, minimum: f32, maximum: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(minimum, maximum)
    } else {
        fallback.clamp(minimum, maximum)
    }
}

fn font_stack<'a>(families: impl IntoIterator<Item = &'a str>, generic: &str) -> String {
    let mut seen = HashSet::new();
    let mut stack = families
        .into_iter()
        .map(str::trim)
        .filter(|family| !family.is_empty())
        .filter(|family| seen.insert(family.to_ascii_lowercase()))
        .map(quote_font_family)
        .collect::<Vec<_>>();
    stack.push(generic.to_owned());
    stack.join(", ")
}

fn quote_font_family(family: &str) -> String {
    format!("\"{}\"", family.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Brush carried through Parley without coupling layout to a paint backend.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextBrush {
    pub color: Rgba,
    pub underline: bool,
}

/// Shared font bytes registered in both the native reader and the Xilem UI.
pub type ReaderFontBlob = parley::fontique::Blob<u8>;

/// One immutable paginated section.
pub struct SectionLayout {
    pub pages: Vec<PageLayout>,
    pub visible_pages: usize,
    pub continuation_offset_x: f32,
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
    /// UTF-8 text shaped by Parley. Kept alongside the layout so retained
    /// renderers can map pointer hit tests back to durable source offsets.
    pub text: Arc<str>,
    /// Byte length of synthetic display text (for example a list marker) that
    /// precedes the authored source text.
    pub source_text_start: usize,
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
    pub text_layer: Option<FixedPageTextLayer>,
    pub replacement: Option<FixedPageTextReplacementPlacement>,
}

/// Translated text repainted inside the original fixed-layout page image.
pub struct FixedPageTextReplacementPlacement {
    pub segments: Vec<FixedPageTextReplacementSegmentPlacement>,
}

/// One shaped translated fragment inside a fixed-page replacement overlay.
pub struct FixedPageTextReplacementSegmentPlacement {
    pub rect: FixedPageTextRect,
    pub text: TextPlacement,
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

    pub fn with_fonts(fonts: impl IntoIterator<Item = ReaderFontBlob>) -> Self {
        let mut engine = Self::new();
        for font in fonts {
            engine.font_context.collection.register_fonts(font, None);
        }
        engine
    }

    pub fn available_font_families(&mut self) -> Vec<String> {
        let mut families = self
            .font_context
            .collection
            .family_names()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        families.sort_by_key(|family| family.to_lowercase());
        families.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
        families
    }

    pub fn layout_section(
        &mut self,
        source: &dyn BookSource,
        section: &Section,
        viewport: LayoutViewport,
        reader_style: &ReaderStyle,
    ) -> Result<SectionLayout, LayoutError> {
        self.layout_blocks(source, &section.blocks, viewport, reader_style)
    }

    /// Lays out one viewport-independent slice of a reflowable section. The
    /// reader uses this entry point for bounded fragment compilation without
    /// manufacturing synthetic authored sections.
    pub fn layout_blocks(
        &mut self,
        source: &dyn BookSource,
        blocks: &[Block],
        viewport: LayoutViewport,
        reader_style: &ReaderStyle,
    ) -> Result<SectionLayout, LayoutError> {
        self.layout_fragments(source, &[blocks], viewport, reader_style)
    }

    /// Continuously paginates several stable content fragments as one bounded
    /// layout segment. Fragment boundaries do not commit the partial page; the
    /// caller controls random-access cost by choosing the segment size.
    #[allow(
        clippy::cast_precision_loss,
        reason = "reader viewport dimensions are bounded far below f32's exact integer range"
    )]
    pub fn layout_fragments(
        &mut self,
        source: &dyn BookSource,
        fragments: &[&[Block]],
        viewport: LayoutViewport,
        reader_style: &ReaderStyle,
    ) -> Result<SectionLayout, LayoutError> {
        let page_width = viewport.width as f32;
        let page_height = viewport.height as f32;
        let geometry = resolve_page_geometry(page_width, page_height, reader_style);
        let content_width = geometry.width;
        let visible_pages = geometry.visible_pages;
        let continuation_offset_x = geometry.continuation_offset_x;

        let center_standalone_image = source.book().metadata.layout
            == RenditionLayout::PrePaginated
            || fragments_are_standalone_cover(fragments, source.book().cover.as_ref());
        let mut paginator = Paginator::new(
            viewport,
            reader_style.background,
            geometry,
            center_standalone_image,
        );

        for blocks in fragments {
            for block in *blocks {
                match block {
                    Block::Text(block) => {
                        let prepared = self.shape_text(block, reader_style, content_width);
                        paginator.push_text(&prepared, block)?;
                    }
                    Block::Image(image) => {
                        let raster = if let Some(raster) = source.raster_resource(&image.href)? {
                            RasterImage {
                                width: raster.width,
                                height: raster.height,
                                pixels: raster.pixels,
                            }
                        } else {
                            let resource = source.resource(&image.href)?;
                            let decoded = image::load_from_memory(&resource.bytes)?.to_rgba8();
                            RasterImage {
                                width: decoded.width(),
                                height: decoded.height(),
                                pixels: decoded.into_raw().into(),
                            }
                        };
                        let replacements = paginator.push_image(
                            raster,
                            image.style,
                            image.source.clone(),
                            image.text_layer.clone(),
                        );
                        for replacement in replacements {
                            let prepared =
                                self.shape_fixed_page_replacement(&replacement, reader_style);
                            paginator.push_fixed_page_replacement(&prepared, replacement)?;
                        }
                    }
                    Block::Separator => paginator.push_separator(),
                    Block::PageBreak => paginator.force_page(),
                }
            }
        }

        Ok(SectionLayout {
            pages: paginator.finish(),
            visible_pages,
            continuation_offset_x,
        })
    }

    fn shape_text(
        &mut self,
        block: &TextBlock,
        reader_style: &ReaderStyle,
        content_width: f32,
    ) -> PreparedText {
        self.shape_text_with_min_width(block, reader_style, content_width, 40.0)
    }

    fn shape_text_with_min_width(
        &mut self,
        block: &TextBlock,
        reader_style: &ReaderStyle,
        content_width: f32,
        minimum_width: f32,
    ) -> PreparedText {
        let (text, spans, source_text_start) = flatten_text(block, reader_style.foreground);
        let available_width = (content_width - block.style.indent).max(minimum_width);
        let typography = &reader_style.typography;
        let font_stack = if block.kind == TextBlockKind::Preformatted {
            typography.monospace_stack()
        } else {
            typography.default_stack()
        };
        let mut builder =
            self.layout_context
                .ranged_builder(&mut self.font_context, &text, 1.0, false);
        builder.push_default(StyleProperty::FontFamily(FontFamily::from(
            font_stack.as_str(),
        )));
        builder.push_default(StyleProperty::FontSize(typography.font_size));
        builder.push_default(StyleProperty::FontWeight(FontWeight::new(f32::from(
            typography.font_weight,
        ))));
        builder.push_default(StyleProperty::LineHeight(LineHeight::FontSizeRelative(
            block.style.line_height,
        )));
        builder.push_default(StyleProperty::Brush(TextBrush {
            color: reader_style.foreground,
            underline: false,
        }));

        for span in spans {
            let size = (typography.font_size * span.style.size_scale.clamp(0.5, 3.0))
                .max(typography.minimum_font_size);
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
                    StyleProperty::FontWeight(FontWeight::new(
                        f32::from(typography.font_weight).max(FontWeight::BOLD.value()),
                    )),
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
        PreparedText {
            layout: Arc::new(layout),
            text: text.into(),
            source_text_start,
        }
    }

    fn shape_fixed_page_replacement(
        &mut self,
        request: &FixedPageReplacementRequest,
        reader_style: &ReaderStyle,
    ) -> PreparedText {
        let block = fixed_page_replacement_block(&request.text, request.source.clone());
        let mut style = reader_style.clone();
        style.typography.font_size = style.typography.font_size.min(14.0);
        style.typography.minimum_font_size = 5.0;
        let available_width = (request.rect.width - 3.0).max(2.0);
        let available_height = (request.rect.height - 3.0).max(2.0);

        loop {
            let prepared = self.shape_text_with_min_width(&block, &style, available_width, 2.0);
            let height = prepared_text_height(&prepared);
            if height <= available_height || style.typography.font_size <= 5.0 {
                return prepared;
            }
            let next_size = (style.typography.font_size * available_height / height)
                .clamp(5.0, style.typography.font_size - 0.5);
            style.typography.font_size = next_size;
        }
    }
}

fn fragments_are_standalone_cover(fragments: &[&[Block]], cover: Option<&PublicationUrl>) -> bool {
    let Some(cover) = cover else {
        return false;
    };
    let mut visible_blocks = fragments
        .iter()
        .flat_map(|blocks| blocks.iter())
        .filter(|block| !matches!(block, Block::PageBreak));
    matches!(visible_blocks.next(), Some(Block::Image(image)) if &image.href == cover)
        && visible_blocks.next().is_none()
}

fn resolve_page_geometry(
    page_width: f32,
    page_height: f32,
    reader_style: &ReaderStyle,
) -> PageGeometry {
    let (content_left, content_width, column_count, continuation_offset_x) =
        resolve_horizontal_page_geometry(page_width, reader_style);
    let max_vertical_margin = page_height.mul_add(0.2, -8.0).max(20.0);
    let top_margin = reader_style.top_margin.min(max_vertical_margin);
    let bottom_margin = reader_style.bottom_margin.min(max_vertical_margin);
    let content_bottom = (page_height - bottom_margin).max(top_margin + 40.0);

    PageGeometry {
        left: content_left,
        top: top_margin,
        width: content_width,
        bottom: content_bottom,
        visible_pages: column_count,
        continuation_offset_x,
    }
}

fn resolve_horizontal_page_geometry(
    page_width: f32,
    reader_style: &ReaderStyle,
) -> (f32, f32, usize, f32) {
    let horizontal_margin = reader_style
        .horizontal_margin
        .min(page_width.mul_add(0.2, -8.0).max(20.0));
    let configured_column_gap = reader_style.column_gap.max(0.0);
    let double_available = page_width - horizontal_margin * 2.0 - configured_column_gap;
    let column_count = if reader_style.spread == SpreadMode::Double
        && double_available >= MIN_COLUMN_WIDTH * 2.0
    {
        2
    } else {
        1
    };
    let column_gap = if column_count == 2 {
        configured_column_gap
    } else {
        0.0
    };
    let column_divisor = if column_count == 2 { 2.0 } else { 1.0 };
    let content_width = ((page_width - horizontal_margin * 2.0 - column_gap) / column_divisor)
        .clamp(80.0, MAX_COLUMN_WIDTH);
    let spread_width = content_width * column_divisor + column_gap;
    let content_left = ((page_width - spread_width) / 2.0).max(horizontal_margin);
    (
        content_left,
        content_width,
        column_count,
        content_width + column_gap,
    )
}

/// Returns the horizontal start of the reading content for a viewport.
///
/// Reader chrome uses this to align its title with the exact same centered
/// single- or double-column geometry used by pagination.
pub fn reading_content_left(page_width: f32, reader_style: &ReaderStyle) -> f32 {
    resolve_horizontal_page_geometry(page_width, reader_style).0
}

struct StyledRange {
    range: Range<usize>,
    style: TextStyle,
}

struct PreparedText {
    layout: Arc<Layout<TextBrush>>,
    text: Arc<str>,
    source_text_start: usize,
}

struct FixedPageReplacementRequest {
    text: String,
    rect: FixedPageTextRect,
    source: Option<SourceRange>,
}

fn fixed_page_replacement_block(text: &str, source: Option<SourceRange>) -> TextBlock {
    let mut content = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if index > 0 {
            content.push(Inline::Break);
        }
        if !line.is_empty() {
            content.push(Inline::Text(TextRun {
                text: line.to_owned(),
                style: TextStyle::default(),
                link: None,
            }));
        }
    }
    TextBlock {
        kind: TextBlockKind::Paragraph,
        content,
        style: rebook_publication::BlockStyle {
            line_height: 1.2,
            ..rebook_publication::BlockStyle::default()
        },
        source,
    }
}

fn prepared_text_height(prepared: &PreparedText) -> f32 {
    let Some(first) = prepared.layout.get(0) else {
        return 0.0;
    };
    let Some(last) = prepared.layout.get(prepared.layout.len().saturating_sub(1)) else {
        return 0.0;
    };
    (last.metrics().block_max_coord - first.metrics().block_min_coord).max(0.0)
}

fn flatten_text(block: &TextBlock, fallback_color: Rgba) -> (String, Vec<StyledRange>, usize) {
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
    let source_text_start = text.len();

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
    (text, spans, source_text_start)
}

struct Paginator {
    viewport: LayoutViewport,
    background: Rgba,
    left: f32,
    top: f32,
    width: f32,
    bottom: f32,
    column_has_content: bool,
    cursor_y: f32,
    pages: Vec<PageLayout>,
    items: Vec<PageItem>,
    center_standalone_image: bool,
}

#[derive(Clone, Copy)]
struct PageGeometry {
    left: f32,
    top: f32,
    width: f32,
    bottom: f32,
    visible_pages: usize,
    continuation_offset_x: f32,
}

impl Paginator {
    fn new(
        viewport: LayoutViewport,
        background: Rgba,
        geometry: PageGeometry,
        center_standalone_image: bool,
    ) -> Self {
        Self {
            viewport,
            background,
            left: geometry.left,
            top: geometry.top,
            width: geometry.width,
            bottom: geometry.bottom,
            column_has_content: false,
            cursor_y: geometry.top,
            pages: Vec::new(),
            items: Vec::new(),
            center_standalone_image,
        }
    }

    fn push_text(&mut self, prepared: &PreparedText, block: &TextBlock) -> Result<(), LayoutError> {
        self.add_spacing(block.style.margin_before);
        let mut line_start = 0;
        while line_start < prepared.layout.len() {
            let first = prepared
                .layout
                .get(line_start)
                .ok_or(LayoutError::InvalidLayout)?;
            let first_top = first.metrics().block_min_coord;
            let mut line_end = line_start;
            let mut slice_height = 0.0;
            while line_end < prepared.layout.len() {
                let line = prepared
                    .layout
                    .get(line_end)
                    .ok_or(LayoutError::InvalidLayout)?;
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
                layout: Arc::clone(&prepared.layout),
                text: Arc::clone(&prepared.text),
                source_text_start: prepared.source_text_start,
                lines: line_start..line_end,
                origin_x: self.column_left() + block.style.indent,
                origin_y: self.cursor_y - first_top,
                source: block.source.clone(),
            }));
            self.column_has_content = true;
            self.cursor_y += slice_height;
            line_start = line_end;
            if line_start < prepared.layout.len() {
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
    fn push_image(
        &mut self,
        image: RasterImage,
        style: ImageStyle,
        source: Option<SourceRange>,
        text_layer: Option<FixedPageTextLayer>,
    ) -> Vec<FixedPageReplacementRequest> {
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
        self.ensure_minimum_spacing(style.margin_before.max(IMAGE_BLOCK_GAP));
        if self.cursor_y + height > self.bottom && self.column_has_content {
            self.advance_column();
        }
        let x = self.column_left() + (self.width - width) / 2.0;
        let replacements = text_layer.as_ref().map_or_else(Vec::new, |layer| {
            let Some(replacement) = layer.replacement.as_ref() else {
                return Vec::new();
            };
            if layer.width <= 0.0 || layer.height <= 0.0 {
                return Vec::new();
            }
            let scale_x = width / layer.width;
            let scale_y = height / layer.height;
            replacement
                .segments
                .iter()
                .map(|segment| FixedPageReplacementRequest {
                    text: segment.text.clone(),
                    rect: FixedPageTextRect {
                        x: x + segment.rect.x * scale_x,
                        y: self.cursor_y + segment.rect.y * scale_y,
                        width: segment.rect.width * scale_x,
                        height: segment.rect.height * scale_y,
                    },
                    source: fixed_page_replacement_source(source.as_ref(), segment),
                })
                .collect()
        });
        self.items.push(PageItem::Image(ImagePlacement {
            image,
            x,
            y: self.cursor_y,
            width,
            height,
            source,
            text_layer,
            replacement: None,
        }));
        self.column_has_content = true;
        self.cursor_y += height + style.margin_after.max(IMAGE_BLOCK_GAP);
        replacements
    }

    fn push_fixed_page_replacement(
        &mut self,
        prepared: &PreparedText,
        request: FixedPageReplacementRequest,
    ) -> Result<(), LayoutError> {
        let Some(first) = prepared.layout.get(0) else {
            return Ok(());
        };
        let Some(PageItem::Image(image)) = self.items.last_mut() else {
            return Err(LayoutError::InvalidLayout);
        };
        let padding = 1.5;
        let segment = FixedPageTextReplacementSegmentPlacement {
            rect: request.rect,
            text: TextPlacement {
                layout: Arc::clone(&prepared.layout),
                text: Arc::clone(&prepared.text),
                source_text_start: prepared.source_text_start,
                lines: 0..prepared.layout.len(),
                origin_x: request.rect.x + padding,
                origin_y: request.rect.y + padding - first.metrics().block_min_coord,
                source: request.source,
            },
        };
        image
            .replacement
            .get_or_insert_with(|| FixedPageTextReplacementPlacement {
                segments: Vec::new(),
            })
            .segments
            .push(segment);
        Ok(())
    }

    fn ensure_minimum_spacing(&mut self, amount: f32) {
        let Some(content_bottom) = self.items.last().and_then(|item| match item {
            PageItem::Text(text) => text
                .lines
                .end
                .checked_sub(1)
                .and_then(|line| text.layout.get(line))
                .map(|line| text.origin_y + line.metrics().block_max_coord),
            PageItem::Image(image) => Some(image.y + image.height),
            PageItem::Separator(separator) => Some(separator.y + 1.0),
        }) else {
            return;
        };
        let target = content_bottom + amount.max(0.0);
        if target > self.bottom {
            self.advance_column();
        } else {
            self.cursor_y = self.cursor_y.max(target);
        }
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
        self.left
    }

    fn advance_column(&mut self) {
        self.commit_page();
    }

    fn commit_page(&mut self) {
        if self.items.is_empty() {
            self.cursor_y = self.top;
            return;
        }
        if self.center_standalone_image
            && let [PageItem::Image(image)] = self.items.as_mut_slice()
        {
            let available_height = self.bottom - self.top;
            let centered_y = self.top + ((available_height - image.height) / 2.0).max(0.0);
            let offset_y = centered_y - image.y;
            image.y = centered_y;
            if let Some(replacement) = image.replacement.as_mut() {
                for segment in &mut replacement.segments {
                    segment.rect.y += offset_y;
                    segment.text.origin_y += offset_y;
                }
            }
        }
        self.pages.push(PageLayout {
            viewport: self.viewport,
            background: self.background,
            items: std::mem::take(&mut self.items),
        });
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

fn fixed_page_replacement_source(
    source: Option<&SourceRange>,
    segment: &rebook_publication::FixedPageTextReplacementSegment,
) -> Option<SourceRange> {
    let mut source = source?.clone();
    let start = source
        .start
        .text_offset
        .saturating_add(segment.source_offset);
    source.start.text_offset = start;
    source.end.spine = source.start.spine.clone();
    source.end.node.clone_from(&source.start.node);
    source.end.text_offset =
        start.saturating_add(u64::try_from(segment.text.chars().count()).unwrap_or(u64::MAX));
    Some(source)
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
    fn reader_typography_matches_readest_defaults_and_builds_cjk_stacks() {
        let typography = ReaderTypography::default();
        assert_eq!(typography.default_font, ReaderDefaultFont::Serif);
        assert_eq!(typography.default_cjk_font, "LXGW WenKai GB Screen");
        assert_eq!(typography.serif_font, "Bitter");
        assert_eq!(typography.sans_serif_font, "Roboto");
        assert_eq!(typography.monospace_font, "Consolas");
        assert!((typography.font_size - 16.0).abs() < f32::EPSILON);
        assert!((typography.minimum_font_size - 8.0).abs() < f32::EPSILON);
        assert_eq!(typography.font_weight, 400);
        assert!(typography.serif_stack().contains("\"Bitter\""));
        assert!(typography.serif_stack().contains("\"SimSun\""));
        assert!(typography.serif_stack().ends_with("serif"));
        assert!(
            typography
                .sans_serif_stack()
                .contains("\"Microsoft YaHei\"")
        );
        assert!(typography.sans_serif_stack().ends_with("sans-serif"));
        assert!(typography.monospace_stack().ends_with("monospace"));
    }

    #[test]
    fn reader_typography_normalizes_persisted_values() {
        let mut typography = ReaderTypography {
            default_cjk_font: "  ".into(),
            serif_font: "  Georgia  ".into(),
            sans_serif_font: String::new(),
            monospace_font: String::new(),
            font_size: f32::NAN,
            minimum_font_size: -4.0,
            font_weight: 455,
            ..ReaderTypography::default()
        };
        typography.normalize();
        assert_eq!(typography.default_cjk_font, "LXGW WenKai GB Screen");
        assert_eq!(typography.serif_font, "Georgia");
        assert_eq!(typography.sans_serif_font, "Roboto");
        assert_eq!(typography.monospace_font, "Consolas");
        assert!((typography.font_size - 16.0).abs() < f32::EPSILON);
        assert!((typography.minimum_font_size - 1.0).abs() < f32::EPSILON);
        assert_eq!(typography.font_weight, 500);
    }

    #[test]
    fn default_page_geometry_compacts_only_the_top_inset() {
        let style = ReaderStyle::default();
        let geometry = resolve_page_geometry(800.0, 600.0, &style);

        assert!((style.top_margin - DEFAULT_TOP_MARGIN).abs() < f32::EPSILON);
        assert!((style.bottom_margin - DEFAULT_BOTTOM_MARGIN).abs() < f32::EPSILON);
        assert!((geometry.top - DEFAULT_TOP_MARGIN).abs() < f32::EPSILON);
        assert!((geometry.bottom - (600.0 - DEFAULT_BOTTOM_MARGIN)).abs() < f32::EPSILON);
    }

    #[test]
    fn wide_viewports_cap_and_center_each_reading_column() {
        let viewport_width = 3_000.0;
        let page_height = 900.0;
        let single = resolve_page_geometry(
            viewport_width,
            page_height,
            &ReaderStyle {
                spread: SpreadMode::Single,
                ..ReaderStyle::default()
            },
        );
        assert_eq!(single.visible_pages, 1);
        assert!((single.width - MAX_COLUMN_WIDTH).abs() < f32::EPSILON);
        assert!((single.left - (viewport_width - MAX_COLUMN_WIDTH) / 2.0).abs() < f32::EPSILON);

        let scroll = resolve_page_geometry(
            viewport_width,
            page_height,
            &ReaderStyle {
                spread: SpreadMode::Scroll,
                ..ReaderStyle::default()
            },
        );
        assert_eq!(scroll.visible_pages, 1);
        assert!((scroll.width - single.width).abs() < f32::EPSILON);
        assert!((scroll.left - single.left).abs() < f32::EPSILON);
        assert!((scroll.top - single.top).abs() < f32::EPSILON);
        assert!((scroll.bottom - single.bottom).abs() < f32::EPSILON);

        let double = resolve_page_geometry(
            viewport_width,
            page_height,
            &ReaderStyle {
                spread: SpreadMode::Double,
                ..ReaderStyle::default()
            },
        );
        let spread_width = MAX_COLUMN_WIDTH * 2.0 + DEFAULT_COLUMN_GAP;
        assert_eq!(double.visible_pages, 2);
        assert!((double.width - MAX_COLUMN_WIDTH).abs() < f32::EPSILON);
        assert!((double.left - (viewport_width - spread_width) / 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn zero_column_gap_places_double_pages_next_to_each_other() {
        let geometry = resolve_page_geometry(
            1_200.0,
            700.0,
            &ReaderStyle {
                spread: SpreadMode::Double,
                column_gap: 0.0,
                ..ReaderStyle::default()
            },
        );

        assert_eq!(geometry.visible_pages, 2);
        assert!((geometry.continuation_offset_x - geometry.width).abs() < f32::EPSILON);
    }
    use rebook_publication::{
        Book, FixedPageTextReplacement, FixedPageTextReplacementSegment, FixedPageTextSpan,
        ImageBlock, ImageLength, Metadata, PublicationId, PublicationUrl, RasterResource,
        RenditionLayout, Resource, SourceAnchor, SpineItemId, TocEntry,
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

        fn raster_resource(
            &self,
            _href: &PublicationUrl,
        ) -> Result<Option<RasterResource>, PublicationError> {
            Ok(Some(RasterResource {
                width: 200,
                height: 100,
                pixels: Vec::new().into(),
            }))
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
            anchors: Vec::new(),
        };
        let layout = LayoutEngine::new()
            .layout_section(
                &source,
                &section,
                LayoutViewport::new(600, 400).unwrap(),
                &ReaderStyle::default(),
            )
            .unwrap();
        assert!(layout.pages.len() > 1);
    }

    #[test]
    fn double_spread_emits_independent_logical_pages_for_composition() {
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
            anchors: Vec::new(),
        };
        let viewport = LayoutViewport::new(900, 700).unwrap();
        let single = LayoutEngine::new()
            .layout_section(
                &source,
                &section,
                viewport,
                &ReaderStyle {
                    spread: SpreadMode::Single,
                    ..ReaderStyle::default()
                },
            )
            .unwrap();
        let double = LayoutEngine::new()
            .layout_section(
                &source,
                &section,
                viewport,
                &ReaderStyle {
                    spread: SpreadMode::Double,
                    ..ReaderStyle::default()
                },
            )
            .unwrap();

        assert_eq!(single.visible_pages, 1);
        assert_eq!(double.visible_pages, 2);
        assert!(double.pages.len() >= 2);
        let first_origin = double.pages[0]
            .items
            .iter()
            .find_map(|item| match item {
                PageItem::Text(text) => Some(text.origin_x),
                _ => None,
            })
            .expect("first logical page should contain text");
        let second_origin = double.pages[1]
            .items
            .iter()
            .find_map(|item| match item {
                PageItem::Text(text) => Some(text.origin_x),
                _ => None,
            })
            .expect("second logical page should contain text");
        assert!((first_origin - second_origin).abs() < f32::EPSILON);
        assert!(double.continuation_offset_x > 0.0);
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
                visible_pages: 1,
                continuation_offset_x: 0.0,
            },
            false,
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

    #[test]
    fn image_after_zero_margin_text_keeps_a_minimum_block_gap() {
        let image_href = PublicationUrl::parse("images/figure.png").unwrap();
        let source = EmptySource {
            book: Book {
                id: PublicationId::new("image-gap-test").unwrap(),
                metadata: Metadata::default(),
                cover: None,
                sections: Vec::new(),
                table_of_contents: Vec::new(),
            },
        };
        let section = Section {
            id: SpineItemId::new("chapter").unwrap(),
            href: PublicationUrl::parse("chapter.xhtml").unwrap(),
            blocks: vec![
                Block::Text(TextBlock {
                    kind: TextBlockKind::Paragraph,
                    content: vec![Inline::Text(rebook_publication::TextRun {
                        text: "Text immediately before a figure.".into(),
                        style: TextStyle::default(),
                        link: None,
                    })],
                    style: rebook_publication::BlockStyle {
                        margin_after: 0.0,
                        ..rebook_publication::BlockStyle::default()
                    },
                    source: None,
                }),
                Block::Image(ImageBlock {
                    href: image_href,
                    alt: "Figure".into(),
                    style: ImageStyle::default(),
                    source: None,
                    text_layer: None,
                }),
            ],
            anchors: Vec::new(),
        };

        let layout = LayoutEngine::new()
            .layout_section(
                &source,
                &section,
                LayoutViewport::new(400, 500).unwrap(),
                &ReaderStyle::default(),
            )
            .unwrap();
        let [PageItem::Text(text), PageItem::Image(image)] = layout.pages[0].items.as_slice()
        else {
            panic!("expected text followed by an image");
        };
        let last_line = text.layout.get(text.lines.end - 1).unwrap();
        let text_bottom = text.origin_y + last_line.metrics().block_max_coord;

        assert!((image.y - text_bottom - IMAGE_BLOCK_GAP).abs() < 0.001);
    }

    #[test]
    fn authored_image_margin_larger_than_the_default_gap_is_preserved() {
        let viewport = LayoutViewport::new(400, 500).unwrap();
        let mut paginator = Paginator::new(
            viewport,
            Rgba::BLACK,
            PageGeometry {
                left: 20.0,
                top: 40.0,
                width: 360.0,
                bottom: 460.0,
                visible_pages: 1,
                continuation_offset_x: 0.0,
            },
            false,
        );
        paginator.push_separator();
        paginator.push_image(
            RasterImage {
                width: 200,
                height: 100,
                pixels: Vec::new().into(),
            },
            ImageStyle {
                margin_before: 25.0,
                ..ImageStyle::default()
            },
            None,
            None,
        );

        let pages = paginator.finish();
        let [PageItem::Separator(separator), PageItem::Image(image)] = pages[0].items.as_slice()
        else {
            panic!("expected a separator followed by an image");
        };

        assert!((image.y - (separator.y + 1.0) - 25.0).abs() < 0.001);
    }

    #[test]
    fn image_moved_to_the_next_page_starts_at_the_page_margin() {
        let image_href = PublicationUrl::parse("images/figure.png").unwrap();
        let source = EmptySource {
            book: Book {
                id: PublicationId::new("image-page-break-test").unwrap(),
                metadata: Metadata::default(),
                cover: None,
                sections: Vec::new(),
                table_of_contents: Vec::new(),
            },
        };
        let section = Section {
            id: SpineItemId::new("chapter").unwrap(),
            href: PublicationUrl::parse("chapter.xhtml").unwrap(),
            blocks: vec![
                Block::Text(TextBlock {
                    kind: TextBlockKind::Paragraph,
                    content: vec![Inline::Text(rebook_publication::TextRun {
                        text: "Text before a figure that must move.".into(),
                        style: TextStyle::default(),
                        link: None,
                    })],
                    style: rebook_publication::BlockStyle {
                        margin_after: 0.0,
                        ..rebook_publication::BlockStyle::default()
                    },
                    source: None,
                }),
                Block::Image(ImageBlock {
                    href: image_href,
                    alt: "Figure".into(),
                    style: ImageStyle::default(),
                    source: None,
                    text_layer: None,
                }),
            ],
            anchors: Vec::new(),
        };
        let viewport = LayoutViewport::new(400, 140).unwrap();
        let style = ReaderStyle::default();
        let page_top = resolve_page_geometry(400.0, 140.0, &style).top;

        let layout = LayoutEngine::new()
            .layout_section(&source, &section, viewport, &style)
            .unwrap();
        let PageItem::Image(image) = &layout.pages[1].items[0] else {
            panic!("expected the image on the next page");
        };

        assert!((image.y - page_top).abs() < 0.001);
    }

    #[test]
    fn fixed_page_image_is_vertically_centered_in_the_content_area() {
        let viewport = LayoutViewport::new(400, 500).unwrap();
        let mut paginator = Paginator::new(
            viewport,
            Rgba::BLACK,
            PageGeometry {
                left: 20.0,
                top: 40.0,
                width: 360.0,
                bottom: 460.0,
                visible_pages: 1,
                continuation_offset_x: 0.0,
            },
            true,
        );
        paginator.push_image(
            RasterImage {
                width: 200,
                height: 100,
                pixels: Vec::new().into(),
            },
            ImageStyle::default(),
            None,
            None,
        );

        let pages = paginator.finish();
        let PageItem::Image(image) = &pages[0].items[0] else {
            panic!("expected an image placement");
        };
        assert!((image.y - 200.0).abs() < 0.001);
    }

    #[test]
    fn fixed_page_replacement_stays_on_the_original_page_image() {
        let href = PublicationUrl::parse("page-1.png").unwrap();
        let spine = SpineItemId::new("pdf-page-1").unwrap();
        let source_range = SourceRange {
            start: SourceAnchor {
                spine: spine.clone(),
                node: "pdf-page-text".into(),
                text_offset: 0,
            },
            end: SourceAnchor {
                spine,
                node: "pdf-page-text".into(),
                text_offset: 4,
            },
        };
        let source = EmptySource {
            book: Book {
                id: PublicationId::new("fixed-page-translation").unwrap(),
                metadata: Metadata {
                    layout: RenditionLayout::PrePaginated,
                    ..Metadata::default()
                },
                cover: None,
                sections: Vec::new(),
                table_of_contents: Vec::new(),
            },
        };
        let replacement_rect = FixedPageTextRect {
            x: 20.0,
            y: 10.0,
            width: 100.0,
            height: 40.0,
        };
        let section = Section {
            id: SpineItemId::new("pdf-page-1").unwrap(),
            href: PublicationUrl::parse("page-1.pdf").unwrap(),
            blocks: vec![Block::Image(ImageBlock {
                href,
                alt: "PDF page 1".into(),
                style: ImageStyle::default(),
                source: Some(source_range),
                text_layer: Some(FixedPageTextLayer {
                    width: 200.0,
                    height: 100.0,
                    text: "PDF text".into(),
                    spans: vec![FixedPageTextSpan {
                        char_range: 0..8,
                        rect: replacement_rect,
                    }],
                    replacement: Some(FixedPageTextReplacement {
                        segments: vec![FixedPageTextReplacementSegment {
                            text: "译文".into(),
                            rect: replacement_rect,
                            source_offset: 0,
                        }],
                    }),
                }),
            })],
            anchors: Vec::new(),
        };

        let layout = LayoutEngine::new()
            .layout_section(
                &source,
                &section,
                LayoutViewport::new(400, 500).unwrap(),
                &ReaderStyle::default(),
            )
            .unwrap();

        assert_eq!(layout.pages.len(), 1);
        let [PageItem::Image(image)] = layout.pages[0].items.as_slice() else {
            panic!("translation must remain attached to the fixed page image");
        };
        let replacement = image
            .replacement
            .as_ref()
            .expect("fixed page should retain its replacement overlay");
        let [segment] = replacement.segments.as_slice() else {
            panic!("expected one translated fixed-page segment");
        };
        assert!(segment.rect.x >= image.x);
        assert!(segment.rect.y >= image.y);
        assert!(segment.rect.x + segment.rect.width <= image.x + image.width);
        assert!(segment.rect.y + segment.rect.height <= image.y + image.height);
        assert_eq!(segment.text.text.as_ref(), "译文");
    }

    #[test]
    fn reflowable_standalone_cover_is_vertically_centered() {
        let cover = PublicationUrl::parse("images/cover.jpg").unwrap();
        let source = EmptySource {
            book: Book {
                id: PublicationId::new("cover-test").unwrap(),
                metadata: Metadata::default(),
                cover: Some(cover.clone()),
                sections: Vec::new(),
                table_of_contents: Vec::new(),
            },
        };
        let section = Section {
            id: SpineItemId::new("cover").unwrap(),
            href: PublicationUrl::parse("cover.xhtml").unwrap(),
            blocks: vec![Block::Image(ImageBlock {
                href: cover,
                alt: "Cover".into(),
                style: ImageStyle::default(),
                source: None,
                text_layer: None,
            })],
            anchors: Vec::new(),
        };

        let layout = LayoutEngine::new()
            .layout_section(
                &source,
                &section,
                LayoutViewport::new(400, 500).unwrap(),
                &ReaderStyle::default(),
            )
            .unwrap();
        let PageItem::Image(image) = &layout.pages[0].items[0] else {
            panic!("expected a cover image placement");
        };

        let style = ReaderStyle::default();
        let geometry = resolve_page_geometry(400.0, 500.0, &style);
        let expected_y = geometry.top + (geometry.bottom - geometry.top - image.height) / 2.0;
        assert!((image.y - expected_y).abs() < 0.001);
    }

    #[test]
    fn reflowable_standalone_non_cover_image_stays_in_normal_flow() {
        let image_href = PublicationUrl::parse("images/illustration.jpg").unwrap();
        let source = EmptySource {
            book: Book {
                id: PublicationId::new("illustration-test").unwrap(),
                metadata: Metadata::default(),
                cover: None,
                sections: Vec::new(),
                table_of_contents: Vec::new(),
            },
        };
        let section = Section {
            id: SpineItemId::new("illustration").unwrap(),
            href: PublicationUrl::parse("illustration.xhtml").unwrap(),
            blocks: vec![Block::Image(ImageBlock {
                href: image_href,
                alt: "Illustration".into(),
                style: ImageStyle::default(),
                source: None,
                text_layer: None,
            })],
            anchors: Vec::new(),
        };

        let layout = LayoutEngine::new()
            .layout_section(
                &source,
                &section,
                LayoutViewport::new(400, 500).unwrap(),
                &ReaderStyle::default(),
            )
            .unwrap();
        let PageItem::Image(image) = &layout.pages[0].items[0] else {
            panic!("expected an illustration image placement");
        };

        assert!((image.y - ReaderStyle::default().top_margin).abs() < 0.001);
    }
}
