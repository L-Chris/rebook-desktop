mod width_probe;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lucide_icons::Icon;
use xilem::core::fork;
use xilem::masonry::peniko::{Blob, ImageData};
use xilem::masonry::properties::LineBreaking;
use xilem::masonry::properties::types::{AsUnit, UnitPoint};
use xilem::style::{Padding, Style};
use xilem::view::{
    CrossAxisAlignment, FlexExt, FlexSpacer, ObjectFit, ZStackExt, flex_col, flex_row, image,
    label, portal, prose, sized_box, task_raw, text_input, zstack,
};
use xilem::{Affine, AnyWidgetView, Color, FontWeight, WidgetView};

use crate::async_task::{TaskResult, TaskSlot};
use crate::library::{LibraryBook, LocalLibrary};
use crate::preferences::{self, AppLanguage};
use crate::reader::{BookDisplayMetadata, DesktopReader, open_reader};
use crate::sync::{
    LocalSyncBook, SyncReport, SyncSettings, SyncSettingsCallbacks, SyncStore, run_sync,
    sync_settings_content,
};
use crate::ui::{
    CONTENT_GAP, CONTENT_PADDING_HORIZONTAL, CONTENT_PADDING_VERTICAL, CONTROL_HEIGHT,
    DIALOG_FOOTER_HEIGHT, DIALOG_HEADER_HEIGHT, NoticeTone, RADIUS_DIALOG, RADIUS_SMALL, UI_ACCENT,
    UI_ACCENT_BORDER, UI_ACCENT_SOFT, UI_BACKGROUND, UI_BORDER, UI_FONT_STACK, UI_MUTED,
    UI_SURFACE, UI_SURFACE_MUTED, UI_TEXT, UI_TEXT_SOFT, button, confirmation_dialog, decode_image,
    divider, ellipsize_display_text, icon_label, notice_card,
};
use width_probe::shelf_width_probe;

const SHELF_CARD_WIDTH: f64 = 144.0;
const SHELF_COVER_HEIGHT: f64 = 216.0;
const SHELF_CARD_GAP: f64 = 24.0;
const SHELF_ROW_GAP: f64 = 28.0;
const SHELF_TITLE_MAX_DISPLAY_UNITS: usize = 18;
const INITIAL_SHELF_GRID_WIDTH: f64 = 1144.0;

pub(crate) struct ShelfFeature {
    shelf: ShelfState,
    pending_reader: Option<DesktopReader>,
    reader_fonts: Arc<[Blob<u8>]>,
    local_store: Option<SyncStore>,
    sync: SyncUiState,
    language: AppLanguage,
    draft_language: AppLanguage,
    settings_tab: ShelfSettingsTab,
    settings_open: bool,
}

struct ShelfState {
    library: LocalLibrary,
    covers: HashMap<String, ImageData>,
    grid_width: f64,
    query: String,
    notice: Option<String>,
    error: Option<String>,
    remove_confirmation: Option<ShelfRemoveConfirmation>,
}

struct SyncUiState {
    settings: SyncSettings,
    password: String,
    draft_settings: SyncSettings,
    draft_password: String,
    task: TaskSlot<SyncTask>,
    status: String,
}

#[derive(Clone)]
struct SyncTask {
    settings: SyncSettings,
    password: String,
    books: Vec<LocalSyncBook>,
}

type SyncTaskMessage = TaskResult<SyncReport>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ShelfSettingsTab {
    #[default]
    General,
    Cloud,
}

#[derive(Clone, Debug)]
struct ShelfRemoveConfirmation {
    id: String,
    title: String,
}

impl ShelfFeature {
    pub(crate) fn new(library: LocalLibrary, reader_fonts: Arc<[Blob<u8>]>) -> Self {
        let (language, language_error) = match preferences::load_app_language() {
            Ok(language) => (language, None),
            Err(error) => (
                AppLanguage::default(),
                Some(format!("加载通用设置失败：{error}")),
            ),
        };
        let (settings, settings_error) = match SyncSettings::load_default() {
            Ok(settings) => (settings, None),
            Err(error) => (
                SyncSettings::new_device(),
                Some(format!("加载 WebDAV 同步设置失败：{error}")),
            ),
        };
        let (password, password_error) = match settings.load_password() {
            Ok(password) => (password, None),
            Err(error) => (
                String::new(),
                Some(format!("读取 Windows 凭据失败：{error}")),
            ),
        };
        let (local_store, store_error) = match SyncStore::open_default(settings.device_id.clone()) {
            Ok(store) => (Some(store), None),
            Err(error) => (None, Some(format!("打开本地阅读数据库失败：{error}"))),
        };
        let mut shelf = ShelfState::new(library);
        shelf.error = language_error
            .or(settings_error)
            .or(password_error)
            .or(store_error);
        Self {
            shelf,
            pending_reader: None,
            reader_fonts,
            local_store,
            sync: SyncUiState {
                draft_settings: settings.clone(),
                draft_password: password.clone(),
                settings,
                password,
                task: TaskSlot::default(),
                status: String::new(),
            },
            language,
            draft_language: language,
            settings_tab: ShelfSettingsTab::General,
            settings_open: false,
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
        let shelf_metadata = self
            .shelf
            .library
            .books()
            .iter()
            .find(|book| book.path.as_path() == path)
            .map(BookDisplayMetadata::from);
        match open_reader(
            path,
            Arc::clone(&self.reader_fonts),
            shelf_metadata,
            local_store,
        ) {
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
                self.shelf.refresh_covers();
                self.shelf.notice = Some(
                    match (self.language, summary.imported, summary.duplicates) {
                        (AppLanguage::SimplifiedChinese, 0, duplicates) => {
                            format!("所选的 {duplicates} 本书已在书架中")
                        }
                        (AppLanguage::SimplifiedChinese, imported, 0) => {
                            format!("已导入 {imported} 本书")
                        }
                        (AppLanguage::SimplifiedChinese, imported, duplicates) => {
                            format!("已导入 {imported} 本书，跳过 {duplicates} 本重复书籍")
                        }
                        (AppLanguage::English, 0, duplicates) => {
                            format!("All {duplicates} selected books are already on the shelf")
                        }
                        (AppLanguage::English, imported, 0) => {
                            format!("Imported {imported} books")
                        }
                        (AppLanguage::English, imported, duplicates) => {
                            format!("Imported {imported} books and skipped {duplicates} duplicates")
                        }
                    },
                );
            }
            Err(error) => {
                self.shelf.notice = None;
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
                self.shelf.covers.remove(id);
                self.shelf.notice = Some(
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

    fn request_remove_book(&mut self, id: String, title: String) {
        self.shelf.remove_confirmation = Some(ShelfRemoveConfirmation { id, title });
    }

    fn cancel_remove_book(&mut self) {
        self.shelf.remove_confirmation = None;
    }

    fn confirm_remove_book(&mut self) {
        let Some(confirmation) = self.shelf.remove_confirmation.take() else {
            return;
        };
        self.remove_book(&confirmation.id);
    }

    fn open_settings(&mut self) {
        self.settings_tab = ShelfSettingsTab::General;
        self.open_settings_dialog();
    }

    fn open_settings_dialog(&mut self) {
        self.sync.draft_settings.clone_from(&self.sync.settings);
        self.sync.draft_password.clear();
        self.draft_language = self.language;
        self.settings_open = true;
    }

    fn close_settings(&mut self) {
        self.settings_open = false;
    }

    fn apply_settings(&mut self) {
        let language = self.draft_language;
        let mut settings = self.sync.draft_settings.clone();
        settings.normalize();
        if settings.enabled
            && let Err(error) = settings.validate()
        {
            self.shelf.error = Some(format!(
                "{}: {error}",
                language.text("云盘设置无效", "Invalid cloud settings")
            ));
            return;
        }
        if let Err(error) = preferences::save_app_language(language) {
            self.shelf.error = Some(format!(
                "{}: {error}",
                language.text("保存通用设置失败", "Failed to save general settings")
            ));
            return;
        }
        if let Err(error) = settings.save_default() {
            self.shelf.error = Some(format!(
                "{}: {error}",
                language.text("保存云盘设置失败", "Failed to save cloud settings")
            ));
            return;
        }
        if !self.sync.draft_password.is_empty() {
            if let Err(error) = settings.save_password(&self.sync.draft_password) {
                self.shelf.error = Some(format!(
                    "{}: {error}",
                    language.text(
                        "保存 Windows 凭据失败",
                        "Failed to save the Windows credential"
                    )
                ));
                return;
            }
            self.sync.password.clone_from(&self.sync.draft_password);
        }
        self.language = language;
        self.sync.settings = settings;
        self.settings_open = false;
        self.shelf.error = None;
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
        self.shelf.error = None;
    }

    fn complete_sync(&mut self, message: SyncTaskMessage) {
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
                            self.shelf.error = Some(format!(
                                "{}: {error}",
                                self.language
                                    .text("导入同步书籍失败", "Failed to import a synced book")
                            ));
                            return;
                        }
                    }
                }
                if imported > 0 {
                    self.shelf.refresh_covers();
                }
                self.sync.status = match self.language {
                    AppLanguage::SimplifiedChinese => format!(
                        "同步完成：上传 {} 本，下载 {} 本，更新 {} 条进度，合并 {} 条批注",
                        report.uploaded_books,
                        imported,
                        report.updated_progress,
                        report.merged_annotations
                    ),
                    AppLanguage::English => format!(
                        "Sync complete: {} uploaded, {} downloaded, {} progress updates, {} annotations merged",
                        report.uploaded_books,
                        imported,
                        report.updated_progress,
                        report.merged_annotations
                    ),
                };
                self.shelf.notice = Some(self.sync.status.clone());
                self.shelf.error = None;
            }
            Err(error) => {
                self.sync.status.clear();
                self.shelf.error = Some(format!(
                    "{}: {error}",
                    self.language.text("WebDAV 同步失败", "WebDAV sync failed")
                ));
            }
        }
    }
    pub(crate) fn take_opened_reader(&mut self) -> Option<DesktopReader> {
        self.pending_reader.take()
    }

    pub(crate) fn resume(&mut self) {
        match preferences::load_app_language() {
            Ok(language) => self.language = language,
            Err(error) => {
                self.shelf.error = Some(format!(
                    "{}: {error}",
                    self.language
                        .text("加载通用设置失败", "Failed to load general settings")
                ));
            }
        }
        match SyncSettings::load_default() {
            Ok(settings) => match settings.load_password() {
                Ok(password) => {
                    self.sync.settings = settings;
                    self.sync.password = password;
                }
                Err(error) => {
                    self.shelf.error = Some(format!(
                        "{}: {error}",
                        self.language.text(
                            "读取 Windows 凭据失败",
                            "Failed to read the Windows credential"
                        )
                    ));
                }
            },
            Err(error) => {
                self.shelf.error = Some(format!(
                    "{}: {error}",
                    self.language.text(
                        "加载 WebDAV 同步设置失败",
                        "Failed to load WebDAV sync settings"
                    )
                ));
            }
        }
        self.start_sync();
    }
}

impl ShelfState {
    fn new(library: LocalLibrary) -> Self {
        let mut state = Self {
            library,
            covers: HashMap::new(),
            grid_width: INITIAL_SHELF_GRID_WIDTH,
            query: String::new(),
            notice: None,
            error: None,
            remove_confirmation: None,
        };
        state.refresh_covers();
        state
    }

    fn refresh_covers(&mut self) {
        self.covers = self
            .library
            .books()
            .iter()
            .filter_map(|book| {
                book.cover_bytes
                    .as_deref()
                    .and_then(|bytes| decode_image(bytes).ok())
                    .map(|cover| (book.id.clone(), cover))
            })
            .collect();
    }
}

pub(crate) fn view(state: &mut ShelfFeature) -> Box<AnyWidgetView<ShelfFeature>> {
    shelf_app_view(state).boxed()
}

fn shelf_app_view(state: &mut ShelfFeature) -> impl WidgetView<ShelfFeature> + use<> {
    let pending = state.sync.task.pending.clone();
    let auto_sync = state.sync.settings.enabled;
    let interval = Duration::from_secs(u64::from(state.sync.settings.interval_minutes.max(1)) * 60);
    let view = fork(
        shelf_view(state),
        pending.map(|request| {
            task_raw(
                move |proxy| {
                    let request = request.clone();
                    async move {
                        let id = request.id;
                        let payload = request.payload;
                        let result = run_sync(payload.settings, payload.password, payload.books)
                            .await
                            .map_err(|error| error.to_string());
                        let _ = proxy.message(SyncTaskMessage { id, result });
                    }
                },
                ShelfFeature::complete_sync,
            )
        }),
    );
    fork(
        view,
        auto_sync.then(|| {
            task_raw(
                move |proxy| async move {
                    let mut timer = xilem::tokio::time::interval(interval);
                    timer.set_missed_tick_behavior(xilem::tokio::time::MissedTickBehavior::Skip);
                    loop {
                        timer.tick().await;
                        if proxy.message(()).is_err() {
                            break;
                        }
                    }
                },
                |state: &mut ShelfFeature, ()| state.start_sync(),
            )
        }),
    )
}

fn shelf_view(state: &mut ShelfFeature) -> impl WidgetView<ShelfFeature> + use<> {
    let query = state.shelf.query.trim().to_lowercase();
    let books = state
        .shelf
        .library
        .books()
        .iter()
        .filter(|book| book_matches_query(book, &query))
        .cloned()
        .collect::<Vec<_>>();
    let book_count = state.shelf.library.books().len();
    let feedback_layer = shelf_feedback_notice(state).alignment(UnitPoint::TOP_RIGHT);
    let content: Box<AnyWidgetView<ShelfFeature>> = if books.is_empty() && !query.is_empty() {
        sized_box(
            flex_col((
                FlexSpacer::Fixed(96.px()),
                icon_label(Icon::Search, 30.0, UI_MUTED),
                label(state.language.text("没有匹配的书籍", "No matching books"))
                    .text_size(14.0)
                    .color(UI_MUTED),
            ))
            .gap(14.px())
            .cross_axis_alignment(CrossAxisAlignment::Center),
        )
        .expand_width()
        .boxed()
    } else {
        shelf_grid(state, books, query.is_empty(), state.shelf.grid_width).boxed()
    };

    let shelf = sized_box(
        flex_col((
            shelf_toolbar(
                state.shelf.query.clone(),
                book_count,
                state.sync.settings.enabled,
                state.sync.task.is_pending(),
                state.language,
            ),
            divider(),
            portal(
                sized_box(zstack((
                    content.alignment(UnitPoint::TOP_LEFT),
                    shelf_width_probe(|state: &mut ShelfFeature, width| {
                        state.shelf.grid_width = width;
                    }),
                )))
                .expand_width()
                .padding(Padding::from_vh(24.0, 28.0)),
            )
            .flex(1.0),
        ))
        .cross_axis_alignment(CrossAxisAlignment::Fill),
    )
    .expand()
    .background_color(UI_BACKGROUND);
    let remove_dialog: Box<AnyWidgetView<ShelfFeature>> =
        state.shelf.remove_confirmation.clone().map_or_else(
            || sized_box(label("")).width(0.px()).height(0.px()).boxed(),
            |confirmation| {
                confirmation_dialog(
                    state.language.text("从书架移除", "Remove from shelf"),
                    match state.language {
                        AppLanguage::SimplifiedChinese => format!(
                            "确定要移除《{}》吗？本地书架中的副本将被删除。",
                            confirmation.title
                        ),
                        AppLanguage::English => format!(
                            "Remove “{}”? The local shelf copy will be deleted.",
                            confirmation.title
                        ),
                    },
                    state.language.text("取消", "Cancel"),
                    state.language.text("移除", "Remove"),
                    ShelfFeature::cancel_remove_book,
                    ShelfFeature::confirm_remove_book,
                )
                .boxed()
            },
        );
    let settings_dialog: Box<AnyWidgetView<ShelfFeature>> = if state.settings_open {
        shelf_settings_dialog(state).boxed()
    } else {
        sized_box(label("")).width(0.px()).height(0.px()).boxed()
    };
    sized_box(zstack((
        shelf,
        feedback_layer,
        remove_dialog,
        settings_dialog,
    )))
    .expand()
}

fn shelf_feedback_notice(state: &ShelfFeature) -> Box<AnyWidgetView<ShelfFeature>> {
    let content: Box<AnyWidgetView<ShelfFeature>> = if let Some(message) = &state.shelf.error {
        notice_card(
            NoticeTone::Error,
            state.language.text("操作失败", "Operation failed"),
            message.clone(),
        )
        .boxed()
    } else if let Some(message) = &state.shelf.notice {
        notice_card(
            NoticeTone::Success,
            state.language.text("操作完成", "Completed"),
            message.clone(),
        )
        .boxed()
    } else if state.sync.task.is_pending() {
        notice_card(
            NoticeTone::Info,
            state.language.text("WebDAV 同步", "WebDAV sync"),
            state.sync.status.clone(),
        )
        .boxed()
    } else {
        return sized_box(label("")).width(0.px()).height(0.px()).boxed();
    };

    sized_box(content)
        .width(380.px())
        .transform(Affine::translate((-16.0, 76.0)))
        .boxed()
}

fn shelf_toolbar(
    query: String,
    book_count: usize,
    sync_enabled: bool,
    syncing: bool,
    language: AppLanguage,
) -> impl WidgetView<ShelfFeature> {
    let search = sized_box(
        flex_row((
            icon_label(Icon::Search, 16.0, UI_MUTED),
            text_input(query, |state: &mut ShelfFeature, value| {
                state.shelf.query = value;
            })
            .placeholder(match language {
                AppLanguage::SimplifiedChinese => format!("搜索 {book_count} 本书"),
                AppLanguage::English => format!("Search {book_count} books"),
            })
            .text_color(UI_TEXT)
            .caret_color(UI_ACCENT)
            .background_color(Color::TRANSPARENT)
            .border_color(Color::TRANSPARENT)
            .border_width(0.0)
            .corner_radius(0.0)
            .padding(0.0)
            .flex(1.0),
        ))
        .gap(8.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(40.px())
    .background_color(UI_SURFACE)
    .border(UI_BORDER, 1.0)
    .corner_radius(9.0)
    .padding(Padding::horizontal(12.0))
    .flex(1.0);

    sized_box(
        flex_row((
            search,
            sized_box(label(""))
                .width(1.px())
                .height(24.px())
                .background_color(UI_BORDER),
            shelf_icon_button(Icon::Settings, ShelfFeature::open_settings),
            sync_enabled.then(|| {
                shelf_icon_button(
                    if syncing {
                        Icon::CloudDownload
                    } else {
                        Icon::CloudSync
                    },
                    ShelfFeature::start_sync,
                )
            }),
            shelf_icon_button(Icon::Plus, import_with_dialog),
        ))
        .gap(12.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(64.px())
    .expand_width()
    .background_color(UI_SURFACE)
    .padding(Padding::horizontal(20.0))
}

#[allow(clippy::too_many_lines)]
fn shelf_settings_dialog(state: &ShelfFeature) -> impl WidgetView<ShelfFeature> + use<> {
    let language = state.draft_language;
    let tab = state.settings_tab;
    let title = match tab {
        ShelfSettingsTab::General => language.text("通用", "General"),
        ShelfSettingsTab::Cloud => language.text("云盘", "Cloud drive"),
    };
    let body: Box<AnyWidgetView<ShelfFeature>> = match tab {
        ShelfSettingsTab::General => shelf_general_settings_content(language).boxed(),
        ShelfSettingsTab::Cloud => sync_settings_content(
            &state.sync.draft_settings,
            state.sync.draft_password.clone(),
            !state.sync.password.is_empty(),
            language,
            &SyncSettingsCallbacks {
                toggle_enabled: toggle_sync_enabled,
                set_base_url: set_sync_base_url,
                set_username: set_sync_username,
                set_password: set_sync_password,
                set_device_name: set_sync_device_name,
            },
        ),
    };

    let panel = sized_box(
        flex_col((
            sized_box(
                flex_row((
                    flex_row((
                        icon_label(Icon::Settings, 17.0, UI_MUTED),
                        label(language.text("设置", "Settings"))
                            .font(UI_FONT_STACK)
                            .text_size(15.0)
                            .weight(FontWeight::BOLD)
                            .color(UI_TEXT),
                    ))
                    .gap(9.px())
                    .cross_axis_alignment(CrossAxisAlignment::Center),
                    FlexSpacer::Flex(1.0),
                    label(title)
                        .font(UI_FONT_STACK)
                        .text_size(12.0)
                        .color(UI_MUTED),
                    shelf_icon_button(Icon::X, ShelfFeature::close_settings),
                ))
                .cross_axis_alignment(CrossAxisAlignment::Center),
            )
            .height(DIALOG_HEADER_HEIGHT.px())
            .expand_width()
            .padding(Padding::horizontal(CONTENT_PADDING_HORIZONTAL)),
            divider(),
            flex_row((shelf_settings_sidebar(language, tab), body.flex(1.0)))
                .gap(0.px())
                .flex(1.0),
            divider(),
            sized_box(
                flex_row((
                    FlexSpacer::Flex(1.0),
                    sized_box(
                        button(
                            label(language.text("取消", "Cancel"))
                                .font(UI_FONT_STACK)
                                .text_size(12.5)
                                .color(UI_TEXT_SOFT),
                            ShelfFeature::close_settings,
                        )
                        .background_color(UI_SURFACE)
                        .active_background_color(UI_SURFACE_MUTED)
                        .border_color(UI_BORDER)
                        .corner_radius(RADIUS_SMALL)
                        .padding(Padding::from_vh(5.0, 12.0)),
                    )
                    .height(CONTROL_HEIGHT.px()),
                    sized_box(
                        button(
                            label(language.text("应用", "Apply"))
                                .font(UI_FONT_STACK)
                                .text_size(12.5)
                                .weight(FontWeight::BOLD)
                                .color(UI_SURFACE),
                            ShelfFeature::apply_settings,
                        )
                        .background_color(UI_ACCENT)
                        .active_background_color(UI_TEXT)
                        .border_color(UI_ACCENT)
                        .corner_radius(RADIUS_SMALL)
                        .padding(Padding::from_vh(5.0, 12.0)),
                    )
                    .height(CONTROL_HEIGHT.px()),
                ))
                .gap(8.px())
                .cross_axis_alignment(CrossAxisAlignment::Center),
            )
            .height(DIALOG_FOOTER_HEIGHT.px())
            .expand_width()
            .padding(Padding::horizontal(CONTENT_PADDING_HORIZONTAL)),
        ))
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .must_fill_major_axis(true),
    )
    .width(660.px())
    .height(500.px())
    .background_color(UI_SURFACE)
    .border(UI_BORDER, 1.0)
    .corner_radius(RADIUS_DIALOG);

    sized_box(zstack((
        sized_box(
            button(label(""), ShelfFeature::close_settings)
                .background_color(Color::TRANSPARENT)
                .active_background_color(Color::TRANSPARENT)
                .border_color(Color::TRANSPARENT)
                .hovered_border_color(Color::TRANSPARENT)
                .border_width(0.0)
                .padding(0.0),
        )
        .expand()
        .background_color(Color::from_rgba8(31, 45, 61, 89)),
        panel,
    )))
    .expand()
}

fn shelf_settings_sidebar(
    language: AppLanguage,
    selected: ShelfSettingsTab,
) -> impl WidgetView<ShelfFeature> {
    sized_box(
        flex_col((
            shelf_settings_tab_button(
                language.text("通用", "General"),
                ShelfSettingsTab::General,
                selected,
            ),
            shelf_settings_tab_button(
                language.text("云盘", "Cloud drive"),
                ShelfSettingsTab::Cloud,
                selected,
            ),
            FlexSpacer::Flex(1.0),
        ))
        .gap(3.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(8.0),
    )
    .width(136.px())
    .expand_height()
    .background_color(UI_SURFACE_MUTED)
}

fn shelf_settings_tab_button(
    text: &'static str,
    value: ShelfSettingsTab,
    selected: ShelfSettingsTab,
) -> impl WidgetView<ShelfFeature> {
    let active = value == selected;
    sized_box(
        button(
            flex_row((
                label(text)
                    .font(UI_FONT_STACK)
                    .text_size(13.0)
                    .weight(if active {
                        FontWeight::BOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .color(if active { UI_TEXT } else { UI_TEXT_SOFT }),
                FlexSpacer::Flex(1.0),
            )),
            move |state: &mut ShelfFeature| state.settings_tab = value,
        )
        .background_color(if active {
            UI_SURFACE
        } else {
            Color::TRANSPARENT
        })
        .active_background_color(UI_ACCENT_SOFT)
        .border_color(if active {
            UI_BORDER
        } else {
            Color::TRANSPARENT
        })
        .hovered_border_color(UI_BORDER)
        .corner_radius(RADIUS_SMALL)
        .padding(Padding::horizontal(10.0)),
    )
    .height(CONTROL_HEIGHT.px())
    .expand_width()
}

fn shelf_general_settings_content(language: AppLanguage) -> impl WidgetView<ShelfFeature> {
    flex_col((
        label(language.text("语言与地区", "Language & region"))
            .font(UI_FONT_STACK)
            .text_size(12.0)
            .weight(FontWeight::BOLD)
            .color(UI_MUTED),
        sized_box(
            flex_row((
                label(language.text("界面语言", "Interface language"))
                    .font(UI_FONT_STACK)
                    .text_size(13.0)
                    .color(UI_TEXT_SOFT),
                FlexSpacer::Flex(1.0),
                shelf_language_choice("简体中文", AppLanguage::SimplifiedChinese, language),
                shelf_language_choice("English", AppLanguage::English, language),
            ))
            .gap(6.px())
            .cross_axis_alignment(CrossAxisAlignment::Center),
        )
        .height(52.px())
        .expand_width()
        .background_color(UI_SURFACE)
        .border(UI_BORDER, 1.0)
        .corner_radius(RADIUS_SMALL)
        .padding(Padding::horizontal(12.0)),
        prose(language.text(
            "界面语言也会作为翻译目标语言的默认值。",
            "The interface language is also the default translation target.",
        ))
        .text_size(10.5)
        .text_color(UI_MUTED),
    ))
    .gap(CONTENT_GAP.px())
    .cross_axis_alignment(CrossAxisAlignment::Fill)
    .padding(Padding::from_vh(
        CONTENT_PADDING_VERTICAL,
        CONTENT_PADDING_HORIZONTAL,
    ))
}

fn shelf_language_choice(
    text: &'static str,
    value: AppLanguage,
    selected: AppLanguage,
) -> impl WidgetView<ShelfFeature> {
    let active = value == selected;
    sized_box(
        button(
            label(text)
                .font(UI_FONT_STACK)
                .text_size(12.0)
                .weight(if active {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                })
                .color(if active { UI_ACCENT } else { UI_TEXT_SOFT }),
            move |state: &mut ShelfFeature| state.draft_language = value,
        )
        .background_color(if active { UI_ACCENT_SOFT } else { UI_SURFACE })
        .active_background_color(UI_ACCENT_SOFT)
        .border_color(if active { UI_ACCENT_BORDER } else { UI_BORDER })
        .hovered_border_color(UI_ACCENT_BORDER)
        .corner_radius(RADIUS_SMALL)
        .padding(Padding::from_vh(5.0, 9.0)),
    )
    .height(CONTROL_HEIGHT.px())
}

fn toggle_sync_enabled(state: &mut ShelfFeature) {
    state.sync.draft_settings.enabled = !state.sync.draft_settings.enabled;
}

fn set_sync_base_url(state: &mut ShelfFeature, value: String) {
    state.sync.draft_settings.base_url = value;
}

fn set_sync_username(state: &mut ShelfFeature, value: String) {
    state.sync.draft_settings.username = value;
}

fn set_sync_password(state: &mut ShelfFeature, value: String) {
    state.sync.draft_password = value;
}

fn set_sync_device_name(state: &mut ShelfFeature, value: String) {
    state.sync.draft_settings.device_name = value;
}

fn shelf_grid(
    state: &ShelfFeature,
    books: Vec<LibraryBook>,
    include_import: bool,
    available_width: f64,
) -> impl WidgetView<ShelfFeature> + use<> {
    let mut cards = books
        .into_iter()
        .map(|book| {
            let cover = state.shelf.covers.get(&book.id).cloned();
            shelf_book_card(&book, cover, state.language).boxed()
        })
        .collect::<Vec<Box<AnyWidgetView<ShelfFeature>>>>();
    if include_import {
        cards.push(import_card(state.language).boxed());
    }

    let columns = shelf_column_count(available_width);
    let mut rows = Vec::new();
    let mut cards = cards.into_iter();
    loop {
        let mut row = cards.by_ref().take(columns).collect::<Vec<_>>();
        if row.is_empty() {
            break;
        }
        while row.len() < columns {
            row.push(
                sized_box(label(""))
                    .width(SHELF_CARD_WIDTH.px())
                    .height(1.px())
                    .boxed(),
            );
        }
        rows.push(
            flex_row(row)
                .gap(SHELF_CARD_GAP.px())
                .cross_axis_alignment(CrossAxisAlignment::Start),
        );
    }

    flex_col(rows)
        .gap(SHELF_ROW_GAP.px())
        .cross_axis_alignment(CrossAxisAlignment::Start)
}

fn shelf_column_count(available_width: f64) -> usize {
    let mut columns = 1;
    let mut occupied_width = SHELF_CARD_WIDTH;
    while occupied_width + SHELF_CARD_GAP + SHELF_CARD_WIDTH <= available_width {
        columns += 1;
        occupied_width += SHELF_CARD_GAP + SHELF_CARD_WIDTH;
    }
    columns
}

fn shelf_book_card(
    book: &LibraryBook,
    cover: Option<ImageData>,
    language: AppLanguage,
) -> impl WidgetView<ShelfFeature> {
    let open_path = book.path.clone();
    let open_path_from_title = book.path.clone();
    let title = ellipsize_shelf_title(&book.title);
    let available = book.path.exists();
    let cover_button = sized_box(
        button(
            shelf_cover_content(book, cover, language),
            move |state: &mut ShelfFeature| {
                state.open_book(&open_path);
            },
        )
        .background_color(cover_color(&book.id))
        .active_background_color(UI_TEXT_SOFT)
        .border_color(UI_BORDER)
        .hovered_border_color(UI_ACCENT_BORDER)
        .border_width(1.0)
        .corner_radius(4.0)
        .padding(0.0),
    )
    .width(SHELF_CARD_WIDTH.px())
    .height(SHELF_COVER_HEIGHT.px());

    sized_box(
        flex_col((
            zstack((
                cover_button,
                shelf_remove_button(book.id.clone(), book.title.clone(), language)
                    .alignment(UnitPoint::TOP_RIGHT),
            )),
            sized_box(
                button(
                    label(title)
                        .text_size(13.5)
                        .weight(FontWeight::BOLD)
                        .line_break_mode(LineBreaking::Clip)
                        .color(UI_TEXT),
                    move |state: &mut ShelfFeature| state.open_book(&open_path_from_title),
                )
                .background_color(Color::TRANSPARENT)
                .active_background_color(Color::TRANSPARENT)
                .border_color(Color::TRANSPARENT)
                .hovered_border_color(Color::TRANSPARENT)
                .border_width(0.0)
                .padding(0.0),
            )
            .height(24.px())
            .expand_width(),
            shelf_book_status(available, language),
        ))
        .gap(7.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill),
    )
    .width(SHELF_CARD_WIDTH.px())
}

fn shelf_cover_content(
    book: &LibraryBook,
    cover: Option<ImageData>,
    language: AppLanguage,
) -> Box<AnyWidgetView<ShelfFeature>> {
    if let Some(cover) = cover {
        return image(cover).fit(ObjectFit::Contain).boxed();
    }
    let author = if book.authors.is_empty() {
        language.text("未知作者", "Unknown author").to_owned()
    } else {
        book.authors.join(" / ")
    };
    flex_col((
        icon_label(Icon::BookOpen, 20.0, Color::from_rgba8(255, 255, 255, 150)),
        FlexSpacer::Flex(1.0),
        prose(book.title.clone())
            .text_size(17.0)
            .weight(FontWeight::BOLD)
            .text_color(Color::WHITE),
        label(author)
            .text_size(11.0)
            .color(Color::from_rgba8(255, 255, 255, 180)),
    ))
    .gap(8.px())
    .cross_axis_alignment(CrossAxisAlignment::Start)
    .padding(18.0)
    .boxed()
}

fn shelf_remove_button(
    id: String,
    title: String,
    language: AppLanguage,
) -> impl WidgetView<ShelfFeature> {
    sized_box(
        button(
            icon_label(Icon::Trash2, 14.0, Color::WHITE),
            move |state: &mut ShelfFeature| {
                state.request_remove_book(id.clone(), title.clone());
            },
        )
        .accessibility_label(language.text("从书架移除", "Remove from shelf"))
        .background_color(Color::from_rgba8(31, 45, 61, 205))
        .active_background_color(Color::from_rgb8(0xb4, 0x23, 0x18))
        .border_color(Color::TRANSPARENT)
        .hovered_border_color(Color::from_rgb8(0xfe, 0xcd, 0xca))
        .corner_radius(15.0)
        .padding(0.0),
    )
    .width(30.px())
    .height(30.px())
    .transform(Affine::translate((-8.0, 8.0)))
}

fn shelf_book_status(available: bool, language: AppLanguage) -> impl WidgetView<ShelfFeature> {
    let (icon, text, color) = if available {
        (Icon::HardDrive, language.text("本地", "Local"), UI_MUTED)
    } else {
        (
            Icon::AlertTriangle,
            language.text("文件缺失", "Missing file"),
            Color::from_rgb8(0xb4, 0x23, 0x18),
        )
    };
    flex_row((
        icon_label(icon, 12.0, color),
        label(text).text_size(11.5).color(color),
    ))
    .gap(5.px())
    .cross_axis_alignment(CrossAxisAlignment::Center)
}

fn import_card(language: AppLanguage) -> impl WidgetView<ShelfFeature> {
    sized_box(
        flex_col((
            sized_box(
                button(icon_label(Icon::Plus, 46.0, UI_MUTED), import_with_dialog)
                    .background_color(UI_SURFACE_MUTED)
                    .active_background_color(UI_ACCENT_SOFT)
                    .border_color(UI_BORDER)
                    .hovered_border_color(UI_ACCENT_BORDER)
                    .corner_radius(4.0)
                    .padding(0.0),
            )
            .width(SHELF_CARD_WIDTH.px())
            .height(SHELF_COVER_HEIGHT.px()),
            label(language.text("导入本地书籍", "Import local books"))
                .text_size(13.5)
                .weight(FontWeight::BOLD)
                .color(UI_MUTED),
            label(language.text("保存在此设备", "Stored on this device"))
                .text_size(11.5)
                .color(UI_MUTED),
        ))
        .gap(7.px())
        .cross_axis_alignment(CrossAxisAlignment::Start),
    )
    .width(SHELF_CARD_WIDTH.px())
}

fn shelf_icon_button(
    icon: Icon,
    callback: impl Fn(&mut ShelfFeature) + Send + Sync + 'static,
) -> impl WidgetView<ShelfFeature> {
    sized_box(
        button(icon_label(icon, 17.0, UI_TEXT_SOFT), callback)
            .background_color(UI_SURFACE)
            .active_background_color(UI_SURFACE_MUTED)
            .border_color(Color::TRANSPARENT)
            .hovered_border_color(UI_BORDER)
            .corner_radius(8.0)
            .padding(0.0),
    )
    .width(36.px())
    .height(36.px())
}

fn import_with_dialog(state: &mut ShelfFeature) {
    let language = state.language;
    let Some(paths) = rfd::FileDialog::new()
        .add_filter(
            language.text(
                "电子书（EPUB / Kindle / FB2 / CBZ / PDF）",
                "E-books (EPUB / Kindle / FB2 / CBZ / PDF)",
            ),
            &[
                "epub", "mobi", "azw", "azw3", "fb2", "fbz", "cbz", "pdf", "zip",
            ],
        )
        .set_title(language.text("导入本地书籍", "Import local books"))
        .pick_files()
    else {
        return;
    };
    state.import_books(&paths);
}

fn book_matches_query(book: &LibraryBook, query: &str) -> bool {
    query.is_empty()
        || book.title.to_lowercase().contains(query)
        || book.file_name.to_lowercase().contains(query)
        || book
            .authors
            .iter()
            .any(|author| author.to_lowercase().contains(query))
}

fn ellipsize_shelf_title(title: &str) -> String {
    ellipsize_display_text(title, SHELF_TITLE_MAX_DISPLAY_UNITS)
}

fn cover_color(id: &str) -> Color {
    const PALETTES: [Color; 6] = [
        Color::from_rgb8(0x20, 0x63, 0x9b),
        Color::from_rgb8(0x9b, 0x4b, 0x5f),
        Color::from_rgb8(0x4f, 0x77, 0x5a),
        Color::from_rgb8(0x8a, 0x69, 0x43),
        Color::from_rgb8(0x75, 0x67, 0xa8),
        Color::from_rgb8(0x5c, 0x7c, 0x81),
    ];
    let index = id
        .bytes()
        .fold(0_usize, |sum, byte| sum + usize::from(byte))
        % PALETTES.len();
    PALETTES[index]
}

#[cfg(test)]
mod tests {
    use super::{
        LibraryBook, PathBuf, SHELF_CARD_GAP, SHELF_CARD_WIDTH, book_matches_query,
        ellipsize_shelf_title, shelf_column_count,
    };
    use crate::ui::{display_character_units, wrap_display_text};

    #[test]
    fn shelf_search_matches_title_author_and_source_file() {
        let book = LibraryBook {
            id: "book-id".into(),
            title: "系统之美".into(),
            authors: vec!["Donella Meadows".into()],
            file_name: "thinking-in-systems.epub".into(),
            path: PathBuf::from("book.epub"),
            cover_bytes: None,
            added_at: 0,
        };

        assert!(book_matches_query(&book, "系统"));
        assert!(book_matches_query(&book, "meadows"));
        assert!(book_matches_query(&book, "thinking-in"));
        assert!(!book_matches_query(&book, "不存在"));
    }

    #[test]
    fn shelf_titles_are_ellipsized_by_approximate_display_width() {
        assert_eq!(ellipsize_shelf_title("短书名"), "短书名");
        assert_eq!(
            ellipsize_shelf_title("这是一本书名非常非常长的电子书"),
            "这是一本书名非常…"
        );
        assert_eq!(
            ellipsize_shelf_title("A very long English book title"),
            "A very long Engl…"
        );
    }

    #[test]
    fn sidebar_titles_wrap_to_at_most_two_ellipsized_lines() {
        let short = wrap_display_text("Short title", 20, 2);
        assert_eq!(short, "Short title");

        let long = wrap_display_text(
            "A very long English book title that should not overflow the sidebar",
            20,
            2,
        );
        assert_eq!(long.lines().count(), 2);
        assert!(long.ends_with('…'));
        assert!(
            long.lines()
                .all(|line| { line.chars().map(display_character_units).sum::<usize>() <= 20 })
        );
    }

    #[test]
    fn shelf_columns_fill_the_available_width_before_wrapping() {
        assert_eq!(shelf_column_count(SHELF_CARD_WIDTH), 1);
        assert_eq!(
            shelf_column_count(SHELF_CARD_WIDTH * 4.0 + SHELF_CARD_GAP * 3.0),
            4
        );
        assert_eq!(
            shelf_column_count(SHELF_CARD_WIDTH * 6.0 + SHELF_CARD_GAP * 5.0),
            6
        );
    }
}
