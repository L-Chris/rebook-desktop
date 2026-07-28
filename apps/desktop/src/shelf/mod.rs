use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Color32, RichText, TextureHandle, Vec2};
use lucide_icons::Icon;
use peniko::Blob;

use crate::async_task::{TaskResult, TaskSlot};
use crate::library::{LibraryBook, LocalLibrary};
use crate::preferences::{self, AppLanguage};
use crate::reader::{BookDisplayMetadata, DesktopReader, open_reader};
use crate::settings::AppliedSettings;
use crate::sync::{LocalSyncBook, SyncReport, SyncSettings, SyncStore, run_sync};
use crate::ui::{
    ACCENT, ACCENT_SOFT, BACKGROUND, BORDER, MUTED, SURFACE, SURFACE_MUTED, TEXT,
    decode_color_image, icon, icon_button,
};

const NOTICE_AUTO_DISMISS_DELAY: Duration = Duration::from_secs(3);
const CARD_WIDTH: f32 = 180.0;
const CARD_HEIGHT: f32 = 318.0;
const COVER_WIDTH: f32 = 160.0;
const COVER_HEIGHT: f32 = 228.0;

pub(crate) struct ShelfFeature {
    shelf: ShelfState,
    pending_reader: Option<DesktopReader>,
    reader_fonts: Arc<[Blob<u8>]>,
    local_store: Option<SyncStore>,
    sync: SyncUiState,
    language: AppLanguage,
    settings_requested: bool,
    cover_textures: HashMap<String, TextureHandle>,
}

struct ShelfState {
    library: LocalLibrary,
    query: String,
    notice: Option<String>,
    notice_dismiss_at: Option<Instant>,
    error: Option<String>,
    remove_confirmation: Option<ShelfRemoveConfirmation>,
}

struct SyncUiState {
    settings: SyncSettings,
    password: String,
    task: TaskSlot<SyncTask>,
    status: String,
}

#[derive(Clone)]
pub(crate) struct SyncTask {
    pub(crate) settings: SyncSettings,
    pub(crate) password: String,
    pub(crate) books: Vec<LocalSyncBook>,
}

pub(crate) type SyncTaskMessage = TaskResult<SyncReport>;

#[derive(Clone, Debug)]
struct ShelfRemoveConfirmation {
    id: String,
    title: String,
}

impl ShelfFeature {
    pub(crate) fn new(library: LocalLibrary, reader_fonts: Arc<[Blob<u8>]>) -> Self {
        let (language, language_error) = preferences::load_app_language().map_or_else(
            |error| {
                (
                    AppLanguage::default(),
                    Some(format!("加载通用设置失败：{error}")),
                )
            },
            |language| (language, None),
        );
        let (settings, settings_error) = SyncSettings::load_default().map_or_else(
            |error| {
                (
                    SyncSettings::new_device(),
                    Some(format!("加载 WebDAV 同步设置失败：{error}")),
                )
            },
            |settings| (settings, None),
        );
        let (password, password_error) = settings.load_password().map_or_else(
            |error| {
                (
                    String::new(),
                    Some(format!("读取 Windows 凭据失败：{error}")),
                )
            },
            |password| (password, None),
        );
        let (local_store, store_error) = SyncStore::open_default(settings.device_id.clone())
            .map_or_else(
                |error| (None, Some(format!("打开本地阅读数据库失败：{error}"))),
                |store| (Some(store), None),
            );
        Self {
            shelf: ShelfState {
                library,
                query: String::new(),
                notice: None,
                notice_dismiss_at: None,
                error: language_error
                    .or(settings_error)
                    .or(password_error)
                    .or(store_error),
                remove_confirmation: None,
            },
            pending_reader: None,
            reader_fonts,
            local_store,
            sync: SyncUiState {
                settings,
                password,
                task: TaskSlot::default(),
                status: String::new(),
            },
            language,
            settings_requested: false,
            cover_textures: HashMap::new(),
        }
    }

    pub(crate) fn open_book(&mut self, path: &Path) {
        let Some(local_store) = self.local_store.clone() else {
            self.shelf.error = Some(
                self.language
                    .text(
                        "本地阅读数据库不可用，无法打开书籍",
                        "The local reading database is unavailable, so the book cannot be opened",
                    )
                    .into(),
            );
            return;
        };
        let metadata = self
            .shelf
            .library
            .books()
            .iter()
            .find(|book| book.path.as_path() == path)
            .map(BookDisplayMetadata::from);
        match open_reader(path, Arc::clone(&self.reader_fonts), metadata, local_store) {
            Ok(reader) => {
                self.pending_reader = Some(reader);
                self.shelf.error = None;
            }
            Err(error) => {
                self.shelf.error = Some(format!(
                    "{}: {error}",
                    self.language.text("无法打开书籍", "Unable to open book")
                ));
            }
        }
    }

    fn import_books(&mut self, paths: &[PathBuf]) {
        self.shelf.error = None;
        match self.shelf.library.import_files(paths) {
            Ok(summary) => {
                self.cover_textures.clear();
                let message = match (self.language, summary.imported, summary.duplicates) {
                    (AppLanguage::SimplifiedChinese, 0, duplicate) => {
                        format!("所选的 {duplicate} 本书已在书架中")
                    }
                    (AppLanguage::SimplifiedChinese, imported, 0) => {
                        format!("已导入 {imported} 本书")
                    }
                    (AppLanguage::SimplifiedChinese, imported, duplicate) => {
                        format!("已导入 {imported} 本书，跳过 {duplicate} 本重复书籍")
                    }
                    (AppLanguage::English, 0, duplicate) => {
                        format!("All {duplicate} selected books are already on the shelf")
                    }
                    (AppLanguage::English, imported, 0) => format!("Imported {imported} books"),
                    (AppLanguage::English, imported, duplicate) => {
                        format!("Imported {imported} books and skipped {duplicate} duplicates")
                    }
                };
                self.show_notice(message);
            }
            Err(error) => {
                self.shelf.error = Some(format!(
                    "{}: {error}",
                    self.language.text("导入失败", "Import failed")
                ));
            }
        }
    }

    fn remove_book(&mut self, id: &str) {
        match self.shelf.library.remove(id) {
            Ok(true) => {
                if let Some(store) = &self.local_store
                    && let Err(error) = store.set_book_present(id, false)
                {
                    tracing::warn!(%error, "failed to persist local book removal tombstone");
                }
                self.cover_textures.remove(id);
                self.show_notice(
                    self.language
                        .text("已从本地书架移除", "Removed from the local shelf")
                        .into(),
                );
                self.shelf.error = None;
            }
            Ok(false) => {
                self.shelf.error = Some(
                    self.language
                        .text(
                            "书籍已不在本地书架中",
                            "The book is no longer on the local shelf",
                        )
                        .into(),
                );
            }
            Err(error) => {
                self.shelf.error = Some(format!(
                    "{}: {error}",
                    self.language.text("移除失败", "Remove failed")
                ));
            }
        }
    }

    pub(crate) fn take_opened_reader(&mut self) -> Option<DesktopReader> {
        self.pending_reader.take()
    }

    pub(crate) fn take_settings_request(&mut self) -> bool {
        std::mem::take(&mut self.settings_requested)
    }

    pub(crate) fn apply_global_settings(&mut self, settings: &AppliedSettings) {
        self.language = settings.language;
        self.sync.settings.clone_from(&settings.sync_settings);
        self.sync.password.clone_from(&settings.sync_password);
        self.start_sync();
    }

    pub(crate) fn resume(&mut self) {
        if let Ok(language) = preferences::load_app_language() {
            self.language = language;
        }
        self.start_sync();
    }

    fn start_sync(&mut self) {
        if self.sync.task.is_pending() || !self.sync.settings.enabled {
            return;
        }
        if let Err(error) = self.sync.settings.validate() {
            self.shelf.error = Some(format!(
                "{}: {error}",
                self.language.text("无法开始同步", "Unable to start sync")
            ));
            return;
        }
        if self.sync.password.is_empty() {
            self.shelf.error = Some(
                self.language
                    .text(
                        "无法开始同步：请先填写 WebDAV 密码",
                        "Unable to start sync: enter the WebDAV password first",
                    )
                    .into(),
            );
            return;
        }
        self.sync.status = self
            .language
            .text("正在同步书籍与阅读数据…", "Syncing books and reading data…")
            .into();
        self.sync.task.begin(SyncTask {
            settings: self.sync.settings.clone(),
            password: self.sync.password.clone(),
            books: self
                .shelf
                .library
                .books()
                .iter()
                .map(|book| LocalSyncBook {
                    id: book.id.clone(),
                    title: book.title.clone(),
                    authors: book.authors.clone(),
                    file_name: book.file_name.clone(),
                    path: book.path.clone(),
                    cover_bytes: book.cover_bytes.clone(),
                    added_at: book.added_at,
                })
                .collect(),
        });
    }

    pub(crate) fn spawn_pending_tasks(
        &mut self,
        runtime: &tokio::runtime::Runtime,
        proxy: &winit::event_loop::EventLoopProxy<crate::platform::UserEvent>,
    ) {
        let Some(request) = self.sync.task.take_pending() else {
            return;
        };
        let proxy = proxy.clone();
        runtime.spawn(async move {
            let id = request.id;
            let payload = request.payload;
            let result = run_sync(payload.settings, payload.password, payload.books)
                .await
                .map_err(|error| error.to_string());
            let _ = proxy.send_event(crate::platform::UserEvent::ShelfSync(SyncTaskMessage {
                id,
                result,
            }));
        });
    }

    pub(crate) fn complete_sync(&mut self, message: SyncTaskMessage) {
        if self.sync.task.complete(message.id).is_none() {
            return;
        }
        match message.result {
            Ok(mut report) => {
                let mut imported = 0;
                for download in report.downloads.drain(..) {
                    match self.shelf.library.import_remote(download) {
                        Ok(true) => imported += 1,
                        Ok(false) => {}
                        Err(error) => {
                            self.shelf.error = Some(error.to_string());
                            return;
                        }
                    }
                }
                if imported > 0 {
                    self.cover_textures.clear();
                }
                self.sync.status = format!(
                    "{} · ↑{} ↓{} · {}",
                    self.language.text("同步完成", "Sync complete"),
                    report.uploaded_books,
                    imported,
                    report.updated_progress,
                );
                self.show_notice(self.sync.status.clone());
            }
            Err(error) => {
                self.shelf.error = Some(format!(
                    "{}: {error}",
                    self.language.text("WebDAV 同步失败", "WebDAV sync failed")
                ));
            }
        }
    }

    pub(crate) fn ui(&mut self, root_ui: &mut egui::Ui) {
        let ctx = root_ui.ctx().clone();
        self.dismiss_notice_if_due();
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(BACKGROUND)
                    .inner_margin(egui::Margin::symmetric(36, 28)),
            )
            .show(root_ui, |ui| {
                self.shelf_header(ui);
                ui.add_space(26.0);

                let query = self.shelf.query.trim().to_lowercase();
                let books: Vec<LibraryBook> = self
                    .shelf
                    .library
                    .books()
                    .iter()
                    .filter(|book| book_matches_query(book, &query))
                    .cloned()
                    .collect();
                if books.is_empty() {
                    self.empty_shelf(ui, query.is_empty());
                } else {
                    egui::ScrollArea::vertical().show(ui, |ui| self.book_grid(ui, &books));
                }
            });
        self.dialogs(&ctx);
    }

    fn shelf_header(&mut self, ui: &mut egui::Ui) {
        let book_count = self.shelf.library.books().len();
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 44.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(icon(Icon::BookOpen).size(23.0).color(ACCENT));
                ui.label(
                    RichText::new(self.language.text("书架", "Library"))
                        .size(22.0)
                        .strong()
                        .color(TEXT),
                );
                ui.label(
                    RichText::new(match self.language {
                        AppLanguage::SimplifiedChinese => format!("{book_count} 本"),
                        AppLanguage::English => format!("{book_count} books"),
                    })
                    .size(12.0)
                    .color(MUTED),
                );
                ui.add_space(22.0);
                shelf_search_field(
                    ui,
                    &mut self.shelf.query,
                    self.language
                        .text("搜索书名或作者", "Search title or author"),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, Icon::Settings)
                        .on_hover_text(self.language.text("设置", "Settings"))
                        .clicked()
                    {
                        self.settings_requested = true;
                    }
                    if shelf_import_button(ui, self.language.text("导入", "Import")).clicked()
                        && let Some(paths) = rfd::FileDialog::new()
                            .add_filter(
                                "E-books",
                                &["epub", "mobi", "azw", "azw3", "fb2", "fbz", "cbz", "pdf"],
                            )
                            .pick_files()
                    {
                        self.import_books(&paths);
                    }
                });
            },
        );
    }

    fn empty_shelf(&mut self, ui: &mut egui::Ui, no_books: bool) {
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 280.0),
            egui::Layout::centered_and_justified(egui::Direction::TopDown),
            |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(icon(Icon::BookOpen).size(30.0).color(MUTED));
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(if no_books {
                            self.language.text("书架还是空的", "Your library is empty")
                        } else {
                            self.language.text("没有匹配的书籍", "No matching books")
                        })
                        .size(16.0)
                        .strong()
                        .color(TEXT),
                    );
                    if no_books {
                        ui.label(
                            RichText::new(self.language.text(
                                "导入 EPUB、MOBI、PDF 等文件开始阅读",
                                "Import EPUB, MOBI, PDF, and other books to begin",
                            ))
                            .size(12.0)
                            .color(MUTED),
                        );
                    }
                });
            },
        );
    }

    fn book_grid(&mut self, ui: &mut egui::Ui, books: &[LibraryBook]) {
        let mut columns = 1_usize;
        let mut occupied = CARD_WIDTH;
        while occupied + 20.0 + CARD_WIDTH <= ui.available_width() {
            columns += 1;
            occupied += 20.0 + CARD_WIDTH;
        }
        egui::Grid::new("shelf-grid")
            .num_columns(columns)
            .spacing(Vec2::new(20.0, 24.0))
            .show(ui, |ui| {
                for (index, book) in books.iter().enumerate() {
                    self.book_card(ui, book);
                    if (index + 1) % columns == 0 {
                        ui.end_row();
                    }
                }
            });
    }

    fn book_card(&mut self, ui: &mut egui::Ui, book: &LibraryBook) {
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(CARD_WIDTH, CARD_HEIGHT), egui::Sense::click());
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        let texture = self.cover_texture(ui.ctx(), book);
        let painter = ui.painter();
        painter.rect_filled(
            rect,
            10.0,
            if response.hovered() {
                ACCENT_SOFT.gamma_multiply(0.42)
            } else {
                SURFACE
            },
        );
        let cover_rect = egui::Rect::from_min_size(
            rect.min + Vec2::splat(10.0),
            Vec2::new(COVER_WIDTH, COVER_HEIGHT),
        );
        painter.rect_filled(cover_rect, 6.0, SURFACE_MUTED);
        if let Some(texture) = texture {
            let image_rect = contain_rect(cover_rect, texture.size_vec2());
            painter.image(
                texture.id(),
                image_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            painter.text(
                cover_rect.center(),
                egui::Align2::CENTER_CENTER,
                Icon::BookOpen.unicode(),
                egui::FontId::new(24.0, egui::FontFamily::Name("lucide".into())),
                MUTED,
            );
        }

        let title_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 12.0, cover_rect.bottom() + 12.0),
            Vec2::new(CARD_WIDTH - 24.0, 42.0),
        );
        let title = painter.layout(
            book.title.clone(),
            egui::FontId::proportional(14.0),
            TEXT,
            title_rect.width(),
        );
        painter
            .with_clip_rect(title_rect)
            .galley(title_rect.min, title, TEXT);

        let authors = book.authors.join(" / ");
        if !authors.is_empty() {
            let author_rect = egui::Rect::from_min_size(
                egui::pos2(rect.left() + 12.0, title_rect.bottom() + 4.0),
                Vec2::new(CARD_WIDTH - 24.0, 20.0),
            );
            let author = painter.layout_no_wrap(authors, egui::FontId::proportional(12.0), MUTED);
            painter
                .with_clip_rect(author_rect)
                .galley(author_rect.min, author, MUTED);
        }

        if response.clicked() {
            self.open_book(&book.path);
        }
        response.context_menu(|ui| {
            if ui
                .button(self.language.text("从书架移除", "Remove from library"))
                .clicked()
            {
                self.shelf.remove_confirmation = Some(ShelfRemoveConfirmation {
                    id: book.id.clone(),
                    title: book.title.clone(),
                });
                ui.close();
            }
        });
        response.on_hover_text(&book.title);
    }

    fn cover_texture(&mut self, ctx: &egui::Context, book: &LibraryBook) -> Option<TextureHandle> {
        if let Some(texture) = self.cover_textures.get(&book.id) {
            return Some(texture.clone());
        }
        let image = decode_color_image(book.cover_bytes.as_deref()?).ok()?;
        let texture = ctx.load_texture(
            format!("cover:{}", book.id),
            image,
            egui::TextureOptions::LINEAR,
        );
        self.cover_textures.insert(book.id.clone(), texture.clone());
        Some(texture)
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(error) = &self.shelf.error {
            egui::Area::new("shelf-error".into())
                .anchor(egui::Align2::RIGHT_TOP, [-24.0, 24.0])
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style())
                        .fill(Color32::from_rgb(69, 32, 32))
                        .show(ui, |ui| {
                            ui.label(RichText::new(error).color(Color32::WHITE));
                        });
                });
        } else if let Some(notice) = &self.shelf.notice {
            egui::Area::new("shelf-notice".into())
                .anchor(egui::Align2::RIGHT_TOP, [-24.0, 24.0])
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).fill(ACCENT).show(ui, |ui| {
                        ui.label(RichText::new(notice).color(Color32::WHITE));
                    });
                });
        }
        let confirmation = self.shelf.remove_confirmation.clone();
        if let Some(confirmation) = confirmation {
            egui::Window::new(self.language.text("移除书籍", "Remove book"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label(format!(
                        "{}\n{}",
                        self.language
                            .text("只会移除本地副本。", "Only the local copy will be removed."),
                        confirmation.title
                    ));
                    ui.horizontal(|ui| {
                        if ui.button(self.language.text("取消", "Cancel")).clicked() {
                            self.shelf.remove_confirmation = None;
                        }
                        if ui.button(self.language.text("移除", "Remove")).clicked() {
                            self.shelf.remove_confirmation = None;
                            self.remove_book(&confirmation.id);
                        }
                    });
                });
        }
    }

    fn show_notice(&mut self, message: String) {
        self.shelf.notice = Some(message);
        self.shelf.notice_dismiss_at = Some(Instant::now() + NOTICE_AUTO_DISMISS_DELAY);
    }

    fn dismiss_notice_if_due(&mut self) {
        if self
            .shelf
            .notice_dismiss_at
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.shelf.notice = None;
            self.shelf.notice_dismiss_at = None;
        }
    }
}

fn shelf_search_field(ui: &mut egui::Ui, query: &mut String, hint: &str) {
    let width = ui.available_width().clamp(180.0, 320.0);
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.set_width(width - 22.0);
            ui.horizontal_centered(|ui| {
                ui.label(icon(Icon::Search).size(15.0).color(MUTED));
                ui.add(
                    egui::TextEdit::singleline(query)
                        .hint_text(hint)
                        .desired_width(ui.available_width())
                        .frame(egui::Frame::NONE)
                        .vertical_align(egui::Align::Center),
                );
            });
        });
}

fn shelf_import_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(80.0, 36.0), egui::Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let fill = if response.is_pointer_button_down_on() {
        ACCENT.gamma_multiply(0.84)
    } else if response.hovered() {
        ACCENT.gamma_multiply(0.92)
    } else {
        ACCENT
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 8.0, fill);
    let icon_font = egui::FontId::new(15.0, egui::FontFamily::Name("lucide".into()));
    let text_font = egui::FontId::proportional(13.0);
    let icon_galley =
        painter.layout_no_wrap(Icon::Plus.unicode().into(), icon_font, Color32::WHITE);
    let text_galley = painter.layout_no_wrap(label.into(), text_font, Color32::WHITE);
    let gap = 6.0;
    let content_width = icon_galley.size().x + gap + text_galley.size().x;
    let start_x = rect.center().x - content_width / 2.0;
    painter.galley(
        egui::pos2(start_x, rect.center().y - icon_galley.size().y / 2.0),
        icon_galley,
        Color32::WHITE,
    );
    painter.galley(
        egui::pos2(
            start_x + content_width - text_galley.size().x,
            rect.center().y - text_galley.size().y / 2.0,
        ),
        text_galley,
        Color32::WHITE,
    );
    response
}

fn contain_rect(bounds: egui::Rect, image_size: Vec2) -> egui::Rect {
    if image_size.x <= 0.0
        || image_size.y <= 0.0
        || !image_size.x.is_finite()
        || !image_size.y.is_finite()
    {
        return bounds;
    }
    let scale = (bounds.width() / image_size.x).min(bounds.height() / image_size.y);
    let size = image_size * scale;
    egui::Rect::from_center_size(bounds.center(), size)
}

fn book_matches_query(book: &LibraryBook, query: &str) -> bool {
    query.is_empty()
        || book.title.to_lowercase().contains(query)
        || book
            .authors
            .iter()
            .any(|author| author.to_lowercase().contains(query))
}
