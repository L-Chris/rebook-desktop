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
use crate::reader::{BookDisplayMetadata, DesktopReader, open_reader};
use crate::sync::{LocalSyncBook, SyncReport, SyncSettings, SyncStore, run_sync};
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
    dialog_open: bool,
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

#[derive(Clone, Debug)]
struct ShelfRemoveConfirmation {
    id: String,
    title: String,
}

impl ShelfFeature {
    pub(crate) fn new(library: LocalLibrary, reader_fonts: Arc<[Blob<u8>]>) -> Self {
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
        shelf.error = settings_error.or(password_error).or(store_error);
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
                dialog_open: false,
                task: TaskSlot::default(),
                status: String::new(),
            },
        }
    }

    pub(crate) fn open_book(&mut self, path: &Path) {
        let Some(local_store) = self.local_store.clone() else {
            self.shelf.error = Some("本地阅读数据库不可用，无法打开书籍".into());
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
            Err(error) => self.shelf.error = Some(format!("无法打开书籍：{error}")),
        }
    }

    fn import_books(&mut self, paths: &[PathBuf]) {
        self.shelf.error = None;
        match self.shelf.library.import_files(paths) {
            Ok(summary) => {
                self.shelf.refresh_covers();
                self.shelf.notice = Some(match (summary.imported, summary.duplicates) {
                    (0, duplicates) => format!("所选的 {duplicates} 本书已在书架中"),
                    (imported, 0) => format!("已导入 {imported} 本书"),
                    (imported, duplicates) => {
                        format!("已导入 {imported} 本书，跳过 {duplicates} 本重复书籍")
                    }
                });
            }
            Err(error) => {
                self.shelf.notice = None;
                self.shelf.error = Some(format!("导入失败：{error}"));
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
                self.shelf.notice = Some("已从本地书架移除".into());
                self.shelf.error = None;
            }
            Ok(false) => self.shelf.error = Some("书籍已不在本地书架中".into()),
            Err(error) => self.shelf.error = Some(format!("移除失败：{error}")),
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

    fn open_sync_settings(&mut self) {
        self.sync.draft_settings.clone_from(&self.sync.settings);
        self.sync.draft_password.clear();
        self.sync.dialog_open = true;
    }

    fn close_sync_settings(&mut self) {
        self.sync.dialog_open = false;
    }

    fn apply_sync_settings(&mut self) {
        let mut settings = self.sync.draft_settings.clone();
        settings.normalize();
        if settings.enabled
            && let Err(error) = settings.validate()
        {
            self.shelf.error = Some(format!("同步设置无效：{error}"));
            return;
        }
        if let Err(error) = settings.save_default() {
            self.shelf.error = Some(format!("保存同步设置失败：{error}"));
            return;
        }
        if !self.sync.draft_password.is_empty() {
            if let Err(error) = settings.save_password(&self.sync.draft_password) {
                self.shelf.error = Some(format!("保存 Windows 凭据失败：{error}"));
                return;
            }
            self.sync.password.clone_from(&self.sync.draft_password);
        }
        self.sync.settings = settings;
        self.sync.dialog_open = false;
        self.shelf.error = None;
        self.start_sync();
    }

    fn start_sync(&mut self) {
        if self.sync.task.is_pending() || !self.sync.settings.enabled {
            return;
        }
        if let Err(error) = self.sync.settings.validate() {
            self.shelf.error = Some(format!("无法开始同步：{error}"));
            return;
        }
        if self.sync.password.is_empty() {
            self.shelf.error = Some("无法开始同步：请先填写 WebDAV 密码".into());
            return;
        }
        self.sync.status = "正在同步书籍与阅读数据…".into();
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
                            self.shelf.error = Some(format!("导入同步书籍失败：{error}"));
                            return;
                        }
                    }
                }
                if imported > 0 {
                    self.shelf.refresh_covers();
                }
                self.sync.status = format!(
                    "同步完成：上传 {} 本，下载 {} 本，更新 {} 条进度，合并 {} 条批注",
                    report.uploaded_books,
                    imported,
                    report.updated_progress,
                    report.merged_annotations
                );
                self.shelf.notice = Some(self.sync.status.clone());
                self.shelf.error = None;
            }
            Err(error) => {
                self.sync.status.clear();
                self.shelf.error = Some(format!("WebDAV 同步失败：{error}"));
            }
        }
    }
    pub(crate) fn take_opened_reader(&mut self) -> Option<DesktopReader> {
        self.pending_reader.take()
    }

    pub(crate) fn resume(&mut self) {
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
                label("没有匹配的书籍").text_size(14.0).color(UI_MUTED),
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
                    "从书架移除",
                    format!(
                        "确定要移除《{}》吗？本地书架中的副本将被删除。",
                        confirmation.title
                    ),
                    "移除",
                    ShelfFeature::cancel_remove_book,
                    ShelfFeature::confirm_remove_book,
                )
                .boxed()
            },
        );
    let sync_dialog: Box<AnyWidgetView<ShelfFeature>> = if state.sync.dialog_open {
        shelf_sync_dialog(state).boxed()
    } else {
        sized_box(label("")).width(0.px()).height(0.px()).boxed()
    };
    sized_box(zstack((shelf, feedback_layer, remove_dialog, sync_dialog))).expand()
}

fn shelf_feedback_notice(state: &ShelfFeature) -> Box<AnyWidgetView<ShelfFeature>> {
    let content: Box<AnyWidgetView<ShelfFeature>> = if let Some(message) = &state.shelf.error {
        notice_card(NoticeTone::Error, "操作失败", message.clone()).boxed()
    } else if let Some(message) = &state.shelf.notice {
        notice_card(NoticeTone::Success, "操作完成", message.clone()).boxed()
    } else if state.sync.task.is_pending() {
        notice_card(NoticeTone::Info, "WebDAV 同步", state.sync.status.clone()).boxed()
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
) -> impl WidgetView<ShelfFeature> {
    let search = sized_box(
        flex_row((
            icon_label(Icon::Search, 16.0, UI_MUTED),
            text_input(query, |state: &mut ShelfFeature, value| {
                state.shelf.query = value;
            })
            .placeholder(format!("搜索 {book_count} 本书"))
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
            shelf_icon_button(Icon::CloudCog, ShelfFeature::open_sync_settings),
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
fn shelf_sync_dialog(state: &ShelfFeature) -> impl WidgetView<ShelfFeature> + use<> {
    let enabled = state.sync.draft_settings.enabled;
    let base_url = state.sync.draft_settings.base_url.clone();
    let username = state.sync.draft_settings.username.clone();
    let password = state.sync.draft_password.clone();
    let device_name = state.sync.draft_settings.device_name.clone();
    let toggle = sized_box(
        button(
            flex_row((
                label(if enabled { "已启用" } else { "未启用" })
                    .font(UI_FONT_STACK)
                    .text_size(12.5)
                    .color(if enabled { UI_ACCENT } else { UI_TEXT_SOFT }),
                FlexSpacer::Flex(1.0),
                sized_box(zstack((
                    sized_box(label(""))
                        .width(32.px())
                        .height(18.px())
                        .background_color(if enabled { UI_ACCENT } else { UI_BORDER })
                        .corner_radius(9.0),
                    sized_box(label(""))
                        .width(14.px())
                        .height(14.px())
                        .background_color(UI_SURFACE)
                        .corner_radius(7.0)
                        .alignment(if enabled {
                            UnitPoint::RIGHT
                        } else {
                            UnitPoint::LEFT
                        }),
                )))
                .width(32.px())
                .height(18.px()),
            )),
            |state: &mut ShelfFeature| {
                state.sync.draft_settings.enabled = !state.sync.draft_settings.enabled;
            },
        )
        .background_color(UI_SURFACE)
        .active_background_color(UI_SURFACE_MUTED)
        .border_color(UI_BORDER)
        .hovered_border_color(UI_ACCENT_BORDER)
        .corner_radius(RADIUS_SMALL)
        .padding(Padding::from_vh(6.0, 10.0)),
    )
    .height(CONTROL_HEIGHT.px())
    .expand_width();

    let panel = sized_box(
        flex_col((
            sized_box(
                flex_row((
                    flex_row((
                        icon_label(Icon::CloudCog, 17.0, UI_ACCENT),
                        label("WebDAV 同步")
                            .font(UI_FONT_STACK)
                            .text_size(15.0)
                            .weight(FontWeight::BOLD)
                            .color(UI_TEXT),
                    ))
                    .gap(9.px())
                    .cross_axis_alignment(CrossAxisAlignment::Center),
                    FlexSpacer::Flex(1.0),
                    shelf_icon_button(Icon::X, ShelfFeature::close_sync_settings),
                ))
                .cross_axis_alignment(CrossAxisAlignment::Center),
            )
            .height(DIALOG_HEADER_HEIGHT.px())
            .expand_width()
            .padding(Padding::horizontal(CONTENT_PADDING_HORIZONTAL)),
            divider(),
            flex_col((
                label("桌面端会直接连接 WebDAV；密码只保存到 Windows 凭据管理器。")
                    .font(UI_FONT_STACK)
                    .text_size(11.5)
                    .color(UI_MUTED),
                sync_settings_row("自动同步", toggle.boxed()),
                sync_text_input_row(
                    "WebDAV 地址",
                    base_url,
                    "https://dav.example.com/path",
                    |state, value| {
                        state.sync.draft_settings.base_url = value;
                    },
                ),
                sync_text_input_row("用户名", username, "WebDAV 用户名", |state, value| {
                    state.sync.draft_settings.username = value;
                }),
                sync_text_input_row(
                    "密码",
                    password,
                    if state.sync.password.is_empty() {
                        "应用专用密码"
                    } else {
                        "已保存；留空不修改"
                    },
                    |state, value| {
                        state.sync.draft_password = value;
                    },
                ),
                sync_text_input_row(
                    "设备名称",
                    device_name,
                    "这台电脑",
                    |state, value| {
                        state.sync.draft_settings.device_name = value;
                    },
                ),
            ))
            .gap(CONTENT_GAP.px())
            .cross_axis_alignment(CrossAxisAlignment::Fill)
            .padding(Padding::from_vh(
                CONTENT_PADDING_VERTICAL,
                CONTENT_PADDING_HORIZONTAL,
            ))
            .flex(1.0),
            divider(),
            sized_box(
                flex_row((
                    FlexSpacer::Flex(1.0),
                    sized_box(
                        button(
                            label("取消")
                                .font(UI_FONT_STACK)
                                .text_size(12.5)
                                .color(UI_TEXT_SOFT),
                            ShelfFeature::close_sync_settings,
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
                            label("保存并同步")
                                .font(UI_FONT_STACK)
                                .text_size(12.5)
                                .weight(FontWeight::BOLD)
                                .color(UI_SURFACE),
                            ShelfFeature::apply_sync_settings,
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
    .width(560.px())
    .height(390.px())
    .background_color(UI_SURFACE)
    .border(UI_BORDER, 1.0)
    .corner_radius(RADIUS_DIALOG);

    sized_box(zstack((
        sized_box(
            button(label(""), ShelfFeature::close_sync_settings)
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

fn sync_settings_row(
    label_text: &'static str,
    control: Box<AnyWidgetView<ShelfFeature>>,
) -> impl WidgetView<ShelfFeature> {
    flex_row((
        sized_box(
            label(label_text)
                .font(UI_FONT_STACK)
                .text_size(12.5)
                .color(UI_TEXT_SOFT),
        )
        .width(116.px()),
        control.flex(1.0),
    ))
    .gap(12.px())
    .cross_axis_alignment(CrossAxisAlignment::Center)
}

fn sync_text_input_row(
    label_text: &'static str,
    value: String,
    placeholder: &'static str,
    callback: impl Fn(&mut ShelfFeature, String) + Send + Sync + 'static,
) -> impl WidgetView<ShelfFeature> {
    let input = sized_box(
        text_input(value, callback)
            .placeholder(placeholder)
            .text_color(UI_TEXT)
            .caret_color(UI_ACCENT)
            .background_color(UI_SURFACE_MUTED)
            .border_color(UI_BORDER)
            .border_width(1.0)
            .corner_radius(RADIUS_SMALL)
            .padding(Padding::from_vh(5.0, 10.0)),
    )
    .height(CONTROL_HEIGHT.px())
    .expand_width();
    sync_settings_row(label_text, input.boxed())
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
            shelf_book_card(&book, cover).boxed()
        })
        .collect::<Vec<Box<AnyWidgetView<ShelfFeature>>>>();
    if include_import {
        cards.push(import_card().boxed());
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

fn shelf_book_card(book: &LibraryBook, cover: Option<ImageData>) -> impl WidgetView<ShelfFeature> {
    let open_path = book.path.clone();
    let open_path_from_title = book.path.clone();
    let title = ellipsize_shelf_title(&book.title);
    let available = book.path.exists();
    let cover_button = sized_box(
        button(
            shelf_cover_content(book, cover),
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
                shelf_remove_button(book.id.clone(), book.title.clone())
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
            shelf_book_status(available),
        ))
        .gap(7.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill),
    )
    .width(SHELF_CARD_WIDTH.px())
}

fn shelf_cover_content(
    book: &LibraryBook,
    cover: Option<ImageData>,
) -> Box<AnyWidgetView<ShelfFeature>> {
    if let Some(cover) = cover {
        return image(cover).fit(ObjectFit::Contain).boxed();
    }
    let author = if book.authors.is_empty() {
        "未知作者".to_owned()
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

fn shelf_remove_button(id: String, title: String) -> impl WidgetView<ShelfFeature> {
    sized_box(
        button(
            icon_label(Icon::Trash2, 14.0, Color::WHITE),
            move |state: &mut ShelfFeature| {
                state.request_remove_book(id.clone(), title.clone());
            },
        )
        .accessibility_label("从书架移除")
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

fn shelf_book_status(available: bool) -> impl WidgetView<ShelfFeature> {
    let (icon, text, color) = if available {
        (Icon::HardDrive, "本地", UI_MUTED)
    } else {
        (
            Icon::AlertTriangle,
            "文件缺失",
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

fn import_card() -> impl WidgetView<ShelfFeature> {
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
            label("导入本地书籍")
                .text_size(13.5)
                .weight(FontWeight::BOLD)
                .color(UI_MUTED),
            label("保存在此设备").text_size(11.5).color(UI_MUTED),
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
    let Some(paths) = rfd::FileDialog::new()
        .add_filter(
            "电子书（EPUB / Kindle / FB2 / CBZ / PDF）",
            &[
                "epub", "mobi", "azw", "azw3", "fb2", "fbz", "cbz", "pdf", "zip",
            ],
        )
        .set_title("导入本地书籍")
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
