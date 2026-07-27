use std::time::Instant;

use lucide_icons::Icon;
use rebook_formats::BookFormat;
use rebook_reader::{PageDirection, ReaderSelection, TocViewItem};
use xilem::core::fork;
use xilem::masonry::peniko::ImageData;
use xilem::masonry::properties::LineBreaking;
use xilem::masonry::properties::types::{AsUnit, UnitPoint};
use xilem::style::{Padding, Style};
use xilem::view::{
    CrossAxisAlignment, FlexExt, FlexSpacer, MainAxisAlignment, ObjectFit, ZStackExt, flex_col,
    flex_row, image, label, portal, prose, sized_box, task, task_raw, text_input, zstack,
};
use xilem::{Affine, AnyWidgetView, Color, FontWeight, WidgetView};

use crate::highlights::StoredHighlight;
use crate::plugins::{BookSearchResult, chat_with_book, search_book, translate_blocks};
use crate::ui::{
    CONTROL_HEIGHT, NoticeTone, RADIUS_SMALL, UI_ACCENT, UI_ACCENT_BORDER, UI_ACCENT_SOFT,
    UI_BACKGROUND, UI_BORDER, UI_FONT_STACK, UI_MUTED, UI_SIDEBAR, UI_SURFACE, UI_SURFACE_MUTED,
    UI_TEXT, UI_TEXT_SOFT, button, dismissible_notice, divider, ellipsize_display_text, icon_label,
    notice_card, ui_color, wrap_display_text,
};

use super::assistant_view::assistant_panel;
use super::render::{ReaderCanvasAction, reader_canvas};
use super::settings_view::settings_overlay;
use super::{
    AssistantPanel, ChatTaskMessage, DesktopReader, FocusedMark, ReaderOverlay, SearchTaskMessage,
    SidebarTab, TranslationTaskMessage,
};

const INITIAL_WIDTH: u32 = 1200;
const INITIAL_HEIGHT: u32 = 800;
const TOOLBAR_HEIGHT: f64 = 44.0;
const PROGRESS_HEIGHT: f64 = 4.0;
const TOC_WIDTH: f64 = 240.0;
const MOTION_FRAME_INTERVAL: std::time::Duration = std::time::Duration::from_millis(16);
const MOTION_EPSILON: f32 = 0.001;
const SIDEBAR_SCRIM_ALPHA: f32 = 0.28;
const MODAL_SCRIM_ALPHA: f32 = 0.35;
const SELECTION_TOOLBAR_WIDTH: f64 = 90.0;
const SELECTION_TOOLBAR_HEIGHT: f64 = 46.0;
const SELECTION_TOOLBAR_GAP: f64 = 10.0;
const SIDEBAR_TITLE_LINE_DISPLAY_UNITS: usize = 20;
const SIDEBAR_TITLE_MAX_LINES: usize = 2;
const SIDEBAR_AUTHOR_MAX_DISPLAY_UNITS: usize = 22;
const ANNOTATION_SWATCH_COLOR: Color = Color::from_rgb8(96, 165, 250);

pub(crate) fn app_view(state: &mut DesktopReader) -> impl WidgetView<DesktopReader> + use<> {
    let animations_running = state.ui.needs_motion_tick()
        || state.translation.dismiss_at.is_some()
        || state.pending_page_turn.is_some();
    let search_request = state.search.task.pending.clone();
    let chat_request = state.chat.task.pending.clone();
    let translation_request = state.translation.task.pending.clone();
    let app = reader_shell(state);

    let app = fork(
        app,
        animations_running.then(|| {
            task(
                |proxy| async move {
                    let mut interval = xilem::tokio::time::interval(MOTION_FRAME_INTERVAL);
                    interval.set_missed_tick_behavior(xilem::tokio::time::MissedTickBehavior::Skip);
                    loop {
                        interval.tick().await;
                        if proxy.message(Instant::now()).is_err() {
                            break;
                        }
                    }
                },
                |state: &mut DesktopReader, now| state.advance_frame(now),
            )
        }),
    );
    let app = fork(
        app,
        search_request.map(|request| {
            task_raw(
                move |proxy| {
                    let request = request.clone();
                    async move {
                        let id = request.id;
                        let payload = request.payload;
                        let result = xilem::tokio::task::spawn_blocking(move || {
                            search_book(payload.source.as_ref(), &payload.query, 100)
                        })
                        .await
                        .map_err(|error| format!("搜索任务失败：{error}"))
                        .and_then(std::convert::identity);
                        let _ = proxy.message(SearchTaskMessage { id, result });
                    }
                },
                DesktopReader::complete_search,
            )
        }),
    );
    let app = fork(
        app,
        chat_request.map(|request| {
            task_raw(
                move |proxy| {
                    let request = request.clone();
                    async move {
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
                        let _ = proxy.message(ChatTaskMessage { id, result });
                    }
                },
                DesktopReader::complete_chat,
            )
        }),
    );
    fork(
        app,
        translation_request.map(|request| {
            task_raw(
                move |proxy| {
                    let request = request.clone();
                    async move {
                        let id = request.id;
                        let payload = request.payload;
                        let result = translate_blocks(payload.settings, payload.blocks).await;
                        let _ = proxy.message(TranslationTaskMessage { id, result });
                    }
                },
                DesktopReader::complete_translation,
            )
        }),
    )
}

fn reader_shell(state: &DesktopReader) -> impl WidgetView<DesktopReader> + use<> {
    let progress = state.progress();
    let sidebar_progress = state.ui.sidebar_motion.value.clamp(0.0, 1.0);
    let sidebar_offset = -TOC_WIDTH * f64::from(1.0 - sidebar_progress);
    let workspace: Box<AnyWidgetView<DesktopReader>> = if !state.ui.sidebar_motion.is_visible() {
        reader_workspace(state, progress).boxed()
    } else if state.ui.sidebar_pinned && !state.ui.sidebar_motion.is_animating() {
        flex_row((toc_view(state), reader_workspace(state, progress).flex(1.0)))
            .gap(0.px())
            .boxed()
    } else if state.ui.sidebar_pinned {
        zstack((
            flex_row((
                sized_box(label(""))
                    .width((TOC_WIDTH * f64::from(sidebar_progress)).px())
                    .expand_height(),
                reader_workspace(state, progress).flex(1.0),
            )),
            toc_view(state)
                .transform(Affine::translate((sidebar_offset, 0.0)))
                .alignment(UnitPoint::TOP_LEFT),
        ))
        .boxed()
    } else {
        zstack((
            reader_workspace(state, progress),
            animated_scrim(
                sidebar_scrim_color(sidebar_progress),
                |state: &mut DesktopReader| state.set_sidebar_open(false),
            ),
            toc_view(state)
                .transform(Affine::translate((sidebar_offset, 0.0)))
                .alignment(UnitPoint::TOP_LEFT),
        ))
        .boxed()
    };
    let workspace: Box<AnyWidgetView<DesktopReader>> = if state.ui.assistant_panel.is_some() {
        flex_row((workspace.flex(1.0), assistant_panel(state)))
            .gap(0.px())
            .boxed()
    } else {
        workspace
    };
    let settings_progress = state.ui.settings_motion.value.clamp(0.0, 1.0);
    let settings_layer: Box<AnyWidgetView<DesktopReader>> = if state.ui.settings_motion.is_visible()
    {
        settings_overlay(state, settings_progress).boxed()
    } else {
        sized_box(label("")).width(0.px()).height(0.px()).boxed()
    };
    sized_box(zstack((workspace, settings_layer)))
        .expand()
        .background_color(UI_BACKGROUND)
}

fn reader_workspace(
    state: &DesktopReader,
    progress: f64,
) -> impl WidgetView<DesktopReader> + use<> {
    let (title, reader_background) = {
        (
            state.display_metadata.title.clone(),
            ui_color(state.reader.style().background),
        )
    };
    let menu_open = state.ui.overlay == ReaderOverlay::Menu;
    let menu_progress = state.ui.menu_motion.value.clamp(0.0, 1.0);
    let menu_visible = state.ui.menu_motion.is_visible();
    let toolbar_progress = if menu_visible {
        1.0
    } else {
        state.ui.toolbar_motion.value.clamp(0.0, 1.0)
    };
    let toolbar_visible = toolbar_progress > MOTION_EPSILON || menu_visible;
    let toolbar_content: Box<AnyWidgetView<DesktopReader>> = if toolbar_visible {
        sized_box(reader_toolbar(
            title,
            state.ui.sidebar_open,
            menu_open,
            state.translation.enabled,
            state.ui.assistant_panel,
            reader_background,
        ))
        .height(TOOLBAR_HEIGHT.px())
        .expand_width()
        .boxed()
    } else {
        sized_box(label("")).width(0.px()).height(0.px()).boxed()
    };
    let toolbar_layer = toolbar_content
        .transform(Affine::translate((
            0.0,
            -TOOLBAR_HEIGHT * f64::from(1.0 - toolbar_progress),
        )))
        .alignment(UnitPoint::TOP);
    let menu_scrim: Box<AnyWidgetView<DesktopReader>> = if menu_visible {
        transparent_catcher(DesktopReader::close_overlay).boxed()
    } else {
        sized_box(label("")).width(0.px()).height(0.px()).boxed()
    };
    let menu_content: Box<AnyWidgetView<DesktopReader>> = if menu_visible {
        flex_col((
            FlexSpacer::Fixed((TOOLBAR_HEIGHT + 8.0).px()),
            reader_menu(state.language).transform(Affine::translate((
                0.0,
                -8.0 * f64::from(1.0 - menu_progress),
            ))),
        ))
        .padding(Padding::horizontal(12.0))
        .boxed()
    } else {
        sized_box(label("")).width(0.px()).height(0.px()).boxed()
    };
    let menu_layer = menu_content.alignment(UnitPoint::TOP_RIGHT);
    let visible_selection = if state.selection_toolbar_visible {
        state.selection.as_ref()
    } else {
        None
    };
    let selection_layer =
        selection_toolbar(visible_selection, state.canvas_size).alignment(UnitPoint::TOP_LEFT);
    let translation_status_layer = translation_status_notice(state).alignment(UnitPoint::TOP_RIGHT);
    let pages = sized_box(flex_col((
        reader_view(state.scene_revision, reader_background).flex(1.0),
        progress_bar(progress),
    )))
    .expand();

    sized_box(zstack((
        pages,
        selection_layer,
        translation_status_layer,
        menu_scrim,
        toolbar_layer,
        menu_layer,
    )))
    .expand()
    .background_color(reader_background)
}

fn translation_status_notice(state: &DesktopReader) -> Box<AnyWidgetView<DesktopReader>> {
    let content: Box<AnyWidgetView<DesktopReader>> = if state.translation.task.is_pending() {
        notice_card(
            NoticeTone::Info,
            state.language.text("正在翻译", "Translating"),
            state.language.text(
                "当前章节完成后会自动刷新正文。",
                "The current section will refresh when translation completes.",
            ),
        )
        .boxed()
    } else if let Some(error) = &state.translation.error {
        dismissible_notice(
            NoticeTone::Error,
            state.language.text("无法完成翻译", "Translation failed"),
            error.clone(),
            DesktopReader::dismiss_translation_notice,
        )
        .boxed()
    } else {
        return sized_box(label("")).width(0.px()).height(0.px()).boxed();
    };
    sized_box(content)
        .width(356.px())
        .transform(Affine::translate((-16.0, TOOLBAR_HEIGHT + 12.0)))
        .boxed()
}

fn reader_view(scene_revision: u64, reader_background: Color) -> impl WidgetView<DesktopReader> {
    sized_box(reader_canvas(
        scene_revision,
        |state: &mut DesktopReader, size| {
            state.resize_canvas(size);
            state.page_scene()
        },
        |state: &mut DesktopReader, action| match action {
            ReaderCanvasAction::ToolbarVisibility(visible) => {
                state.set_toolbar_hovered(visible);
            }
            ReaderCanvasAction::OpenSearch => state.open_search(),
            ReaderCanvasAction::PreviousPage if !state.ui.overlay_visible() => {
                state.turn_page(PageDirection::Previous);
            }
            ReaderCanvasAction::NextPage if !state.ui.overlay_visible() => {
                state.turn_page(PageDirection::Next);
            }
            ReaderCanvasAction::SelectionStart { x, y } if !state.ui.overlay_visible() => {
                state.begin_text_selection(x, y);
            }
            ReaderCanvasAction::SelectionUpdate { x, y } if !state.ui.overlay_visible() => {
                state.update_text_selection(x, y);
            }
            ReaderCanvasAction::SelectionFinish { x, y, moved } if !state.ui.overlay_visible() => {
                state.finish_text_selection(x, y, moved);
            }
            ReaderCanvasAction::SelectionCancel => state.cancel_text_selection(),
            _ => {}
        },
    ))
    .expand()
    .background_color(reader_background)
}

fn selection_toolbar(
    selection: Option<&ReaderSelection>,
    canvas_size: Option<(u32, u32)>,
) -> impl WidgetView<DesktopReader> + use<> {
    let anchor = selection
        .and_then(|selection| selection.rects.last())
        .copied()
        .unwrap_or(rebook_reader::ReaderSelectionRect {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        });
    let selection_top = selection
        .into_iter()
        .flat_map(|selection| &selection.rects)
        .map(|rect| f64::from(rect.y))
        .fold(f64::INFINITY, f64::min);
    let selection_bottom = selection
        .into_iter()
        .flat_map(|selection| &selection.rects)
        .map(|rect| f64::from(rect.y + rect.height))
        .fold(0.0_f64, f64::max);
    let canvas_width = canvas_size.map_or(f64::from(INITIAL_WIDTH), |size| f64::from(size.0));
    let canvas_height = canvas_size.map_or(f64::from(INITIAL_HEIGHT), |size| f64::from(size.1));
    let ideal_left = f64::from(anchor.x + anchor.width / 2.0) - SELECTION_TOOLBAR_WIDTH / 2.0;
    let left = ideal_left.clamp(8.0, (canvas_width - SELECTION_TOOLBAR_WIDTH - 8.0).max(8.0));
    let top = if selection_top >= SELECTION_TOOLBAR_HEIGHT + SELECTION_TOOLBAR_GAP + 8.0 {
        selection_top - SELECTION_TOOLBAR_HEIGHT - SELECTION_TOOLBAR_GAP
    } else {
        (selection_bottom + SELECTION_TOOLBAR_GAP)
            .min((canvas_height - SELECTION_TOOLBAR_HEIGHT - 8.0).max(8.0))
    };
    let toolbar = sized_box(
        flex_row((
            icon_button(Icon::Highlighter, false, DesktopReader::create_highlight),
            icon_button(
                Icon::MessageCircleQuestion,
                false,
                DesktopReader::explain_selection,
            ),
        ))
        .gap(8.px())
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .padding(Padding::from_vh(7.0, 9.0)),
    )
    .width(SELECTION_TOOLBAR_WIDTH.px())
    .height(SELECTION_TOOLBAR_HEIGHT.px())
    .background_color(UI_SURFACE)
    .border(UI_BORDER, 1.0)
    .corner_radius(12.0);
    let visible = selection.is_some();
    sized_box(if visible {
        toolbar.boxed()
    } else {
        sized_box(label("")).width(0.px()).height(0.px()).boxed()
    })
    .width(if visible {
        SELECTION_TOOLBAR_WIDTH.px()
    } else {
        0.px()
    })
    .height(if visible {
        SELECTION_TOOLBAR_HEIGHT.px()
    } else {
        0.px()
    })
    .transform(Affine::translate((left, top)))
}

fn toc_view(state: &DesktopReader) -> impl WidgetView<DesktopReader> + use<> {
    let (title, author) = sidebar_book_metadata(state);
    let cover = state.cover.clone();
    let format = state.format;
    let active_row_id = state.snapshot.active_toc_id.clone();
    let toc_rows = state
        .reader
        .toc_items()
        .iter()
        .filter(|row| {
            row.ancestors
                .iter()
                .all(|ancestor| state.ui.expanded_toc.contains(ancestor))
        })
        .cloned()
        .map(|row| {
            let selected = active_row_id.as_ref() == Some(&row.id);
            let expanded = state.ui.expanded_toc.contains(&row.id);
            toc_row_view(row, selected, expanded)
        })
        .collect::<Vec<_>>();
    let panel: Box<AnyWidgetView<DesktopReader>> = match state.ui.sidebar_tab {
        SidebarTab::Toc => sized_box(zstack((
            portal(
                flex_col(toc_rows)
                    .gap(2.px())
                    .cross_axis_alignment(CrossAxisAlignment::Fill),
            ),
            // Masonry 0.4 clips the rounded scrollbar thumb at y=0, which leaves
            // a jagged cap under the CPU renderer. Keep the draggable scrollbar,
            // but mask only that defective top edge.
            sized_box(label(""))
                .width(12.px())
                .height(8.px())
                .background_color(UI_SIDEBAR)
                .alignment(UnitPoint::TOP_RIGHT),
        )))
        .background_color(UI_SIDEBAR)
        .padding(Padding::from_vh(6.0, 0.0))
        .boxed(),
        SidebarTab::Highlights => highlights_panel(state).boxed(),
        SidebarTab::Search => search_panel(state).boxed(),
    };
    sized_box(
        flex_col((
            sidebar_toolbar(state.ui.sidebar_pinned, state.ui.sidebar_tab),
            sidebar_book_summary(cover, &title, &author, format, state.language),
            divider(),
            panel.flex(1.0),
        ))
        .gap(4.px()),
    )
    .width(TOC_WIDTH.px())
    .expand_height()
    .background_color(UI_SIDEBAR)
    .padding(Padding::from_vh(6.0, 4.0))
}

fn sidebar_toolbar(pinned: bool, tab: SidebarTab) -> impl WidgetView<DesktopReader> {
    flex_row((
        icon_button(Icon::PanelLeft, false, |state: &mut DesktopReader| {
            state.set_sidebar_open(false);
        }),
        FlexSpacer::Flex(1.0),
        icon_button(Icon::Search, tab == SidebarTab::Search, |state| {
            state.set_sidebar_tab(SidebarTab::Search);
        }),
        icon_button(Icon::Highlighter, tab == SidebarTab::Highlights, |state| {
            state.set_sidebar_tab(SidebarTab::Highlights);
        }),
        icon_button(Icon::ListTree, tab == SidebarTab::Toc, |state| {
            state.set_sidebar_tab(SidebarTab::Toc);
        }),
        icon_button(
            if pinned { Icon::Pin } else { Icon::PinOff },
            pinned,
            |state: &mut DesktopReader| {
                state.ui.sidebar_pinned = !state.ui.sidebar_pinned;
            },
        ),
    ))
    .cross_axis_alignment(CrossAxisAlignment::Center)
}

fn search_panel(state: &DesktopReader) -> impl WidgetView<DesktopReader> + use<> {
    let query = state.search.query.clone();
    let busy = state.search.task.is_pending();
    let status = state.search.status.clone();
    let active_range = state
        .focused_mark
        .as_ref()
        .and_then(FocusedMark::search_range)
        .cloned();
    let rows = state
        .search
        .results
        .iter()
        .cloned()
        .map(|result| {
            let selected = active_range.as_ref() == Some(&result.range);
            search_result_row(result, selected)
        })
        .collect::<Vec<_>>();
    let results: Box<AnyWidgetView<DesktopReader>> = if rows.is_empty() {
        flex_col((
            icon_label(Icon::Search, 24.0, UI_MUTED),
            label(if busy {
                state.language.text("正在扫描正文…", "Scanning book…")
            } else {
                state.language.text("搜索书中内容", "Search this book")
            })
            .font(UI_FONT_STACK)
            .text_size(12.5)
            .color(UI_MUTED),
        ))
        .gap(8.px())
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .main_axis_alignment(MainAxisAlignment::Center)
        .boxed()
    } else {
        portal(
            flex_col(rows)
                .gap(7.px())
                .cross_axis_alignment(CrossAxisAlignment::Fill),
        )
        .boxed()
    };
    let input = sized_box(
        flex_row((
            icon_label(Icon::Search, 15.0, UI_MUTED),
            text_input(query, |state: &mut DesktopReader, value| {
                state.search.query = value;
            })
            .on_enter(|state: &mut DesktopReader, value| {
                state.search.query = value;
                state.start_search();
            })
            .placeholder(state.language.text("搜索全文…", "Search full text…"))
            .text_color(UI_TEXT)
            .caret_color(UI_ACCENT)
            .background_color(Color::TRANSPARENT)
            .border_color(Color::TRANSPARENT)
            .border_width(0.0)
            .padding(0.0)
            .flex(1.0),
            icon_button(Icon::ArrowRight, busy, DesktopReader::start_search),
        ))
        .gap(5.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(40.px())
    .background_color(UI_SURFACE)
    .border(UI_BORDER, 1.0)
    .corner_radius(9.0)
    .padding(Padding::horizontal(8.0));

    flex_col((
        input,
        label(status)
            .font(UI_FONT_STACK)
            .text_size(11.0)
            .color(UI_MUTED),
        results.flex(1.0),
    ))
    .gap(8.px())
    .cross_axis_alignment(CrossAxisAlignment::Fill)
    .padding(Padding::from_vh(8.0, 4.0))
}

fn search_result_row(
    result: BookSearchResult,
    selected: bool,
) -> impl WidgetView<DesktopReader> + use<> {
    let target = result.clone();
    let section = ellipsize_display_text(&result.section_title, 24);
    let excerpt = result.excerpt;
    sized_box(
        button(
            flex_col((
                label(section)
                    .font(UI_FONT_STACK)
                    .text_size(11.0)
                    .weight(FontWeight::BOLD)
                    .color(if selected { UI_ACCENT } else { UI_MUTED }),
                prose(excerpt).text_size(12.0).text_color(UI_TEXT_SOFT),
            ))
            .gap(4.px())
            .cross_axis_alignment(CrossAxisAlignment::Start),
            move |state: &mut DesktopReader| state.go_to_search_result(&target),
        )
        .background_color(if selected { UI_ACCENT_SOFT } else { UI_SURFACE })
        .active_background_color(UI_ACCENT_SOFT)
        .border_color(if selected {
            UI_ACCENT_BORDER
        } else {
            UI_BORDER
        })
        .hovered_border_color(UI_ACCENT_BORDER)
        .corner_radius(9.0)
        .padding(Padding::from_vh(9.0, 10.0)),
    )
    .expand_width()
}

fn highlights_panel(state: &DesktopReader) -> impl WidgetView<DesktopReader> + use<> {
    let selected_id = state.selected_highlight_id.clone();
    let rows = state
        .highlights
        .iter()
        .cloned()
        .map(|highlight| {
            let section_index = highlight
                .ranges
                .first()
                .and_then(|range| {
                    state
                        .reader
                        .book()
                        .sections
                        .iter()
                        .position(|section| section.id == range.start.spine)
                })
                .unwrap_or(0);
            let selected = selected_id.as_deref() == Some(&highlight.id);
            highlight_row_view(highlight, section_index, selected, state.language)
        })
        .collect::<Vec<_>>();
    let count = state.highlights.len();
    let content: Box<AnyWidgetView<DesktopReader>> = if rows.is_empty() {
        flex_col((
            icon_label(Icon::Highlighter, 24.0, UI_MUTED),
            label(state.language.text("还没有高亮", "No highlights yet"))
                .font(UI_FONT_STACK)
                .text_size(13.0)
                .color(UI_MUTED),
            label(
                state
                    .language
                    .text("拖选正文后即可添加", "Select text in the book to add one"),
            )
            .font(UI_FONT_STACK)
            .text_size(11.5)
            .color(UI_MUTED),
        ))
        .gap(8.px())
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .main_axis_alignment(MainAxisAlignment::Center)
        .boxed()
    } else {
        portal(
            flex_col(rows)
                .gap(7.px())
                .cross_axis_alignment(CrossAxisAlignment::Fill),
        )
        .boxed()
    };

    flex_col((
        flex_row((
            label(state.language.text("高亮", "Highlights"))
                .font(UI_FONT_STACK)
                .text_size(13.0)
                .weight(FontWeight::BOLD)
                .color(UI_TEXT),
            FlexSpacer::Flex(1.0),
            label(match state.language {
                crate::preferences::AppLanguage::SimplifiedChinese => format!("{count} 条"),
                crate::preferences::AppLanguage::English => format!("{count}"),
            })
            .font(UI_FONT_STACK)
            .text_size(11.5)
            .color(UI_MUTED),
        ))
        .padding(Padding::from_vh(8.0, 8.0)),
        content.flex(1.0),
    ))
    .gap(2.px())
    .padding(Padding::from_vh(4.0, 2.0))
}

fn highlight_row_view(
    highlight: StoredHighlight,
    section_index: usize,
    selected: bool,
    language: crate::preferences::AppLanguage,
) -> impl WidgetView<DesktopReader> + use<> {
    let navigate_id = highlight.id.clone();
    let remove_id = highlight.id;
    let quote = ellipsize_display_text(&highlight.quote.replace(['\r', '\n'], " "), 76);
    let background = if selected { UI_ACCENT_SOFT } else { UI_SURFACE };
    let border = if selected {
        UI_ACCENT_BORDER
    } else {
        UI_BORDER
    };
    sized_box(
        flex_row((
            sized_box(label(""))
                .width(4.px())
                .expand_height()
                .background_color(ANNOTATION_SWATCH_COLOR)
                .corner_radius(3.0),
            button(
                flex_col((
                    label(match language {
                        crate::preferences::AppLanguage::SimplifiedChinese => {
                            format!("第 {} 章", section_index + 1)
                        }
                        crate::preferences::AppLanguage::English => {
                            format!("Chapter {}", section_index + 1)
                        }
                    })
                    .font(UI_FONT_STACK)
                    .text_size(11.0)
                    .color(UI_MUTED),
                    label(quote)
                        .font(UI_FONT_STACK)
                        .text_size(12.5)
                        .line_break_mode(LineBreaking::Clip)
                        .color(UI_TEXT_SOFT),
                ))
                .gap(5.px())
                .cross_axis_alignment(CrossAxisAlignment::Start),
                move |state: &mut DesktopReader| state.go_to_highlight(&navigate_id),
            )
            .background_color(Color::TRANSPARENT)
            .active_background_color(UI_SURFACE_MUTED)
            .border_color(Color::TRANSPARENT)
            .hovered_border_color(Color::TRANSPARENT)
            .border_width(0.0)
            .padding(Padding::from_vh(8.0, 8.0))
            .flex(1.0),
            sized_box(
                button(
                    icon_label(Icon::Trash2, 14.0, UI_MUTED),
                    move |state: &mut DesktopReader| state.remove_highlight(&remove_id),
                )
                .background_color(Color::TRANSPARENT)
                .active_background_color(UI_SURFACE_MUTED)
                .border_color(Color::TRANSPARENT)
                .hovered_border_color(UI_BORDER)
                .corner_radius(7.0)
                .padding(0.0),
            )
            .width(28.px())
            .height(28.px()),
        ))
        .gap(5.px())
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .padding(Padding::from_vh(5.0, 6.0)),
    )
    .height(78.px())
    .expand_width()
    .background_color(background)
    .border(border, 1.0)
    .corner_radius(10.0)
}

fn sidebar_book_metadata(state: &DesktopReader) -> (String, String) {
    let title = state.display_metadata.title.clone();
    let author = if state.display_metadata.authors.is_empty() {
        state.language.text("未知作者", "Unknown author").to_owned()
    } else {
        state.display_metadata.authors.join(" / ")
    };
    (title, author)
}

fn sidebar_book_summary(
    cover: Option<ImageData>,
    title: &str,
    author: &str,
    format: BookFormat,
    language: crate::preferences::AppLanguage,
) -> impl WidgetView<DesktopReader> + use<> {
    let title = wrap_display_text(
        title,
        SIDEBAR_TITLE_LINE_DISPLAY_UNITS,
        SIDEBAR_TITLE_MAX_LINES,
    );
    let author = ellipsize_display_text(
        &author.split_whitespace().collect::<Vec<_>>().join(" "),
        SIDEBAR_AUTHOR_MAX_DISPLAY_UNITS,
    );
    flex_row((
        sidebar_book_cover(cover, format, language),
        flex_col((
            prose(title)
                .text_size(13.5)
                .weight(FontWeight::BOLD)
                .text_color(UI_TEXT)
                .line_break_mode(LineBreaking::Clip),
            label(author)
                .text_size(11.5)
                .color(UI_MUTED)
                .line_break_mode(LineBreaking::Clip),
        ))
        .gap(4.px())
        .cross_axis_alignment(CrossAxisAlignment::Start)
        .flex(1.0),
    ))
    .gap(12.px())
    .cross_axis_alignment(CrossAxisAlignment::Start)
    .padding(Padding::from_vh(14.0, 4.0))
}

fn sidebar_book_cover(
    cover: Option<ImageData>,
    format: BookFormat,
    language: crate::preferences::AppLanguage,
) -> Box<AnyWidgetView<DesktopReader>> {
    if let Some(cover) = cover {
        sized_box(image(cover).fit(ObjectFit::Contain))
            .width(54.px())
            .height(74.px())
            .background_color(UI_SURFACE_MUTED)
            .border(UI_BORDER, 1.0)
            .corner_radius(8.0)
            .boxed()
    } else {
        sized_box(
            flex_col((
                label(language.text("电子书", "E-book"))
                    .text_size(10.0)
                    .weight(FontWeight::BOLD)
                    .color(UI_ACCENT),
                label(format.label()).text_size(10.0).color(UI_MUTED),
            ))
            .gap(2.px())
            .cross_axis_alignment(CrossAxisAlignment::Center)
            .main_axis_alignment(MainAxisAlignment::Center),
        )
        .width(54.px())
        .height(74.px())
        .background_color(UI_ACCENT_SOFT)
        .border(UI_ACCENT_BORDER, 1.0)
        .corner_radius(8.0)
        .boxed()
    }
}

fn toc_row_view(
    row: TocViewItem,
    selected: bool,
    expanded: bool,
) -> impl WidgetView<DesktopReader> + use<> {
    let target = row.target;
    let disclosure_row_id = row.id;
    let label_units = 22_usize.saturating_sub(row.depth.saturating_mul(2)).max(10);
    let row_label = ellipsize_display_text(&row.label, label_units);
    let (background, foreground) = if selected {
        (UI_ACCENT_SOFT, UI_ACCENT)
    } else {
        (UI_SIDEBAR, UI_TEXT_SOFT)
    };
    let disclosure: Box<AnyWidgetView<DesktopReader>> = if row.has_children {
        sized_box(
            button(
                icon_label(
                    if expanded {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    },
                    13.0,
                    foreground,
                ),
                move |state: &mut DesktopReader| state.toggle_toc(&disclosure_row_id),
            )
            .background_color(Color::TRANSPARENT)
            .active_background_color(Color::TRANSPARENT)
            .border_color(Color::TRANSPARENT)
            .hovered_border_color(Color::TRANSPARENT)
            .border_width(0.0)
            .corner_radius(6.0)
            .padding(0.0),
        )
        .width(14.px())
        .height(30.px())
        .boxed()
    } else {
        sized_box(label("")).width(14.px()).height(30.px()).boxed()
    };
    sized_box(
        flex_row((
            FlexSpacer::Fixed(
                (f64::from(u32::try_from(row.depth).unwrap_or(u32::MAX)) * 14.0).px(),
            ),
            disclosure,
            button(
                flex_row((
                    label(row_label)
                        .font(UI_FONT_STACK)
                        .text_size(13.0)
                        .weight(if selected {
                            FontWeight::BOLD
                        } else {
                            FontWeight::NORMAL
                        })
                        .line_break_mode(LineBreaking::Clip)
                        .color(foreground),
                    FlexSpacer::Flex(1.0),
                ))
                .cross_axis_alignment(CrossAxisAlignment::Center),
                move |state: &mut DesktopReader| {
                    if let Some(target) = &target {
                        state.go_to(target);
                    }
                },
            )
            .background_color(Color::TRANSPARENT)
            .active_background_color(Color::TRANSPARENT)
            .border_color(Color::TRANSPARENT)
            .hovered_border_color(Color::TRANSPARENT)
            .border_width(0.0)
            .padding(0.0)
            .flex(1.0),
        ))
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(38.px())
    .expand_width()
    .background_color(background)
    .border(
        if selected {
            UI_ACCENT_BORDER
        } else {
            background
        },
        1.0,
    )
    .corner_radius(9.0)
}

fn reader_toolbar(
    title: String,
    toc_open: bool,
    menu_open: bool,
    translation_enabled: bool,
    assistant_panel: Option<AssistantPanel>,
    reader_background: Color,
) -> impl WidgetView<DesktopReader> {
    let left: Box<AnyWidgetView<DesktopReader>> = if toc_open {
        sized_box(label("")).width(32.px()).height(32.px()).boxed()
    } else {
        icon_button(Icon::PanelLeft, false, |state: &mut DesktopReader| {
            state.set_sidebar_open(true);
        })
        .boxed()
    };
    flex_row((
        flex_row((
            left,
            icon_button(
                Icon::Languages,
                translation_enabled,
                DesktopReader::toggle_translation,
            ),
        ))
        .gap(8.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
        FlexSpacer::Flex(1.0),
        label(title)
            .text_size(13.5)
            .weight(FontWeight::BOLD)
            .color(UI_TEXT),
        FlexSpacer::Flex(1.0),
        icon_button(
            Icon::MessageCircle,
            assistant_panel == Some(AssistantPanel::Chat),
            |state: &mut DesktopReader| {
                state.toggle_assistant_panel(AssistantPanel::Chat);
            },
        ),
        icon_button(Icon::Menu, menu_open, DesktopReader::toggle_menu),
    ))
    .gap(8.px())
    .cross_axis_alignment(CrossAxisAlignment::Center)
    .background_color(reader_background)
    .padding(Padding::from_vh(6.0, 12.0))
}

pub(super) fn icon_button(
    icon: Icon,
    selected: bool,
    callback: impl Fn(&mut DesktopReader) + Send + Sync + 'static,
) -> impl WidgetView<DesktopReader> {
    let background = if selected {
        UI_SURFACE_MUTED
    } else {
        Color::TRANSPARENT
    };
    sized_box(
        button(
            icon_label(icon, 16.0, if selected { UI_TEXT } else { UI_MUTED }),
            callback,
        )
        .background_color(background)
        .active_background_color(UI_SURFACE_MUTED)
        .border_color(Color::TRANSPARENT)
        .hovered_border_color(Color::TRANSPARENT)
        .border_width(0.0)
        .corner_radius(8.0)
        .padding(0.0),
    )
    .width(32.px())
    .height(32.px())
}

fn reader_menu(language: crate::preferences::AppLanguage) -> impl WidgetView<DesktopReader> {
    sized_box(flex_col((
        menu_row(
            Icon::Library,
            language.text("返回书架", "Back to shelf"),
            DesktopReader::request_exit,
        ),
        divider(),
        menu_row(
            Icon::Settings,
            language.text("设置", "Settings"),
            DesktopReader::open_settings,
        ),
    )))
    .width(180.px())
    .background_color(UI_SURFACE)
    .border(UI_BORDER, 1.0)
    .corner_radius(12.0)
    .padding(6.0)
}

fn menu_row(
    icon: Icon,
    text: &'static str,
    callback: impl Fn(&mut DesktopReader) + Send + Sync + 'static,
) -> impl WidgetView<DesktopReader> {
    sized_box(
        button(
            flex_row((
                icon_label(icon, 16.0, UI_MUTED),
                label(text).text_size(14.0).color(UI_TEXT_SOFT),
                FlexSpacer::Flex(1.0),
            ))
            .gap(12.px())
            .cross_axis_alignment(CrossAxisAlignment::Center)
            .must_fill_major_axis(true),
            callback,
        )
        .background_color(UI_SURFACE)
        .active_background_color(UI_SURFACE_MUTED)
        .border_color(Color::TRANSPARENT)
        .hovered_border_color(UI_BORDER)
        .corner_radius(8.0)
        .padding(Padding::from_vh(10.0, 12.0)),
    )
    .height(48.px())
    .expand_width()
}

fn progress_bar(progress: f64) -> impl WidgetView<DesktopReader> {
    let completed = progress.clamp(0.0, 1.0).max(0.0001);
    let remaining = (1.0 - progress).clamp(0.0, 1.0).max(0.0001);
    sized_box(
        flex_row((
            sized_box(label(""))
                .expand_width()
                .height(PROGRESS_HEIGHT.px())
                .background_color(UI_ACCENT)
                .flex(completed),
            sized_box(label(""))
                .expand_width()
                .height(PROGRESS_HEIGHT.px())
                .background_color(UI_SURFACE_MUTED)
                .flex(remaining),
        ))
        .cross_axis_alignment(CrossAxisAlignment::Fill),
    )
    .height(PROGRESS_HEIGHT.px())
    .expand_width()
}

pub(super) fn value_badge(text: &'static str) -> impl WidgetView<DesktopReader> {
    sized_box(label(text).text_size(12.0).color(UI_TEXT_SOFT))
        .height(CONTROL_HEIGHT.px())
        .background_color(UI_SURFACE)
        .border(UI_BORDER, 1.0)
        .corner_radius(RADIUS_SMALL)
        .padding(Padding::from_vh(5.0, 10.0))
}

pub(super) fn primary_action_button(
    text: &'static str,
    callback: impl Fn(&mut DesktopReader) + Send + Sync + 'static,
) -> impl WidgetView<DesktopReader> {
    sized_box(
        button(
            label(text)
                .text_size(12.5)
                .weight(FontWeight::BOLD)
                .color(UI_SURFACE),
            callback,
        )
        .background_color(UI_ACCENT)
        .active_background_color(UI_TEXT)
        .border_color(UI_ACCENT)
        .corner_radius(RADIUS_SMALL)
        .padding(Padding::from_vh(5.0, 12.0)),
    )
    .height(CONTROL_HEIGHT.px())
}

pub(super) fn secondary_action_button(
    text: &'static str,
    callback: impl Fn(&mut DesktopReader) + Send + Sync + 'static,
) -> impl WidgetView<DesktopReader> {
    sized_box(
        button(label(text).text_size(12.5).color(UI_TEXT_SOFT), callback)
            .background_color(UI_SURFACE)
            .active_background_color(UI_SURFACE_MUTED)
            .border_color(UI_SURFACE)
            .hovered_border_color(UI_BORDER)
            .corner_radius(RADIUS_SMALL)
            .padding(Padding::from_vh(5.0, 10.0)),
    )
    .height(CONTROL_HEIGHT.px())
}

pub(super) fn animated_scrim(
    color: Color,
    callback: impl Fn(&mut DesktopReader) + Send + Sync + 'static,
) -> impl WidgetView<DesktopReader> {
    sized_box(
        button(label(""), callback)
            .background_color(Color::TRANSPARENT)
            .active_background_color(Color::TRANSPARENT)
            .border_color(Color::TRANSPARENT)
            .hovered_border_color(Color::TRANSPARENT)
            .border_width(0.0)
            .padding(0.0),
    )
    .expand()
    .background_color(color)
}

pub(super) fn transparent_catcher(
    callback: impl Fn(&mut DesktopReader) + Send + Sync + 'static,
) -> impl WidgetView<DesktopReader> {
    sized_box(
        button(label(""), callback)
            .background_color(Color::TRANSPARENT)
            .active_background_color(Color::TRANSPARENT)
            .border_color(Color::TRANSPARENT)
            .hovered_border_color(Color::TRANSPARENT)
            .border_width(0.0)
            .padding(0.0),
    )
    .expand()
}

fn sidebar_scrim_color(progress: f32) -> Color {
    Color::from_rgb8(0, 0, 0).with_alpha(SIDEBAR_SCRIM_ALPHA * progress)
}

pub(super) fn modal_scrim_color(progress: f32) -> Color {
    Color::from_rgb8(0x1f, 0x2d, 0x3d).with_alpha(MODAL_SCRIM_ALPHA * progress)
}
