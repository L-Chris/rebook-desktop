use std::collections::HashMap;
use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use iepub::parser::HtmlParser;
use iepub::prelude::MobiReader;
use iepub::{ContentItem, ContentType};
use rebook_publication::{Metadata, RenditionLayout};
use sha2::{Digest, Sha256};

use crate::source::{
    DirectBookSource, SectionContent, SourceBook, SourceResource, SourceSection, escape_attribute,
    escape_text,
};
use crate::{BookFormat, FormatError, conversion_error, kf8};

pub(crate) fn open(
    bytes: &[u8],
    file_name: &str,
    format: BookFormat,
) -> Result<DirectBookSource, FormatError> {
    catch_unwind(AssertUnwindSafe(|| convert(bytes, file_name, format))).unwrap_or_else(|panic| {
        let message = panic
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("解析器意外终止");
        Err(conversion_error(format, message))
    })
}

#[allow(clippy::too_many_lines)]
fn convert(
    bytes: &[u8],
    file_name: &str,
    format: BookFormat,
) -> Result<DirectBookSource, FormatError> {
    let mut reader =
        MobiReader::new(Cursor::new(bytes)).map_err(|error| conversion_error(format, error))?;
    let mobi = reader
        .load()
        .map_err(|error| conversion_error(format, error))?;

    let parsed_kf8 = if kf8::is_kf8(bytes) {
        Some(kf8::parse(bytes, format)?)
    } else {
        None
    };

    let mut resources = Vec::new();
    let mut image_sources = HashMap::new();
    if parsed_kf8.is_none() {
        for asset in mobi.assets() {
            let Some(data) = asset.data() else {
                continue;
            };
            let Some((extension, media_type)) = image_type(asset.file_name(), data) else {
                continue;
            };
            let path = format!("Images/image-{}.{}", resources.len() + 1, extension);
            if let Some(index) = Path::new(asset.file_name())
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| stem.parse::<usize>().ok())
            {
                image_sources.insert(index, path.clone());
            }
            resources.push(SourceResource {
                path,
                media_type: media_type.to_owned(),
                bytes: data.to_vec(),
            });
        }
    }

    let cover_path = mobi.cover().and_then(|cover| {
        let data = cover.data()?;
        let (extension, media_type) = image_type(cover.file_name(), data)?;
        let path = format!("Images/cover.{extension}");
        resources.push(SourceResource {
            path: path.clone(),
            media_type: media_type.to_owned(),
            bytes: data.to_vec(),
        });
        Some(path)
    });

    let mut sections = Vec::new();
    let mut table_of_contents = Vec::new();
    if let Some(parsed) = parsed_kf8 {
        let kf8::Kf8Book {
            sections: kf8_sections,
            table_of_contents: kf8_toc,
            resources: kf8_resources,
        } = parsed;
        resources.extend(kf8_resources);
        table_of_contents = kf8_toc;
        for section in kf8_sections {
            let body = normalize_chapter(&section.html, &image_sources, format)?;
            sections.push(SourceSection {
                title: section.title,
                content: SectionContent::Html(body),
                linear: true,
            });
        }
    } else {
        for (index, chapter) in mobi.chapters().enumerate() {
            let title = if chapter.title().trim().is_empty() {
                format!("第 {} 节", index + 1)
            } else {
                chapter.title().trim().to_owned()
            };
            let body = normalize_chapter(&chapter.string_data(), &image_sources, format)?;
            if !body.trim().is_empty() {
                sections.push(SourceSection {
                    title,
                    content: SectionContent::Html(body),
                    linear: true,
                });
            }
        }
    }
    if sections.is_empty() {
        return Err(conversion_error(format, "没有可阅读的正文"));
    }
    let title = if mobi.title().trim().is_empty() {
        Path::new(file_name)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("未命名书籍")
            .to_owned()
    } else {
        mobi.title().trim().to_owned()
    };
    let authors = mobi
        .creator()
        .map(|creator| {
            creator
                .split([';', ','])
                .map(str::trim)
                .filter(|author| !author.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default();
    DirectBookSource::open(
        SourceBook {
            id: format!("{:x}", Sha256::digest(bytes)),
            metadata: Metadata {
                title,
                authors,
                languages: Vec::new(),
                layout: RenditionLayout::Reflowable,
            },
            sections,
            table_of_contents,
            resources,
            cover_path,
        },
        format,
    )
}

fn normalize_chapter(
    source: &str,
    images: &HashMap<usize, String>,
    format: BookFormat,
) -> Result<String, FormatError> {
    let mut source = source.to_owned();
    for (index, path) in images {
        let replacement = format!("src=\"../{path}\"");
        for recindex in [format!("{index:05}"), index.to_string()] {
            source = source
                .replace(&format!("recindex=\"{recindex}\""), &replacement)
                .replace(&format!("recindex='{recindex}'"), &replacement)
                .replace(&format!("recindex={recindex}"), &replacement);
        }
    }
    let source = protect_entities(&source);
    let mut parser = HtmlParser::new();
    parser
        .parse(&source)
        .map_err(|error| conversion_error(format, error))?;
    let body = parser.items.iter().map(render_item).collect::<String>();
    if body.trim().is_empty() {
        let plain = strip_markup(&source);
        return if plain.trim().is_empty() {
            Ok(String::new())
        } else {
            Ok(format!("<p>{}</p>", escape_text(plain.trim())))
        };
    }
    Ok(body)
}

fn render_item(item: &ContentItem) -> String {
    let content = format!(
        "{}{}",
        escape_text(&decode_entities(item.text.trim())),
        item.children.iter().map(render_item).collect::<String>()
    );
    match &item.content_type {
        ContentType::Paragraph => wrap("p", item, &content),
        ContentType::Heading(level) => wrap(&format!("h{}", (*level).clamp(1, 6)), item, &content),
        ContentType::Image => {
            let Some(src) = attribute(item, "src") else {
                return String::new();
            };
            if !src.starts_with("../Images/") {
                return String::new();
            }
            let alt = attribute(item, "alt").unwrap_or_default();
            format!(
                "<img src=\"{}\" alt=\"{}\"/>",
                escape_attribute(src),
                escape_attribute(alt)
            )
        }
        ContentType::Link => {
            let href = attribute(item, "href").unwrap_or_default();
            let id = authored_identifier(item);
            if !href.starts_with('#') && id.is_none() {
                content
            } else {
                let id = id
                    .map(|id| format!(" id=\"{}\"", escape_attribute(id)))
                    .unwrap_or_default();
                let href = if href.starts_with('#') {
                    format!(" href=\"{}\"", escape_attribute(href))
                } else {
                    String::new()
                };
                format!("<a{id}{href}>{content}</a>")
            }
        }
        ContentType::ListItem => wrap("li", item, &content),
        ContentType::BlockQuote => wrap("blockquote", item, &content),
        ContentType::CodeBlock => wrap("pre", item, &content),
        ContentType::HorizontalRule => "<hr/>".to_owned(),
        ContentType::Text => content,
        ContentType::Other(tag) => match tag.to_ascii_lowercase().as_str() {
            "br" | "mbp:pagebreak" => "<br/>".to_owned(),
            "div" | "section" | "article" => wrap("div", item, &content),
            "ul" => wrap("ul", item, &content),
            "ol" => wrap("ol", item, &content),
            "strong" | "b" => wrap("strong", item, &content),
            "em" | "i" => wrap("em", item, &content),
            "sup" => wrap("sup", item, &content),
            "sub" => wrap("sub", item, &content),
            _ => content,
        },
    }
}

fn wrap(tag: &str, item: &ContentItem, content: &str) -> String {
    let id = authored_identifier(item)
        .map(|id| format!(" id=\"{}\"", escape_attribute(id)))
        .unwrap_or_default();
    format!("<{tag}{id}>{content}</{tag}>")
}

fn authored_identifier(item: &ContentItem) -> Option<&str> {
    attribute(item, "id")
        .or_else(|| attribute(item, "name"))
        .or_else(|| attribute(item, "aid"))
}

fn attribute<'a>(item: &'a ContentItem, name: &str) -> Option<&'a str> {
    item.attributes
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn decode_entities(value: &str) -> String {
    value
        .replace('\u{e000}', "&")
        .replace('\u{e001}', "<")
        .replace('\u{e002}', ">")
        .replace('\u{e003}', "\"")
        .replace('\u{e004}', "'")
        .replace('\u{e005}', " ")
        .replace("&nbsp;", " ")
        .replace("&#160;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn protect_entities(value: &str) -> String {
    value
        .replace("&amp;", "\u{e000}")
        .replace("&lt;", "\u{e001}")
        .replace("&gt;", "\u{e002}")
        .replace("&quot;", "\u{e003}")
        .replace("&apos;", "\u{e004}")
        .replace("&nbsp;", "\u{e005}")
        .replace("&#160;", "\u{e005}")
}

fn strip_markup(value: &str) -> String {
    let mut output = String::new();
    let mut in_tag = false;
    for character in value.chars() {
        match character {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                output.push(' ');
            }
            _ if !in_tag => output.push(character),
            _ => {}
        }
    }
    decode_entities(&output)
}

fn image_type(file_name: &str, bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    let extension = Path::new(file_name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "jpg" | "jpeg" => return Some(("jpg", "image/jpeg")),
        "png" => return Some(("png", "image/png")),
        "gif" => return Some(("gif", "image/gif")),
        "webp" => return Some(("webp", "image/webp")),
        "bmp" => return Some(("bmp", "image/bmp")),
        _ => {}
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("png", "image/png"))
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some(("jpg", "image/jpeg"))
    } else if bytes.starts_with(b"GIF8") {
        Some(("gif", "image/gif"))
    } else if bytes.starts_with(b"BM") {
        Some(("bmp", "image/bmp"))
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(("webp", "image/webp"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_mobi_html_and_embedded_images() {
        let images = HashMap::from([(1, "Images/image-1.jpg".to_owned())]);
        let body = normalize_chapter(
            "<a id=\"chapter-start\"></a><h1 aid=\"kindle-heading\">Title</h1><p>Hello &amp; world</p><img recindex=\"00001\">",
            &images,
            BookFormat::Mobi,
        )
        .unwrap();
        assert!(body.contains("<h1 id=\"kindle-heading\">Title</h1>"));
        assert!(body.contains("<a id=\"chapter-start\"></a>"), "{body}");
        assert!(body.contains("Hello &amp; world"), "{body}");
        assert!(body.contains("src=\"../Images/image-1.jpg\""));
    }
}
