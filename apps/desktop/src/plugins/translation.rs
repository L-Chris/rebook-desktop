use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use rebook_publication::{
    Block, BlockStyle, Book, BookSource, Inline, PublicationError, PublicationUrl, RasterResource,
    Resource, Section, SectionAnchor, SourceRange, TextBlock, TextBlockKind, TextRun, TextStyle,
};

use super::TranslationMode;
use super::search::text_block_text;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranslationBlockInput {
    pub block_index: usize,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockTranslation {
    pub block_index: usize,
    pub text: String,
}

#[derive(Default)]
struct TranslationState {
    enabled: bool,
    mode: TranslationMode,
    sections: HashMap<usize, HashMap<usize, String>>,
}

/// An in-memory view of a publication that overlays translated text blocks.
/// The canonical source remains unchanged, so translation can be toggled off
/// without reopening the book.
pub struct TranslationBookSource {
    inner: Arc<dyn BookSource>,
    state: RwLock<TranslationState>,
}

impl TranslationBookSource {
    pub fn new(inner: Arc<dyn BookSource>, mode: TranslationMode) -> Self {
        Self {
            inner,
            state: RwLock::new(TranslationState {
                mode,
                ..TranslationState::default()
            }),
        }
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        self.state
            .write()
            .map_err(|_| "正文翻译状态已损坏".to_owned())?
            .enabled = enabled;
        Ok(())
    }

    pub fn set_mode(&self, mode: TranslationMode) -> Result<(), String> {
        self.state
            .write()
            .map_err(|_| "正文翻译状态已损坏".to_owned())?
            .mode = mode;
        Ok(())
    }

    pub fn has_section(&self, section_index: usize) -> bool {
        self.state
            .read()
            .is_ok_and(|state| state.sections.contains_key(&section_index))
    }

    pub fn clear(&self) -> Result<(), String> {
        self.state
            .write()
            .map_err(|_| "正文翻译状态已损坏".to_owned())?
            .sections
            .clear();
        Ok(())
    }

    pub fn translatable_blocks(
        &self,
        section_index: usize,
    ) -> Result<Vec<TranslationBlockInput>, String> {
        let section = self
            .inner
            .parse_section(section_index)
            .map_err(|error| format!("解析第 {} 节失败：{error}", section_index + 1))?;
        Ok(section
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(block_index, block)| {
                let text = match block {
                    Block::Text(block) => text_block_text(block),
                    Block::Image(image) => image
                        .text_layer
                        .as_ref()
                        .map(|layer| layer.text.clone())
                        .unwrap_or_default(),
                    Block::Separator | Block::PageBreak => String::new(),
                };
                (!text.trim().is_empty()).then_some(TranslationBlockInput { block_index, text })
            })
            .collect())
    }

    pub fn store_section(
        &self,
        section_index: usize,
        translations: &[BlockTranslation],
    ) -> Result<(), String> {
        let values = translations
            .iter()
            .filter(|translation| !translation.text.trim().is_empty())
            .map(|translation| (translation.block_index, translation.text.clone()))
            .collect();
        self.state
            .write()
            .map_err(|_| "正文翻译状态已损坏".to_owned())?
            .sections
            .insert(section_index, values);
        Ok(())
    }
}

impl BookSource for TranslationBookSource {
    fn book(&self) -> &Book {
        self.inner.book()
    }

    fn parse_section(&self, index: usize) -> Result<Section, PublicationError> {
        let mut section = self.inner.parse_section(index)?;
        let state = self
            .state
            .read()
            .map_err(|_| PublicationError::InvalidPublication("正文翻译状态已损坏".to_owned()))?;
        if !state.enabled {
            return Ok(section);
        }
        let Some(translations) = state.sections.get(&index) else {
            return Ok(section);
        };

        let mut rendered = Vec::with_capacity(section.blocks.len() * 2);
        for (block_index, block) in section.blocks.into_iter().enumerate() {
            let Some(translation) = translations.get(&block_index) else {
                rendered.push(block);
                continue;
            };
            match block {
                Block::Text(mut original) => {
                    let style = original
                        .content
                        .iter()
                        .find_map(|inline| match inline {
                            Inline::Text(run) => Some(run.style),
                            Inline::Break => None,
                        })
                        .unwrap_or_default();
                    match state.mode {
                        TranslationMode::Replace => {
                            original.content = replacement_content(translation, style);
                            update_translated_source(
                                original.source.as_mut(),
                                translation,
                                &mut section.anchors,
                            );
                            rendered.push(Block::Text(original));
                        }
                        TranslationMode::Bilingual => {
                            let mut translated = original.clone();
                            let original_margin_after = original.style.margin_after;
                            original.style.margin_after = original_margin_after.min(6.0);
                            translated.content = replacement_content(translation, style);
                            translated.source = None;
                            translated.style.margin_before = 0.0;
                            translated.style.margin_after = original_margin_after;
                            rendered.push(Block::Text(original));
                            rendered.push(Block::Text(translated));
                        }
                    }
                }
                Block::Image(image) if image.text_layer.is_some() => {
                    let translated = translated_fixed_page_block(
                        translation,
                        (state.mode == TranslationMode::Replace)
                            .then(|| image.source.clone())
                            .flatten(),
                    );
                    if state.mode == TranslationMode::Replace {
                        let mut translated = translated;
                        update_translated_source(
                            translated.source.as_mut(),
                            translation,
                            &mut section.anchors,
                        );
                        rendered.push(Block::Text(translated));
                    } else {
                        rendered.push(Block::Image(image));
                        rendered.push(Block::Text(translated));
                    }
                }
                other => rendered.push(other),
            }
        }
        section.blocks = rendered;
        Ok(section)
    }

    fn resource(&self, href: &PublicationUrl) -> Result<Resource, PublicationError> {
        self.inner.resource(href)
    }

    fn raster_resource(
        &self,
        href: &PublicationUrl,
    ) -> Result<Option<RasterResource>, PublicationError> {
        self.inner.raster_resource(href)
    }
}

fn translated_fixed_page_block(text: &str, source: Option<SourceRange>) -> TextBlock {
    TextBlock {
        kind: TextBlockKind::Paragraph,
        content: replacement_content(text, TextStyle::default()),
        style: BlockStyle {
            margin_before: 16.0,
            margin_after: 16.0,
            ..BlockStyle::default()
        },
        source,
    }
}

fn update_translated_source(
    source: Option<&mut SourceRange>,
    translation: &str,
    anchors: &mut [SectionAnchor],
) {
    let Some(source) = source else {
        return;
    };
    source.end.spine = source.start.spine.clone();
    source.end.node.clone_from(&source.start.node);
    source.end.text_offset = source
        .start
        .text_offset
        .saturating_add(u64::try_from(translation.chars().count()).unwrap_or(u64::MAX));
    for anchor in anchors {
        if anchor.source.spine == source.start.spine && anchor.source.node == source.start.node {
            anchor.source.text_offset = anchor
                .source
                .text_offset
                .clamp(source.start.text_offset, source.end.text_offset);
        }
    }
}

fn replacement_content(text: &str, style: TextStyle) -> Vec<Inline> {
    let mut content = Vec::new();
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            content.push(Inline::Break);
        }
        if !line.is_empty() {
            content.push(Inline::Text(TextRun {
                text: line.to_owned(),
                style,
                link: None,
            }));
        }
    }
    content
}

#[cfg(test)]
mod tests {
    use rebook_publication::{
        BlockStyle, FixedPageTextLayer, FixedPageTextRect, FixedPageTextSpan, ImageBlock,
        ImageStyle, Metadata, PublicationId, RenditionLayout, SourceAnchor, SourceRange, SpineItem,
        SpineItemId, TextBlock, TextBlockKind,
    };

    use super::*;

    struct TestSource {
        book: Book,
        section: Section,
    }

    impl BookSource for TestSource {
        fn book(&self) -> &Book {
            &self.book
        }

        fn parse_section(&self, _index: usize) -> Result<Section, PublicationError> {
            Ok(self.section.clone())
        }

        fn resource(&self, _href: &PublicationUrl) -> Result<Resource, PublicationError> {
            unreachable!()
        }
    }

    fn source() -> Arc<dyn BookSource> {
        let spine = SpineItemId::new("chapter").unwrap();
        let href = PublicationUrl::parse("chapter.xhtml").unwrap();
        Arc::new(TestSource {
            book: Book {
                id: PublicationId::new("translation-test").unwrap(),
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
            section: Section {
                id: spine.clone(),
                href,
                blocks: vec![Block::Text(TextBlock {
                    kind: TextBlockKind::Paragraph,
                    content: vec![Inline::Text(TextRun {
                        text: "Hello".into(),
                        style: TextStyle::default(),
                        link: None,
                    })],
                    style: BlockStyle::default(),
                    source: Some(SourceRange {
                        start: SourceAnchor {
                            spine: spine.clone(),
                            node: "p-1".into(),
                            text_offset: 0,
                        },
                        end: SourceAnchor {
                            spine,
                            node: "p-1".into(),
                            text_offset: 5,
                        },
                    }),
                })],
                anchors: Vec::new(),
            },
        })
    }

    fn fixed_page_source() -> Arc<dyn BookSource> {
        let spine = SpineItemId::new("pdf-page").unwrap();
        let href = PublicationUrl::parse("page.xhtml").unwrap();
        let image_href = PublicationUrl::parse("page.png").unwrap();
        Arc::new(TestSource {
            book: Book {
                id: PublicationId::new("translation-pdf-test").unwrap(),
                metadata: Metadata {
                    layout: RenditionLayout::PrePaginated,
                    ..Metadata::default()
                },
                cover: None,
                sections: vec![SpineItem {
                    id: spine.clone(),
                    href: href.clone(),
                    media_type: "application/pdf".into(),
                    linear: true,
                    properties: Vec::new(),
                }],
                table_of_contents: Vec::new(),
            },
            section: Section {
                id: spine.clone(),
                href,
                blocks: vec![Block::Image(ImageBlock {
                    href: image_href,
                    alt: "PDF page".into(),
                    style: ImageStyle::default(),
                    source: Some(SourceRange {
                        start: SourceAnchor {
                            spine: spine.clone(),
                            node: "pdf-page-text".into(),
                            text_offset: 0,
                        },
                        end: SourceAnchor {
                            spine,
                            node: "pdf-page-text".into(),
                            text_offset: 9,
                        },
                    }),
                    text_layer: Some(FixedPageTextLayer {
                        width: 100.0,
                        height: 100.0,
                        text: "Hello PDF".into(),
                        spans: vec![FixedPageTextSpan {
                            char_range: 0..9,
                            rect: FixedPageTextRect {
                                x: 10.0,
                                y: 10.0,
                                width: 60.0,
                                height: 12.0,
                            },
                        }],
                    }),
                })],
                anchors: Vec::new(),
            },
        })
    }

    fn block_text(block: &Block) -> String {
        let Block::Text(block) = block else {
            panic!("expected text block");
        };
        text_block_text(block)
    }

    #[test]
    fn toggles_between_original_replace_and_bilingual_views() {
        let source = TranslationBookSource::new(source(), TranslationMode::Replace);
        source
            .store_section(
                0,
                &[BlockTranslation {
                    block_index: 0,
                    text: "你好".into(),
                }],
            )
            .unwrap();

        assert_eq!(
            block_text(&source.parse_section(0).unwrap().blocks[0]),
            "Hello"
        );

        source.set_enabled(true).unwrap();
        let replaced = source.parse_section(0).unwrap();
        assert_eq!(replaced.blocks.len(), 1);
        assert_eq!(block_text(&replaced.blocks[0]), "你好");
        assert!(matches!(&replaced.blocks[0], Block::Text(block) if block.source.is_some()));
        assert!(matches!(
            &replaced.blocks[0],
            Block::Text(block) if block.source.as_ref().unwrap().end.text_offset == 2
        ));

        source.set_mode(TranslationMode::Bilingual).unwrap();
        let bilingual = source.parse_section(0).unwrap();
        assert_eq!(bilingual.blocks.len(), 2);
        assert_eq!(block_text(&bilingual.blocks[0]), "Hello");
        assert_eq!(block_text(&bilingual.blocks[1]), "你好");
        assert!(matches!(&bilingual.blocks[1], Block::Text(block) if block.source.is_none()));
    }

    #[test]
    fn translates_fixed_page_text_layers_for_text_based_pdfs() {
        let source = TranslationBookSource::new(fixed_page_source(), TranslationMode::Replace);
        let blocks = source.translatable_blocks(0).unwrap();
        assert_eq!(
            blocks,
            [TranslationBlockInput {
                block_index: 0,
                text: "Hello PDF".into(),
            }]
        );
        source
            .store_section(
                0,
                &[BlockTranslation {
                    block_index: 0,
                    text: "你好，PDF".into(),
                }],
            )
            .unwrap();
        source.set_enabled(true).unwrap();

        let replaced = source.parse_section(0).unwrap();
        assert_eq!(replaced.blocks.len(), 1);
        assert_eq!(block_text(&replaced.blocks[0]), "你好，PDF");
        assert!(matches!(&replaced.blocks[0], Block::Text(block) if block.source.is_some()));

        source.set_mode(TranslationMode::Bilingual).unwrap();
        let bilingual = source.parse_section(0).unwrap();
        assert_eq!(bilingual.blocks.len(), 2);
        assert!(matches!(&bilingual.blocks[0], Block::Image(_)));
        assert_eq!(block_text(&bilingual.blocks[1]), "你好，PDF");
        assert!(matches!(&bilingual.blocks[1], Block::Text(block) if block.source.is_none()));
    }
}
