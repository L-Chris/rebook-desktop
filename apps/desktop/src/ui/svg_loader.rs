//! SVG image loader shared by book content, chat formulas, and Mermaid previews.
//!
//! The loader is adapted from `egui_extras` 0.35 (MIT OR Apache-2.0), but uses
//! the same `resvg` version as the rest of Torto to avoid compiling a second SVG
//! and font-rendering stack.

use std::collections::HashMap;
use std::mem::size_of;
use std::path::Path;
use std::sync::Arc;

use egui::load::{BytesPoll, ImageLoadResult, ImageLoader, ImagePoll, LoadError, SizeHint};
use egui::mutex::Mutex;
use egui::{ColorImage, Vec2};

struct Entry {
    last_used: u64,
    result: Result<Arc<ColorImage>, String>,
}

struct State {
    pass_index: u64,
    cache: HashMap<String, HashMap<SizeHint, Entry>>,
    options: resvg::usvg::Options<'static>,
}

struct SvgLoader {
    state: Mutex<State>,
}

impl SvgLoader {
    const ID: &'static str = egui::generate_loader_id!(SvgLoader);
}

impl Default for SvgLoader {
    fn default() -> Self {
        let mut options = resvg::usvg::Options::default();
        options.fontdb_mut().load_system_fonts();
        Self {
            state: Mutex::new(State {
                pass_index: 0,
                cache: HashMap::new(),
                options,
            }),
        }
    }
}

pub(super) fn install(ctx: &egui::Context) {
    if !ctx.is_loader_installed(SvgLoader::ID) {
        ctx.add_image_loader(Arc::new(SvgLoader::default()));
    }
}

impl ImageLoader for SvgLoader {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(&self, ctx: &egui::Context, uri: &str, size_hint: SizeHint) -> ImageLoadResult {
        if !Path::new(uri)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
        {
            return Err(LoadError::NotSupported);
        }

        let mut state = self.state.lock();
        let State {
            pass_index,
            cache,
            options,
        } = &mut *state;
        let bucket = cache.entry(uri.to_owned()).or_default();

        if let Some(entry) = bucket.get_mut(&size_hint) {
            entry.last_used = *pass_index;
            return entry
                .result
                .clone()
                .map(|image| ImagePoll::Ready { image })
                .map_err(LoadError::Loading);
        }

        match ctx.try_load_bytes(uri)? {
            BytesPoll::Ready { bytes, .. } => {
                let result = rasterize(&bytes, size_hint, options).map(Arc::new);
                bucket.insert(
                    size_hint,
                    Entry {
                        last_used: *pass_index,
                        result: result.clone(),
                    },
                );
                result
                    .map(|image| ImagePoll::Ready { image })
                    .map_err(LoadError::Loading)
            }
            BytesPoll::Pending { size } => Ok(ImagePoll::Pending { size }),
        }
    }

    fn forget(&self, uri: &str) {
        self.state.lock().cache.remove(uri);
    }

    fn forget_all(&self) {
        self.state.lock().cache.clear();
    }

    fn byte_size(&self) -> usize {
        self.state
            .lock()
            .cache
            .values()
            .flat_map(HashMap::values)
            .map(|entry| match &entry.result {
                Ok(image) => image.pixels.len() * size_of::<egui::Color32>(),
                Err(error) => error.len(),
            })
            .sum()
    }

    fn end_pass(&self, pass_index: u64) {
        let mut state = self.state.lock();
        state.pass_index = pass_index;
        state.cache.retain(|_, bucket| {
            if bucket.len() >= 2 {
                bucket.retain(|_, entry| pass_index <= entry.last_used + 1);
            }
            !bucket.is_empty()
        });
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "egui size hints and raster dimensions are positive u32-sized textures"
)]
fn rasterize(
    svg_bytes: &[u8],
    size_hint: SizeHint,
    options: &resvg::usvg::Options<'_>,
) -> Result<ColorImage, String> {
    use resvg::tiny_skia::Pixmap;
    use resvg::usvg::{Transform, Tree};

    let tree = Tree::from_data(svg_bytes, options).map_err(|error| error.to_string())?;
    let source_size = Vec2::new(tree.size().width(), tree.size().height());
    let scaled_size = hinted_size(source_size, size_hint)
        .round()
        .max(Vec2::splat(1.0));
    let width = scaled_size.x as u32;
    let height = scaled_size.y as u32;
    let mut pixmap = Pixmap::new(width, height)
        .ok_or_else(|| format!("failed to create SVG pixmap of size {width}x{height}"))?;

    resvg::render(
        &tree,
        Transform::from_scale(width as f32 / source_size.x, height as f32 / source_size.y),
        &mut pixmap.as_mut(),
    );

    Ok(
        ColorImage::from_rgba_premultiplied([width as usize, height as usize], pixmap.data())
            .with_source_size(source_size),
    )
}

#[allow(
    clippy::cast_precision_loss,
    reason = "texture dimensions are converted to the f32 coordinate space used by egui and resvg"
)]
fn hinted_size(source: Vec2, hint: SizeHint) -> Vec2 {
    match hint {
        SizeHint::Size {
            width,
            height,
            maintain_aspect_ratio,
        } if maintain_aspect_ratio => {
            let scale = (width as f32 / source.x).min(height as f32 / source.y);
            source * scale
        }
        SizeHint::Size { width, height, .. } => Vec2::new(width as f32, height as f32),
        SizeHint::Height(height) => source * (height as f32 / source.y),
        SizeHint::Width(width) => source * (width as f32 / source.x),
        SizeHint::Scale(scale) => source * scale.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rasterizes_svg_with_the_requested_aspect_ratio() {
        let image = rasterize(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="#448967"/></svg>"##,
            SizeHint::Width(40),
            &resvg::usvg::Options::default(),
        )
        .unwrap();

        assert_eq!(image.size, [40, 20]);
    }
}
