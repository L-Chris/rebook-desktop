use std::sync::Arc;
use std::time::Instant;

use crate::plugins::{
    BookSearchResult, ChatCommand, ChatCommandResolution, ChatRole, ChatTurn, resolve_chat_command,
};

use super::{
    AssistantPanel, ChatTask, ChatTaskMessage, DesktopReader, FocusedMark, MarkRetention,
    SearchTask, SearchTaskMessage, SidebarTab, SnapshotEffects, TranslationTask,
    TranslationTaskMessage,
};

impl DesktopReader {
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

    pub(super) fn complete_search(&mut self, message: SearchTaskMessage) {
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
        self.ui.assistant_panel = if self.ui.assistant_panel == Some(panel) {
            None
        } else {
            Some(panel)
        };
    }

    pub(super) fn close_assistant_panel(&mut self) {
        self.ui.assistant_panel = None;
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
        self.queue_chat(question, None);
    }

    pub(super) fn queue_chat(&mut self, question: String, display_content: Option<String>) {
        if let Err(error) = self.plugin_settings.chat_endpoint() {
            self.chat.error = Some(error);
            self.ui.assistant_panel = Some(AssistantPanel::Chat);
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

    pub(super) fn complete_chat(&mut self, message: ChatTaskMessage) {
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
            if let Err(error) = self.translation_source.set_enabled(false) {
                self.translation.show_error(error, Instant::now());
                return;
            }
            self.refresh_translation_view();
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
            .and_then(|()| self.translation_source.set_enabled(true))
        {
            self.translation.show_error(error, Instant::now());
            return;
        }
        self.translation.enabled = true;
        if self
            .translation_source
            .has_section(self.snapshot.location.section_index)
        {
            self.refresh_translation_view();
        }
        self.queue_current_section_translation();
    }

    pub(super) fn dismiss_translation_notice(&mut self) {
        self.translation.clear_error();
    }

    pub(super) fn queue_current_section_translation(&mut self) {
        if !self.translation.enabled || self.translation.task.is_pending() {
            return;
        }
        let section_index = self.snapshot.location.section_index;
        if self.translation_source.has_section(section_index) {
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
            }
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

    pub(super) fn complete_translation(&mut self, message: TranslationTaskMessage) {
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
                if self.translation.enabled
                    && self.snapshot.location.section_index == request.section_index
                {
                    self.refresh_translation_view();
                }
                self.queue_current_section_translation();
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
}
