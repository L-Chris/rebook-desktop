use rebook_publication::{
    Block, BookSource, Inline, SourceAnchor, SourceRange, TextBlock, TextBlockKind, TocEntry,
};
use regex::RegexBuilder;

const DEFAULT_CONTEXT_CHARS: usize = 72;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookSearchResult {
    pub section_index: usize,
    pub section_title: String,
    pub excerpt: String,
    pub matched_text: String,
    pub range: SourceRange,
}

pub fn search_book(
    source: &dyn BookSource,
    query: &str,
    max_results: usize,
) -> Result<Vec<BookSearchResult>, String> {
    let query = query.trim();
    if query.is_empty() || max_results == 0 {
        return Ok(Vec::new());
    }
    let matcher = RegexBuilder::new(&regex::escape(query))
        .case_insensitive(true)
        .unicode(true)
        .build()
        .map_err(|error| format!("搜索表达式无效：{error}"))?;
    let book = source.book();
    let mut results = Vec::new();

    for section_index in 0..book.sections.len() {
        let section = source
            .parse_section(section_index)
            .map_err(|error| format!("解析第 {} 节失败：{error}", section_index + 1))?;
        let section_title = section_title(source, section_index, &section.blocks);
        for block in &section.blocks {
            let Block::Text(block) = block else {
                continue;
            };
            let Some(source_range) = &block.source else {
                continue;
            };
            let text = text_block_text(block);
            for found in matcher.find_iter(&text) {
                let range = source_range_for_match(source_range, &text, found.start(), found.end());
                results.push(BookSearchResult {
                    section_index,
                    section_title: section_title.clone(),
                    excerpt: excerpt(&text, found.start(), found.end(), DEFAULT_CONTEXT_CHARS),
                    matched_text: found.as_str().to_owned(),
                    range,
                });
                if results.len() >= max_results {
                    return Ok(results);
                }
            }
        }
    }
    Ok(results)
}

pub(crate) fn section_text(
    source: &dyn BookSource,
    section_index: usize,
) -> Result<String, String> {
    let section = source
        .parse_section(section_index)
        .map_err(|error| format!("解析第 {} 节失败：{error}", section_index + 1))?;
    Ok(section
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Text(block) => Some(text_block_text(block)),
            Block::Image(image) if !image.alt.trim().is_empty() => Some(image.alt.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n"))
}

pub(crate) fn text_block_text(block: &TextBlock) -> String {
    block
        .content
        .iter()
        .map(|inline| match inline {
            Inline::Text(run) => run.text.as_str(),
            Inline::Break => "\n",
        })
        .collect()
}

fn section_title(source: &dyn BookSource, section_index: usize, blocks: &[Block]) -> String {
    if let Some(title) = blocks.iter().find_map(|block| match block {
        Block::Text(block) if matches!(block.kind, TextBlockKind::Heading(_)) => {
            let text = text_block_text(block);
            (!text.trim().is_empty()).then(|| text.trim().to_owned())
        }
        _ => None,
    }) {
        return title;
    }
    let book = source.book();
    let href = &book.sections[section_index].href;
    toc_label_for_href(&book.table_of_contents, href)
        .unwrap_or_else(|| format!("第 {} 节", section_index + 1))
}

fn toc_label_for_href(
    entries: &[TocEntry],
    href: &rebook_publication::PublicationUrl,
) -> Option<String> {
    for entry in entries {
        if entry
            .href
            .as_ref()
            .is_some_and(|target| target.resource_url() == href.resource_url())
        {
            return Some(entry.label.clone());
        }
        if let Some(label) = toc_label_for_href(&entry.children, href) {
            return Some(label);
        }
    }
    None
}

fn source_range_for_match(
    source: &SourceRange,
    text: &str,
    byte_start: usize,
    byte_end: usize,
) -> SourceRange {
    if source.start.spine != source.end.spine || source.start.node != source.end.node {
        return source.clone();
    }
    let start_offset = source.start.text_offset
        + u64::try_from(text[..byte_start].chars().count()).unwrap_or(u64::MAX);
    let end_offset = source.start.text_offset
        + u64::try_from(text[..byte_end].chars().count()).unwrap_or(u64::MAX);
    if start_offset >= end_offset || end_offset > source.end.text_offset {
        return source.clone();
    }
    SourceRange {
        start: SourceAnchor {
            spine: source.start.spine.clone(),
            node: source.start.node.clone(),
            text_offset: start_offset,
        },
        end: SourceAnchor {
            spine: source.end.spine.clone(),
            node: source.end.node.clone(),
            text_offset: end_offset,
        },
    }
}

fn excerpt(text: &str, start: usize, end: usize, context_chars: usize) -> String {
    let context_start = text[..start]
        .char_indices()
        .rev()
        .nth(context_chars.saturating_sub(1))
        .map_or(0, |(index, _)| index);
    let context_end = text[end..]
        .char_indices()
        .nth(context_chars)
        .map_or(text.len(), |(index, _)| end + index);
    format!(
        "{}{}{}",
        if context_start > 0 { "…" } else { "" },
        text[context_start..context_end].trim(),
        if context_end < text.len() { "…" } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use rebook_publication::{
        BlockStyle, Book, Metadata, PublicationError, PublicationId, PublicationUrl, Resource,
        Section, SpineItem, SpineItemId, TextRun, TextStyle,
    };

    use super::*;

    struct SearchSource {
        book: Book,
        sections: Vec<Section>,
    }

    impl BookSource for SearchSource {
        fn book(&self) -> &Book {
            &self.book
        }

        fn parse_section(&self, index: usize) -> Result<Section, PublicationError> {
            Ok(self.sections[index].clone())
        }

        fn resource(&self, _href: &PublicationUrl) -> Result<Resource, PublicationError> {
            unreachable!()
        }
    }

    #[test]
    fn search_returns_source_backed_unicode_matches() {
        let spine = SpineItemId::new("chapter-1").unwrap();
        let href = PublicationUrl::parse("chapter-1.xhtml").unwrap();
        let text = "Systems thinking helps us see systems.";
        let source = SourceRange {
            start: SourceAnchor {
                spine: spine.clone(),
                node: "paragraph-1".into(),
                text_offset: 4,
            },
            end: SourceAnchor {
                spine: spine.clone(),
                node: "paragraph-1".into(),
                text_offset: 4 + u64::try_from(text.chars().count()).unwrap(),
            },
        };
        let search_source = SearchSource {
            book: Book {
                id: PublicationId::new("search-book").unwrap(),
                metadata: Metadata::default(),
                cover: None,
                sections: vec![SpineItem {
                    id: spine.clone(),
                    href: href.clone(),
                    media_type: "application/xhtml+xml".into(),
                    linear: true,
                    properties: Vec::new(),
                }],
                table_of_contents: Vec::new(),
            },
            sections: vec![Section {
                id: spine,
                href,
                blocks: vec![Block::Text(TextBlock {
                    kind: TextBlockKind::Paragraph,
                    content: vec![Inline::Text(TextRun {
                        text: text.into(),
                        style: TextStyle::default(),
                        link: None,
                    })],
                    style: BlockStyle::default(),
                    source: Some(source),
                })],
                anchors: Vec::new(),
            }],
        };

        let results = search_book(&search_source, "SYSTEMS", 10).unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].matched_text, "Systems");
        assert_eq!(results[0].range.start.text_offset, 4);
        assert_eq!(results[0].range.end.text_offset, 11);
        assert_eq!(results[1].range.start.text_offset, 34);
    }

    #[test]
    fn excerpt_keeps_unicode_boundaries_and_context() {
        assert_eq!(excerpt("alpha beta gamma", 6, 10, 2), "…a beta g…");
    }
}
