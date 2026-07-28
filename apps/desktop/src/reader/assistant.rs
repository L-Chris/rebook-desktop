use std::sync::Arc;
use std::time::Instant;

use crate::platform::UserEvent;
use crate::plugins::{
    BookSearchResult, ChatCommand, ChatCommandResolution, ChatRole, ChatTurn,
    TranslationBlockInput, chat_with_book, resolve_chat_command, search_book, translate_blocks,
};

use super::{
    AssistantPanel, ChatTask, ChatTaskMessage, DesktopReader, FocusedMark, MarkRetention,
    SearchTask, SearchTaskMessage, SidebarTab, SnapshotEffects, TocTranslationTask,
    TocTranslationTaskMessage, TranslationTask, TranslationTaskMessage,
};

impl DesktopReader {
    pub(crate) fn spawn_pending_tasks(
        &mut self,
        runtime: &tokio::runtime::Runtime,
        proxy: &winit::event_loop::EventLoopProxy<UserEvent>,
    ) {
        if let Some(request) = self.search.task.take_pending() {
            let proxy = proxy.clone();
            runtime.spawn(async move {
                let id = request.id;
                let payload = request.payload;
                let result = tokio::task::spawn_blocking(move || {
                    search_book(payload.source.as_ref(), &payload.query, 200)
                })
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result);
                let _ = proxy.send_event(UserEvent::ReaderSearch(SearchTaskMessage { id, result }));
            });
        }
        if let Some(request) = self.chat.task.take_pending() {
            let proxy = proxy.clone();
            runtime.spawn(async move {
                let id = request.id;
                let payload = request.payload;
                let result = chat_with_book(
                    payload.source,
                    payload.settings,
                    payload.history,
                    payload.question,
                    payload.current_section,
                    payload.response_language,
                )
                .await;
                let _ = proxy.send_event(UserEvent::ReaderChat(ChatTaskMessage { id, result }));
            });
        }
        if let Some(request) = self.translation.task.take_pending() {
            let proxy = proxy.clone();
            runtime.spawn(async move {
                let id = request.id;
                let payload = request.payload;
                let result = translate_blocks(payload.settings, payload.blocks).await;
                let _ = proxy.send_event(UserEvent::ReaderTranslation(TranslationTaskMessage {
                    id,
                    result,
                }));
            });
        }
        if let Some(request) = self.translation.toc_task.take_pending() {
            let proxy = proxy.clone();
            runtime.spawn(async move {
                let id = request.id;
                let payload = request.payload;
                let result = translate_blocks(payload.settings, payload.blocks).await;
                let _ =
                    proxy.send_event(UserEvent::ReaderTocTranslation(TocTranslationTaskMessage {
                        id,
                        result,
                    }));
            });
        }
    }

    pub(super) fn open_search(&mut self) {
        self.ui.sidebar_tab = SidebarTab::Search;
        self.set_sidebar_open(true);
    }

    pub(super) fn start_search(&mut self) {
        if self.search.task.is_pending() {
            return;
        }
        let query = self.search.query.trim().to_owned();
        if query.is_empty() {
            self.search.status = self
                .language
                .text("请输入搜索内容", "Enter a search query")
                .into();
            return;
        }
        self.search.status = self.language.text("正在搜索…", "Searching…").into();
        self.search.results.clear();
        self.focused_mark = None;
        self.search.task.begin(SearchTask {
            source: Arc::clone(&self.source),
            query,
        });
        self.bump_scene_revision();
    }

    pub(crate) fn complete_search(&mut self, message: SearchTaskMessage) {
        if self.search.task.complete(message.id).is_none() {
            return;
        }
        match message.result {
            Ok(results) => {
                self.search.status = if results.is_empty() {
                    self.language
                        .text("没有找到匹配内容", "No matches found")
                        .into()
                } else {
                    match self.language {
                        crate::preferences::AppLanguage::SimplifiedChinese => {
                            format!("找到 {} 处结果", results.len())
                        }
                        crate::preferences::AppLanguage::English => {
                            format!("Found {} matches", results.len())
                        }
                    }
                };
                self.search.results = results;
            }
            Err(error) => {
                self.search.results.clear();
                self.search.status = error;
            }
        }
    }

    pub(super) fn go_to_search_result(&mut self, result: &BookSearchResult) {
        match self.reader.go_to_source(&result.range.start) {
            Ok(navigation) => {
                self.focused_mark = Some(FocusedMark::search(result.range.clone()));
                self.apply_snapshot(navigation.snapshot, SnapshotEffects::navigation());
            }
            Err(error) => {
                self.search.status = format!(
                    "{}: {error}",
                    self.language
                        .text("搜索结果跳转失败", "Failed to open search result")
                );
            }
        }
    }

    pub(super) fn toggle_assistant_panel(&mut self, panel: AssistantPanel) {
        self.cancel_text_selection();
        if self.ui.assistant_panel == Some(panel) && self.ui.assistant_motion.target > 0.5 {
            self.close_assistant_panel();
        } else {
            self.ui.assistant_panel = Some(panel);
            if self.ui.assistant_motion.animate_to(1.0) {
                self.ui.last_motion_tick = Some(std::time::Instant::now());
            }
        }
    }

    pub(super) fn close_assistant_panel(&mut self) {
        if self.ui.assistant_motion.animate_to(0.0) {
            self.ui.last_motion_tick = Some(std::time::Instant::now());
        }
    }

    pub(super) fn send_chat(&mut self) {
        let raw = self.chat.input.trim().to_owned();
        if raw.is_empty() || self.chat.task.is_pending() {
            return;
        }
        match resolve_chat_command(&raw) {
            ChatCommandResolution::MissingArguments {
                message,
                insert_text,
            } => {
                self.chat.messages.push(ChatTurn {
                    role: ChatRole::User,
                    content: raw.clone(),
                    display_content: Some(raw),
                });
                self.chat.messages.push(ChatTurn {
                    role: ChatRole::Assistant,
                    content: message,
                    display_content: None,
                });
                self.chat.input = insert_text.into();
                self.chat.error = None;
            }
            ChatCommandResolution::Resolved { display, prompt } => {
                self.chat.input.clear();
                self.queue_chat(prompt, Some(display));
            }
            ChatCommandResolution::NotCommand | ChatCommandResolution::Unknown => {
                self.chat.input.clear();
                self.queue_chat(raw, None);
            }
        }
    }

    pub(super) fn select_chat_command(&mut self, command: ChatCommand) {
        if !self.chat.task.is_pending() {
            self.chat.input = command.insert_text.into();
            self.chat.error = None;
        }
    }

    pub(super) fn explain_selection(&mut self) {
        let Some(selection) = self.selection.clone() else {
            return;
        };
        let question = match self.language {
            crate::preferences::AppLanguage::SimplifiedChinese => format!(
                "请结合当前段落和章节语境解释选中的内容。说明它的直接含义、在本段中的作用，以及理解它所需的背景；不要脱离原文进行无依据推测。\n\n选中文字：\n{}",
                selection.text.trim()
            ),
            crate::preferences::AppLanguage::English => format!(
                "Explain the selected text in the context of the current paragraph and section. Cover its direct meaning, its role in the passage, and any background needed to understand it. Do not speculate beyond the source.\n\nSelected text:\n{}",
                selection.text.trim()
            ),
        };
        self.focused_mark = Some(FocusedMark::assistant(selection.ranges.clone()));
        self.cancel_text_selection();
        self.ui.assistant_panel = Some(AssistantPanel::Chat);
        if self.ui.assistant_motion.animate_to(1.0) {
            self.ui.last_motion_tick = Some(std::time::Instant::now());
        }
        self.queue_chat(question, None);
    }

    pub(super) fn queue_chat(&mut self, question: String, display_content: Option<String>) {
        if let Err(error) = self.plugin_settings.chat_endpoint() {
            self.chat.error = Some(error);
            self.ui.assistant_panel = Some(AssistantPanel::Chat);
            if self.ui.assistant_motion.animate_to(1.0) {
                self.ui.last_motion_tick = Some(std::time::Instant::now());
            }
            return;
        }
        let history = self.chat.messages.clone();
        self.chat.messages.push(ChatTurn {
            role: ChatRole::User,
            content: question.clone(),
            display_content,
        });
        self.chat.error = None;
        self.chat.task.begin(ChatTask {
            source: Arc::clone(&self.source),
            settings: self.plugin_settings.clone(),
            history,
            question,
            current_section: self.snapshot.location.section_index,
            response_language: self.language.translation_target().into(),
        });
    }

    pub(crate) fn complete_chat(&mut self, message: ChatTaskMessage) {
        if self.chat.task.complete(message.id).is_none() {
            return;
        }
        match message.result {
            Ok(response) => {
                if !response.rewrites.is_empty() {
                    let transaction = match self.rewrite_source.apply_rewrites(&response.rewrites) {
                        Ok(transaction) => transaction,
                        Err(error) => {
                            self.chat.error = Some(format!(
                                "{}: {error}",
                                self.language.text(
                                    "应用正文改写失败",
                                    "Failed to apply the content rewrite"
                                )
                            ));
                            return;
                        }
                    };
                    match self.reader.refresh_source() {
                        Ok(snapshot) => {
                            self.apply_snapshot(
                                snapshot,
                                SnapshotEffects {
                                    marks: MarkRetention::ClearAll,
                                    ..SnapshotEffects::static_content_change()
                                },
                            );
                        }
                        Err(error) => {
                            let rollback_error = self.rewrite_source.rollback(transaction).err();
                            self.chat.error = Some(match (self.language, rollback_error) {
                                (
                                    crate::preferences::AppLanguage::SimplifiedChinese,
                                    Some(rollback_error),
                                ) => format!(
                                    "应用正文改写失败：{error}；回滚也失败：{rollback_error}"
                                ),
                                (
                                    crate::preferences::AppLanguage::English,
                                    Some(rollback_error),
                                ) => {
                                    format!(
                                        "Failed to apply the content rewrite: {error}; rollback also failed: {rollback_error}"
                                    )
                                }
                                (crate::preferences::AppLanguage::SimplifiedChinese, None) => {
                                    format!("应用正文改写失败：{error}")
                                }
                                (crate::preferences::AppLanguage::English, None) => {
                                    format!("Failed to apply the content rewrite: {error}")
                                }
                            });
                            return;
                        }
                    }
                }
                self.chat.messages.push(ChatTurn {
                    role: ChatRole::Assistant,
                    content: response.content,
                    display_content: None,
                });
                self.chat.error = None;
            }
            Err(error) => self.chat.error = Some(error),
        }
    }

    pub(super) fn clear_chat(&mut self) {
        if !self.chat.task.is_pending() {
            self.chat.messages.clear();
            self.chat.error = None;
        }
    }

    pub(super) fn toggle_translation(&mut self) {
        self.cancel_text_selection();
        self.translation.clear_error();
        if self.translation.enabled {
            self.translation.enabled = false;
            self.translation.task.cancel();
            self.translation.toc_task.cancel();
            let was_rendering = self.translation.render_enabled;
            if !self.set_translation_rendering(false) {
                return;
            }
            if was_rendering {
                self.refresh_translation_view();
            }
            return;
        }

        if let Err(error) = self.plugin_settings.translation_endpoint() {
            self.translation.show_error(error.clone(), Instant::now());
            self.error = Some(error);
            return;
        }
        if let Err(error) = self
            .translation_source
            .set_mode(self.plugin_settings.translation_mode)
        {
            self.translation.show_error(error, Instant::now());
            return;
        }
        self.translation.enabled = true;
        self.queue_visible_section_translation();
        self.queue_toc_translation();
    }

    pub(super) fn dismiss_translation_notice(&mut self) {
        self.translation.clear_error();
    }

    pub(super) fn queue_visible_section_translation(&mut self) {
        if !self.translation.enabled {
            return;
        }
        let first_missing = self
            .current_translation_sections()
            .into_iter()
            .find(|section_index| !self.translation_source.has_section(*section_index));
        let Some(section_index) = first_missing else {
            if !self.translation.render_enabled && self.set_translation_rendering(true) {
                self.refresh_translation_view();
                self.queue_visible_section_translation();
            }
            return;
        };
        if self.translation.render_enabled {
            if !self.set_translation_rendering(false) {
                return;
            }
            self.refresh_translation_view();
        }
        if self.translation.task.is_pending() {
            return;
        }
        let blocks = match self.translation_source.translatable_blocks(section_index) {
            Ok(blocks) => blocks,
            Err(error) => {
                self.translation.show_error(error, Instant::now());
                return;
            }
        };
        if blocks.is_empty() {
            if let Err(error) = self.translation_source.store_section(section_index, &[]) {
                self.translation.show_error(error, Instant::now());
                return;
            }
            self.queue_visible_section_translation();
            return;
        }
        self.translation.clear_error();
        let mut settings = self.plugin_settings.clone();
        settings.target_language =
            settings.resolved_target_language(self.language.translation_target());
        self.translation.task.begin(TranslationTask {
            section_index,
            settings,
            blocks,
        });
    }

    pub(super) fn queue_toc_translation(&mut self) {
        if !self.translation.enabled
            || !self.plugin_settings.translate_toc
            || self.translation.toc_task.is_pending()
            || !self.translation.toc_labels.is_empty()
        {
            return;
        }
        let mut toc_ids = Vec::new();
        let mut blocks = Vec::new();
        for row in self.reader.toc_items() {
            if row.label.trim().is_empty() {
                continue;
            }
            let block_index = toc_ids.len();
            toc_ids.push(row.id.clone());
            blocks.push(TranslationBlockInput {
                block_index,
                text: row.label.clone(),
            });
        }
        if blocks.is_empty() {
            return;
        }
        let mut settings = self.plugin_settings.clone();
        settings.target_language =
            settings.resolved_target_language(self.language.translation_target());
        self.translation.toc_task.begin(TocTranslationTask {
            toc_ids,
            settings,
            blocks,
        });
    }

    pub(crate) fn complete_translation(&mut self, message: TranslationTaskMessage) {
        let Some(request) = self.translation.task.complete(message.id) else {
            return;
        };
        match message.result {
            Ok(translations) => {
                if let Err(error) = self
                    .translation_source
                    .store_section(request.section_index, &translations)
                {
                    self.translation.show_error(error, Instant::now());
                    return;
                }
                self.translation.clear_error();
                self.queue_visible_section_translation();
            }
            Err(error) => {
                self.error = Some(format!(
                    "{}: {error}",
                    self.language
                        .text("翻译正文失败", "Failed to translate book content")
                ));
                self.translation.show_error(error, Instant::now());
            }
        }
    }

    pub(crate) fn complete_toc_translation(&mut self, message: TocTranslationTaskMessage) {
        let Some(request) = self.translation.toc_task.complete(message.id) else {
            return;
        };
        match message.result {
            Ok(translations) => {
                self.translation.toc_labels =
                    translated_toc_labels(&request.toc_ids, &translations);
                self.translation.clear_error();
            }
            Err(error) => {
                self.error = Some(format!(
                    "{}: {error}",
                    self.language
                        .text("翻译目录失败", "Failed to translate table of contents")
                ));
                self.translation.show_error(error, Instant::now());
            }
        }
    }

    pub(super) fn refresh_translation_view(&mut self) {
        match self.reader.refresh_source() {
            Ok(snapshot) => {
                self.apply_snapshot(
                    snapshot,
                    SnapshotEffects {
                        marks: MarkRetention::ClearSelectedHighlight,
                        ..SnapshotEffects::static_content_change()
                    },
                );
            }
            Err(error) => self.translation.show_error(
                format!(
                    "{}: {error}",
                    self.language
                        .text("刷新翻译正文失败", "Failed to refresh translated content")
                ),
                Instant::now(),
            ),
        }
    }

    fn current_translation_sections(&mut self) -> Vec<usize> {
        self.reader
            .current_spread_section_indices()
            .unwrap_or_else(|_| vec![self.snapshot.location.section_index])
    }

    fn set_translation_rendering(&mut self, enabled: bool) -> bool {
        if self.translation.render_enabled == enabled {
            return true;
        }
        if let Err(error) = self.translation_source.set_enabled(enabled) {
            self.translation.show_error(error, Instant::now());
            return false;
        }
        self.translation.render_enabled = enabled;
        true
    }
}

fn translated_toc_labels(
    toc_ids: &[String],
    translations: &[crate::plugins::BlockTranslation],
) -> std::collections::HashMap<String, String> {
    translations
        .iter()
        .filter_map(|translation| {
            let id = toc_ids.get(translation.block_index)?;
            (!translation.text.trim().is_empty()).then(|| (id.clone(), translation.text.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::BlockTranslation;

    #[test]
    fn toc_translations_are_mapped_by_their_stable_row_ids() {
        let ids = vec!["cover".into(), "chapter-1".into()];
        let labels = translated_toc_labels(
            &ids,
            &[
                BlockTranslation {
                    block_index: 1,
                    text: "第一章".into(),
                },
                BlockTranslation {
                    block_index: 99,
                    text: "ignored".into(),
                },
            ],
        );

        assert_eq!(labels.len(), 1);
        assert_eq!(labels.get("chapter-1").map(String::as_str), Some("第一章"));
    }
}
