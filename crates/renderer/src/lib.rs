//! Compiles immutable page layouts into cheap-to-replay display lists.

use std::sync::Arc;

use anyrender::{Glyph, NormalizedCoord, PaintScene};
use kurbo::{Affine, Line, Rect, Stroke, Vec2};
use parley::{FontData, PositionedLayoutItem};
use peniko::{Blob, Color, Fill, ImageAlphaType, ImageBrush, ImageData, ImageFormat};
use rebook_layout::{PageItem, PageLayout};
use rebook_publication::Rgba;

/// Retained drawing commands for one page. No parsing, shaping, or pagination
/// occurs while this list is replayed.
pub struct PageDisplayList {
    width: u32,
    height: u32,
    background: Color,
    commands: Vec<DisplayCommand>,
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
        scene.fill(
            Fill::NonZero,
            scale,
            self.background,
            None,
            &Rect::new(0.0, 0.0, f64::from(self.width), f64::from(self.height)),
        );
        for command in &self.commands {
            command.paint(scene, scale);
        }
    }
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
        for item in &page.items {
            match item {
                PageItem::Text(text) => {
                    let transform =
                        Affine::translate((f64::from(text.origin_x), f64::from(text.origin_y)));
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
                            let glyph_transform = synthesis.skew().map(|angle| {
                                Affine::skew(f64::from(angle.to_radians().tan()), 0.0)
                            });
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
                                    glyph_run.baseline() - metrics.underline_offset
                                        + metrics.underline_size / 2.0,
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
        }
    }
}

fn color(value: Rgba) -> Color {
    Color::from_rgba8(value.red, value.green, value.blue, value.alpha)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rebook_layout::{LayoutViewport, PageLayout};

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
}
