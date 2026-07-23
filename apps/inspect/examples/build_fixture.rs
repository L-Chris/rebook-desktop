//! Builds the self-authored Phase 0 EPUB fixture without relying on a system ZIP command.

use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const CONTENT_ENTRIES: [&str; 5] = [
    "META-INF/container.xml",
    "OPS/package.opf",
    "OPS/nav.xhtml",
    "OPS/Styles/book.css",
    "OPS/Text/chapter.xhtml",
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fixture_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-data/minimal-epub")
        .canonicalize()?;
    let default_output =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/fixtures/minimal.epub");
    let output = env::args_os().nth(1).map_or(default_output, PathBuf::from);
    build_fixture(&fixture_root, &output)?;
    println!("{}", output.canonicalize()?.display());
    Ok(())
}

fn build_fixture(root: &Path, output: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = File::create(output)?;
    let mut archive = ZipWriter::new(file);

    // OCF requires this exact byte sequence as the first, uncompressed ZIP entry. The source
    // text file may contain a trailing newline, so deliberately write the normative value here.
    archive.start_file(
        "mimetype",
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
    )?;
    archive.write_all(b"application/epub+zip")?;

    for entry in CONTENT_ENTRIES {
        archive.start_file(
            entry,
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
        )?;
        archive.write_all(&fs::read(root.join(entry))?)?;
    }
    archive.finish()?;
    Ok(())
}
