//! Builds a self-authored reflow benchmark EPUB with a configurable minimum text length.

use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const MIMETYPE: &[u8] = b"application/epub+zip";
const CONTAINER: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OPS/package.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;
const PACKAGE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Rebook 两万字排版基准</dc:title><dc:language>zh-CN</dc:language>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/>
    <item id="style" href="book.css" media-type="text/css"/>
  </manifest>
  <spine><itemref idref="chapter"/></spine>
</package>"#;
const NAV: &str = r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<body><nav epub:type="toc"><ol><li><a href="chapter.xhtml">排版基准章</a></li></ol></nav></body></html>"#;
const CSS: &[u8] =
    b"body { max-width: 46rem; margin: 0 auto; padding: 3rem; font: 20px/1.75 serif; }
p { margin: 0 0 1em; text-indent: 2em; }";
const PARAGRAPH: &str = "清晨的光穿过窗帘，落在摊开的书页上。这个章节由程序生成，用来测量 Rust 原生电子书引擎的中文分词、样式级联、文字塑形、断行布局与首屏绘制性能。";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let default_output =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/fixtures/benchmark-20k.epub");
    let mut arguments = env::args_os().skip(1);
    let output = arguments.next().map_or(default_output, PathBuf::from);
    let minimum_characters = arguments
        .next()
        .map(|value| value.to_string_lossy().parse::<usize>())
        .transpose()?
        .unwrap_or(20_000);
    if arguments.next().is_some() || minimum_characters == 0 {
        return Err("usage: build_benchmark [output.epub] [minimum-characters]".into());
    }

    let (chapter, actual_characters) = chapter(minimum_characters);
    build_epub(&output, chapter.as_bytes())?;
    println!(
        "{} ({} text characters)",
        output.canonicalize()?.display(),
        actual_characters
    );
    Ok(())
}

fn chapter(minimum_characters: usize) -> (String, usize) {
    let paragraph_characters = PARAGRAPH.chars().count();
    let paragraph_count = minimum_characters.div_ceil(paragraph_characters);
    let mut document = String::from(
        r#"<html xmlns="http://www.w3.org/1999/xhtml" lang="zh-CN" xml:lang="zh-CN"><head>
        <title>排版基准章</title><link rel="stylesheet" href="book.css"/></head><body>"#,
    );
    for _ in 0..paragraph_count {
        document.push_str("<p>");
        document.push_str(PARAGRAPH);
        document.push_str("</p>");
    }
    document.push_str("</body></html>");
    (document, paragraph_count * paragraph_characters)
}

fn build_epub(output: &Path, chapter: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut archive = ZipWriter::new(File::create(output)?);
    write_entry(
        &mut archive,
        "mimetype",
        MIMETYPE,
        CompressionMethod::Stored,
    )?;
    for (name, bytes) in [
        ("META-INF/container.xml", CONTAINER),
        ("OPS/package.opf", PACKAGE.as_bytes()),
        ("OPS/nav.xhtml", NAV.as_bytes()),
        ("OPS/book.css", CSS),
        ("OPS/chapter.xhtml", chapter),
    ] {
        write_entry(&mut archive, name, bytes, CompressionMethod::Deflated)?;
    }
    archive.finish()?;
    Ok(())
}

fn write_entry(
    archive: &mut ZipWriter<File>,
    name: &str,
    bytes: &[u8],
    compression: CompressionMethod,
) -> Result<(), Box<dyn std::error::Error>> {
    archive.start_file(
        name,
        SimpleFileOptions::default().compression_method(compression),
    )?;
    archive.write_all(bytes)?;
    Ok(())
}
