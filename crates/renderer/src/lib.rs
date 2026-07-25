//! Compiles immutable page layouts into cheap-to-replay display lists.

use std::ops::Range;
use std::sync::Arc;

use anyrender::{Glyph, NormalizedCoord, PaintScene};
use kurbo::{Affine, Line, Rect, Stroke, Vec2};
use parley::editing::{Cursor, Selection};
use parley::layout::{Affinity, Cluster, ClusterSide};
use parley::{FontData, Layout, PositionedLayoutItem};
use peniko::{Blob, Color, Fill, ImageAlphaType, ImageBrush, ImageData, ImageFormat};
use rebook_layout::{PageItem, PageLayout, TextBrush, TextPlacement};
use rebook_publication::{Rgba, SourceAnchor, SourceRange};

/// Pointer hit inside one retained text placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageTextHit {
    pub region_index: usize,
    pub byte_index: usize,
}

/// One durable, single-block piece of a visual text selection.
#[derive(Debug, Clone)]
pub struct PageSelectionFragment {
    pub range: SourceRange,
    pub quote: String,
    pub rects: Vec<Rect>,
}

/// Retained drawing commands for one page. No parsing, shaping, or pagination
/// occurs while this list is replayed.
pub struct PageDisplayList {
    width: u32,
    height: u32,
    background: Color,
    commands: Vec<DisplayCommand>,
    text_regions: Vec<TextRegion>,
}

impl PageDisplayList {
    /// Logical width of the compiled page.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Logical height of the compiled page.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Number of retained commands, useful for diagnostics.
    pub fn command_count(&self) -> usize {
        self.commands.len()
    }

    /// Number of source-backed text placements on this logical page.
    pub fn text_region_count(&self) -> usize {
        self.text_regions.len()
    }

    /// Visible UTF-8 byte range for a retained text placement.
    pub fn text_region_visible_range(&self, region_index: usize) -> Option<Range<usize>> {
        self.text_regions
            .get(region_index)
            .and_then(TextRegion::visible_byte_range)
    }

    /// Hit-tests source-backed text. Exact mode is used when a drag starts;
    /// nearest mode lets a drag extend naturally through line/column whitespace.
    pub fn hit_test_text(&self, x: f32, y: f32, exact: bool) -> Option<PageTextHit> {
        if exact {
            return self
                .text_regions
                .iter()
                .enumerate()
                .find_map(|(index, region)| {
                    region.hit_test(x, y, true).map(|byte_index| PageTextHit {
                        region_index: index,
                        byte_index,
                    })
                });
        }

        self.text_regions
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| {
                left.vertical_distance(y)
                    .total_cmp(&right.vertical_distance(y))
            })
            .and_then(|(index, region)| {
                region.hit_test(x, y, false).map(|byte_index| PageTextHit {
                    region_index: index,
                    byte_index,
                })
            })
    }

    /// Resolves a byte range in one retained placement to source anchors,
    /// selected text, and page-coordinate rectangles.
    pub fn selection_fragment(
        &self,
        region_index: usize,
        byte_range: Range<usize>,
    ) -> Option<PageSelectionFragment> {
        self.text_regions
            .get(region_index)?
            .selection_fragment(byte_range)
    }

    /// Resolves durable source ranges to page-coordinate highlight rectangles.
    pub fn source_rects(&self, ranges: &[SourceRange]) -> Vec<Rect> {
        self.text_regions
            .iter()
            .flat_map(|region| {
                ranges
                    .iter()
                    .filter_map(|range| region.byte_range_for_source(range))
                    .flat_map(|range| region.selection_rects(range))
            })
            .collect()
    }

    pub fn contains_source_anchor(&self, anchor: &SourceAnchor) -> bool {
        self.text_regions
            .iter()
            .any(|region| region.contains_source_anchor(anchor))
    }

    pub fn source_ranges_contain_point(&self, ranges: &[SourceRange], x: f32, y: f32) -> bool {
        self.source_rects(ranges)
            .iter()
            .any(|rect| rect.contains(kurbo::Point::new(f64::from(x), f64::from(y))))
    }

    /// Replays this page into any `AnyRender` backend, including Vello GPU and CPU.
    pub fn paint(&self, scene: &mut impl PaintScene) {
        self.paint_scaled(scene, 1.0);
    }

    /// Replays logical page coordinates at the window's device scale.
    pub fn paint_scaled(&self, scene: &mut impl PaintScene, scale_factor: f32) {
        self.paint_scaled_at(scene, scale_factor, 0.0, 0.0);
    }

    /// Replays the page at a logical offset, used to compose reader chrome and
    /// the book surface without re-compiling either display list.
    pub fn paint_scaled_at(
        &self,
        scene: &mut impl PaintScene,
        scale_factor: f32,
        offset_x: f32,
        offset_y: f32,
    ) {
        let scale = Affine::scale(f64::from(scale_factor.max(0.1)))
            * Affine::translate((f64::from(offset_x), f64::from(offset_y)));
        self.paint_background_with_transform(scene, scale);
        self.paint_content_with_transform(scene, scale);
    }

    /// Paints only the page background. Spread composition paints this once,
    /// then overlays one or two logical page display lists.
    pub fn paint_background(&self, scene: &mut impl PaintScene) {
        self.paint_background_with_transform(scene, Affine::IDENTITY);
    }

    /// Paints retained page content without covering content already composed
    /// into the same spread.
    pub fn paint_content_at(&self, scene: &mut impl PaintScene, offset_x: f32) {
        self.paint_content_with_transform(scene, Affine::translate((f64::from(offset_x), 0.0)));
    }

    /// Paints translucent source-backed marks below page content.
    pub fn paint_source_ranges(
        &self,
        scene: &mut impl PaintScene,
        ranges: &[SourceRange],
        color: Color,
        offset_x: f32,
    ) {
        let transform = Affine::translate((f64::from(offset_x), 0.0));
        for rect in self.source_rects(ranges) {
            scene.fill(Fill::NonZero, transform, color, None, &rect);
        }
    }

    fn paint_background_with_transform(&self, scene: &mut impl PaintScene, transform: Affine) {
        scene.fill(
            Fill::NonZero,
            transform,
            self.background,
            None,
            &Rect::new(0.0, 0.0, f64::from(self.width), f64::from(self.height)),
        );
    }

    fn paint_content_with_transform(&self, scene: &mut impl PaintScene, transform: Affine) {
        for command in &self.commands {
            command.paint(scene, transform);
        }
    }
}

struct TextRegion {
    layout: Arc<Layout<TextBrush>>,
    text: Arc<str>,
    source_text_start: usize,
    lines: Range<usize>,
    origin_x: f32,
    origin_y: f32,
    source: SourceRange,
}

impl TextRegion {
    fn visible_byte_range(&self) -> Option<Range<usize>> {
        let first = self.layout.get(self.lines.start)?;
        let last = self.layout.get(self.lines.end.checked_sub(1)?)?;
        let start = first.text_range().start.max(self.source_text_start);
        let end = last.text_range().end.min(self.text.len());
        (end > start).then_some(start..end)
    }

    fn vertical_bounds(&self) -> Option<(f32, f32)> {
        let first = self.layout.get(self.lines.start)?;
        let last = self.layout.get(self.lines.end.checked_sub(1)?)?;
        Some((
            first.metrics().block_min_coord + self.origin_y,
            last.metrics().block_max_coord + self.origin_y,
        ))
    }

    fn vertical_distance(&self, y: f32) -> f32 {
        let Some((top, bottom)) = self.vertical_bounds() else {
            return f32::MAX;
        };
        if y < top {
            top - y
        } else if y > bottom {
            y - bottom
        } else {
            0.0
        }
    }

    fn hit_test(&self, x: f32, y: f32, exact: bool) -> Option<usize> {
        let (top, bottom) = self.vertical_bounds()?;
        if exact && !(top..=bottom).contains(&y) {
            return None;
        }
        let local_x = x - self.origin_x;
        let local_y = if exact {
            y - self.origin_y
        } else {
            y.clamp(top + 0.01, bottom - 0.01) - self.origin_y
        };
        let byte_index = if exact {
            let (cluster, side) = Cluster::from_point_exact(&self.layout, local_x, local_y)?;
            let range = cluster.text_range();
            if cluster.is_rtl() {
                if side == ClusterSide::Left {
                    range.end
                } else {
                    range.start
                }
            } else if side == ClusterSide::Left {
                range.start
            } else {
                range.end
            }
        } else {
            Cursor::from_point(&self.layout, local_x, local_y).index()
        };
        let visible = self.visible_byte_range()?;
        Some(byte_index.clamp(visible.start, visible.end))
    }

    fn selection_fragment(&self, byte_range: Range<usize>) -> Option<PageSelectionFragment> {
        let visible = self.visible_byte_range()?;
        let start = floor_char_boundary(
            &self.text,
            byte_range.start.clamp(visible.start, visible.end),
        );
        let end = floor_char_boundary(&self.text, byte_range.end.clamp(visible.start, visible.end));
        if end <= start {
            return None;
        }
        let range = self.source_range_for_bytes(start..end)?;
        Some(PageSelectionFragment {
            range,
            quote: self.text.get(start..end)?.to_owned(),
            rects: self.selection_rects(start..end),
        })
    }

    fn selection_rects(&self, byte_range: Range<usize>) -> Vec<Rect> {
        if byte_range.end <= byte_range.start {
            return Vec::new();
        }
        let selection = Selection::new(
            Cursor::from_byte_index(&self.layout, byte_range.start, Affinity::Downstream),
            Cursor::from_byte_index(&self.layout, byte_range.end, Affinity::Upstream),
        );
        selection
            .geometry(&self.layout)
            .into_iter()
            .filter(|(_, line_index)| self.lines.contains(line_index))
            .map(|(rect, _)| {
                Rect::new(
                    rect.x0 + f64::from(self.origin_x),
                    rect.y0 + f64::from(self.origin_y),
                    rect.x1 + f64::from(self.origin_x),
                    rect.y1 + f64::from(self.origin_y),
                )
            })
            .collect()
    }

    fn source_range_for_bytes(&self, byte_range: Range<usize>) -> Option<SourceRange> {
        if self.source.start.spine != self.source.end.spine
            || self.source.start.node != self.source.end.node
        {
            return None;
        }
        let source_start = self.source.start.text_offset;
        let start = source_start
            + u64::try_from(
                self.text
                    .get(self.source_text_start..byte_range.start)?
                    .chars()
                    .count(),
            )
            .ok()?;
        let end = source_start
            + u64::try_from(
                self.text
                    .get(self.source_text_start..byte_range.end)?
                    .chars()
                    .count(),
            )
            .ok()?;
        Some(SourceRange {
            start: SourceAnchor {
                spine: self.source.start.spine.clone(),
                node: self.source.start.node.clone(),
                text_offset: start,
            },
            end: SourceAnchor {
                spine: self.source.start.spine.clone(),
                node: self.source.start.node.clone(),
                text_offset: end,
            },
        })
    }

    fn byte_range_for_source(&self, range: &SourceRange) -> Option<Range<usize>> {
        if self.source.start.spine != self.source.end.spine
            || self.source.start.node != self.source.end.node
            || range.start.spine != range.end.spine
            || range.start.node != range.end.node
            || self.source.start.spine != range.start.spine
            || self.source.start.node != range.start.node
        {
            return None;
        }
        let start_offset = range
            .start
            .text_offset
            .max(self.source.start.text_offset)
            .min(self.source.end.text_offset);
        let end_offset = range
            .end
            .text_offset
            .max(self.source.start.text_offset)
            .min(self.source.end.text_offset);
        if end_offset <= start_offset {
            return None;
        }
        let source_text = self.text.get(self.source_text_start..)?;
        let start_chars = usize::try_from(start_offset - self.source.start.text_offset).ok()?;
        let end_chars = usize::try_from(end_offset - self.source.start.text_offset).ok()?;
        let start = self.source_text_start + byte_index_for_char_offset(source_text, start_chars);
        let end = self.source_text_start + byte_index_for_char_offset(source_text, end_chars);
        let visible = self.visible_byte_range()?;
        let start = start.max(visible.start).min(visible.end);
        let end = end.max(visible.start).min(visible.end);
        (end > start).then_some(start..end)
    }

    fn contains_source_anchor(&self, anchor: &SourceAnchor) -> bool {
        self.visible_byte_range()
            .and_then(|range| self.source_range_for_bytes(range))
            .is_some_and(|range| source_range_contains(&range, anchor))
    }
}

fn byte_index_for_char_offset(text: &str, offset: usize) -> usize {
    text.char_indices()
        .nth(offset)
        .map_or(text.len(), |(index, _)| index)
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn source_range_contains(range: &SourceRange, anchor: &SourceAnchor) -> bool {
    range.start.spine == anchor.spine
        && range.start.node == anchor.node
        && anchor.text_offset >= range.start.text_offset
        && (anchor.text_offset < range.end.text_offset
            || (range.start.text_offset == range.end.text_offset
                && anchor.text_offset == range.start.text_offset))
}

enum DisplayCommand {
    Glyphs(GlyphCommand),
    Image(ImageCommand),
    Rule(RuleCommand),
}

impl DisplayCommand {
    fn paint(&self, scene: &mut impl PaintScene, page_transform: Affine) {
        match self {
            Self::Glyphs(command) => scene.draw_glyphs(
                &command.font,
                command.font_size,
                true,
                &command.normalized_coords,
                Vec2::ZERO,
                Fill::NonZero,
                command.color,
                1.0,
                page_transform * command.transform,
                command.glyph_transform,
                command.glyphs.iter().copied(),
            ),
            Self::Image(command) => {
                scene.draw_image(command.image.as_ref(), page_transform * command.transform);
            }
            Self::Rule(command) => scene.stroke(
                &Stroke::new(command.width),
                page_transform,
                command.color,
                None,
                &Line::new(command.start, command.end),
            ),
        }
    }
}

struct GlyphCommand {
    font: FontData,
    font_size: f32,
    normalized_coords: Arc<[NormalizedCoord]>,
    color: Color,
    transform: Affine,
    glyph_transform: Option<Affine>,
    glyphs: Arc<[Glyph]>,
}

struct ImageCommand {
    image: ImageBrush,
    transform: Affine,
}

struct RuleCommand {
    start: (f64, f64),
    end: (f64, f64),
    width: f64,
    color: Color,
}

/// Stateless compiler from layout IR to retained paint commands.
#[derive(Debug, Default)]
pub struct DisplayListCompiler;

impl DisplayListCompiler {
    pub fn compile(&self, page: &PageLayout) -> PageDisplayList {
        let mut commands = Vec::new();
        let mut text_regions = Vec::new();
        for item in &page.items {
            match item {
                PageItem::Text(text) => {
                    if let Some(region) = text_region(text) {
                        text_regions.push(region);
                    }
                    compile_text_commands(&mut commands, text);
                }
                PageItem::Image(image) => {
                    let data = ImageData {
                        data: Blob::new(Arc::new(image.image.pixels.clone())),
                        format: ImageFormat::Rgba8,
                        alpha_type: ImageAlphaType::Alpha,
                        width: image.image.width,
                        height: image.image.height,
                    };
                    let transform = Affine::translate((f64::from(image.x), f64::from(image.y)))
                        * Affine::scale_non_uniform(
                            f64::from(image.width) / f64::from(image.image.width.max(1)),
                            f64::from(image.height) / f64::from(image.image.height.max(1)),
                        );
                    commands.push(DisplayCommand::Image(ImageCommand {
                        image: ImageBrush::new(data),
                        transform,
                    }));
                }
                PageItem::Separator(separator) => {
                    commands.push(DisplayCommand::Rule(RuleCommand {
                        start: (f64::from(separator.x), f64::from(separator.y)),
                        end: (
                            f64::from(separator.x + separator.width),
                            f64::from(separator.y),
                        ),
                        width: 1.0,
                        color: Color::from_rgba8(120, 116, 108, 160),
                    }));
                }
            }
        }

        PageDisplayList {
            width: page.viewport.width,
            height: page.viewport.height,
            background: color(page.background),
            commands,
            text_regions,
        }
    }
}

fn text_region(text: &TextPlacement) -> Option<TextRegion> {
    Some(TextRegion {
        layout: Arc::clone(&text.layout),
        text: Arc::clone(&text.text),
        source_text_start: text.source_text_start,
        lines: text.lines.clone(),
        origin_x: text.origin_x,
        origin_y: text.origin_y,
        source: text.source.clone()?,
    })
}

fn compile_text_commands(commands: &mut Vec<DisplayCommand>, text: &TextPlacement) {
    let transform = Affine::translate((f64::from(text.origin_x), f64::from(text.origin_y)));
    for line in text
        .layout
        .lines()
        .skip(text.lines.start)
        .take(text.lines.len())
    {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                continue;
            };
            let run = glyph_run.run();
            let brush = glyph_run.style().brush;
            let synthesis = run.synthesis();
            let glyph_transform = synthesis
                .skew()
                .map(|angle| Affine::skew(f64::from(angle.to_radians().tan()), 0.0));
            let glyphs = glyph_run
                .positioned_glyphs()
                .map(|glyph| Glyph {
                    id: glyph.id,
                    x: glyph.x,
                    y: glyph.y,
                })
                .collect::<Vec<_>>()
                .into();
            commands.push(DisplayCommand::Glyphs(GlyphCommand {
                font: run.font().clone(),
                font_size: run.font_size(),
                normalized_coords: run.normalized_coords().to_vec().into(),
                color: color(brush.color),
                transform,
                glyph_transform,
                glyphs,
            }));

            if brush.underline {
                let metrics = run.metrics();
                let y = f64::from(
                    glyph_run.baseline() - metrics.underline_offset + metrics.underline_size / 2.0,
                ) + f64::from(text.origin_y);
                let x = f64::from(glyph_run.offset() + text.origin_x);
                commands.push(DisplayCommand::Rule(RuleCommand {
                    start: (x, y),
                    end: (x + f64::from(glyph_run.advance()), y),
                    width: f64::from(metrics.underline_size.max(1.0)),
                    color: color(brush.color),
                }));
            }
        }
    }
}

fn color(value: Rgba) -> Color {
    Color::from_rgba8(value.red, value.green, value.blue, value.alpha)
}

#[cfg(test)]
mod tests {
    use super::*;
    use parley::{Alignment, AlignmentOptions, FontContext, LayoutContext, StyleProperty};
    use rebook_layout::{LayoutViewport, PageItem, PageLayout, TextBrush, TextPlacement};
    use rebook_publication::{SourceAnchor, SourceRange, SpineItemId};

    #[test]
    fn empty_page_still_has_a_background() {
        let page = PageLayout {
            viewport: LayoutViewport::new(320, 240).unwrap(),
            background: Rgba::BLACK,
            items: Vec::new(),
        };
        let list = DisplayListCompiler.compile(&page);
        assert_eq!(list.width(), 320);
        assert_eq!(list.height(), 240);
        assert_eq!(list.command_count(), 0);
    }

    #[test]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "the test uses small, bounded logical page coordinates"
    )]
    fn text_hits_and_source_ranges_round_trip_through_retained_geometry() {
        let text: Arc<str> = "hello world".into();
        let mut font_context = FontContext::new();
        let mut layout_context = LayoutContext::new();
        let mut builder =
            layout_context.ranged_builder(&mut font_context, text.as_ref(), 1.0, false);
        builder.push_default(StyleProperty::FontSize(18.0));
        builder.push_default(StyleProperty::Brush(TextBrush {
            color: Rgba::BLACK,
            underline: false,
        }));
        let mut layout = builder.build(text.as_ref());
        layout.break_all_lines(Some(240.0));
        layout.align(Alignment::Start, AlignmentOptions::default());
        let spine = SpineItemId::new("chapter-1").unwrap();
        let source = SourceRange {
            start: SourceAnchor {
                spine: spine.clone(),
                node: "paragraph-1".into(),
                text_offset: 0,
            },
            end: SourceAnchor {
                spine,
                node: "paragraph-1".into(),
                text_offset: 11,
            },
        };
        let page = PageLayout {
            viewport: LayoutViewport::new(320, 240).unwrap(),
            background: Rgba::BLACK,
            items: vec![PageItem::Text(TextPlacement {
                layout: Arc::new(layout),
                text,
                source_text_start: 0,
                lines: 0..1,
                origin_x: 24.0,
                origin_y: 24.0,
                source: Some(source),
            })],
        };
        let list = DisplayListCompiler.compile(&page);
        let selected_source = SourceRange {
            start: SourceAnchor {
                spine: SpineItemId::new("chapter-1").unwrap(),
                node: "paragraph-1".into(),
                text_offset: 1,
            },
            end: SourceAnchor {
                spine: SpineItemId::new("chapter-1").unwrap(),
                node: "paragraph-1".into(),
                text_offset: 5,
            },
        };
        let rects = list.source_rects(std::slice::from_ref(&selected_source));
        assert!(!rects.is_empty());
        let point = rects[0].center();
        assert!(
            list.hit_test_text(point.x as f32, point.y as f32, true)
                .is_some()
        );

        let fragment = list.selection_fragment(0, 1..5).unwrap();
        assert_eq!(fragment.quote, "ello");
        assert_eq!(fragment.range, selected_source);
        assert!(!fragment.rects.is_empty());
    }
}
