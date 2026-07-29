use std::time::{Duration, Instant};

use egui::text::{CCursor, CCursorRange};
use egui::{Color32, Pos2, Rect, RichText, TextureId, Vec2};
use lucide_icons::Icon;
use rebook_layout::reading_content_left;
use rebook_reader::PageDirection;

use super::chat_autocomplete::{
    ChatReference, ChatReferenceKind, chat_reference_token, move_suggestion_index,
};
use super::chat_markdown::ChatMarkdownState;
use super::{AssistantPanel, DesktopReader, ReaderOverlay, SidebarTab};
use crate::plugins::{ChatCommand, ChatRole, chat_command_suggestions};
use crate::ui::{
    ACCENT, ACCENT_SOFT, BACKGROUND, BORDER, MUTED, SURFACE, SURFACE_MUTED, TEXT,
    decode_color_image, icon, icon_button, navigation_button, navigation_text_button,
    selectable_icon_button, toggle_icon_button,
};

const SIDEBAR_WIDTH: f32 = 256.0;
const SIDEBAR_PADDING: i8 = 8;
const ASSISTANT_WIDTH: f32 = 340.0;
const ASSISTANT_SIDE_PADDING: i8 = 14;
const ASSISTANT_EMPTY_TOP_PADDING: f32 = 12.0;
const ASSISTANT_BOTTOM_PADDING: f32 = 12.0;
const ASSISTANT_COMPOSER_RESERVED_HEIGHT: f32 = 52.0;
const ASSISTANT_INPUT_HEIGHT: f32 = 32.0;
const TOOLBAR_HEIGHT: f32 = 48.0;
const TOOLBAR_CONTROL_SIZE: f32 = 32.0;
const TOOLBAR_TITLE_SIZE: f32 = 15.0;
const WHEEL_PAGE_THRESHOLD: f32 = 18.0;
const WHEEL_TURN_COOLDOWN: Duration = Duration::from_millis(180);

#[derive(Clone, Copy)]
struct AssistantComposerKeys {
    input_had_focus: bool,
    initial_suggestion_count: usize,
    movement: AssistantSuggestionMovement,
    acceptance: AssistantSuggestionAcceptance,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AssistantSuggestionMovement {
    None,
    Forward,
    Backward,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AssistantSuggestionAcceptance {
    None,
    Tab,
    Enter,
}

struct AssistantComposerRender {
    composer_rect: Rect,
    input_response: egui::Response,
    submit: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ReaderFramePlan {
    pub(crate) rect: Rect,
    pub(crate) scene_revision: u64,
    pub(crate) background: peniko::Color,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ReaderPageTexture {
    pub(crate) id: TextureId,
    pub(crate) size: Vec2,
}

impl DesktopReader {
    pub(crate) fn ui(
        &mut self,
        root_ui: &mut egui::Ui,
        page_texture: Option<ReaderPageTexture>,
        interaction_blocked: bool,
    ) -> ReaderFramePlan {
        let ctx = root_ui.ctx().clone();
        self.advance_frame(Instant::now());
        self.keyboard_shortcuts(&ctx, interaction_blocked);
        if self.ui.needs_motion_tick() || self.pending_page_turn.is_some() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }

        let sidebar_progress = self.ui.sidebar_motion.value.clamp(0.0, 1.0);
        let assistant_progress = self.ui.assistant_motion.value.clamp(0.0, 1.0);
        if self.ui.sidebar_pinned && sidebar_progress > 0.001 {
            egui::Panel::left("reader-sidebar")
                .exact_size(SIDEBAR_WIDTH * sidebar_progress)
                .resizable(false)
                .show_separator_line(false)
                .frame(
                    egui::Frame::new()
                        .fill(SURFACE)
                        .inner_margin(SIDEBAR_PADDING),
                )
                .show(root_ui, |ui| self.sidebar(ui));
        }
        if self.ui.assistant_panel.is_some() && assistant_progress > 0.001 {
            egui::Panel::right("reader-assistant")
                .exact_size(ASSISTANT_WIDTH * assistant_progress)
                .resizable(false)
                .show_separator_line(false)
                .frame(
                    egui::Frame::new()
                        .fill(BACKGROUND)
                        .inner_margin(egui::Margin::symmetric(ASSISTANT_SIDE_PADDING, 0)),
                )
                .show(root_ui, |ui| self.assistant(ui));
        }

        let background = self.reader.style().background;
        let background_ui = color32(background);
        let mut page_rect = Rect::NOTHING;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(background_ui))
            .show(root_ui, |ui| {
                self.toolbar(ui, background_ui);
                let size = Vec2::new(ui.available_width(), (ui.available_height() - 3.0).max(1.0));
                let response = if let Some(texture) = page_texture {
                    let (rect, response) =
                        ui.allocate_exact_size(size, egui::Sense::click_and_drag());
                    let painter = ui.painter().with_clip_rect(rect);
                    painter.rect_filled(rect, 0.0, background_ui);
                    let texture_rect = page_texture_destination(rect, texture.size);
                    painter.image(
                        texture.id,
                        texture_rect,
                        Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                    response
                } else {
                    let (rect, response) =
                        ui.allocate_exact_size(size, egui::Sense::click_and_drag());
                    ui.painter().rect_filled(rect, 0.0, background_ui);
                    response
                };
                page_rect = response.rect;
                self.pointer_interaction(&response);
                self.wheel_interaction(&response, interaction_blocked);
                self.resize_canvas(f64::from(page_rect.width()), f64::from(page_rect.height()));

                let progress = unit_f32(self.progress());
                let (track, _) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), 3.0),
                    egui::Sense::hover(),
                );
                ui.painter()
                    .rect_filled(track, 0.0, Color32::from_black_alpha(18));
                let filled = Rect::from_min_size(
                    track.min,
                    Vec2::new(track.width() * progress, track.height()),
                );
                ui.painter().rect_filled(filled, 0.0, ACCENT);
            });

        if !self.ui.sidebar_pinned && sidebar_progress > 0.001 {
            self.floating_sidebar(&ctx, sidebar_progress);
        }
        self.menu(&ctx);
        self.selection_actions(&ctx, page_rect);
        self.feedback(&ctx);

        ReaderFramePlan {
            rect: page_rect,
            scene_revision: self.scene_revision,
            background: peniko::Color::from_rgba8(
                background.red,
                background.green,
                background.blue,
                background.alpha,
            ),
        }
    }

    fn keyboard_shortcuts(&mut self, ctx: &egui::Context, interaction_blocked: bool) {
        let open_search = !interaction_blocked
            && !self.ui.overlay_visible()
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::F));
        if open_search {
            self.open_search();
            return;
        }
        if interaction_blocked || ctx.egui_wants_keyboard_input() || self.ui.overlay_visible() {
            return;
        }
        let previous = ctx.input(|input| {
            input.key_pressed(egui::Key::ArrowLeft) || input.key_pressed(egui::Key::PageUp)
        });
        let next = ctx.input(|input| {
            input.key_pressed(egui::Key::ArrowRight)
                || input.key_pressed(egui::Key::PageDown)
                || input.key_pressed(egui::Key::Space)
        });
        if previous {
            self.turn_page(PageDirection::Previous);
        }
        if next {
            self.turn_page(PageDirection::Next);
        }
    }

    fn wheel_interaction(&mut self, response: &egui::Response, interaction_blocked: bool) {
        if interaction_blocked
            || !response.hovered()
            || self.ui.overlay_visible()
            || self.selection.is_some()
            || self.pending_page_turn.is_some()
        {
            self.ui.wheel_accumulator = 0.0;
            return;
        }

        let delta = response.ctx.input(|input| {
            input
                .raw
                .events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::MouseWheel {
                        unit,
                        delta,
                        modifiers,
                        ..
                    } if !modifiers.ctrl && !modifiers.command => Some(
                        delta.y
                            * match unit {
                                egui::MouseWheelUnit::Point => 1.0,
                                egui::MouseWheelUnit::Line => 50.0,
                                egui::MouseWheelUnit::Page => 240.0,
                            },
                    ),
                    _ => None,
                })
                .sum::<f32>()
        });
        if delta.abs() <= f32::EPSILON {
            return;
        }
        if self.ui.wheel_accumulator.signum() != delta.signum() {
            self.ui.wheel_accumulator = 0.0;
        }
        self.ui.wheel_accumulator += delta;
        if self.ui.wheel_accumulator.abs() < WHEEL_PAGE_THRESHOLD {
            return;
        }

        let now = Instant::now();
        if self
            .ui
            .last_wheel_turn
            .is_some_and(|last| now.saturating_duration_since(last) < WHEEL_TURN_COOLDOWN)
        {
            return;
        }
        let direction = if self.ui.wheel_accumulator < 0.0 {
            PageDirection::Next
        } else {
            PageDirection::Previous
        };
        self.ui.wheel_accumulator = 0.0;
        self.ui.last_wheel_turn = Some(now);
        response.ctx.input_mut(|input| {
            input.smooth_scroll_delta.y = 0.0;
        });
        self.turn_page(direction);
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, background: Color32) {
        let toolbar_width = ui.available_width();
        let content_left = reading_content_left(toolbar_width, &self.reader.style());
        let response = egui::Frame::new()
            .fill(background)
            .inner_margin(egui::Margin::symmetric(0, SIDEBAR_PADDING))
            .show(ui, |ui| {
                ui.set_min_width(toolbar_width);
                ui.set_min_height(TOOLBAR_HEIGHT - f32::from(SIDEBAR_PADDING) * 2.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if self.ui.sidebar_open {
                        // Establish the same row height as the toolbar controls before
                        // laying out the title, so its vertical center never depends on
                        // whether the sidebar toggle is present.
                        ui.allocate_space(Vec2::new(content_left, TOOLBAR_CONTROL_SIZE));
                    } else {
                        let button_left = f32::from(SIDEBAR_PADDING)
                            .min((content_left - TOOLBAR_CONTROL_SIZE).max(0.0));
                        ui.add_space(button_left);
                        if icon_button(ui, Icon::PanelLeft)
                            .on_hover_text(self.language.text("展开侧栏", "Open sidebar"))
                            .clicked()
                        {
                            self.set_sidebar_open(true);
                        }
                        ui.add_space((content_left - button_left - TOOLBAR_CONTROL_SIZE).max(0.0));
                    }
                    let opacity = self.ui.toolbar_motion.value.clamp(0.0, 1.0);
                    if opacity > 0.02 || self.ui.overlay == ReaderOverlay::Menu {
                        ui.label(
                            RichText::new(&self.display_metadata.title)
                                .color(TEXT)
                                .size(TOOLBAR_TITLE_SIZE),
                        );
                        ui.scope_builder(
                            egui::UiBuilder::new()
                                .id_salt("reader-toolbar-actions")
                                .layout(egui::Layout::right_to_left(egui::Align::Center)),
                            |ui| {
                                ui.add_space(12.0);
                                if icon_button(ui, Icon::Menu)
                                    .on_hover_text(self.language.text("菜单", "Menu"))
                                    .clicked()
                                {
                                    self.toggle_menu();
                                }
                                if icon_button(ui, Icon::MessageCircle)
                                    .on_hover_text(self.language.text("AI 助手", "AI assistant"))
                                    .clicked()
                                {
                                    self.toggle_assistant_panel(AssistantPanel::Chat);
                                }
                                if toggle_icon_button(
                                    ui,
                                    Icon::Languages,
                                    self.translation.enabled,
                                    self.language.text("开", "On"),
                                    self.language.text("关", "Off"),
                                )
                                .on_hover_text(if self.translation.enabled {
                                    self.language.text("关闭翻译", "Turn translation off")
                                } else {
                                    self.language.text("开启翻译", "Turn translation on")
                                })
                                .clicked()
                                {
                                    self.toggle_translation();
                                }
                            },
                        );
                    }
                });
            })
            .response;
        let hovered = ui.ctx().input(|input| {
            input
                .pointer
                .hover_pos()
                .is_some_and(|position| response.rect.contains(position))
        });
        self.set_toolbar_hovered(hovered);
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if icon_button(ui, Icon::PanelLeft)
                .on_hover_text(self.language.text("收起侧栏", "Close sidebar"))
                .clicked()
            {
                self.set_sidebar_open(false);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(
                    ui,
                    if self.ui.sidebar_pinned {
                        Icon::Pin
                    } else {
                        Icon::PinOff
                    },
                )
                .on_hover_text(if self.ui.sidebar_pinned {
                    self.language.text("取消固定", "Unpin sidebar")
                } else {
                    self.language.text("固定侧栏", "Pin sidebar")
                })
                .clicked()
                {
                    self.ui.sidebar_pinned = !self.ui.sidebar_pinned;
                }

                if selectable_icon_button(
                    ui,
                    Icon::ListTree,
                    self.ui.sidebar_tab == SidebarTab::Toc,
                )
                .on_hover_text(self.language.text("目录", "Contents"))
                .clicked()
                {
                    self.set_sidebar_tab(SidebarTab::Toc);
                }
                if selectable_icon_button(
                    ui,
                    Icon::Highlighter,
                    self.ui.sidebar_tab == SidebarTab::Highlights,
                )
                .on_hover_text(self.language.text("高亮", "Highlights"))
                .clicked()
                {
                    self.set_sidebar_tab(SidebarTab::Highlights);
                }
                if selectable_icon_button(
                    ui,
                    Icon::Search,
                    self.ui.sidebar_tab == SidebarTab::Search,
                )
                .on_hover_text(self.language.text("搜索", "Search"))
                .clicked()
                {
                    self.open_search();
                }
            });
        });
        self.book_summary(ui);
        ui.separator();
        ui.add_space(4.0);
        match self.ui.sidebar_tab {
            SidebarTab::Toc => self.toc(ui),
            SidebarTab::Highlights => self.highlights(ui),
            SidebarTab::Search => self.search(ui),
        }
    }

    fn floating_sidebar(&mut self, ctx: &egui::Context, progress: f32) {
        let screen = ctx.content_rect();
        egui::Area::new("reader-sidebar-scrim".into())
            .order(egui::Order::Middle)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (rect, response) = ui.allocate_exact_size(screen.size(), egui::Sense::click());
                ui.painter()
                    .rect_filled(rect, 0.0, Color32::BLACK.gamma_multiply(0.31 * progress));
                if response.clicked() {
                    self.set_sidebar_open(false);
                }
            });
        egui::Area::new("reader-sidebar-floating".into())
            .order(egui::Order::Foreground)
            .fixed_pos(Pos2::new(-SIDEBAR_WIDTH * (1.0 - progress), 0.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(SURFACE)
                    .stroke(egui::Stroke::new(1.0, BORDER))
                    .inner_margin(SIDEBAR_PADDING)
                    .show(ui, |ui| {
                        let sidebar_inset = f32::from(SIDEBAR_PADDING) * 2.0;
                        ui.set_width(SIDEBAR_WIDTH - sidebar_inset);
                        ui.set_height(ctx.content_rect().height() - sidebar_inset);
                        self.sidebar(ui);
                    });
            });
    }

    fn book_summary(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if self.cover_texture.is_none()
                && let Some(bytes) = &self.cover
                && let Ok(image) = decode_color_image(bytes)
            {
                self.cover_texture = Some(ui.ctx().load_texture(
                    "reader-cover",
                    image,
                    egui::TextureOptions::LINEAR,
                ));
            }
            if let Some(texture) = &self.cover_texture {
                ui.add(egui::Image::new(texture).fit_to_exact_size(Vec2::new(52.0, 74.0)));
            } else {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(52.0, 74.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 5.0, SURFACE_MUTED);
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    self.format.label(),
                    egui::FontId::proportional(10.0),
                    ACCENT,
                );
            }
            let summary_width = ui.available_width().max(1.0);
            ui.allocate_ui_with_layout(
                Vec2::new(summary_width, 74.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.label(
                        RichText::new(&self.display_metadata.title)
                            .strong()
                            .color(TEXT),
                    )
                    .on_hover_text(&self.display_metadata.title);
                    let authors = self.display_metadata.authors.join(" / ");
                    if !authors.is_empty() {
                        ui.label(RichText::new(authors).size(12.0).color(MUTED));
                    }
                },
            );
        });
        ui.add_space(10.0);
    }

    fn toc(&mut self, ui: &mut egui::Ui) {
        let rows = self
            .reader
            .toc_items()
            .iter()
            .filter(|row| {
                row.ancestors
                    .iter()
                    .all(|ancestor| self.ui.expanded_toc.contains(ancestor))
            })
            .cloned()
            .collect::<Vec<_>>();
        let active = self.snapshot.active_toc_id.clone();
        let (should_auto_scroll, mut did_auto_scroll) =
            (active != self.ui.last_auto_scrolled_toc, false);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let content_width = (ui.available_width() - 12.0).max(1.0);
                ui.set_width(content_width);
                for row in rows {
                    let selected = active.as_ref() == Some(&row.id);
                    let display_label =
                        if self.translation.enabled && self.plugin_settings.translate_toc {
                            self.translation
                                .toc_labels
                                .get(&row.id)
                                .unwrap_or(&row.label)
                        } else {
                            &row.label
                        };
                    let (row_rect, row_response) = ui
                        .allocate_exact_size(Vec2::new(content_width, 36.0), egui::Sense::click());
                    let mut row_response =
                        row_response.on_hover_cursor(egui::CursorIcon::PointingHand);
                    if selected && should_auto_scroll {
                        ui.scroll_to_rect(row_rect, Some(egui::Align::Center));
                        did_auto_scroll = true;
                    }
                    let row_fill = if selected {
                        ACCENT_SOFT
                    } else if row_response.hovered() {
                        ui.visuals().widgets.hovered.weak_bg_fill
                    } else {
                        Color32::TRANSPARENT
                    };
                    if row_fill != Color32::TRANSPARENT {
                        ui.painter().rect_filled(row_rect, 6.0, row_fill);
                    }
                    let depth = u16::try_from(row.depth).unwrap_or(u16::MAX);
                    let toggle_rect = Rect::from_min_size(
                        Pos2::new(
                            row_rect.left() + 2.0 + f32::from(depth) * 12.0,
                            row_rect.top() + 5.0,
                        ),
                        Vec2::splat(26.0),
                    );
                    let toggle = if row.has_children {
                        let expanded = self.ui.expanded_toc.contains(&row.id);
                        toc_toggle_button(
                            ui,
                            toggle_rect.center(),
                            &row.id,
                            expanded,
                            selected,
                            self.language.text("折叠", "Collapse"),
                            self.language.text("展开", "Expand"),
                        )
                    } else {
                        false
                    };
                    let label_rect = toc_label_rect(row_rect, toggle_rect);
                    if paint_toc_label(ui, label_rect, display_label, selected) {
                        row_response = row_response.on_hover_text(display_label);
                    }
                    row_response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            ui.is_enabled(),
                            display_label,
                        )
                    });
                    let navigate = row_response.clicked() && !toggle;

                    if toggle {
                        self.toggle_toc(&row.id);
                    }
                    if navigate && let Some(target) = row.target {
                        self.go_to(&target);
                    }
                }
            });
        if active.is_none() || did_auto_scroll {
            self.ui.last_auto_scrolled_toc = active;
        }
    }

    fn highlights(&mut self, ui: &mut egui::Ui) {
        let highlights = self.highlights.clone();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let content_width = (ui.available_width() - 12.0).max(1.0);
                ui.set_width(content_width);
                for highlight in highlights {
                    let selected = self.selected_highlight_id.as_deref() == Some(&highlight.id);
                    ui.horizontal(|ui| {
                        ui.set_width(content_width);
                        let quote_width = (ui.available_width()
                            - TOOLBAR_CONTROL_SIZE
                            - ui.spacing().item_spacing.x)
                            .max(1.0);
                        let quote_response = ui
                            .add_sized(
                                [quote_width, TOOLBAR_CONTROL_SIZE],
                                egui::Button::selectable(
                                    selected,
                                    RichText::new(&highlight.quote).size(12.0),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&highlight.quote);
                        if quote_response.clicked() {
                            self.go_to_highlight(&highlight.id);
                        }
                        if icon_button(ui, Icon::Trash2)
                            .on_hover_text(self.language.text("删除", "Delete"))
                            .clicked()
                        {
                            self.remove_highlight(&highlight.id);
                        }
                    });
                    ui.separator();
                }
            });
    }

    fn search(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width();
        let (response, clicked) = compact_input_frame()
            .show(ui, |ui| {
                ui.set_min_width((width - 16.0).max(1.0));
                ui.horizontal(|ui| {
                    let input_width = (ui.available_width() - 40.0).max(48.0);
                    let response = ui.add_sized(
                        [input_width, 32.0],
                        egui::TextEdit::singleline(&mut self.search.query)
                            .hint_text(self.language.text("搜索正文", "Search book"))
                            .frame(egui::Frame::NONE)
                            .vertical_align(egui::Align::Center)
                            .margin(egui::Margin::symmetric(2, 0)),
                    );
                    let clicked = icon_button(ui, Icon::Search)
                        .on_hover_text(self.language.text("搜索", "Search"))
                        .clicked();
                    (response, clicked)
                })
                .inner
            })
            .inner;
        if std::mem::take(&mut self.search.focus_input) {
            response.request_focus();
        }
        if (response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)))
            || clicked
        {
            self.start_search();
        }
        if !self.search.status.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new(&self.search.status).size(12.0).color(MUTED));
        }
        let results = self.search.results.clone();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for result in results {
                if ui
                    .button(RichText::new(&result.excerpt).size(12.0))
                    .clicked()
                {
                    self.go_to_search_result(&result);
                }
            }
        });
    }

    fn assistant(&mut self, ui: &mut egui::Ui) {
        self.assistant_header(ui);

        let busy = self.chat.task.is_pending();
        let reference_rows =
            u16::try_from(self.chat.references.len().div_ceil(2)).unwrap_or(u16::MAX);
        let reference_height = f32::from(reference_rows) * 28.0;
        let error_height = if self.chat.error.is_some() { 54.0 } else { 0.0 };
        let conversation_height = (ui.available_height()
            - ASSISTANT_COMPOSER_RESERVED_HEIGHT
            - ASSISTANT_BOTTOM_PADDING
            - reference_height
            - error_height)
            .max(96.0);
        self.assistant_conversation(ui, conversation_height, busy);
        self.assistant_error(ui);
        self.assistant_composer(ui);
    }

    fn assistant_header(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width();
        let header = ui.allocate_ui_with_layout(
            Vec2::new(width, TOOLBAR_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(icon(Icon::MessageCircle).color(MUTED));
                ui.label(
                    RichText::new(self.language.text("AI 对话", "AI chat"))
                        .size(14.0)
                        .strong()
                        .color(TEXT),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, Icon::X)
                        .on_hover_text(self.language.text("关闭", "Close"))
                        .clicked()
                    {
                        self.close_assistant_panel();
                    }
                    ui.add_enabled_ui(!self.chat.messages.is_empty(), |ui| {
                        if icon_button(ui, Icon::Trash2)
                            .on_hover_text(self.language.text("清空", "Clear"))
                            .clicked()
                        {
                            self.clear_chat();
                        }
                    });
                });
            },
        );
        ui.painter().hline(
            header.response.rect.left()..=header.response.rect.right(),
            header.response.rect.bottom(),
            egui::Stroke::new(1.0, BORDER),
        );
        ui.add_space(10.0);
    }

    fn assistant_conversation(&mut self, ui: &mut egui::Ui, height: f32, busy: bool) {
        let messages = self.chat.messages.clone();
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .max_height(height)
            .min_scrolled_height(height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if messages.is_empty() && !busy {
                    ui.allocate_ui_with_layout(
                        Vec2::new(ui.available_width(), height),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            ui.add_space(ASSISTANT_EMPTY_TOP_PADDING);
                            ui.label(icon(Icon::MessageCircle).size(27.0).color(MUTED));
                            ui.label(
                                RichText::new(
                                    self.language
                                        .text("围绕当前书籍提问", "Ask about this book"),
                                )
                                .strong()
                                .color(TEXT),
                            );
                            ui.label(
                                RichText::new(self.language.text(
                                    "可以总结章节、解释选中的段落，\n或搜索书中的概念。",
                                    "Summarize sections, explain a selection,\nor find concepts in the book.",
                                ))
                                .size(12.0)
                                .color(MUTED),
                            );
                        },
                    );
                }
                for message in messages {
                    chat_message_card(
                        ui,
                        message.role,
                        message
                            .display_content
                            .as_deref()
                            .unwrap_or(&message.content),
                        self.language,
                        &mut self.chat_markdown,
                    );
                    ui.add_space(10.0);
                }
                if busy {
                    chat_message_card(
                        ui,
                        ChatRole::Assistant,
                        self.language.text(
                            "正在阅读和检索书籍…",
                            "Reading and searching the book…",
                        ),
                        self.language,
                        &mut self.chat_markdown,
                    );
                }
            });
    }

    fn assistant_error(&self, ui: &mut egui::Ui) {
        if let Some(error) = &self.chat.error {
            egui::Frame::new()
                .fill(Color32::from_rgb(252, 239, 238))
                .stroke(egui::Stroke::new(1.0, Color32::from_rgb(226, 180, 176)))
                .corner_radius(8)
                .inner_margin(9)
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(error)
                            .size(12.0)
                            .color(Color32::from_rgb(151, 54, 50)),
                    );
                });
        }
    }

    fn assistant_composer(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        let busy = self.chat.task.is_pending();
        let input_id = ui.make_persistent_id("assistant-chat-input");
        let (initial_references, initial_commands) = self.assistant_suggestions(busy);
        let keys = assistant_composer_keys(
            ui,
            input_id,
            active_suggestion_count(&initial_references, &initial_commands),
        );
        let render = self.assistant_composer_input(ui, input_id);
        let (reference_suggestions, command_suggestions) = self.assistant_suggestions(busy);
        let suggestion_count =
            active_suggestion_count(&reference_suggestions, &command_suggestions);
        self.chat.suggestion_index = self
            .chat
            .suggestion_index
            .min(suggestion_count.saturating_sub(1));

        let mut submit = render.submit;
        let suggestion_applied = self.apply_assistant_suggestion_key(
            keys,
            &reference_suggestions,
            &command_suggestions,
            &render.input_response,
            &mut submit,
        );
        if !suggestion_applied && suggestion_count > 0 {
            let (picked_reference, picked_command, hovered_index) = assistant_suggestion_popup(
                ui,
                render.composer_rect,
                &reference_suggestions,
                &command_suggestions,
                self.chat.suggestion_index,
                self.language,
            );
            if let Some(index) = hovered_index {
                self.chat.suggestion_index = index;
            }
            if let Some(reference) = picked_reference {
                self.select_chat_reference(reference);
                render.input_response.request_focus();
            } else if let Some(command) = picked_command {
                self.select_chat_command(command);
                render.input_response.request_focus();
            }
        }
        if submit {
            self.send_chat();
        }
        ui.add_space(ASSISTANT_BOTTOM_PADDING);
    }

    fn assistant_suggestions(&mut self, busy: bool) -> (Vec<ChatReference>, Vec<ChatCommand>) {
        let reference_token_active = chat_reference_token(
            &self.chat.input,
            self.chat.cursor_char_index,
            &self.chat.references,
        )
        .is_some();
        let references = if busy {
            Vec::new()
        } else {
            self.current_chat_reference_suggestions()
        };
        let commands = if busy || reference_token_active {
            Vec::new()
        } else {
            chat_command_suggestions(&self.chat.input)
        };
        (references, commands)
    }

    fn assistant_composer_input(
        &mut self,
        ui: &mut egui::Ui,
        input_id: egui::Id,
    ) -> AssistantComposerRender {
        let references = self.chat.references.clone();
        let mut remove_reference = None;
        let mut input_response = None;
        let mut submit = false;
        let move_cursor_to_end = std::mem::take(&mut self.chat.move_cursor_to_end);
        let composer = compact_input_frame().show(ui, |ui| {
            remove_reference = chat_reference_chips(ui, &references, self.language);
            ui.horizontal(|ui| {
                let input_width = (ui.available_width() - 38.0).max(48.0);
                let hint_text = self.language.text(
                    "询问这本书，输入 / 使用技能或 @ 引用…",
                    "Ask this book, type / for skills or @ to reference…",
                );
                let (mut output, input_rect) =
                    centered_assistant_text_edit(ui, &mut self.chat.input, input_id, input_width);
                if self.chat.input.is_empty() {
                    // egui 0.35 forces its built-in hint atom to LEFT_TOP, so
                    // paint the placeholder ourselves to honor vertical centering.
                    ui.painter().with_clip_rect(input_rect).text(
                        input_rect.left_center() + Vec2::new(4.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        hint_text,
                        egui::TextStyle::Body.resolve(ui.style()),
                        ui.visuals().weak_text_color(),
                    );
                }
                if output.response.changed() {
                    self.chat.suggestion_index = 0;
                }
                self.chat.cursor_char_index = output.cursor_range.map_or_else(
                    || self.chat.input.chars().count(),
                    |range| range.primary.index.into(),
                );
                if move_cursor_to_end {
                    let cursor = CCursor::new(self.chat.input.chars().count());
                    output
                        .state
                        .cursor
                        .set_char_range(Some(CCursorRange::one(cursor)));
                    output.state.store(ui.ctx(), output.response.id);
                    output.response.request_focus();
                    self.chat.cursor_char_index = cursor.index.into();
                }
                input_response = Some(output.response.response.clone());
                submit = icon_button(ui, Icon::Send)
                    .on_hover_text(self.language.text("发送", "Send"))
                    .clicked();
            });
        });
        if let Some(id) = remove_reference {
            self.remove_chat_reference(&id);
        }
        AssistantComposerRender {
            composer_rect: composer.response.rect,
            input_response: input_response.expect("chat input is always rendered"),
            submit,
        }
    }

    fn apply_assistant_suggestion_key(
        &mut self,
        keys: AssistantComposerKeys,
        references: &[ChatReference],
        commands: &[ChatCommand],
        input_response: &egui::Response,
        submit: &mut bool,
    ) -> bool {
        let suggestion_count = active_suggestion_count(references, commands);
        if keys.input_had_focus && keys.initial_suggestion_count > 0 && suggestion_count > 0 {
            match keys.movement {
                AssistantSuggestionMovement::Forward => {
                    self.chat.suggestion_index =
                        move_suggestion_index(self.chat.suggestion_index, suggestion_count, true);
                }
                AssistantSuggestionMovement::Backward => {
                    self.chat.suggestion_index =
                        move_suggestion_index(self.chat.suggestion_index, suggestion_count, false);
                }
                AssistantSuggestionMovement::None => {}
            }
        }
        let input_is_active = input_response.has_focus() || input_response.lost_focus();
        let apply = input_is_active
            && keys.initial_suggestion_count > 0
            && suggestion_count > 0
            && keys.acceptance != AssistantSuggestionAcceptance::None;
        if !apply {
            if input_is_active
                && keys.acceptance == AssistantSuggestionAcceptance::Enter
                && suggestion_count == 0
            {
                *submit = true;
            }
            return false;
        }
        if let Some(reference) = references.get(self.chat.suggestion_index).cloned() {
            self.select_chat_reference(reference);
            input_response.request_focus();
            return true;
        }
        let Some(command) = commands.get(self.chat.suggestion_index).copied() else {
            return false;
        };
        let exact_non_argument_command =
            !command.requires_args && self.chat.input.trim().eq_ignore_ascii_case(command.name);
        if keys.acceptance == AssistantSuggestionAcceptance::Enter && exact_non_argument_command {
            *submit = true;
        } else {
            self.select_chat_command(command);
            input_response.request_focus();
        }
        true
    }

    fn menu(&mut self, ctx: &egui::Context) {
        let progress = self.ui.menu_motion.value.clamp(0.0, 1.0);
        if progress <= 0.001 {
            return;
        }
        let assistant_inset = ASSISTANT_WIDTH * self.ui.assistant_motion.value.clamp(0.0, 1.0);
        let menu = egui::Area::new("reader-menu".into())
            .order(egui::Order::Tooltip)
            .anchor(
                egui::Align2::RIGHT_TOP,
                Vec2::new(
                    -12.0 - assistant_inset,
                    TOOLBAR_HEIGHT + 8.0 - (1.0 - progress) * 8.0,
                ),
            )
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(SURFACE)
                    .corner_radius(9)
                    .inner_margin(6)
                    .show(ui, |ui| {
                        ui.set_width(180.0);
                        if navigation_button(
                            ui,
                            Icon::Settings,
                            self.language.text("设置", "Settings"),
                            false,
                        )
                        .clicked()
                        {
                            self.request_settings();
                        }
                        if navigation_button(
                            ui,
                            Icon::Library,
                            self.language.text("返回书架", "Back to library"),
                            false,
                        )
                        .clicked()
                        {
                            self.request_exit();
                        }
                    });
            });
        let clicked_outside = self.ui.overlay == ReaderOverlay::Menu
            && !self.ui.menu_motion.is_animating()
            && ctx.input(|input| {
                input.pointer.any_click()
                    && input
                        .pointer
                        .interact_pos()
                        .is_some_and(|position| !menu.response.rect.contains(position))
            });
        if clicked_outside {
            self.close_overlay();
        }
    }

    fn selection_actions(&mut self, ctx: &egui::Context, page_rect: Rect) {
        if !self.selection_toolbar_visible {
            return;
        }
        let Some(selection) = &self.selection else {
            return;
        };
        let anchor = selection.rects.last().copied();
        let position = anchor.map_or(page_rect.center(), |rect| {
            Pos2::new(
                page_rect.left() + rect.x + rect.width * 0.5,
                page_rect.top() + rect.y + rect.height + 8.0,
            )
        });
        egui::Area::new("selection-actions".into())
            .order(egui::Order::Tooltip)
            .pivot(egui::Align2::CENTER_TOP)
            .fixed_pos(position)
            .constrain(true)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(4)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if icon_button(ui, Icon::Highlighter)
                                .on_hover_text(self.language.text("高亮", "Highlight"))
                                .clicked()
                            {
                                self.create_highlight();
                            }
                            if icon_button(ui, Icon::MessageCircleQuestion)
                                .on_hover_text(self.language.text("解释", "Explain"))
                                .clicked()
                            {
                                self.explain_selection();
                            }
                        });
                    });
            });
    }

    fn pointer_interaction(&mut self, response: &egui::Response) {
        let Some(position) = response.interact_pointer_pos() else {
            return;
        };
        let x = position.x - response.rect.min.x;
        let y = position.y - response.rect.min.y;
        if response.drag_started() {
            self.begin_text_selection(x, y);
        }
        if response.dragged() {
            self.update_text_selection(x, y);
        }
        if response.drag_stopped() {
            // `drag_delta()` is zero on egui's release frame. A
            // `drag_stopped` response has already crossed the drag threshold,
            // so treating it as moved preserves the completed selection.
            self.finish_text_selection(x, y, true);
        }
        if response.clicked() {
            self.finish_text_selection(x, y, false);
        }
    }

    fn feedback(&mut self, ctx: &egui::Context) {
        if let Some(error) = self.translation.error.clone() {
            egui::Area::new("translation-error".into())
                .order(egui::Order::Tooltip)
                .anchor(egui::Align2::RIGHT_TOP, [-18.0, 62.0])
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style())
                        .fill(Color32::from_rgb(78, 39, 39))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(error).color(Color32::WHITE));
                                if icon_button(ui, Icon::X).clicked() {
                                    self.dismiss_translation_notice();
                                }
                            });
                        });
                });
            return;
        }
        if let Some(error) = &self.error {
            egui::Area::new("reader-error".into())
                .order(egui::Order::Tooltip)
                .anchor(egui::Align2::RIGHT_TOP, [-18.0, 62.0])
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style())
                        .fill(Color32::from_rgb(78, 39, 39))
                        .show(ui, |ui| {
                            ui.label(RichText::new(error).color(Color32::WHITE));
                        });
                });
        }
    }
}

fn paint_toc_label(ui: &egui::Ui, rect: Rect, label: &str, selected: bool) -> bool {
    if rect.width() <= 0.0 {
        return false;
    }
    let color = if selected { ACCENT } else { TEXT };
    let font_id = egui::TextStyle::Body.resolve(ui.style());
    let painter = ui.painter();
    let (display_label, elided) = elide_text_to_width(label, rect.width(), |text| {
        painter
            .layout_no_wrap(text.to_owned(), font_id.clone(), color)
            .size()
            .x
    });
    let galley = painter.layout_no_wrap(display_label, font_id, color);
    painter.with_clip_rect(rect).galley(
        Pos2::new(rect.left(), rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    elided
}

fn elide_text_to_width(
    label: &str,
    max_width: f32,
    mut measure: impl FnMut(&str) -> f32,
) -> (String, bool) {
    const ELLIPSIS: &str = "…";

    if measure(label) <= max_width {
        return (label.to_owned(), false);
    }
    if measure(ELLIPSIS) > max_width {
        return (String::new(), true);
    }

    let mut boundaries = label
        .char_indices()
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    boundaries.push(label.len());
    let (mut lower, mut upper) = (0, boundaries.len() - 1);
    while lower < upper {
        let middle = (lower + upper).div_ceil(2);
        let candidate = format!("{}…", &label[..boundaries[middle]]);
        if measure(&candidate) <= max_width {
            lower = middle;
        } else {
            upper = middle - 1;
        }
    }
    (format!("{}…", &label[..boundaries[lower]]), true)
}

fn toc_label_rect(row: Rect, toggle: Rect) -> Rect {
    // Keep an arrow-sized leading slot even for leaf entries so labels align
    // with expandable rows instead of crowding the sidebar edge.
    let left = toggle.right() + 2.0;
    Rect::from_min_max(
        Pos2::new(left, row.top()),
        Pos2::new(row.right() - 8.0, row.bottom()),
    )
}

fn toc_toggle_button(
    ui: &mut egui::Ui,
    center: Pos2,
    id: &str,
    expanded: bool,
    selected: bool,
    collapse_text: &str,
    expand_text: &str,
) -> bool {
    let rect = Rect::from_center_size(center, Vec2::splat(26.0));
    let response = ui
        .interact(rect, ui.id().with(("toc-toggle", id)), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if expanded { collapse_text } else { expand_text });
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 6.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    ui.painter().text(
        center,
        egui::Align2::CENTER_CENTER,
        if expanded {
            Icon::ChevronDown.unicode()
        } else {
            Icon::ChevronRight.unicode()
        },
        egui::FontId::new(15.0, egui::FontFamily::Name("lucide".into())),
        if selected { ACCENT } else { TEXT },
    );
    response.clicked()
}

fn assistant_composer_keys(
    ui: &mut egui::Ui,
    input_id: egui::Id,
    initial_suggestion_count: usize,
) -> AssistantComposerKeys {
    let input_had_focus = ui.memory(|memory| memory.has_focus(input_id));
    let (arrow_down, arrow_up, tab, enter) = ui.input(|input| {
        (
            input.key_pressed(egui::Key::ArrowDown),
            input.key_pressed(egui::Key::ArrowUp),
            input.key_pressed(egui::Key::Tab),
            input.key_pressed(egui::Key::Enter),
        )
    });
    if input_had_focus && initial_suggestion_count > 0 {
        ui.input_mut(|input| {
            if arrow_down {
                input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown);
            }
            if arrow_up {
                input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp);
            }
            if tab {
                input.consume_key(egui::Modifiers::NONE, egui::Key::Tab);
            }
            if enter {
                input.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
            }
        });
    }
    AssistantComposerKeys {
        input_had_focus,
        initial_suggestion_count,
        movement: if arrow_down {
            AssistantSuggestionMovement::Forward
        } else if arrow_up {
            AssistantSuggestionMovement::Backward
        } else {
            AssistantSuggestionMovement::None
        },
        acceptance: if tab {
            AssistantSuggestionAcceptance::Tab
        } else if enter {
            AssistantSuggestionAcceptance::Enter
        } else {
            AssistantSuggestionAcceptance::None
        },
    }
}

fn active_suggestion_count(references: &[ChatReference], commands: &[ChatCommand]) -> usize {
    if references.is_empty() {
        commands.len()
    } else {
        references.len()
    }
}

fn chat_reference_chips(
    ui: &mut egui::Ui,
    references: &[ChatReference],
    language: crate::preferences::AppLanguage,
) -> Option<String> {
    if references.is_empty() {
        return None;
    }
    let mut removed = None;
    ui.horizontal_wrapped(|ui| {
        for reference in references {
            let kind = chat_reference_kind_label(language, reference.kind);
            let label = format!("{kind} · {}  ×", reference.label);
            if ui
                .add(
                    egui::Button::new(RichText::new(label).size(10.5).color(ACCENT))
                        .fill(ACCENT_SOFT)
                        .stroke(egui::Stroke::new(1.0, BORDER))
                        .corner_radius(10),
                )
                .on_hover_text(&reference.description)
                .clicked()
            {
                removed = Some(reference.id.clone());
            }
        }
    });
    ui.add_space(3.0);
    removed
}

const fn chat_reference_kind_label(
    language: crate::preferences::AppLanguage,
    kind: ChatReferenceKind,
) -> &'static str {
    match (language, kind) {
        (crate::preferences::AppLanguage::SimplifiedChinese, ChatReferenceKind::Book) => "全文",
        (crate::preferences::AppLanguage::SimplifiedChinese, ChatReferenceKind::Section) => "章节",
        (crate::preferences::AppLanguage::SimplifiedChinese, ChatReferenceKind::Paragraph) => {
            "段落"
        }
        (crate::preferences::AppLanguage::English, ChatReferenceKind::Book) => "Book",
        (crate::preferences::AppLanguage::English, ChatReferenceKind::Section) => "Chapter",
        (crate::preferences::AppLanguage::English, ChatReferenceKind::Paragraph) => "Paragraph",
    }
}

fn assistant_suggestion_popup(
    ui: &egui::Ui,
    anchor: Rect,
    references: &[ChatReference],
    commands: &[ChatCommand],
    selected_index: usize,
    language: crate::preferences::AppLanguage,
) -> (Option<ChatReference>, Option<ChatCommand>, Option<usize>) {
    let mut picked_reference = None;
    let mut picked_command = None;
    let mut hovered_index = None;
    let context = ui.ctx().clone();
    egui::Area::new("assistant-chat-suggestions".into())
        .order(egui::Order::Tooltip)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(Pos2::new(anchor.left(), anchor.top() - 7.0))
        .show(&context, |ui| {
            egui::Frame::new()
                .fill(SURFACE)
                .stroke(egui::Stroke::new(1.0, BORDER))
                .corner_radius(8)
                .inner_margin(4)
                .show(ui, |ui| {
                    ui.set_width((anchor.width() - 8.0).max(1.0));
                    if references.is_empty() {
                        for (index, command) in commands.iter().enumerate() {
                            let label = format!("{}  {}", command.name, command.description);
                            let response =
                                navigation_text_button(ui, &label, index == selected_index);
                            if response.hovered() {
                                hovered_index = Some(index);
                            }
                            if response.clicked() {
                                picked_command = Some(*command);
                            }
                        }
                    } else {
                        for (index, reference) in references.iter().enumerate() {
                            let label = chat_reference_suggestion_label(reference, language);
                            let response =
                                navigation_text_button(ui, &label, index == selected_index)
                                    .on_hover_text(&reference.description);
                            if response.hovered() {
                                hovered_index = Some(index);
                            }
                            if response.clicked() {
                                picked_reference = Some(reference.clone());
                            }
                        }
                    }
                });
        });
    (picked_reference, picked_command, hovered_index)
}

fn chat_reference_suggestion_label(
    reference: &ChatReference,
    language: crate::preferences::AppLanguage,
) -> String {
    let kind = chat_reference_kind_label(language, reference.kind);
    if reference.kind != ChatReferenceKind::Book {
        return format!("{kind}  {}", reference.label);
    }
    let fallback = match language {
        crate::preferences::AppLanguage::SimplifiedChinese => "整本书",
        crate::preferences::AppLanguage::English => "Entire book",
    };
    if reference.description == fallback {
        kind.to_owned()
    } else {
        format!("{kind}  {}", reference.description)
    }
}

fn centered_assistant_text_edit(
    ui: &mut egui::Ui,
    input: &mut String,
    input_id: egui::Id,
    width: f32,
) -> (egui::text_edit::TextEditOutput, Rect) {
    let mut input_rect = Rect::NOTHING;
    let output = ui.allocate_ui_with_layout(
        Vec2::new(width, ASSISTANT_INPUT_HEIGHT),
        egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
        |ui| {
            input_rect = ui.max_rect();
            egui::TextEdit::singleline(input)
                .id(input_id)
                .desired_width(width)
                .frame(egui::Frame::NONE)
                .vertical_align(egui::Align::Center)
                .show(ui)
        },
    );
    (output.inner, input_rect)
}

fn page_texture_destination(page_rect: Rect, texture_size: Vec2) -> Rect {
    Rect::from_min_size(page_rect.min, texture_size)
}

fn compact_input_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(9)
        .inner_margin(egui::Margin::symmetric(8, 4))
}

fn chat_message_card(
    ui: &mut egui::Ui,
    role: ChatRole,
    content: &str,
    language: crate::preferences::AppLanguage,
    markdown: &mut ChatMarkdownState,
) {
    let is_user = role == ChatRole::User;
    let width = ui.available_width();
    egui::Frame::new()
        .fill(if is_user { ACCENT_SOFT } else { SURFACE })
        .stroke(egui::Stroke::new(
            1.0,
            if is_user {
                Color32::from_rgb(177, 209, 190)
            } else {
                BORDER
            },
        ))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(10, 9))
        .show(ui, |ui| {
            ui.set_min_width((width - 20.0).max(1.0));
            ui.label(
                RichText::new(if is_user {
                    language.text("你", "You")
                } else {
                    "Torto AI"
                })
                .size(10.5)
                .strong()
                .color(if is_user { ACCENT } else { MUTED }),
            );
            ui.add_space(3.0);
            if is_user {
                ui.add(
                    egui::Label::new(RichText::new(content).size(12.5).color(TEXT))
                        .wrap()
                        .selectable(true),
                );
            } else {
                markdown.show(ui, content, language);
            }
        });
}

fn color32(color: rebook_publication::Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(color.red, color.green, color.blue, color.alpha)
}

#[allow(clippy::cast_possible_truncation)]
fn unit_f32(value: f64) -> f32 {
    value.clamp(0.0, 1.0) as f32
}

#[cfg(test)]
mod reference_suggestion_label_tests {
    use super::*;

    fn reference(kind: ChatReferenceKind, label: &str, description: &str) -> ChatReference {
        ChatReference {
            id: "test".into(),
            kind,
            label: label.into(),
            description: description.into(),
            locator: "rebook://test".into(),
            excerpt: None,
        }
    }

    #[test]
    fn reference_rows_show_only_user_facing_content() {
        let language = crate::preferences::AppLanguage::SimplifiedChinese;
        assert_eq!(
            chat_reference_suggestion_label(
                &reference(ChatReferenceKind::Book, "全文", "Structured Writing"),
                language,
            ),
            "全文  Structured Writing"
        );
        assert_eq!(
            chat_reference_suggestion_label(
                &reference(
                    ChatReferenceKind::Section,
                    "Chapter 7. Rhetorical Structure",
                    "当前章节 · 7",
                ),
                language,
            ),
            "章节  Chapter 7. Rhetorical Structure"
        );
        assert_eq!(
            chat_reference_suggestion_label(
                &reference(ChatReferenceKind::Book, "全文", "整本书"),
                language,
            ),
            "全文"
        );
    }

    #[test]
    fn assistant_text_edit_uses_the_full_centered_input_row() {
        egui::__run_test_ui(|ui| {
            let mut input = String::new();
            let input_id = ui.make_persistent_id("centered-input-test");
            let (output, input_rect) =
                centered_assistant_text_edit(ui, &mut input, input_id, 240.0);
            let galley_center = output.galley_pos.y + output.galley.size().y / 2.0;

            assert!((output.response.rect.height() - ASSISTANT_INPUT_HEIGHT).abs() < 0.01);
            assert!((output.response.rect.center().y - input_rect.center().y).abs() < 0.01);
            assert!((galley_center - input_rect.center().y).abs() < 0.01);
        });
    }

    #[test]
    fn long_toc_labels_are_elided_to_the_sidebar_row() {
        fn measured_width(text: &str) -> f32 {
            f32::from(u16::try_from(text.chars().count()).unwrap_or(u16::MAX)) * 10.0
        }

        let original = "Separate font system that any application can use";
        let (display, elided) = elide_text_to_width(original, 170.0, measured_width);

        assert!(elided);
        assert!(display.ends_with('…'));
        assert!(measured_width(&display) <= 170.0);
        assert!(display.len() < original.len());
    }

    #[test]
    fn stale_page_texture_keeps_its_size_and_starts_at_the_moving_canvas() {
        let page_rect = Rect::from_min_size(Pos2::new(256.0, 48.0), Vec2::new(944.0, 700.0));
        let previous_texture_size = Vec2::new(1_200.0, 700.0);

        let destination = page_texture_destination(page_rect, previous_texture_size);

        assert!((destination.left() - page_rect.left()).abs() < 0.01);
        assert!((destination.top() - page_rect.top()).abs() < 0.01);
        assert!((destination.width() - previous_texture_size.x).abs() < 0.01);
        assert!((destination.height() - previous_texture_size.y).abs() < 0.01);
    }
}
