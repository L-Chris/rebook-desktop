use std::fs;
use std::path::PathBuf;

use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::{ColorType, ImageEncoder};

// cargo-bundle maps each source PNG to an icns entry by exact pixel size and
// fails with "No matching IconType" for anything outside this set.
const ICON_SIZES: [u32; 7] = [16, 32, 64, 128, 256, 512, 1024];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source_path = root.join("assets/logo.png");
    let output_dir = root.join("assets/macos");
    fs::create_dir_all(&output_dir)?;

    let source = image::open(&source_path)?.into_rgba8();
    let (width, height) = source.dimensions();
    if width != height {
        return Err(format!("logo must be square, got {width}x{height}").into());
    }

    for size in ICON_SIZES {
        let resized = image::imageops::resize(&source, size, size, FilterType::Lanczos3);
        let mut png = Vec::new();
        PngEncoder::new(&mut png).write_image(
            resized.as_raw(),
            size,
            size,
            ColorType::Rgba8.into(),
        )?;
        // cargo-bundle maps 1024x1024 only at retina density: the icns type
        // for 512@2x is selected by an "@2x" filename suffix, and there is no
        // 1024@1x type at all.
        let name = if size == 1024 {
            "torto-512@2x.png".to_owned()
        } else {
            format!("torto-{size}.png")
        };
        fs::write(output_dir.join(name), &png)?;
    }

    println!(
        "generated {} macOS icon sizes from {}x{} source",
        ICON_SIZES.len(),
        width,
        height
    );
    Ok(())
}
