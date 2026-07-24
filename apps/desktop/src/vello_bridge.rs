//! Thin adapter from the renderer-neutral `AnyRender` display list to the
//! Vello version used by the published Xilem release.

use std::sync::Arc;

use anyrender::{Filter, NormalizedCoord, Paint, PaintRef, PaintScene, RenderContext};
use kurbo::{Affine, PathEl, Rect, Shape, Stroke, Vec2};
use peniko::{BlendMode, Color, Fill, FontData, ImageAlphaType, ImageFormat, StyleRef};
use xilem::masonry::{kurbo as target_kurbo, peniko as target_peniko, vello};

pub struct XilemVelloScene<'a> {
    scene: &'a mut vello::Scene,
}

impl<'a> XilemVelloScene<'a> {
    pub fn new(scene: &'a mut vello::Scene) -> Self {
        Self { scene }
    }
}

impl RenderContext for XilemVelloScene<'_> {}

impl PaintScene for XilemVelloScene<'_> {
    fn reset(&mut self) {
        self.scene.reset();
    }

    fn push_layer(
        &mut self,
        _blend: impl Into<BlendMode>,
        alpha: f32,
        transform: Affine,
        clip: &impl Shape,
        _filter: Option<Arc<Filter>>,
        _backdrop_filter: Option<Arc<Filter>>,
    ) {
        self.scene.push_layer(
            target_peniko::BlendMode::default(),
            alpha,
            affine(transform),
            &shape(clip),
        );
    }

    fn push_clip_layer(&mut self, transform: Affine, clip: &impl Shape) {
        self.scene.push_clip_layer(affine(transform), &shape(clip));
    }

    fn pop_layer(&mut self) {
        self.scene.pop_layer();
    }

    fn stroke<'a>(
        &mut self,
        style: &Stroke,
        transform: Affine,
        paint: impl Into<PaintRef<'a>>,
        brush_transform: Option<Affine>,
        source_shape: &impl Shape,
    ) {
        let paint = paint.into();
        let brush = solid_brush(&paint);
        self.scene.stroke(
            &target_kurbo::Stroke::new(style.width),
            affine(transform),
            brush,
            brush_transform.map(affine),
            &shape(source_shape),
        );
    }

    fn fill<'a>(
        &mut self,
        fill: Fill,
        transform: Affine,
        paint: impl Into<PaintRef<'a>>,
        brush_transform: Option<Affine>,
        source_shape: &impl Shape,
    ) {
        let fill = match fill {
            Fill::NonZero => target_peniko::Fill::NonZero,
            Fill::EvenOdd => target_peniko::Fill::EvenOdd,
        };
        let transform = affine(transform);
        let brush_transform = brush_transform.map(affine);
        let path = shape(source_shape);
        match paint.into() {
            Paint::Solid(value) => {
                self.scene
                    .fill(fill, transform, color(value), brush_transform, &path);
            }
            Paint::Image(value) => {
                let image = image_brush(value);
                self.scene
                    .fill(fill, transform, &image, brush_transform, &path);
            }
            Paint::Gradient(_) | Paint::Resource(_) | Paint::Custom(_) => {
                self.scene.fill(
                    fill,
                    transform,
                    target_peniko::Color::TRANSPARENT,
                    brush_transform,
                    &path,
                );
            }
        }
    }

    fn draw_glyphs<'a, 's: 'a>(
        &'s mut self,
        font: &'a FontData,
        font_size: f32,
        hint: bool,
        normalized_coords: &'a [NormalizedCoord],
        _embolden: Vec2,
        style: impl Into<StyleRef<'a>>,
        paint: impl Into<PaintRef<'a>>,
        brush_alpha: f32,
        transform: Affine,
        glyph_transform: Option<Affine>,
        glyphs: impl Iterator<Item = anyrender::Glyph>,
    ) {
        let paint = paint.into();
        let builder = self
            .scene
            .draw_glyphs(font)
            .font_size(font_size)
            .hint(hint)
            .normalized_coords(normalized_coords)
            .brush(solid_brush(&paint))
            .brush_alpha(brush_alpha)
            .transform(affine(transform))
            .glyph_transform(glyph_transform.map(affine));
        let glyphs = glyphs.map(|glyph| vello::Glyph {
            id: glyph.id,
            x: glyph.x,
            y: glyph.y,
        });
        match style.into() {
            StyleRef::Fill(Fill::NonZero) => builder.draw(target_peniko::Fill::NonZero, glyphs),
            StyleRef::Fill(Fill::EvenOdd) => builder.draw(target_peniko::Fill::EvenOdd, glyphs),
            StyleRef::Stroke(stroke) => {
                builder.draw(&target_kurbo::Stroke::new(stroke.width), glyphs);
            }
        }
    }

    fn draw_box_shadow(
        &mut self,
        transform: Affine,
        rect: Rect,
        brush: Color,
        radius: f64,
        std_dev: f64,
    ) {
        self.scene.draw_blurred_rounded_rect(
            affine(transform),
            target_kurbo::Rect::new(rect.x0, rect.y0, rect.x1, rect.y1),
            color(brush),
            radius,
            std_dev,
        );
    }
}

fn affine(value: Affine) -> target_kurbo::Affine {
    target_kurbo::Affine::new(value.as_coeffs())
}

fn point(value: kurbo::Point) -> target_kurbo::Point {
    target_kurbo::Point::new(value.x, value.y)
}

fn shape(value: &impl Shape) -> target_kurbo::BezPath {
    let mut path = target_kurbo::BezPath::new();
    for element in value.path_elements(0.1) {
        match element {
            PathEl::MoveTo(p) => path.move_to(point(p)),
            PathEl::LineTo(p) => path.line_to(point(p)),
            PathEl::QuadTo(p1, p2) => path.quad_to(point(p1), point(p2)),
            PathEl::CurveTo(p1, p2, p3) => path.curve_to(point(p1), point(p2), point(p3)),
            PathEl::ClosePath => path.close_path(),
        }
    }
    path
}

fn color(value: Color) -> target_peniko::Color {
    let rgba = value.to_rgba8();
    target_peniko::Color::from_rgba8(rgba.r, rgba.g, rgba.b, rgba.a)
}

fn solid_brush(paint: &PaintRef<'_>) -> target_peniko::Color {
    match paint {
        Paint::Solid(value) => color(*value),
        Paint::Gradient(_) | Paint::Image(_) | Paint::Resource(_) | Paint::Custom(_) => {
            target_peniko::Color::TRANSPARENT
        }
    }
}

fn image_brush(value: peniko::ImageBrushRef<'_>) -> target_peniko::ImageBrush {
    let format = if value.image.format == ImageFormat::Bgra8 {
        target_peniko::ImageFormat::Bgra8
    } else {
        target_peniko::ImageFormat::Rgba8
    };
    let alpha_type = match value.image.alpha_type {
        ImageAlphaType::Alpha => target_peniko::ImageAlphaType::Alpha,
        ImageAlphaType::AlphaPremultiplied => target_peniko::ImageAlphaType::AlphaPremultiplied,
    };
    target_peniko::ImageBrush::new(target_peniko::ImageData {
        data: value.image.data.clone(),
        format,
        alpha_type,
        width: value.image.width,
        height: value.image.height,
    })
}
