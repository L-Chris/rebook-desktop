use std::sync::Arc;
use std::time::Instant;

use rebook_publication::{Block, BookSource, Inline, SourceAnchor, SourceRange};
use rebook_reader::ReaderVisibleTextFragment;

use crate::platform::UserEvent;
use crate::plugins::{
    BookSearchResult, ChatCommand, ChatCommandResolution, ChatResponse, ChatRole, ChatTurn,
    TranslationBlockInput, chat_with_book, resolve_chat_command, search_book, section_title,
    translate_blocks,
};

use super::chat_autocomplete::{
    ChatReference, ChatReferenceKind, build_chat_prompt_with_references,
    chat_reference_suggestions, chat_reference_token, insert_chat_reference, parse_chat_citation,
};
use super::{
    AssistantPanel, ChatStreamMessage, ChatStreamingState, ChatTask, ChatTaskMessage,
    DesktopReader, FocusedMark, MarkRetention, SearchTask, SearchTaskMessage, SidebarTab,
    SnapshotEffects, TocTranslationTask, TocTranslationTaskMessage, TranslationTask,
    TranslationTaskMessage,
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
            crate::diagnostics::log(
                "chat.task.start",
                &[
                    crate::diagnostics::Field::U64("id", request.id),
                    crate::diagnostics::Field::Usize(
                        "history_turns",
                        request.payload.history.len(),
                    ),
                    crate::diagnostics::Field::Usize(
                        "question_chars",
                        request.payload.question.chars().count(),
                    ),
                ],
            );
            let stream_proxy = proxy.clone();
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
                    move |content| {
                        let _ = stream_proxy.send_event(UserEvent::ReaderChatStream(
                            ChatStreamMessage { id, content },
                        ));
                    },
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
        self.search.focus_input = true;
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
        self.log_diagnostic_snapshot("assistant.toggle.before", None);
        self.cancel_text_selection();
        if self.ui.assistant_panel == Some(panel) && self.ui.assistant_motion.target > 0.5 {
            self.close_assistant_panel();
        } else {
            self.ui.assistant_panel = Some(panel);
            if self.ui.assistant_motion.animate_to(1.0) {
                self.ui.last_motion_tick = Some(std::time::Instant::now());
            }
        }
        self.log_diagnostic_snapshot("assistant.toggle.after", None);
    }

    pub(super) fn close_assistant_panel(&mut self) {
        self.log_diagnostic_snapshot("assistant.close.before", None);
        if self.ui.assistant_motion.animate_to(0.0) {
            self.ui.last_motion_tick = Some(std::time::Instant::now());
        }
        self.log_diagnostic_snapshot("assistant.close.after", None);
    }

    pub(super) fn send_chat(&mut self) {
        let raw = self.chat.input.trim().to_owned();
        if (raw.is_empty() && self.chat.references.is_empty()) || self.chat.task.is_pending() {
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
                self.chat.cursor_char_index = self.chat.input.chars().count();
                self.chat.move_cursor_to_end = true;
                self.chat.suggestion_index = 0;
                self.chat.error = None;
            }
            ChatCommandResolution::Resolved { display, prompt } => {
                let references = std::mem::take(&mut self.chat.references);
                let prompt = build_chat_prompt_with_references(
                    &prompt,
                    &references,
                    self.language == crate::preferences::AppLanguage::English,
                );
                self.chat.input.clear();
                self.chat.cursor_char_index = 0;
                self.chat.suggestion_index = 0;
                self.queue_chat(prompt, Some(display));
            }
            ChatCommandResolution::NotCommand | ChatCommandResolution::Unknown => {
                let references = std::mem::take(&mut self.chat.references);
                let display = if raw.is_empty() {
                    references
                        .iter()
                        .map(|reference| format!("@{}", reference.label))
                        .collect::<Vec<_>>()
                        .join(" ")
                } else {
                    raw.clone()
                };
                let prompt = build_chat_prompt_with_references(
                    &raw,
                    &references,
                    self.language == crate::preferences::AppLanguage::English,
                );
                self.chat.input.clear();
                self.chat.cursor_char_index = 0;
                self.chat.suggestion_index = 0;
                self.queue_chat(prompt, (!references.is_empty()).then_some(display));
            }
        }
    }

    pub(super) fn select_chat_command(&mut self, command: ChatCommand) {
        if !self.chat.task.is_pending() {
            self.chat.input = command.insert_text.into();
            self.chat.cursor_char_index = self.chat.input.chars().count();
            self.chat.suggestion_index = 0;
            self.chat.move_cursor_to_end = true;
            self.chat.error = None;
        }
    }

    pub(super) fn current_chat_reference_suggestions(&mut self) -> Vec<ChatReference> {
        let Some(token) = chat_reference_token(
            &self.chat.input,
            self.chat.cursor_char_index,
            &self.chat.references,
        ) else {
            return Vec::new();
        };
        self.refresh_chat_reference_options();
        chat_reference_suggestions(
            &self.chat.reference_options,
            &self.chat.references,
            &token.query,
        )
    }

    pub(super) fn select_chat_reference(&mut self, reference: ChatReference) {
        if self.chat.task.is_pending() {
            return;
        }
        let Some(token) = chat_reference_token(
            &self.chat.input,
            self.chat.cursor_char_index,
            &self.chat.references,
        ) else {
            return;
        };
        let (input, cursor_char_index) =
            insert_chat_reference(&self.chat.input, &token, &reference);
        if !self
            .chat
            .references
            .iter()
            .any(|item| item.id == reference.id)
        {
            self.chat.references.push(reference);
        }
        self.chat.input = input;
        self.chat.cursor_char_index = cursor_char_index;
        self.chat.suggestion_index = 0;
        self.chat.move_cursor_to_end = true;
        self.chat.error = None;
    }

    pub(super) fn remove_chat_reference(&mut self, id: &str) {
        self.chat.references.retain(|reference| reference.id != id);
    }

    fn refresh_chat_reference_options(&mut self) {
        let location = (
            self.snapshot.location.section_index,
            self.snapshot.location.segment_index,
            self.snapshot.location.page_index,
        );
        if self.chat.reference_options_location == Some(location) {
            return;
        }
        let section_index = location.0;
        let english = self.language == crate::preferences::AppLanguage::English;
        let book_title = self.source.book().metadata.title.trim().to_owned();
        let mut options = vec![ChatReference {
            id: "book:full-text".into(),
            kind: ChatReferenceKind::Book,
            label: if english { "Full text" } else { "全文" }.into(),
            description: if book_title.is_empty() {
                if english { "Entire book" } else { "整本书" }.into()
            } else {
                book_title
            },
            link: "rebook://book".into(),
            excerpt: None,
        }];

        let mut section_titles = Vec::new();
        if let Ok(section) = self.source.parse_section(section_index) {
            let title = section_title(self.source.as_ref(), section_index, &section.blocks);
            section_titles.push((section_index, title.clone()));
            options.push(ChatReference {
                id: format!("section:{section_index}"),
                kind: ChatReferenceKind::Section,
                label: title.clone(),
                description: if english {
                    format!("Current chapter · {}", section_index + 1)
                } else {
                    format!("当前章节 · {}", section_index + 1)
                },
                link: format!("rebook://j/{section_index}"),
                excerpt: None,
            });
        }

        let Ok(fragments) = self.reader.current_visible_text_fragments() else {
            self.chat.reference_options = options;
            return;
        };
        let visible_paragraphs = visible_chat_paragraphs(fragments);
        for (paragraph_index, (section_index, node, part_index, text)) in
            visible_paragraphs.into_iter().enumerate()
        {
            let title_index = section_titles
                .iter()
                .position(|(candidate, _)| *candidate == section_index);
            let title_index = title_index.unwrap_or_else(|| {
                let title = self.source.parse_section(section_index).map_or_else(
                    |_| {
                        format!(
                            "{} {}",
                            if english { "Chapter" } else { "章节" },
                            section_index + 1
                        )
                    },
                    |section| section_title(self.source.as_ref(), section_index, &section.blocks),
                );
                section_titles.push((section_index, title));
                section_titles.len() - 1
            });
            options.push(paragraph_reference(
                section_index,
                paragraph_index + 1,
                &section_titles[title_index].1,
                &node,
                part_index,
                &text,
                english,
            ));
            if options.len() >= 120 {
                break;
            }
        }
        self.chat.reference_options = options;
        self.chat.reference_options_location = Some(location);
    }

    pub(super) fn explain_selection(&mut self) {
        let Some(selection) = self.selection.clone() else {
            return;
        };
        let selected_text = selection.text.trim();
        let english = self.language == crate::preferences::AppLanguage::English;
        let question = match self.language {
            crate::preferences::AppLanguage::SimplifiedChinese => format!(
                "请结合所引用的原文语境解释选中的内容。说明它的直接含义、在本段中的作用，以及理解它所需的背景；不要脱离原文进行无依据推测。\n\n选中文字：\n{selected_text}"
            ),
            crate::preferences::AppLanguage::English => format!(
                "Explain the selected text using the referenced source context. Cover its direct meaning, its role in the passage, and any background needed to understand it. Do not speculate beyond the source.\n\nSelected text:\n{selected_text}"
            ),
        };
        let references = selection_reference(
            self.source.as_ref(),
            &selection.ranges,
            selected_text,
            english,
        )
        .into_iter()
        .collect::<Vec<_>>();
        let prompt = build_chat_prompt_with_references(&question, &references, english);
        let display_content = Some(if english {
            format!("Explain: “{}”", clip_chat_reference_text(selected_text, 72))
        } else {
            format!("解释：“{}”", clip_chat_reference_text(selected_text, 72))
        });
        self.focused_mark = Some(FocusedMark::assistant(selection.ranges.clone()));
        self.cancel_text_selection();
        self.ui.assistant_panel = Some(AssistantPanel::Chat);
        if self.ui.assistant_motion.animate_to(1.0) {
            self.ui.last_motion_tick = Some(std::time::Instant::now());
        }
        self.queue_chat(prompt, display_content);
    }

    pub(super) fn open_chat_citation(&mut self, locator: &str) {
        let Some(citation) = parse_chat_citation(locator) else {
            return;
        };
        let target_range = citation.node.as_deref().and_then(|node| {
            source_range_for_node(self.source.as_ref(), citation.section_index, node)
        });
        let result = if let Some(node) = citation.node {
            if let Some(section) = self.source.book().sections.get(citation.section_index) {
                self.reader.go_to_source(&SourceAnchor {
                    spine: section.id.clone(),
                    node,
                    text_offset: 0,
                })
            } else {
                self.reader.go_to_section(citation.section_index)
            }
        } else {
            self.reader.go_to_section(citation.section_index)
        };
        match result {
            Ok(result) => {
                self.apply_snapshot(
                    result.snapshot,
                    SnapshotEffects {
                        marks: MarkRetention::ClearSelectedHighlight,
                        ..SnapshotEffects::navigation()
                    },
                );
                self.focused_mark = target_range.map(|range| FocusedMark::assistant(vec![range]));
            }
            Err(error) => {
                self.chat.error = Some(format!(
                    "{}: {error}",
                    self.language
                        .text("无法跳转到引用", "Unable to open citation")
                ));
            }
        }
    }

    pub(super) fn queue_chat(&mut self, question: String, display_content: Option<String>) {
        if let Err(error) = self.plugin_settings.chat_endpoint() {
            crate::diagnostics::log(
                "chat.queue.rejected",
                &[
                    crate::diagnostics::Field::Text("reason", "invalid_endpoint"),
                    crate::diagnostics::Field::Usize("error_chars", error.chars().count()),
                ],
            );
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
        let history_turns = history.len();
        let question_chars = question.chars().count();
        let question_lines = question.lines().count();
        let id = self.chat.task.begin(ChatTask {
            source: Arc::clone(&self.source),
            settings: self.plugin_settings.clone(),
            history,
            question,
            current_section: self.snapshot.location.section_index,
            response_language: self.language.translation_target().into(),
        });
        self.chat.streaming = Some(ChatStreamingState {
            task_id: id,
            content: String::new(),
        });
        crate::diagnostics::log(
            "chat.queue",
            &[
                crate::diagnostics::Field::U64("id", id),
                crate::diagnostics::Field::Usize("history_turns", history_turns),
                crate::diagnostics::Field::Usize("question_chars", question_chars),
                crate::diagnostics::Field::Usize("question_lines", question_lines),
            ],
        );
    }

    pub(crate) fn complete_chat(&mut self, message: ChatTaskMessage) {
        if self.chat.task.complete(message.id).is_none() {
            crate::diagnostics::log(
                "chat.complete.stale",
                &[crate::diagnostics::Field::U64("id", message.id)],
            );
            return;
        }
        self.chat.streaming = None;
        match message.result {
            Ok(response) => {
                log_completed_chat(message.id, &response);
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
            Err(error) => {
                crate::diagnostics::log(
                    "chat.complete.error",
                    &[
                        crate::diagnostics::Field::U64("id", message.id),
                        crate::diagnostics::Field::Usize("error_chars", error.chars().count()),
                    ],
                );
                self.chat.error = Some(error);
            }
        }
    }

    pub(crate) fn update_chat_stream(&mut self, message: ChatStreamMessage) {
        let Some(streaming) = self.chat.streaming.as_mut() else {
            return;
        };
        if streaming.task_id != message.id {
            return;
        }
        let first_content = streaming.content.is_empty() && !message.content.is_empty();
        streaming.content = message.content;
        if first_content {
            crate::diagnostics::log(
                "chat.stream.first",
                &[crate::diagnostics::Field::U64("id", message.id)],
            );
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
                segment_index: None,
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

fn paragraph_reference(
    section_index: usize,
    paragraph_index: usize,
    section_title: &str,
    node: &str,
    part_index: usize,
    text: &str,
    english: bool,
) -> ChatReference {
    let label = clip_chat_reference_text(text, 32);
    let excerpt = clip_chat_reference_text(text, 220);
    ChatReference {
        id: format!("paragraph:{section_index}:{node}:{part_index}"),
        kind: ChatReferenceKind::Paragraph,
        label,
        description: if english {
            format!("Paragraph {paragraph_index} · {section_title}")
        } else {
            format!("段落 {paragraph_index} · {section_title}")
        },
        link: format!("rebook://j/{section_index}/{node}"),
        excerpt: Some(excerpt),
    }
}

fn log_completed_chat(id: u64, response: &ChatResponse) {
    let summary = super::chat_markdown::diagnostic_summary(&response.content);
    crate::diagnostics::log(
        "chat.complete.ok",
        &[
            crate::diagnostics::Field::U64("id", id),
            crate::diagnostics::Field::Usize("response_chars", response.content.chars().count()),
            crate::diagnostics::Field::Usize("response_lines", response.content.lines().count()),
            crate::diagnostics::Field::Usize("rewrites", response.rewrites.len()),
            crate::diagnostics::Field::Usize("render_blocks", summary.render_blocks),
            crate::diagnostics::Field::Usize("plain_fences", summary.plain_fenced_code),
            crate::diagnostics::Field::Usize("tables", summary.tables),
            crate::diagnostics::Field::Usize("emoji_like", summary.emoji_like),
            crate::diagnostics::Field::Usize("svg_previews", summary.svg_previews),
            crate::diagnostics::Field::Usize("mermaid_previews", summary.mermaid_previews),
            crate::diagnostics::Field::Usize("formulas", summary.formulas),
        ],
    );
}

fn selection_reference(
    source: &dyn BookSource,
    ranges: &[SourceRange],
    selected_text: &str,
    english: bool,
) -> Option<ChatReference> {
    let range = ranges.first()?;
    let section_index = source
        .book()
        .sections
        .iter()
        .position(|section| section.id == range.start.spine)?;
    let section = source.parse_section(section_index).ok()?;
    let title = section_title(source, section_index, &section.blocks);
    let paragraph = section.blocks.iter().find_map(|block| {
        let source_range = block_source_range(block)?;
        (source_range.start.node == range.start.node).then(|| block_text(block))
    });
    Some(ChatReference {
        id: format!("selection:{section_index}:{}", range.start.node),
        kind: ChatReferenceKind::Paragraph,
        label: clip_chat_reference_text(selected_text, 32),
        description: if english {
            format!("Selected paragraph · {title}")
        } else {
            format!("选中段落 · {title}")
        },
        link: format!("rebook://j/{section_index}/{}", range.start.node),
        excerpt: paragraph
            .filter(|text| !text.trim().is_empty())
            .map(|text| clip_chat_reference_text(&text, 500)),
    })
}

fn source_range_for_node(
    source: &dyn BookSource,
    section_index: usize,
    node: &str,
) -> Option<SourceRange> {
    source
        .parse_section(section_index)
        .ok()?
        .blocks
        .iter()
        .find_map(|block| {
            let range = block_source_range(block)?;
            (range.start.node == node).then(|| range.clone())
        })
}

fn block_source_range(block: &Block) -> Option<&SourceRange> {
    match block {
        Block::Text(block) => block.source.as_ref(),
        Block::Image(block) => block.source.as_ref(),
        Block::Separator | Block::PageBreak => None,
    }
}

fn block_text(block: &Block) -> String {
    match block {
        Block::Text(block) => block
            .content
            .iter()
            .map(|inline| match inline {
                Inline::Text(run) => run.text.as_str(),
                Inline::Break => "\n",
            })
            .collect(),
        Block::Image(block) => block
            .text_layer
            .as_ref()
            .map_or_else(|| block.alt.clone(), |layer| layer.text.clone()),
        Block::Separator | Block::PageBreak => String::new(),
    }
}

type VisibleChatParagraph = (usize, String, usize, String);

fn visible_chat_paragraphs(fragments: Vec<ReaderVisibleTextFragment>) -> Vec<VisibleChatParagraph> {
    let mut paragraphs = Vec::<VisibleChatParagraph>::new();
    for fragment in fragments {
        for (part_index, part) in fragment.text.split("\n\n").enumerate() {
            let text = normalize_chat_reference_text(part);
            if text.chars().count() < 2 {
                continue;
            }
            let section_index = fragment.position.section_index;
            let node = fragment.range.start.node.clone();
            if let Some((_, _, _, combined)) = paragraphs.iter_mut().find(
                |(candidate_section, candidate_node, candidate_part, _)| {
                    *candidate_section == section_index
                        && *candidate_node == node
                        && *candidate_part == part_index
                },
            ) {
                combined.push(' ');
                combined.push_str(&text);
            } else {
                paragraphs.push((section_index, node, part_index, text));
            }
        }
    }
    paragraphs
}

fn normalize_chat_reference_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip_chat_reference_text(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let mut clipped = value.chars().take(max_chars).collect::<String>();
    clipped.push_str("...");
    clipped
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
                    segment_index: None,
                    text: "第一章".into(),
                },
                BlockTranslation {
                    block_index: 99,
                    segment_index: None,
                    text: "ignored".into(),
                },
            ],
        );

        assert_eq!(labels.len(), 1);
        assert_eq!(labels.get("chapter-1").map(String::as_str), Some("第一章"));
    }
}
