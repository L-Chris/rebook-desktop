//! Native e-book reader: parser -> reading IR -> page layout -> display list -> Xilem/Vello.

mod async_task;
mod design;
mod dialog;
mod feedback;
mod fonts;
mod highlights;
mod library;
mod persistence;
mod plugins;
mod pointer_button;
mod preferences;
mod reader_canvas;
mod shelf_width_probe;
mod sync;
mod ui;
mod vello_bridge;

use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_task::{TaskResult, TaskSlot};
use design::{
    CONTENT_GAP, CONTENT_PADDING_HORIZONTAL, CONTENT_PADDING_VERTICAL, CONTROL_HEIGHT,
    CONTROL_HEIGHT_COMPACT, DIALOG_FOOTER_HEIGHT, DIALOG_HEADER_HEIGHT, RADIUS_DIALOG,
    RADIUS_LARGE, RADIUS_MEDIUM, RADIUS_SMALL, SETTINGS_ROW_HEIGHT,
};
use dialog::confirmation_dialog;
use feedback::{NoticeTone, dismissible_notice, notice_card};
use highlights::{HighlightStore, StoredHighlight};
use library::{LibraryBook, LocalLibrary};
use lucide_icons::{Icon, LUCIDE_FONT_BYTES};
use plugins::{
    AiProvider, BUILTIN_PLUGINS, BlockTranslation, BookSearchResult, ChatCommand,
    ChatCommandResolution, ChatResponse, ChatRole, ChatTurn, PluginSettings, RewriteBookSource,
    TranslationBlockInput, TranslationBookSource, TranslationMode, chat_command_suggestions,
    chat_with_book, resolve_chat_command, search_book, translate_blocks,
};
use pointer_button::button;
use reader_canvas::{ReaderCanvasAction, reader_canvas};
use rebook_formats::{BookFormat, open_file as open_publication_file};
use rebook_layout::{LayoutViewport, ReaderDefaultFont, ReaderStyle, ReaderTypography, SpreadMode};
use rebook_publication::{BookSource, PublicationUrl, Rgba, SourceRange};
use rebook_reader::{
    NavigationAttempt, NavigationOutcome, PageDirection, ReaderSelection, ReaderSession,
    ReaderSnapshot, ReaderTextHit, TocViewItem,
};
use rebook_renderer::PageDisplayList;
use shelf_width_probe::shelf_width_probe;
use sync::{LocalSyncBook, SyncReport, SyncSettings, SyncStore, run_sync};
use ui::{divider, icon_label};
use vello_bridge::XilemVelloScene;
use xilem::core::{fork, map_state};
use xilem::masonry::kurbo::Size;
use xilem::masonry::parley::style::FontStack;
use xilem::masonry::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
use xilem::masonry::properties::LineBreaking;
use xilem::masonry::properties::types::{AsUnit, UnitPoint};
use xilem::masonry::vello::Scene;
use xilem::style::{Padding, Style};
use xilem::view::{
    CrossAxisAlignment, FlexExt, FlexSpacer, MainAxisAlignment, ObjectFit, ZStackExt, flex_col,
    flex_row, image, label, portal, prose, sized_box, task, task_raw, text_input, zstack,
};
use xilem::{
    Affine, AnyWidgetView, Color, EventLoop, FontWeight, WidgetView, WindowOptions, Xilem,
};

const INITIAL_WIDTH: u32 = 1200;
const INITIAL_HEIGHT: u32 = 800;
const TOOLBAR_HEIGHT: f64 = 44.0;
const PROGRESS_HEIGHT: f64 = 4.0;
const TOC_WIDTH: f64 = 240.0;
const ASSISTANT_PANEL_WIDTH: f64 = 340.0;
const SETTINGS_WIDTH: f64 = 660.0;
const SETTINGS_HEIGHT: f64 = 500.0;
const SHELF_CARD_WIDTH: f64 = 144.0;
const SHELF_COVER_HEIGHT: f64 = 216.0;
const SHELF_CARD_GAP: f64 = 24.0;
const SHELF_ROW_GAP: f64 = 28.0;
const SHELF_TITLE_MAX_DISPLAY_UNITS: usize = 18;
const SIDEBAR_TITLE_LINE_DISPLAY_UNITS: usize = 20;
const SIDEBAR_TITLE_MAX_LINES: usize = 2;
const SIDEBAR_AUTHOR_MAX_DISPLAY_UNITS: usize = 22;
const PAGE_SCENE_CACHE_CAPACITY: usize = 32;
const PDF_PAGE_SCENE_CACHE_CAPACITY: usize = 4;
const MOTION_DURATION: Duration = Duration::from_millis(180);
const TOOLBAR_MOTION_DURATION: Duration = Duration::from_millis(200);
const SETTINGS_MOTION_DURATION: Duration = Duration::from_millis(200);
const TOOLBAR_HIDE_DELAY: Duration = Duration::from_millis(500);
const NOTICE_AUTO_DISMISS_DELAY: Duration = Duration::from_secs(3);
const MOTION_FRAME_INTERVAL: Duration = Duration::from_millis(16);
const SIDEBAR_SCRIM_ALPHA: f32 = 0.28;
const MODAL_SCRIM_ALPHA: f32 = 0.35;
const MOTION_EPSILON: f32 = 0.001;
const SELECTION_TOOLBAR_WIDTH: f64 = 90.0;
const SELECTION_TOOLBAR_HEIGHT: f64 = 46.0;
const SELECTION_TOOLBAR_GAP: f64 = 10.0;

// Keep these in sync with rebook-web's light reader tokens.
const UI_BACKGROUND: Color = Color::from_rgb8(0xff, 0xff, 0xff);
const UI_SURFACE: Color = Color::from_rgb8(0xff, 0xff, 0xff);
const UI_SIDEBAR: Color = Color::from_rgb8(0xfb, 0xfc, 0xfd);
const UI_SURFACE_MUTED: Color = Color::from_rgb8(0xf2, 0xf5, 0xf8);
const UI_TEXT: Color = Color::from_rgb8(0x1f, 0x2d, 0x3d);
const UI_TEXT_SOFT: Color = Color::from_rgb8(0x43, 0x55, 0x6b);
const UI_MUTED: Color = Color::from_rgb8(0x70, 0x82, 0x98);
const UI_BORDER: Color = Color::from_rgb8(0xdd, 0xe5, 0xee);
const UI_ACCENT: Color = Color::from_rgb8(0x0f, 0x76, 0x6e);
const UI_ACCENT_SOFT: Color = Color::from_rgb8(0xe2, 0xf3, 0xf1);
const UI_ACCENT_BORDER: Color = Color::from_rgb8(0xba, 0xe6, 0xe1);
const ANNOTATION_MARK_COLOR: Color = Color::from_rgba8(96, 165, 250, 72);
const ANNOTATION_SWATCH_COLOR: Color = Color::from_rgb8(96, 165, 250);
const SEARCH_MARK_COLOR: Color = Color::from_rgba8(250, 204, 21, 89);
const ASSISTANT_MARK_COLOR: Color = Color::from_rgba8(245, 158, 11, 56);
const TEXT_SELECTION_COLOR: Color = Color::from_rgba8(96, 165, 250, 89);
const UI_FONT_STACK: &str = "'Microsoft YaHei UI', 'Microsoft YaHei', 'PingFang SC', 'Noto Sans CJK SC', 'Segoe UI Symbol', sans-serif";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("rebook-desktop failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let launch = parse_arguments()?;
    let reader_fonts = fonts::embedded_reader_fonts();

    let library =
        LocalLibrary::load_default().map_err(|error| io::Error::other(error.to_string()))?;
    let mut state = DesktopApp::new(library, Arc::clone(&reader_fonts));
    if let LaunchMode::Open(path) = launch {
        state.open_book(&path);
    }
    let window = WindowOptions::new("Rebook")
        .with_initial_inner_size(xilem::winit::dpi::LogicalSize::new(
            INITIAL_WIDTH,
            INITIAL_HEIGHT,
        ))
        .with_min_inner_size(xilem::winit::dpi::LogicalSize::new(720_u32, 520_u32));
    let mut application =
        Xilem::new_simple(state, root_view, window).with_font(LUCIDE_FONT_BYTES.to_vec());
    for font in reader_fonts.iter() {
        application = application.with_font(font.clone());
    }
    application.run_in(EventLoop::with_user_event())?;
    Ok(())
}

fn open_reader(
    path: &Path,
    reader_fonts: Arc<[Blob<u8>]>,
    shelf_metadata: Option<BookDisplayMetadata>,
    local_store: SyncStore,
) -> Result<DesktopReader, Box<dyn std::error::Error + Send + Sync>> {
    let started = Instant::now();
    let publication = open_publication_file(path)?;
    let format = publication.format();
    let cover = publication
        .cover_bytes()
        .and_then(|bytes| decode_cover(bytes).ok());
    let canonical_source = publication.source();
    let book_id = canonical_source.book().id.to_string();
    let display_metadata = resolve_book_display_metadata(
        shelf_metadata,
        &canonical_source.book().metadata.title,
        &canonical_source.book().metadata.authors,
    );
    let rewrite_source = Arc::new(RewriteBookSource::new(canonical_source));
    let plugin_settings = PluginSettings::load_default().unwrap_or_else(|error| {
        tracing::warn!(%error, "failed to load plugin settings; using defaults");
        PluginSettings::default()
    });
    let translation_source = Arc::new(TranslationBookSource::new(
        rewrite_source.clone(),
        plugin_settings.translation_mode,
    ));
    let source: Arc<dyn BookSource> = translation_source.clone();
    let highlight_store = HighlightStore::from_store(local_store.clone())?;
    let highlights = highlight_store.for_book(&book_id);
    let viewport = LayoutViewport::new(INITIAL_WIDTH, INITIAL_HEIGHT)?;
    let typography = preferences::load_reader_typography().unwrap_or_else(|error| {
        tracing::warn!(%error, "failed to load reader typography; using defaults");
        ReaderTypography::default()
    });
    let style = ReaderStyle {
        typography,
        ..ReaderStyle::default()
    };
    let mut reader =
        ReaderSession::open_with_fonts(Arc::clone(&source), viewport, style, reader_fonts)?;
    let progress_store = Some(local_store);
    if let Some(store) = &progress_store
        && let Some(progress) = store.load_progress(&book_id)?
        && let Err(error) = reader.restore_locator(&progress.locator)
    {
        tracing::warn!(%error, "failed to restore durable reading locator");
    }
    let available_font_families = reader.available_font_families().into();
    tracing::debug!(
        elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
        "opened book"
    );
    Ok(DesktopReader::new(
        reader,
        DesktopReaderResources {
            source,
            rewrite_source,
            translation_source,
            cover,
            format,
            book_id,
            display_metadata,
            highlight_store,
            highlights,
            progress_store,
            plugin_settings,
            available_font_families,
        },
    ))
}

enum LaunchMode {
    Shelf,
    Open(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BookDisplayMetadata {
    title: String,
    authors: Vec<String>,
}

impl From<&LibraryBook> for BookDisplayMetadata {
    fn from(book: &LibraryBook) -> Self {
        Self {
            title: book.title.clone(),
            authors: book.authors.clone(),
        }
    }
}

fn resolve_book_display_metadata(
    shelf_metadata: Option<BookDisplayMetadata>,
    parsed_title: &str,
    parsed_authors: &[String],
) -> BookDisplayMetadata {
    shelf_metadata.unwrap_or_else(|| BookDisplayMetadata {
        title: parsed_title.to_owned(),
        authors: parsed_authors.to_vec(),
    })
}

fn parse_arguments() -> Result<LaunchMode, Box<dyn std::error::Error>> {
    let mut arguments = env::args_os();
    let executable = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .unwrap_or_else(|| "rebook-desktop".into());
    let Some(first) = arguments.next() else {
        return Ok(LaunchMode::Shelf);
    };
    let launch = LaunchMode::Open(PathBuf::from(first));
    if arguments.next().is_some() {
        return Err(usage(&executable).into());
    }
    Ok(launch)
}

struct DesktopApp {
    shelf: ShelfState,
    reader: Option<DesktopReader>,
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

impl DesktopApp {
    fn new(library: LocalLibrary, reader_fonts: Arc<[Blob<u8>]>) -> Self {
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
            reader: None,
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

    fn open_book(&mut self, path: &Path) {
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
                self.reader = Some(reader);
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
}

impl ShelfState {
    fn new(library: LocalLibrary) -> Self {
        let mut state = Self {
            library,
            covers: HashMap::new(),
            grid_width: f64::from(INITIAL_WIDTH) - 56.0,
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
                    .and_then(|bytes| decode_cover(bytes).ok())
                    .map(|cover| (book.id.clone(), cover))
            })
            .collect();
    }
}

struct DesktopReader {
    reader: ReaderSession,
    source: Arc<dyn BookSource>,
    rewrite_source: Arc<RewriteBookSource>,
    translation_source: Arc<TranslationBookSource>,
    snapshot: ReaderSnapshot,
    cover: Option<ImageData>,
    format: BookFormat,
    book_id: String,
    display_metadata: BookDisplayMetadata,
    highlight_store: HighlightStore,
    highlights: Vec<StoredHighlight>,
    progress_store: Option<SyncStore>,
    selection_anchor: Option<ReaderTextHit>,
    selection: Option<ReaderSelection>,
    selection_toolbar_visible: bool,
    selected_highlight_id: Option<String>,
    focused_mark: Option<FocusedMark>,
    plugin_settings: PluginSettings,
    available_font_families: Arc<[String]>,
    search: SearchUiState,
    chat: ChatUiState,
    translation: TranslationUiState,
    ui: ReaderUiState,
    canvas_size: Option<(u32, u32)>,
    scene_revision: u64,
    page_scenes: HashMap<PageSceneKey, Arc<PageSceneLayers>>,
    page_scene_lru: VecDeque<PageSceneKey>,
    pending_page_turn: Option<PageDirection>,
    error: Option<String>,
    exit_requested: bool,
}

struct DesktopReaderResources {
    source: Arc<dyn BookSource>,
    rewrite_source: Arc<RewriteBookSource>,
    translation_source: Arc<TranslationBookSource>,
    cover: Option<ImageData>,
    format: BookFormat,
    book_id: String,
    display_metadata: BookDisplayMetadata,
    highlight_store: HighlightStore,
    highlights: Vec<StoredHighlight>,
    progress_store: Option<SyncStore>,
    plugin_settings: PluginSettings,
    available_font_families: Arc<[String]>,
}

#[derive(Clone)]
struct SearchTask {
    source: Arc<dyn BookSource>,
    query: String,
}

type SearchTaskMessage = TaskResult<Vec<BookSearchResult>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FocusedMarkKind {
    Search,
    Assistant,
}

#[derive(Clone, Debug)]
struct FocusedMark {
    ranges: Vec<SourceRange>,
    kind: FocusedMarkKind,
}

impl FocusedMark {
    fn search(range: SourceRange) -> Self {
        Self {
            ranges: vec![range],
            kind: FocusedMarkKind::Search,
        }
    }

    fn assistant(ranges: Vec<SourceRange>) -> Self {
        Self {
            ranges,
            kind: FocusedMarkKind::Assistant,
        }
    }

    fn color(&self) -> Color {
        match self.kind {
            FocusedMarkKind::Search => SEARCH_MARK_COLOR,
            FocusedMarkKind::Assistant => ASSISTANT_MARK_COLOR,
        }
    }

    fn search_range(&self) -> Option<&SourceRange> {
        (self.kind == FocusedMarkKind::Search)
            .then(|| self.ranges.first())
            .flatten()
    }
}

#[derive(Default)]
struct SearchUiState {
    query: String,
    results: Vec<BookSearchResult>,
    status: String,
    task: TaskSlot<SearchTask>,
}

#[derive(Clone)]
struct ChatTask {
    source: Arc<dyn BookSource>,
    settings: PluginSettings,
    history: Vec<ChatTurn>,
    question: String,
    current_section: usize,
}

type ChatTaskMessage = TaskResult<ChatResponse>;

#[derive(Default)]
struct ChatUiState {
    input: String,
    messages: Vec<ChatTurn>,
    error: Option<String>,
    task: TaskSlot<ChatTask>,
}

#[derive(Clone)]
struct TranslationTask {
    section_index: usize,
    settings: PluginSettings,
    blocks: Vec<TranslationBlockInput>,
}

type TranslationTaskMessage = TaskResult<Vec<BlockTranslation>>;

#[derive(Default)]
struct TranslationUiState {
    enabled: bool,
    error: Option<String>,
    dismiss_at: Option<Instant>,
    task: TaskSlot<TranslationTask>,
}

impl TranslationUiState {
    fn show_error(&mut self, error: String, now: Instant) {
        self.error = Some(error);
        self.dismiss_at = Some(now + NOTICE_AUTO_DISMISS_DELAY);
    }

    fn clear_error(&mut self) {
        self.error = None;
        self.dismiss_at = None;
    }

    fn dismiss_if_due(&mut self, now: Instant) -> bool {
        if self.dismiss_at.is_none_or(|deadline| now < deadline) {
            return false;
        }
        self.clear_error();
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct PageSceneKey {
    section: usize,
    segment: usize,
    page: usize,
}

struct PageSceneLayers {
    underlay: Arc<Scene>,
    content: Arc<Scene>,
}

#[derive(Clone, Copy)]
enum SceneChange {
    Overlays,
    StaticContent,
}

#[derive(Clone, Copy)]
enum MarkRetention {
    Keep,
    ClearSelectedHighlight,
    ClearAll,
}

#[derive(Clone, Copy)]
enum FollowUp {
    None,
    Run,
}

#[derive(Clone, Copy)]
enum ProgressChange {
    Keep,
    Persist,
}

#[derive(Clone, Copy)]
struct SnapshotEffects {
    scene: SceneChange,
    marks: MarkRetention,
    prefetch: FollowUp,
    translation: FollowUp,
    progress: ProgressChange,
}

impl SnapshotEffects {
    const fn navigation() -> Self {
        Self {
            scene: SceneChange::Overlays,
            marks: MarkRetention::ClearSelectedHighlight,
            prefetch: FollowUp::Run,
            translation: FollowUp::Run,
            progress: ProgressChange::Persist,
        }
    }

    const fn static_content_change() -> Self {
        Self {
            scene: SceneChange::StaticContent,
            marks: MarkRetention::Keep,
            prefetch: FollowUp::Run,
            translation: FollowUp::None,
            progress: ProgressChange::Keep,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReaderOverlay {
    None,
    Menu,
    Settings,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SettingsTab {
    #[default]
    Reading,
    Font,
    Ai,
    AiChat,
    Translation,
    Plugins,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FontPickerKind {
    Cjk,
    Serif,
    SansSerif,
    Monospace,
}

impl FontPickerKind {
    const fn title(self) -> &'static str {
        match self {
            Self::Cjk => "中文字体",
            Self::Serif => "衬线字体",
            Self::SansSerif => "无衬线字体",
            Self::Monospace => "等宽字体",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SidebarTab {
    #[default]
    Toc,
    Highlights,
    Search,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AssistantPanel {
    Chat,
}

#[derive(Clone, Copy, Debug)]
enum MotionCurve {
    EaseOut,
    EnterExit,
}

#[derive(Clone, Copy, Debug)]
struct Motion {
    value: f32,
    start: f32,
    target: f32,
    elapsed: Duration,
    duration: Duration,
    curve: MotionCurve,
}

impl Motion {
    const fn settled(value: f32) -> Self {
        Self::settled_with_duration(value, MOTION_DURATION)
    }

    const fn settled_with_duration(value: f32, duration: Duration) -> Self {
        Self::settled_with_curve(value, duration, MotionCurve::EaseOut)
    }

    const fn settled_with_curve(value: f32, duration: Duration, curve: MotionCurve) -> Self {
        Self {
            value,
            start: value,
            target: value,
            elapsed: Duration::ZERO,
            duration,
            curve,
        }
    }

    fn animate_to(&mut self, target: f32) -> bool {
        if (self.target - target).abs() <= MOTION_EPSILON {
            return false;
        }
        self.start = self.value;
        self.target = target;
        self.elapsed = Duration::ZERO;
        true
    }

    fn advance(&mut self, delta: Duration) {
        if !self.is_animating() {
            return;
        }
        self.elapsed = self.elapsed.saturating_add(delta);
        let progress = if self.duration.is_zero() {
            1.0
        } else {
            (self.elapsed.as_secs_f32() / self.duration.as_secs_f32()).min(1.0)
        };
        let eased = match self.curve {
            MotionCurve::EnterExit if self.target < self.start => progress.powi(2),
            MotionCurve::EaseOut | MotionCurve::EnterExit => 1.0 - (1.0 - progress).powi(3),
        };
        self.value = self.start + (self.target - self.start) * eased;
        if progress >= 1.0 {
            self.value = self.target;
            self.start = self.target;
            self.elapsed = Duration::ZERO;
        }
    }

    fn is_animating(self) -> bool {
        (self.value - self.target).abs() > MOTION_EPSILON
    }

    fn is_visible(self) -> bool {
        self.value > MOTION_EPSILON
    }
}

struct ReaderUiState {
    sidebar_open: bool,
    sidebar_pinned: bool,
    sidebar_tab: SidebarTab,
    toolbar_hovered: bool,
    toolbar_hide_at: Option<Instant>,
    overlay: ReaderOverlay,
    settings_tab: SettingsTab,
    draft_spread: SpreadMode,
    draft_typography: ReaderTypography,
    font_picker: Option<FontPickerKind>,
    draft_plugin_settings: PluginSettings,
    assistant_panel: Option<AssistantPanel>,
    toolbar_motion: Motion,
    sidebar_motion: Motion,
    menu_motion: Motion,
    settings_motion: Motion,
    last_motion_tick: Option<Instant>,
    expanded_toc: HashSet<String>,
}

impl ReaderUiState {
    fn is_animating(&self) -> bool {
        self.toolbar_motion.is_animating()
            || self.sidebar_motion.is_animating()
            || self.menu_motion.is_animating()
            || self.settings_motion.is_animating()
    }

    fn needs_motion_tick(&self) -> bool {
        self.is_animating() || self.toolbar_hide_at.is_some()
    }

    fn refresh_motion_clock(&mut self, now: Instant) {
        if self.needs_motion_tick() {
            self.last_motion_tick.get_or_insert(now);
        } else {
            self.last_motion_tick = None;
        }
    }

    fn reveal_toolbar(&mut self, now: Instant) {
        self.toolbar_hide_at = None;
        self.toolbar_motion.animate_to(1.0);
        self.refresh_motion_clock(now);
    }

    fn schedule_toolbar_hide(&mut self, now: Instant) {
        if self.toolbar_motion.is_visible() || self.toolbar_motion.is_animating() {
            self.toolbar_hide_at = Some(now + TOOLBAR_HIDE_DELAY);
        }
        self.refresh_motion_clock(now);
    }

    fn overlay_visible(&self) -> bool {
        self.menu_motion.is_visible() || self.settings_motion.is_visible()
    }
}

impl DesktopReader {
    fn new(mut reader: ReaderSession, resources: DesktopReaderResources) -> Self {
        let DesktopReaderResources {
            source,
            rewrite_source,
            translation_source,
            cover,
            format,
            book_id,
            display_metadata,
            highlight_store,
            highlights,
            progress_store,
            plugin_settings,
            available_font_families,
        } = resources;
        let draft_style = reader.style();
        let error = reader
            .prefetch_adjacent()
            .err()
            .map(|error| error.to_string());
        let snapshot = reader.snapshot();
        let expanded_toc = snapshot.active_toc_path.iter().cloned().collect();
        Self {
            reader,
            source,
            rewrite_source,
            translation_source,
            snapshot,
            cover,
            format,
            book_id,
            display_metadata,
            highlight_store,
            highlights,
            progress_store,
            selection_anchor: None,
            selection: None,
            selection_toolbar_visible: false,
            selected_highlight_id: None,
            focused_mark: None,
            search: SearchUiState::default(),
            chat: ChatUiState::default(),
            translation: TranslationUiState::default(),
            ui: ReaderUiState {
                sidebar_open: true,
                sidebar_pinned: true,
                sidebar_tab: SidebarTab::Toc,
                toolbar_hovered: false,
                toolbar_hide_at: None,
                overlay: ReaderOverlay::None,
                settings_tab: SettingsTab::Reading,
                draft_spread: draft_style.spread,
                draft_typography: draft_style.typography,
                font_picker: None,
                draft_plugin_settings: plugin_settings.clone(),
                assistant_panel: None,
                toolbar_motion: Motion::settled_with_duration(0.0, TOOLBAR_MOTION_DURATION),
                sidebar_motion: Motion::settled(1.0),
                menu_motion: Motion::settled(0.0),
                settings_motion: Motion::settled_with_curve(
                    0.0,
                    SETTINGS_MOTION_DURATION,
                    MotionCurve::EnterExit,
                ),
                last_motion_tick: None,
                expanded_toc,
            },
            plugin_settings,
            available_font_families,
            canvas_size: None,
            scene_revision: 0,
            page_scenes: HashMap::new(),
            page_scene_lru: VecDeque::new(),
            pending_page_turn: None,
            error,
            exit_requested: false,
        }
    }

    fn request_exit(&mut self) {
        self.persist_progress();
        self.exit_requested = true;
    }

    fn begin_text_selection(&mut self, x: f32, y: f32) {
        self.selection_toolbar_visible = false;
        match self.reader.hit_test_current_spread(x, y, true) {
            Ok(anchor) => {
                self.selection_anchor = anchor;
                self.selection = None;
                self.selected_highlight_id = None;
                self.bump_scene_revision();
            }
            Err(error) => self.error = Some(format!("选择文字失败：{error}")),
        }
    }

    fn update_text_selection(&mut self, x: f32, y: f32) {
        let Some(anchor) = self.selection_anchor.clone() else {
            return;
        };
        let result = self
            .reader
            .hit_test_current_spread(x, y, false)
            .and_then(|focus| {
                focus.map_or(Ok(None), |focus| {
                    self.reader.selection_between(&anchor, &focus)
                })
            });
        match result {
            Ok(selection) if self.selection != selection => {
                self.selection = selection;
                self.bump_scene_revision();
            }
            Ok(_) => {}
            Err(error) => self.error = Some(format!("选择文字失败：{error}")),
        }
    }

    fn finish_text_selection(&mut self, x: f32, y: f32, moved: bool) {
        if moved {
            self.update_text_selection(x, y);
            if self.selection.is_none() {
                self.selection_anchor = None;
            }
            self.selection_toolbar_visible = self.selection.is_some();
            return;
        }

        self.selection_toolbar_visible = false;
        self.selection_anchor = None;
        self.selection = None;
        self.bump_scene_revision();
        let candidates = self
            .highlights
            .iter()
            .map(|highlight| (highlight.id.clone(), highlight.ranges.clone()))
            .collect::<Vec<_>>();
        let activated = candidates.into_iter().find_map(|(id, ranges)| {
            self.reader
                .source_ranges_contain_point(&ranges, x, y)
                .ok()
                .filter(|contains| *contains)
                .map(|_| id)
        });
        if let Some(id) = activated {
            self.selected_highlight_id = Some(id);
            self.ui.sidebar_tab = SidebarTab::Highlights;
            self.set_sidebar_open(true);
        } else {
            self.selected_highlight_id = None;
        }
    }

    fn cancel_text_selection(&mut self) {
        self.selection_toolbar_visible = false;
        self.selection_anchor = None;
        if self.selection.take().is_some() {
            self.bump_scene_revision();
        }
    }

    fn create_highlight(&mut self) {
        let Some(selection) = self.selection.clone() else {
            return;
        };
        let highlight =
            StoredHighlight::new(self.book_id.clone(), selection.ranges, selection.text);
        match self.highlight_store.insert(highlight.clone()) {
            Ok(()) => {
                self.highlights.insert(0, highlight);
                self.selection_anchor = None;
                self.selection = None;
                self.selected_highlight_id = None;
                self.bump_scene_revision();
                self.error = None;
            }
            Err(error) => self.error = Some(format!("保存高亮失败：{error}")),
        }
    }

    fn remove_highlight(&mut self, id: &str) {
        match self.highlight_store.remove(id) {
            Ok(true) => {
                self.highlights.retain(|highlight| highlight.id != id);
                if self.selected_highlight_id.as_deref() == Some(id) {
                    self.selected_highlight_id = None;
                }
                self.bump_scene_revision();
                self.error = None;
            }
            Ok(false) => {}
            Err(error) => self.error = Some(format!("删除高亮失败：{error}")),
        }
    }

    fn go_to_highlight(&mut self, id: &str) {
        let Some(anchor) = self
            .highlights
            .iter()
            .find(|highlight| highlight.id == id)
            .and_then(|highlight| highlight.ranges.first())
            .map(|range| range.start.clone())
        else {
            return;
        };
        match self.reader.go_to_source(&anchor) {
            Ok(result) => {
                self.apply_snapshot(
                    result.snapshot,
                    SnapshotEffects {
                        marks: MarkRetention::Keep,
                        ..SnapshotEffects::navigation()
                    },
                );
                self.selected_highlight_id = Some(id.to_owned());
            }
            Err(error) => self.error = Some(format!("高亮跳转失败：{error}")),
        }
    }

    fn set_sidebar_tab(&mut self, tab: SidebarTab) {
        self.ui.sidebar_tab = tab;
    }

    fn open_search(&mut self) {
        self.ui.sidebar_tab = SidebarTab::Search;
        self.set_sidebar_open(true);
    }

    fn start_search(&mut self) {
        if self.search.task.is_pending() {
            return;
        }
        let query = self.search.query.trim().to_owned();
        if query.is_empty() {
            self.search.status = "请输入搜索内容".into();
            return;
        }
        self.search.status = "正在搜索…".into();
        self.search.results.clear();
        self.focused_mark = None;
        self.search.task.begin(SearchTask {
            source: Arc::clone(&self.source),
            query,
        });
        self.bump_scene_revision();
    }

    fn complete_search(&mut self, message: SearchTaskMessage) {
        if self.search.task.complete(message.id).is_none() {
            return;
        }
        match message.result {
            Ok(results) => {
                self.search.status = if results.is_empty() {
                    "没有找到匹配内容".into()
                } else {
                    format!("找到 {} 处结果", results.len())
                };
                self.search.results = results;
            }
            Err(error) => {
                self.search.results.clear();
                self.search.status = error;
            }
        }
    }

    fn go_to_search_result(&mut self, result: &BookSearchResult) {
        match self.reader.go_to_source(&result.range.start) {
            Ok(navigation) => {
                self.focused_mark = Some(FocusedMark::search(result.range.clone()));
                self.apply_snapshot(navigation.snapshot, SnapshotEffects::navigation());
            }
            Err(error) => self.search.status = format!("搜索结果跳转失败：{error}"),
        }
    }

    fn toggle_assistant_panel(&mut self, panel: AssistantPanel) {
        self.cancel_text_selection();
        self.ui.assistant_panel = if self.ui.assistant_panel == Some(panel) {
            None
        } else {
            Some(panel)
        };
    }

    fn close_assistant_panel(&mut self) {
        self.ui.assistant_panel = None;
    }

    fn send_chat(&mut self) {
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

    fn select_chat_command(&mut self, command: ChatCommand) {
        if !self.chat.task.is_pending() {
            self.chat.input = command.insert_text.into();
            self.chat.error = None;
        }
    }

    fn explain_selection(&mut self) {
        let Some(selection) = self.selection.clone() else {
            return;
        };
        let question = format!(
            "请结合当前段落和章节语境解释选中的内容。说明它的直接含义、在本段中的作用，以及理解它所需的背景；不要脱离原文进行无依据推测。\n\n选中文字：\n{}",
            selection.text.trim()
        );
        self.focused_mark = Some(FocusedMark::assistant(selection.ranges.clone()));
        self.cancel_text_selection();
        self.ui.assistant_panel = Some(AssistantPanel::Chat);
        self.queue_chat(question, None);
    }

    fn queue_chat(&mut self, question: String, display_content: Option<String>) {
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
        });
    }

    fn complete_chat(&mut self, message: ChatTaskMessage) {
        if self.chat.task.complete(message.id).is_none() {
            return;
        }
        match message.result {
            Ok(response) => {
                if !response.rewrites.is_empty() {
                    let transaction = match self.rewrite_source.apply_rewrites(&response.rewrites) {
                        Ok(transaction) => transaction,
                        Err(error) => {
                            self.chat.error = Some(format!("应用正文改写失败：{error}"));
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
                            self.chat.error = Some(match rollback_error {
                                Some(rollback_error) => format!(
                                    "应用正文改写失败：{error}；回滚也失败：{rollback_error}"
                                ),
                                None => format!("应用正文改写失败：{error}"),
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

    fn clear_chat(&mut self) {
        if !self.chat.task.is_pending() {
            self.chat.messages.clear();
            self.chat.error = None;
        }
    }

    fn toggle_translation(&mut self) {
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

    fn dismiss_translation_notice(&mut self) {
        self.translation.clear_error();
    }

    fn queue_current_section_translation(&mut self) {
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
        self.translation.task.begin(TranslationTask {
            section_index,
            settings: self.plugin_settings.clone(),
            blocks,
        });
    }

    fn complete_translation(&mut self, message: TranslationTaskMessage) {
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
                self.error = Some(format!("翻译正文失败：{error}"));
                self.translation.show_error(error, Instant::now());
            }
        }
    }

    fn refresh_translation_view(&mut self) {
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
            Err(error) => self
                .translation
                .show_error(format!("刷新翻译正文失败：{error}"), Instant::now()),
        }
    }

    fn turn_page(&mut self, direction: PageDirection) {
        if self.pending_page_turn.is_some() {
            return;
        }
        self.pending_page_turn = Some(direction);
        self.retry_pending_page_turn();
    }

    fn retry_pending_page_turn(&mut self) {
        let Some(direction) = self.pending_page_turn else {
            return;
        };
        let previous_section = self.snapshot.location.section_index;
        let previous_segment = self.snapshot.location.segment_index;
        let result = self.reader.try_turn_page(direction);
        if result.is_err() {
            self.pending_page_turn = None;
        }
        match result {
            Ok(NavigationAttempt::Pending) => {}
            Ok(NavigationAttempt::Ready(result)) => {
                let moved = result.outcome == NavigationOutcome::Moved;
                let section_changed = result.snapshot.location.section_index != previous_section;
                let segment_changed = result.snapshot.location.segment_index != previous_segment;
                self.apply_snapshot(
                    result.snapshot,
                    SnapshotEffects {
                        prefetch: if moved && (section_changed || segment_changed) {
                            FollowUp::Run
                        } else {
                            FollowUp::None
                        },
                        translation: if moved { FollowUp::Run } else { FollowUp::None },
                        progress: if moved {
                            ProgressChange::Persist
                        } else {
                            ProgressChange::Keep
                        },
                        ..SnapshotEffects::navigation()
                    },
                );
            }
            Err(error) => self.error = Some(format!("翻页失败：{error}")),
        }
    }

    fn open_settings(&mut self) {
        self.cancel_text_selection();
        let style = self.reader.style();
        self.ui.draft_spread = style.spread;
        self.ui.draft_typography = style.typography;
        self.ui.font_picker = None;
        self.ui
            .draft_plugin_settings
            .clone_from(&self.plugin_settings);
        self.set_overlay(ReaderOverlay::Settings);
    }

    fn apply_settings(&mut self) {
        let mut plugin_settings = self.ui.draft_plugin_settings.clone();
        plugin_settings.normalize();
        let translation_backend_changed = self.plugin_settings.translation_provider
            != plugin_settings.translation_provider
            || self.plugin_settings.translation_model != plugin_settings.translation_model
            || self.plugin_settings.target_language != plugin_settings.target_language
            || self.plugin_settings.providers != plugin_settings.providers;
        if let Err(error) = plugin_settings.save_default() {
            self.error = Some(format!("保存插件设置失败：{error}"));
            return;
        }
        let mut typography = self.ui.draft_typography.clone();
        typography.normalize();
        if let Err(error) = preferences::save_reader_typography(&typography) {
            self.error = Some(format!("保存字体设置失败：{error}"));
            return;
        }
        let mut style = self.reader.style();
        style.spread = self.ui.draft_spread;
        style.typography = typography;
        if let Err(error) = self
            .translation_source
            .set_mode(plugin_settings.translation_mode)
            .and_then(|()| {
                if translation_backend_changed {
                    self.translation_source.clear()
                } else {
                    Ok(())
                }
            })
        {
            self.error = Some(format!("应用翻译设置失败：{error}"));
            return;
        }
        let result = self.reader.set_style(style);
        match result {
            Ok(snapshot) => {
                self.plugin_settings = plugin_settings;
                self.translation.clear_error();
                if translation_backend_changed {
                    self.translation.task.cancel();
                }
                self.apply_snapshot(
                    snapshot,
                    SnapshotEffects {
                        translation: FollowUp::Run,
                        ..SnapshotEffects::static_content_change()
                    },
                );
                self.close_overlay();
            }
            Err(error) => self.error = Some(format!("应用阅读设置失败：{error}")),
        }
    }

    fn go_to(&mut self, target: &PublicationUrl) {
        let result = self.reader.go_to_href(target);
        match result {
            Ok(result) => {
                self.apply_snapshot(result.snapshot, SnapshotEffects::navigation());
            }
            Err(error) => self.error = Some(format!("目录跳转失败：{error}")),
        }
    }

    fn resize_canvas(&mut self, size: Size) {
        let width = logical_dimension(size.width);
        let height = logical_dimension(size.height);
        if width == 0 || height == 0 || self.canvas_size == Some((width, height)) {
            return;
        }
        if self.ui.sidebar_motion.is_animating() {
            return;
        }
        let Ok(viewport) = LayoutViewport::new(width, height) else {
            return;
        };
        let result = self.reader.resize(viewport);
        match result {
            Ok(snapshot) => {
                self.canvas_size = Some((width, height));
                self.apply_snapshot(snapshot, SnapshotEffects::static_content_change());
            }
            Err(error) => self.error = Some(format!("调整页面失败：{error}")),
        }
    }

    fn prefetch(&mut self) {
        let result = self
            .reader
            .prefetch_adjacent()
            .err()
            .map(|error| format!("章节预取失败：{error}"));
        self.error = result;
    }

    fn toggle_toc(&mut self, id: &str) {
        if !self.ui.expanded_toc.remove(id) {
            self.ui.expanded_toc.insert(id.to_owned());
        }
    }

    fn install_snapshot(&mut self, snapshot: ReaderSnapshot) {
        self.ui
            .expanded_toc
            .extend(snapshot.active_toc_path.iter().cloned());
        self.snapshot = snapshot;
    }

    fn apply_snapshot(&mut self, snapshot: ReaderSnapshot, effects: SnapshotEffects) {
        self.pending_page_turn = None;
        self.install_snapshot(snapshot);
        self.selection_toolbar_visible = false;
        self.selection_anchor = None;
        self.selection = None;
        match effects.marks {
            MarkRetention::Keep => {}
            MarkRetention::ClearSelectedHighlight => self.selected_highlight_id = None,
            MarkRetention::ClearAll => {
                self.selected_highlight_id = None;
                self.focused_mark = None;
            }
        }
        match effects.scene {
            SceneChange::Overlays => self.bump_scene_revision(),
            SceneChange::StaticContent => self.invalidate_page_scenes(),
        }
        self.error = None;
        if matches!(effects.progress, ProgressChange::Persist) {
            self.persist_progress();
        }
        if matches!(effects.prefetch, FollowUp::Run) {
            self.prefetch();
        }
        if matches!(effects.translation, FollowUp::Run) {
            self.queue_current_section_translation();
        }
    }

    fn persist_progress(&self) {
        let Some(store) = &self.progress_store else {
            return;
        };
        let locator = self.reader.current_locator();
        if let Err(error) = store.save_progress(&self.book_id, &locator) {
            tracing::warn!(%error, book_id = %self.book_id, "failed to persist reading progress");
        }
    }

    fn page_scene(&mut self) -> Arc<Scene> {
        let layers = self.page_scene_layers();
        let mut scene = Scene::new();
        scene.append(&layers.underlay, None);
        match self.reader.current_spread() {
            Ok(spread) => {
                let mut bridge = XilemVelloScene::new(&mut scene);
                self.paint_page_overlays(&spread.primary, &mut bridge, 0.0);
                if let Some(secondary) = spread.secondary {
                    self.paint_page_overlays(&secondary, &mut bridge, spread.secondary_offset_x);
                }
            }
            Err(error) => self.error = Some(format!("组合双页失败：{error}")),
        }
        scene.append(&layers.content, None);
        Arc::new(scene)
    }

    fn page_scene_layers(&mut self) -> Arc<PageSceneLayers> {
        let key = PageSceneKey {
            section: self.snapshot.location.section_index,
            segment: self.snapshot.location.segment_index,
            page: self.snapshot.location.page_index,
        };
        if let Some(layers) = self.page_scenes.get(&key).cloned() {
            self.touch_page_scene(key);
            return layers;
        }

        let mut underlay = Scene::new();
        let mut content = Scene::new();
        match self.reader.current_spread() {
            Ok(spread) => {
                let mut underlay_bridge = XilemVelloScene::new(&mut underlay);
                spread.primary.paint_background(&mut underlay_bridge);
                spread.primary.paint_images_at(&mut underlay_bridge, 0.0);
                if let Some(secondary) = &spread.secondary {
                    secondary.paint_images_at(&mut underlay_bridge, spread.secondary_offset_x);
                }

                let mut content_bridge = XilemVelloScene::new(&mut content);
                spread
                    .primary
                    .paint_non_image_content_at(&mut content_bridge, 0.0);
                if let Some(secondary) = spread.secondary {
                    secondary
                        .paint_non_image_content_at(&mut content_bridge, spread.secondary_offset_x);
                }
            }
            Err(error) => {
                self.error = Some(format!("组合双页失败：{error}"));
                self.reader
                    .current_page()
                    .paint(&mut XilemVelloScene::new(&mut underlay));
            }
        }
        let layers = Arc::new(PageSceneLayers {
            underlay: Arc::new(underlay),
            content: Arc::new(content),
        });
        self.page_scenes.insert(key, Arc::clone(&layers));
        self.touch_page_scene(key);
        let cache_capacity = match self.format {
            BookFormat::Pdf => PDF_PAGE_SCENE_CACHE_CAPACITY,
            _ => PAGE_SCENE_CACHE_CAPACITY,
        };
        while self.page_scenes.len() > cache_capacity {
            let Some(oldest) = self.page_scene_lru.pop_front() else {
                break;
            };
            if oldest != key {
                self.page_scenes.remove(&oldest);
            }
        }
        layers
    }

    fn paint_page_overlays(
        &self,
        page: &PageDisplayList,
        scene: &mut XilemVelloScene<'_>,
        offset_x: f32,
    ) {
        for highlight in &self.highlights {
            page.paint_source_ranges(scene, &highlight.ranges, ANNOTATION_MARK_COLOR, offset_x);
        }
        if let Some(mark) = &self.focused_mark {
            page.paint_source_ranges(scene, &mark.ranges, mark.color(), offset_x);
        }
        if let Some(selection) = &self.selection {
            page.paint_source_ranges(scene, &selection.ranges, TEXT_SELECTION_COLOR, offset_x);
        }
    }

    fn touch_page_scene(&mut self, key: PageSceneKey) {
        if let Some(position) = self.page_scene_lru.iter().position(|entry| *entry == key) {
            self.page_scene_lru.remove(position);
        }
        self.page_scene_lru.push_back(key);
    }

    fn bump_scene_revision(&mut self) {
        self.scene_revision = self.scene_revision.wrapping_add(1);
    }

    fn invalidate_page_scenes(&mut self) {
        self.page_scenes.clear();
        self.page_scene_lru.clear();
        self.bump_scene_revision();
    }

    fn progress(&self) -> f64 {
        self.snapshot.total_progression
    }

    fn set_sidebar_open(&mut self, open: bool) {
        self.ui.sidebar_open = open;
        if self
            .ui
            .sidebar_motion
            .animate_to(if open { 1.0 } else { 0.0 })
        {
            self.ui.last_motion_tick = Some(Instant::now());
        }
    }

    fn set_toolbar_hovered(&mut self, hovered: bool) {
        let now = Instant::now();
        self.ui.toolbar_hovered = hovered;
        if hovered {
            self.ui.reveal_toolbar(now);
        } else if self.ui.overlay != ReaderOverlay::Menu {
            self.ui.schedule_toolbar_hide(now);
        }
    }

    fn toggle_menu(&mut self) {
        if self.ui.overlay == ReaderOverlay::Menu {
            self.close_overlay();
        } else {
            self.set_overlay(ReaderOverlay::Menu);
        }
    }

    fn close_overlay(&mut self) {
        self.set_overlay(ReaderOverlay::None);
    }

    fn set_overlay(&mut self, overlay: ReaderOverlay) {
        let was_menu_open = self.ui.overlay == ReaderOverlay::Menu;
        self.ui.overlay = overlay;
        let menu_changed = self
            .ui
            .menu_motion
            .animate_to(if overlay == ReaderOverlay::Menu {
                1.0
            } else {
                0.0
            });
        let settings_changed =
            self.ui
                .settings_motion
                .animate_to(if overlay == ReaderOverlay::Settings {
                    1.0
                } else {
                    0.0
                });
        let now = Instant::now();
        if overlay == ReaderOverlay::Menu {
            self.ui.reveal_toolbar(now);
        } else if was_menu_open && !self.ui.toolbar_hovered {
            self.ui.schedule_toolbar_hide(now);
        }
        if menu_changed || settings_changed {
            self.ui.last_motion_tick = Some(now);
        }
    }

    fn advance_motion(&mut self, now: Instant) {
        let delta = self
            .ui
            .last_motion_tick
            .replace(now)
            .map_or(Duration::ZERO, |last| now.saturating_duration_since(last));
        let sidebar_was_animating = self.ui.sidebar_motion.is_animating();
        if self
            .ui
            .toolbar_hide_at
            .is_some_and(|deadline| now >= deadline)
        {
            self.ui.toolbar_hide_at = None;
            if !self.ui.toolbar_hovered && self.ui.overlay != ReaderOverlay::Menu {
                self.ui.toolbar_motion.animate_to(0.0);
            }
        }
        self.ui.toolbar_motion.advance(delta);
        self.ui.sidebar_motion.advance(delta);
        self.ui.menu_motion.advance(delta);
        self.ui.settings_motion.advance(delta);
        self.translation.dismiss_if_due(now);

        if sidebar_was_animating && !self.ui.sidebar_motion.is_animating() {
            // Reader layout is deliberately held stable during the slide. Trigger one
            // final canvas draw so the EPUB is reflowed only once at the settled width.
            self.bump_scene_revision();
        }
        if !self.ui.needs_motion_tick() {
            self.ui.last_motion_tick = None;
        }
    }

    fn advance_frame(&mut self, now: Instant) {
        self.advance_motion(now);
        self.retry_pending_page_turn();
    }
}

fn root_view(state: &mut DesktopApp) -> Box<AnyWidgetView<DesktopApp>> {
    if state
        .reader
        .as_ref()
        .is_some_and(|reader| reader.exit_requested)
    {
        state.reader = None;
        state.start_sync();
    }

    if let Some(reader) = state.reader.as_mut() {
        let reader_view = app_view(reader);
        map_state(reader_view, |state: &mut DesktopApp| {
            state.reader.as_mut().expect("reader exists")
        })
        .boxed()
    } else {
        shelf_app_view(state).boxed()
    }
}

fn shelf_app_view(state: &mut DesktopApp) -> impl WidgetView<DesktopApp> + use<> {
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
                DesktopApp::complete_sync,
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
                |state: &mut DesktopApp, ()| state.start_sync(),
            )
        }),
    )
}

fn shelf_view(state: &mut DesktopApp) -> impl WidgetView<DesktopApp> + use<> {
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
    let content: Box<AnyWidgetView<DesktopApp>> = if books.is_empty() && !query.is_empty() {
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
                    shelf_width_probe(|state: &mut DesktopApp, width| {
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
    let remove_dialog: Box<AnyWidgetView<DesktopApp>> =
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
                    DesktopApp::cancel_remove_book,
                    DesktopApp::confirm_remove_book,
                )
                .boxed()
            },
        );
    let sync_dialog: Box<AnyWidgetView<DesktopApp>> = if state.sync.dialog_open {
        shelf_sync_dialog(state).boxed()
    } else {
        sized_box(label("")).width(0.px()).height(0.px()).boxed()
    };
    sized_box(zstack((shelf, feedback_layer, remove_dialog, sync_dialog))).expand()
}

fn shelf_feedback_notice(state: &DesktopApp) -> Box<AnyWidgetView<DesktopApp>> {
    let content: Box<AnyWidgetView<DesktopApp>> = if let Some(message) = &state.shelf.error {
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
) -> impl WidgetView<DesktopApp> {
    let search = sized_box(
        flex_row((
            icon_label(Icon::Search, 16.0, UI_MUTED),
            text_input(query, |state: &mut DesktopApp, value| {
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
            shelf_icon_button(Icon::CloudCog, DesktopApp::open_sync_settings),
            sync_enabled.then(|| {
                shelf_icon_button(
                    if syncing {
                        Icon::CloudDownload
                    } else {
                        Icon::CloudSync
                    },
                    DesktopApp::start_sync,
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
fn shelf_sync_dialog(state: &DesktopApp) -> impl WidgetView<DesktopApp> + use<> {
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
            |state: &mut DesktopApp| {
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
                    shelf_icon_button(Icon::X, DesktopApp::close_sync_settings),
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
                            DesktopApp::close_sync_settings,
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
                            DesktopApp::apply_sync_settings,
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
            button(label(""), DesktopApp::close_sync_settings)
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
    control: Box<AnyWidgetView<DesktopApp>>,
) -> impl WidgetView<DesktopApp> {
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
    callback: impl Fn(&mut DesktopApp, String) + Send + Sync + 'static,
) -> impl WidgetView<DesktopApp> {
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
    state: &DesktopApp,
    books: Vec<LibraryBook>,
    include_import: bool,
    available_width: f64,
) -> impl WidgetView<DesktopApp> + use<> {
    let mut cards = books
        .into_iter()
        .map(|book| {
            let cover = state.shelf.covers.get(&book.id).cloned();
            shelf_book_card(&book, cover).boxed()
        })
        .collect::<Vec<Box<AnyWidgetView<DesktopApp>>>>();
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

fn shelf_book_card(book: &LibraryBook, cover: Option<ImageData>) -> impl WidgetView<DesktopApp> {
    let open_path = book.path.clone();
    let open_path_from_title = book.path.clone();
    let title = ellipsize_shelf_title(&book.title);
    let available = book.path.exists();
    let cover_button = sized_box(
        button(
            shelf_cover_content(book, cover),
            move |state: &mut DesktopApp| {
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
                    move |state: &mut DesktopApp| state.open_book(&open_path_from_title),
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
) -> Box<AnyWidgetView<DesktopApp>> {
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

fn shelf_remove_button(id: String, title: String) -> impl WidgetView<DesktopApp> {
    sized_box(
        button(
            icon_label(Icon::Trash2, 14.0, Color::WHITE),
            move |state: &mut DesktopApp| {
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

fn shelf_book_status(available: bool) -> impl WidgetView<DesktopApp> {
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

fn import_card() -> impl WidgetView<DesktopApp> {
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
    callback: impl Fn(&mut DesktopApp) + Send + Sync + 'static,
) -> impl WidgetView<DesktopApp> {
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

fn import_with_dialog(state: &mut DesktopApp) {
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

fn ellipsize_display_text(text: &str, max_units: usize) -> String {
    let display_units = text.chars().map(shelf_title_character_units).sum::<usize>();
    if display_units <= max_units {
        return text.to_owned();
    }

    let mut used_units = 0;
    let mut end = 0;
    for (index, character) in text.char_indices() {
        let character_units = shelf_title_character_units(character);
        if used_units + character_units > max_units.saturating_sub(2) {
            break;
        }
        used_units += character_units;
        end = index + character.len_utf8();
    }
    format!("{}…", &text[..end])
}

fn wrap_display_text(text: &str, line_units: usize, max_lines: usize) -> String {
    if line_units == 0 || max_lines == 0 {
        return String::new();
    }

    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = ellipsize_display_text(&normalized, line_units.saturating_mul(max_lines));
    let mut lines = Vec::with_capacity(max_lines);
    let mut current = String::new();
    let mut used_units = 0;

    for character in text.chars() {
        let character_units = shelf_title_character_units(character);
        if !current.is_empty() && used_units + character_units > line_units {
            lines.push(current.trim_end().to_owned());
            if lines.len() == max_lines {
                break;
            }
            current.clear();
            used_units = 0;
            if character.is_whitespace() {
                continue;
            }
        }
        used_units += character_units;
        current.push(character);
    }

    if lines.len() < max_lines && !current.is_empty() {
        lines.push(current.trim_end().to_owned());
    }
    lines.join("\n")
}

fn shelf_title_character_units(character: char) -> usize {
    if character.is_ascii() { 1 } else { 2 }
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

fn app_view(state: &mut DesktopReader) -> impl WidgetView<DesktopReader> + use<> {
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
            reader_menu().transform(Affine::translate((
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
            "正在翻译",
            "当前章节完成后会自动刷新正文。",
        )
        .boxed()
    } else if let Some(error) = &state.translation.error {
        dismissible_notice(
            NoticeTone::Error,
            "无法完成翻译",
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
            sidebar_book_summary(cover, &title, &author, format),
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
                "正在扫描正文…"
            } else {
                "搜索书中内容"
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
            .placeholder("搜索全文…")
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
            highlight_row_view(highlight, section_index, selected)
        })
        .collect::<Vec<_>>();
    let count = state.highlights.len();
    let content: Box<AnyWidgetView<DesktopReader>> = if rows.is_empty() {
        flex_col((
            icon_label(Icon::Highlighter, 24.0, UI_MUTED),
            label("还没有高亮")
                .font(UI_FONT_STACK)
                .text_size(13.0)
                .color(UI_MUTED),
            label("拖选正文后即可添加")
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
            label("高亮")
                .font(UI_FONT_STACK)
                .text_size(13.0)
                .weight(FontWeight::BOLD)
                .color(UI_TEXT),
            FlexSpacer::Flex(1.0),
            label(format!("{count} 条"))
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
                    label(format!("第 {} 章", section_index + 1))
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
        "未知作者".to_owned()
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
        sidebar_book_cover(cover, format),
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
                label("电子书")
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

fn assistant_panel(state: &DesktopReader) -> impl WidgetView<DesktopReader> + use<> {
    chat_panel(state)
}

fn chat_panel(state: &DesktopReader) -> impl WidgetView<DesktopReader> + use<> {
    let input = state.chat.input.clone();
    let busy = state.chat.task.is_pending();
    let mut rows = state
        .chat
        .messages
        .iter()
        .cloned()
        .map(|turn| chat_message_row(turn).boxed())
        .collect::<Vec<_>>();
    if busy {
        rows.push(
            chat_message_row(ChatTurn {
                role: ChatRole::Assistant,
                content: "正在阅读和检索书籍…".into(),
                display_content: None,
            })
            .boxed(),
        );
    }
    let conversation: Box<AnyWidgetView<DesktopReader>> = if rows.is_empty() {
        flex_col((
            icon_label(Icon::MessageCircle, 28.0, UI_MUTED),
            label("围绕当前书籍提问")
                .font(UI_FONT_STACK)
                .text_size(13.0)
                .weight(FontWeight::BOLD)
                .color(UI_TEXT_SOFT),
            prose("可以总结章节、解释选中的段落，或让 AI 搜索书中的概念。")
                .text_size(12.0)
                .text_color(UI_MUTED),
        ))
        .gap(8.px())
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .main_axis_alignment(MainAxisAlignment::Center)
        .padding(24.0)
        .boxed()
    } else {
        portal(
            flex_col(rows)
                .gap(10.px())
                .cross_axis_alignment(CrossAxisAlignment::Fill)
                .padding(12.0),
        )
        .boxed()
    };
    let error: Box<AnyWidgetView<DesktopReader>> = state.chat.error.as_ref().map_or_else(
        || sized_box(label("")).width(0.px()).height(0.px()).boxed(),
        |error| notice_card(NoticeTone::Error, "AI 请求失败", error.clone()).boxed(),
    );
    let command_menu = chat_command_menu(&input, busy);
    let composer = chat_composer(input, busy);
    let conversation_layer = flex_col((
        assistant_panel_header(Icon::MessageCircle, "AI 对话", true),
        divider(),
        conversation.flex(1.0),
        sized_box(label("")).height(48.px()),
    ))
    .must_fill_major_axis(true)
    .gap(8.px())
    .cross_axis_alignment(CrossAxisAlignment::Fill)
    .alignment(UnitPoint::TOP_LEFT);
    let composer_layer = sized_box(
        flex_col((error, command_menu, composer))
            .gap(8.px())
            .cross_axis_alignment(CrossAxisAlignment::Fill),
    )
    .expand_width()
    .alignment(UnitPoint::BOTTOM);

    sized_box(zstack((
        sized_box(label("")).expand(),
        conversation_layer,
        composer_layer,
    )))
    .width(ASSISTANT_PANEL_WIDTH.px())
    .expand_height()
    .background_color(UI_SIDEBAR)
    .border(UI_BORDER, 1.0)
    .padding(Padding::from_vh(6.0, 10.0))
}

fn chat_command_menu(input: &str, busy: bool) -> Box<AnyWidgetView<DesktopReader>> {
    let command_suggestions = chat_command_suggestions(input);
    if command_suggestions.is_empty() || busy {
        return sized_box(label("")).width(0.px()).height(0.px()).boxed();
    }
    let rows = command_suggestions
        .into_iter()
        .map(|command| chat_command_suggestion_row(command).boxed())
        .collect::<Vec<_>>();
    flex_col(rows)
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .background_color(UI_SURFACE)
        .border(UI_BORDER, 1.0)
        .corner_radius(RADIUS_MEDIUM)
        .padding(4.0)
        .boxed()
}

fn chat_composer(input: String, busy: bool) -> impl WidgetView<DesktopReader> + use<> {
    sized_box(
        flex_row((
            text_input(input, |state: &mut DesktopReader, value| {
                state.chat.input = value;
            })
            .on_enter(|state: &mut DesktopReader, value| {
                state.chat.input = value;
                state.send_chat();
            })
            .placeholder("询问这本书，或输入 / 使用技能…")
            .text_color(UI_TEXT)
            .caret_color(UI_ACCENT)
            .background_color(Color::TRANSPARENT)
            .border_color(Color::TRANSPARENT)
            .border_width(0.0)
            .padding(0.0)
            .flex(1.0),
            icon_button(Icon::Send, busy, DesktopReader::send_chat),
        ))
        .gap(6.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(40.px())
    .background_color(UI_SURFACE)
    .border(UI_BORDER, 1.0)
    .corner_radius(RADIUS_MEDIUM)
    .padding(Padding::horizontal(8.0))
}

fn chat_message_row(turn: ChatTurn) -> impl WidgetView<DesktopReader> + use<> {
    let (role, background, border, label_color) = match turn.role {
        ChatRole::User => ("你", UI_ACCENT_SOFT, UI_ACCENT_BORDER, UI_ACCENT),
        ChatRole::Assistant => ("Rebook AI", UI_SURFACE, UI_BORDER, UI_MUTED),
    };
    let content = turn.display_content.unwrap_or(turn.content);
    sized_box(
        flex_col((
            label(role)
                .font(UI_FONT_STACK)
                .text_size(10.5)
                .weight(FontWeight::BOLD)
                .color(label_color),
            prose(content).text_size(12.5).text_color(UI_TEXT_SOFT),
        ))
        .gap(5.px())
        .cross_axis_alignment(CrossAxisAlignment::Start),
    )
    .expand_width()
    .background_color(background)
    .border(border, 1.0)
    .corner_radius(RADIUS_MEDIUM)
    .padding(Padding::from_vh(9.0, 10.0))
}

fn chat_command_suggestion_row(command: ChatCommand) -> impl WidgetView<DesktopReader> + use<> {
    sized_box(
        button(
            flex_row((
                label(command.name)
                    .font(UI_FONT_STACK)
                    .text_size(12.0)
                    .weight(FontWeight::BOLD)
                    .color(UI_ACCENT),
                label(command.description)
                    .font(UI_FONT_STACK)
                    .text_size(11.0)
                    .color(UI_MUTED)
                    .flex(1.0),
            ))
            .gap(9.px())
            .cross_axis_alignment(CrossAxisAlignment::Center),
            move |state: &mut DesktopReader| state.select_chat_command(command),
        )
        .background_color(Color::TRANSPARENT)
        .active_background_color(UI_ACCENT_SOFT)
        .border_color(Color::TRANSPARENT)
        .hovered_border_color(UI_ACCENT_BORDER)
        .border_width(1.0)
        .corner_radius(7.0)
        .padding(Padding::horizontal(8.0)),
    )
    .height(34.px())
    .expand_width()
}

fn assistant_panel_header(
    icon: Icon,
    title: &'static str,
    clearable: bool,
) -> impl WidgetView<DesktopReader> {
    let clear: Box<AnyWidgetView<DesktopReader>> = if clearable {
        icon_button(Icon::Trash2, false, DesktopReader::clear_chat).boxed()
    } else {
        sized_box(label("")).width(0.px()).height(0.px()).boxed()
    };
    flex_row((
        icon_label(icon, 16.0, UI_MUTED),
        label(title)
            .font(UI_FONT_STACK)
            .text_size(13.5)
            .weight(FontWeight::BOLD)
            .color(UI_TEXT),
        FlexSpacer::Flex(1.0),
        clear,
        icon_button(Icon::X, false, DesktopReader::close_assistant_panel),
    ))
    .gap(6.px())
    .cross_axis_alignment(CrossAxisAlignment::Center)
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

fn icon_button(
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

fn reader_menu() -> impl WidgetView<DesktopReader> {
    sized_box(flex_col((
        menu_row(Icon::Library, "返回书架", DesktopReader::request_exit),
        divider(),
        menu_row(Icon::Settings, "设置", DesktopReader::open_settings),
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

fn settings_overlay(
    state: &DesktopReader,
    progress: f32,
) -> impl WidgetView<DesktopReader> + use<> {
    // Keep glyphs and one-pixel borders at their native scale throughout the
    // transition. Scaling the complete dialog makes text shimmer as Vello
    // resamples it on every frame, which reads as dropped frames on Windows.
    let offset = 8.0 * f64::from(1.0 - progress);
    let dialog_transform = Affine::translate((0.0, offset));
    sized_box(zstack((
        animated_scrim(modal_scrim_color(progress), DesktopReader::close_overlay),
        sized_box(settings_dialog(state))
            .width(SETTINGS_WIDTH.px())
            .height(SETTINGS_HEIGHT.px())
            .background_color(UI_SURFACE)
            .border(UI_BORDER, 1.0)
            .corner_radius(RADIUS_DIALOG)
            .transform(dialog_transform),
    )))
    .expand()
}

fn settings_dialog(state: &DesktopReader) -> impl WidgetView<DesktopReader> + use<> {
    settings_content(state)
}

fn settings_content(state: &DesktopReader) -> impl WidgetView<DesktopReader> + use<> {
    let spread = state.ui.draft_spread;
    let typography = &state.ui.draft_typography;
    let font_picker = state.ui.font_picker;
    let tab = state.ui.settings_tab;
    let title = match tab {
        SettingsTab::Reading => "阅读",
        SettingsTab::Font => font_picker.map_or("字体", FontPickerKind::title),
        SettingsTab::Ai => "AI",
        SettingsTab::AiChat => "AI Chat",
        SettingsTab::Translation => "翻译",
        SettingsTab::Plugins => "插件",
    };
    let body: Box<AnyWidgetView<DesktopReader>> = match tab {
        SettingsTab::Reading => reading_settings_content(spread).boxed(),
        SettingsTab::Font => match font_picker {
            Some(kind) => {
                font_picker_content(kind, typography, &state.available_font_families).boxed()
            }
            None => font_settings_content(typography).boxed(),
        },
        SettingsTab::Ai => ai_settings_content(state.ui.draft_plugin_settings.clone()).boxed(),
        SettingsTab::AiChat => ai_chat_settings_content(&state.ui.draft_plugin_settings).boxed(),
        SettingsTab::Translation => {
            translation_settings_content(&state.ui.draft_plugin_settings).boxed()
        }
        SettingsTab::Plugins => plugin_settings_content().boxed(),
    };

    flex_row((
        sized_box(zstack((
            sized_box(label(""))
                .expand()
                .background_color(UI_SURFACE_MUTED)
                .corner_radius(RADIUS_LARGE),
            sized_box(label(""))
                .width(RADIUS_DIALOG.px())
                .expand_height()
                .background_color(UI_SURFACE_MUTED)
                .alignment(UnitPoint::RIGHT),
            sized_box(
                flex_col((
                    flex_row((
                        icon_label(Icon::Settings, 17.0, UI_MUTED),
                        label("设置")
                            .font(UI_FONT_STACK)
                            .text_size(15.0)
                            .weight(FontWeight::BOLD)
                            .color(UI_TEXT),
                    ))
                    .gap(9.px())
                    .cross_axis_alignment(CrossAxisAlignment::Center)
                    .padding(Padding::from_vh(9.0, 8.0)),
                    settings_tab_button("阅读", SettingsTab::Reading, tab),
                    settings_tab_button("字体", SettingsTab::Font, tab),
                    settings_tab_button("AI", SettingsTab::Ai, tab),
                    settings_tab_button("AI Chat", SettingsTab::AiChat, tab),
                    settings_tab_button("翻译", SettingsTab::Translation, tab),
                    settings_tab_button("插件", SettingsTab::Plugins, tab),
                    FlexSpacer::Flex(1.0),
                ))
                .gap(3.px())
                .cross_axis_alignment(CrossAxisAlignment::Fill)
                .padding(8.0),
            )
            .expand(),
        )))
        .width(136.px())
        .expand_height(),
        flex_col((
            settings_dialog_header(title),
            divider(),
            body.flex(1.0),
            divider(),
            sized_box(
                flex_row((
                    FlexSpacer::Flex(1.0),
                    secondary_action_button("取消", DesktopReader::close_overlay),
                    primary_action_button("应用", DesktopReader::apply_settings),
                ))
                .gap(8.px())
                .cross_axis_alignment(CrossAxisAlignment::Center),
            )
            .height(DIALOG_FOOTER_HEIGHT.px())
            .expand_width()
            .padding(Padding::horizontal(CONTENT_PADDING_HORIZONTAL)),
        ))
        .must_fill_major_axis(true)
        .flex(1.0),
    ))
}

fn settings_dialog_header(title: &'static str) -> impl WidgetView<DesktopReader> {
    sized_box(
        flex_row((
            label(title)
                .font(UI_FONT_STACK)
                .text_size(15.0)
                .weight(FontWeight::BOLD)
                .color(UI_TEXT),
            FlexSpacer::Flex(1.0),
            icon_button(Icon::X, false, DesktopReader::close_overlay),
        ))
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(DIALOG_HEADER_HEIGHT.px())
    .padding(Padding::horizontal(CONTENT_PADDING_HORIZONTAL))
}

fn settings_tab_button(
    text: &'static str,
    value: SettingsTab,
    selected: SettingsTab,
) -> impl WidgetView<DesktopReader> {
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
            move |state: &mut DesktopReader| {
                state.ui.settings_tab = value;
                state.ui.font_picker = None;
            },
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

fn reading_settings_content(spread: SpreadMode) -> impl WidgetView<DesktopReader> {
    flex_col((
        label("页面布局")
            .font(UI_FONT_STACK)
            .text_size(12.0)
            .weight(FontWeight::BOLD)
            .color(UI_MUTED),
        sized_box(flex_col((
            settings_value_row("阅读模式", "分页"),
            divider(),
            spread_settings_row(spread),
        )))
        .background_color(UI_SURFACE)
        .border(UI_BORDER, 1.0)
        .corner_radius(RADIUS_MEDIUM),
    ))
    .gap(CONTENT_GAP.px())
    .cross_axis_alignment(CrossAxisAlignment::Fill)
    .padding(Padding::from_vh(
        CONTENT_PADDING_VERTICAL,
        CONTENT_PADDING_HORIZONTAL,
    ))
}

fn font_settings_content(typography: &ReaderTypography) -> impl WidgetView<DesktopReader> + use<> {
    let preview_font = typography.default_stack();
    let preview_size = typography.font_size.min(24.0);
    let preview_weight = FontWeight::new(f32::from(typography.font_weight));
    let default_font = typography.default_font;
    let font_size = typography.font_size;
    let minimum_font_size = typography.minimum_font_size;
    let font_weight = typography.font_weight;

    portal(
        flex_col((
            settings_section_label("字号与字重"),
            typography_metrics_card(font_size, minimum_font_size, font_weight),
            settings_section_label("字体"),
            flex_col((
                default_font_row(default_font),
                divider(),
                font_family_settings_row(
                    "中文字体",
                    typography.default_cjk_font.clone(),
                    FontPickerKind::Cjk,
                ),
            ))
            .cross_axis_alignment(CrossAxisAlignment::Fill)
            .background_color(UI_SURFACE)
            .border(UI_BORDER, 1.0)
            .corner_radius(RADIUS_MEDIUM),
            settings_section_label("字型"),
            flex_col((
                font_family_settings_row(
                    "衬线字体",
                    typography.serif_font.clone(),
                    FontPickerKind::Serif,
                ),
                divider(),
                font_family_settings_row(
                    "无衬线字体",
                    typography.sans_serif_font.clone(),
                    FontPickerKind::SansSerif,
                ),
                divider(),
                font_family_settings_row(
                    "等宽字体",
                    typography.monospace_font.clone(),
                    FontPickerKind::Monospace,
                ),
            ))
            .cross_axis_alignment(CrossAxisAlignment::Fill)
            .background_color(UI_SURFACE)
            .border(UI_BORDER, 1.0)
            .corner_radius(RADIUS_MEDIUM),
            sized_box(
                flex_col((
                    label("字体预览")
                        .font(UI_FONT_STACK)
                        .text_size(11.0)
                        .color(UI_MUTED),
                    label("阅读让思想抵达更远的地方 Reading 0123")
                        .font(ui_font_stack(preview_font))
                        .text_size(preview_size)
                        .weight(preview_weight)
                        .color(UI_TEXT),
                ))
                .gap(6.px())
                .cross_axis_alignment(CrossAxisAlignment::Start),
            )
            .background_color(UI_SURFACE_MUTED)
            .border(UI_BORDER, 1.0)
            .corner_radius(RADIUS_MEDIUM)
            .padding(Padding::from_vh(10.0, 12.0)),
        ))
        .gap(CONTENT_GAP.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::from_vh(
            CONTENT_PADDING_VERTICAL,
            CONTENT_PADDING_HORIZONTAL,
        )),
    )
}

fn typography_metrics_card(
    font_size: f32,
    minimum_font_size: f32,
    font_weight: u16,
) -> impl WidgetView<DesktopReader> {
    flex_col((
        typography_stepper_row(
            "默认字号",
            format!("{font_size:.0} px"),
            |state: &mut DesktopReader| {
                let minimum = state.ui.draft_typography.minimum_font_size;
                state.ui.draft_typography.font_size =
                    (state.ui.draft_typography.font_size - 1.0).max(minimum);
            },
            |state: &mut DesktopReader| {
                state.ui.draft_typography.font_size =
                    (state.ui.draft_typography.font_size + 1.0).min(120.0);
            },
        ),
        divider(),
        typography_stepper_row(
            "最小字号",
            format!("{minimum_font_size:.0} px"),
            |state: &mut DesktopReader| {
                state.ui.draft_typography.minimum_font_size =
                    (state.ui.draft_typography.minimum_font_size - 1.0).max(1.0);
            },
            |state: &mut DesktopReader| {
                let typography = &mut state.ui.draft_typography;
                typography.minimum_font_size = (typography.minimum_font_size + 1.0).min(120.0);
                typography.font_size = typography.font_size.max(typography.minimum_font_size);
            },
        ),
        divider(),
        typography_stepper_row(
            "字体粗细",
            font_weight.to_string(),
            |state: &mut DesktopReader| {
                state.ui.draft_typography.font_weight = state
                    .ui
                    .draft_typography
                    .font_weight
                    .saturating_sub(100)
                    .max(100);
            },
            |state: &mut DesktopReader| {
                state.ui.draft_typography.font_weight = state
                    .ui
                    .draft_typography
                    .font_weight
                    .saturating_add(100)
                    .min(900);
            },
        ),
    ))
    .cross_axis_alignment(CrossAxisAlignment::Fill)
    .background_color(UI_SURFACE)
    .border(UI_BORDER, 1.0)
    .corner_radius(RADIUS_MEDIUM)
}

fn settings_section_label(text: &'static str) -> impl WidgetView<DesktopReader> {
    label(text)
        .font(UI_FONT_STACK)
        .text_size(12.0)
        .weight(FontWeight::BOLD)
        .color(UI_MUTED)
}

fn typography_stepper_row(
    name: &'static str,
    value: String,
    decrease: impl Fn(&mut DesktopReader) + Send + Sync + 'static,
    increase: impl Fn(&mut DesktopReader) + Send + Sync + 'static,
) -> impl WidgetView<DesktopReader> {
    sized_box(
        flex_row((
            label(name)
                .font(UI_FONT_STACK)
                .text_size(13.0)
                .color(UI_TEXT_SOFT),
            FlexSpacer::Flex(1.0),
            stepper_button(Icon::Minus, decrease),
            sized_box(
                label(value)
                    .font(UI_FONT_STACK)
                    .text_size(12.0)
                    .color(UI_TEXT),
            )
            .width(62.px()),
            stepper_button(Icon::Plus, increase),
        ))
        .gap(5.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(SETTINGS_ROW_HEIGHT.px())
    .expand_width()
    .padding(Padding::horizontal(12.0))
}

fn stepper_button(
    icon: Icon,
    callback: impl Fn(&mut DesktopReader) + Send + Sync + 'static,
) -> impl WidgetView<DesktopReader> {
    sized_box(
        button(icon_label(icon, 13.0, UI_TEXT_SOFT), callback)
            .background_color(UI_SURFACE_MUTED)
            .active_background_color(UI_ACCENT_SOFT)
            .border_color(UI_BORDER)
            .hovered_border_color(UI_ACCENT_BORDER)
            .corner_radius(RADIUS_SMALL)
            .padding(0.0),
    )
    .width(CONTROL_HEIGHT_COMPACT.px())
    .height(CONTROL_HEIGHT_COMPACT.px())
}

fn default_font_row(selected: ReaderDefaultFont) -> impl WidgetView<DesktopReader> {
    sized_box(
        flex_row((
            label("默认字体")
                .font(UI_FONT_STACK)
                .text_size(13.0)
                .color(UI_TEXT_SOFT),
            FlexSpacer::Flex(1.0),
            default_font_choice("衬线", ReaderDefaultFont::Serif, selected),
            default_font_choice("无衬线", ReaderDefaultFont::SansSerif, selected),
        ))
        .gap(6.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(SETTINGS_ROW_HEIGHT.px())
    .expand_width()
    .padding(Padding::horizontal(12.0))
}

fn default_font_choice(
    text: &'static str,
    value: ReaderDefaultFont,
    selected: ReaderDefaultFont,
) -> impl WidgetView<DesktopReader> {
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
            move |state: &mut DesktopReader| state.ui.draft_typography.default_font = value,
        )
        .background_color(if active { UI_ACCENT_SOFT } else { UI_SURFACE })
        .active_background_color(UI_ACCENT_SOFT)
        .border_color(if active { UI_ACCENT_BORDER } else { UI_BORDER })
        .hovered_border_color(UI_ACCENT_BORDER)
        .corner_radius(RADIUS_SMALL)
        .padding(Padding::from_vh(5.0, 9.0)),
    )
    .height(CONTROL_HEIGHT_COMPACT.px())
}

fn font_family_settings_row(
    name: &'static str,
    value: String,
    picker: FontPickerKind,
) -> impl WidgetView<DesktopReader> {
    let display_value = value.clone();
    sized_box(
        button(
            flex_row((
                label(name)
                    .font(UI_FONT_STACK)
                    .text_size(13.0)
                    .color(UI_TEXT_SOFT),
                FlexSpacer::Flex(1.0),
                label(display_value)
                    .font(ui_font_stack(value))
                    .text_size(12.0)
                    .color(UI_TEXT),
                icon_label(Icon::ChevronRight, 14.0, UI_MUTED),
            ))
            .gap(8.px())
            .cross_axis_alignment(CrossAxisAlignment::Center),
            move |state: &mut DesktopReader| state.ui.font_picker = Some(picker),
        )
        .background_color(UI_SURFACE)
        .active_background_color(UI_SURFACE_MUTED)
        .border_color(Color::TRANSPARENT)
        .hovered_border_color(Color::TRANSPARENT)
        .border_width(0.0)
        .padding(Padding::horizontal(12.0)),
    )
    .height(SETTINGS_ROW_HEIGHT.px())
    .expand_width()
}

fn font_picker_content(
    kind: FontPickerKind,
    typography: &ReaderTypography,
    available_families: &[String],
) -> impl WidgetView<DesktopReader> + use<> {
    let selected = selected_font_family(typography, kind).to_owned();
    let rows = font_candidates(kind, available_families)
        .into_iter()
        .map(|family| font_picker_row(family, &selected, kind))
        .collect::<Vec<_>>();

    portal(
        flex_col((
            sized_box(
                button(
                    flex_row((
                        icon_label(Icon::ChevronLeft, 14.0, UI_MUTED),
                        label("返回字体设置")
                            .font(UI_FONT_STACK)
                            .text_size(12.0)
                            .color(UI_TEXT_SOFT),
                    ))
                    .gap(6.px())
                    .cross_axis_alignment(CrossAxisAlignment::Center),
                    |state: &mut DesktopReader| state.ui.font_picker = None,
                )
                .background_color(Color::TRANSPARENT)
                .active_background_color(UI_SURFACE_MUTED)
                .border_color(Color::TRANSPARENT)
                .hovered_border_color(Color::TRANSPARENT)
                .border_width(0.0)
                .padding(Padding::from_vh(6.0, 8.0)),
            )
            .height(CONTROL_HEIGHT.px()),
            flex_col(rows)
                .cross_axis_alignment(CrossAxisAlignment::Fill)
                .background_color(UI_SURFACE)
                .border(UI_BORDER, 1.0)
                .corner_radius(RADIUS_MEDIUM),
        ))
        .gap(CONTENT_GAP.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::from_vh(
            CONTENT_PADDING_VERTICAL,
            CONTENT_PADDING_HORIZONTAL,
        )),
    )
}

fn selected_font_family(typography: &ReaderTypography, kind: FontPickerKind) -> &str {
    match kind {
        FontPickerKind::Cjk => &typography.default_cjk_font,
        FontPickerKind::Serif => &typography.serif_font,
        FontPickerKind::SansSerif => &typography.sans_serif_font,
        FontPickerKind::Monospace => &typography.monospace_font,
    }
}

fn font_picker_row(
    family: String,
    selected: &str,
    kind: FontPickerKind,
) -> impl WidgetView<DesktopReader> + use<> {
    let active = family.eq_ignore_ascii_case(selected);
    let label_text = family.clone();
    let label_font = family.clone();
    sized_box(
        button(
            flex_row((
                label(label_text)
                    .font(ui_font_stack(label_font))
                    .text_size(13.0)
                    .weight(if active {
                        FontWeight::BOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .color(if active { UI_ACCENT } else { UI_TEXT }),
                FlexSpacer::Flex(1.0),
                icon_label(
                    Icon::Check,
                    14.0,
                    if active {
                        UI_ACCENT
                    } else {
                        Color::TRANSPARENT
                    },
                ),
            ))
            .cross_axis_alignment(CrossAxisAlignment::Center),
            move |state: &mut DesktopReader| {
                match kind {
                    FontPickerKind::Cjk => {
                        state
                            .ui
                            .draft_typography
                            .default_cjk_font
                            .clone_from(&family);
                    }
                    FontPickerKind::Serif => {
                        state.ui.draft_typography.serif_font.clone_from(&family);
                    }
                    FontPickerKind::SansSerif => {
                        state
                            .ui
                            .draft_typography
                            .sans_serif_font
                            .clone_from(&family);
                    }
                    FontPickerKind::Monospace => {
                        state.ui.draft_typography.monospace_font.clone_from(&family);
                    }
                }
                state.ui.font_picker = None;
            },
        )
        .background_color(if active { UI_ACCENT_SOFT } else { UI_SURFACE })
        .active_background_color(UI_ACCENT_SOFT)
        .border_color(Color::TRANSPARENT)
        .hovered_border_color(Color::TRANSPARENT)
        .border_width(0.0)
        .padding(Padding::horizontal(12.0)),
    )
    .height(42.px())
    .expand_width()
}

fn font_candidates(kind: FontPickerKind, available_families: &[String]) -> Vec<String> {
    let curated: &[&str] = match kind {
        FontPickerKind::Cjk => &[
            "LXGW WenKai GB Screen",
            "LXGW WenKai",
            "Noto Serif SC",
            "Noto Sans SC",
            "Microsoft YaHei",
            "SimSun",
            "KaiTi",
        ],
        FontPickerKind::Serif => &[
            "Bitter",
            "Literata",
            "Merriweather",
            "Noto Serif",
            "Georgia",
        ],
        FontPickerKind::SansSerif => &[
            "Roboto",
            "Noto Sans",
            "Open Sans",
            "Inter",
            "Microsoft YaHei",
        ],
        FontPickerKind::Monospace => &["Consolas", "Fira Code", "Roboto Mono", "IBM Plex Mono"],
    };
    let available = available_families
        .iter()
        .map(|family| family.to_lowercase())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for family in curated
        .iter()
        .filter(|family| {
            **family == "LXGW WenKai GB Screen" || available.contains(&family.to_lowercase())
        })
        .map(|family| (*family).to_owned())
        .chain(available_families.iter().filter_map(|family| {
            if kind != FontPickerKind::Cjk || looks_like_cjk_font(family) {
                Some(family.clone())
            } else {
                None
            }
        }))
    {
        if seen.insert(family.to_lowercase()) {
            candidates.push(family);
        }
    }
    candidates
}

fn looks_like_cjk_font(family: &str) -> bool {
    let name = family.to_lowercase();
    [
        "cjk", "han", "song", "ming", "hei", "kai", "yahei", "wenkai", "gothic", "meiryo",
        "malgun", "pingfang", "fangsong", "simsun", "simhei",
    ]
    .iter()
    .any(|keyword| name.contains(keyword))
}

fn ui_font_stack(source: String) -> FontStack<'static> {
    FontStack::Source(Cow::Owned(source))
}

#[derive(Clone, Copy)]
enum AiSettingField {
    ProviderName(usize),
    ProviderBaseUrl(usize),
    ProviderApiKey(usize),
    ProviderModel {
        provider_index: usize,
        model_index: usize,
    },
    TargetLanguage,
}

#[derive(Clone, Copy)]
enum AiFeature {
    Chat,
    Translation,
}

fn ai_settings_content(settings: PluginSettings) -> impl WidgetView<DesktopReader> + use<> {
    let provider_count = settings.providers.len();
    let provider_cards = settings
        .providers
        .into_iter()
        .enumerate()
        .map(|(index, provider)| ai_provider_card(index, provider, provider_count > 1))
        .collect::<Vec<_>>();
    portal(
        flex_col((
            settings_section_label("Providers"),
            flex_col(provider_cards)
                .gap(CONTENT_GAP.px())
                .cross_axis_alignment(CrossAxisAlignment::Fill),
            secondary_action_button("新增 Provider", |state: &mut DesktopReader| {
                state.ui.draft_plugin_settings.add_provider();
            }),
            prose(
                "每个 Provider 可以维护多个模型。API Key 只保存在当前运行内存中，不会写入 plugins.json；默认 Provider 也可以通过 REBOOK_AI_API_KEY 环境变量提供密钥。",
            )
            .text_size(10.5)
            .text_color(UI_MUTED),
        ))
        .gap(CONTENT_GAP.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::from_vh(
            CONTENT_PADDING_VERTICAL,
            CONTENT_PADDING_HORIZONTAL,
        )),
    )
}

fn ai_provider_card(
    index: usize,
    provider: AiProvider,
    can_remove_provider: bool,
) -> impl WidgetView<DesktopReader> {
    let AiProvider {
        name,
        base_url,
        models,
        api_key,
        ..
    } = provider;
    let title = if name.trim().is_empty() {
        format!("Provider {}", index + 1)
    } else {
        name.clone()
    };
    let model_count = models.len();
    let model_rows = models
        .into_iter()
        .enumerate()
        .map(|(model_index, model)| {
            ai_provider_model_row(index, model_index, model, model_count > 1)
        })
        .collect::<Vec<_>>();
    let remove_provider: Box<AnyWidgetView<DesktopReader>> = if can_remove_provider {
        icon_button(Icon::Trash2, false, move |state: &mut DesktopReader| {
            state.ui.draft_plugin_settings.remove_provider(index);
        })
        .boxed()
    } else {
        sized_box(label("")).width(32.px()).height(32.px()).boxed()
    };

    flex_col((
        flex_row((
            icon_label(Icon::Bot, 16.0, UI_ACCENT),
            label(title)
                .font(UI_FONT_STACK)
                .text_size(12.5)
                .weight(FontWeight::BOLD)
                .color(UI_TEXT),
            FlexSpacer::Flex(1.0),
            remove_provider,
        ))
        .gap(8.px())
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .padding(Padding::from_vh(7.0, 10.0)),
        divider(),
        ai_settings_input_row(
            "名称",
            name,
            "例如 OpenAI、Ollama",
            AiSettingField::ProviderName(index),
        ),
        divider(),
        ai_settings_input_row(
            "API 地址",
            base_url,
            "https://api.openai.com/v1",
            AiSettingField::ProviderBaseUrl(index),
        ),
        divider(),
        ai_settings_input_row(
            "API Key（仅本次会话）",
            api_key,
            "sk-…",
            AiSettingField::ProviderApiKey(index),
        ),
        divider(),
        flex_col((
            label("模型")
                .font(UI_FONT_STACK)
                .text_size(11.5)
                .weight(FontWeight::BOLD)
                .color(UI_MUTED),
            flex_col(model_rows).cross_axis_alignment(CrossAxisAlignment::Fill),
            secondary_action_button("新增模型", move |state: &mut DesktopReader| {
                if let Some(provider) = state.ui.draft_plugin_settings.providers.get_mut(index) {
                    provider.models.push(String::new());
                }
            }),
        ))
        .gap(6.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::from_vh(8.0, 10.0)),
    ))
    .cross_axis_alignment(CrossAxisAlignment::Fill)
    .background_color(UI_SURFACE)
    .border(UI_BORDER, 1.0)
    .corner_radius(RADIUS_MEDIUM)
}

fn ai_provider_model_row(
    provider_index: usize,
    model_index: usize,
    value: String,
    can_remove: bool,
) -> impl WidgetView<DesktopReader> {
    let remove: Box<AnyWidgetView<DesktopReader>> = if can_remove {
        icon_button(Icon::X, false, move |state: &mut DesktopReader| {
            state
                .ui
                .draft_plugin_settings
                .remove_model(provider_index, model_index);
        })
        .boxed()
    } else {
        sized_box(label("")).width(32.px()).height(32.px()).boxed()
    };
    sized_box(
        flex_row((
            label(format!("模型 {}", model_index + 1))
                .font(UI_FONT_STACK)
                .text_size(11.5)
                .color(UI_TEXT_SOFT),
            FlexSpacer::Flex(1.0),
            sized_box(
                text_input(value, move |state: &mut DesktopReader, value| {
                    set_ai_setting(
                        state,
                        AiSettingField::ProviderModel {
                            provider_index,
                            model_index,
                        },
                        value,
                    );
                })
                .placeholder("模型 ID，例如 gpt-4o-mini")
                .text_color(UI_TEXT)
                .caret_color(UI_ACCENT)
                .background_color(UI_SURFACE_MUTED)
                .border_color(UI_BORDER)
                .border_width(1.0)
                .corner_radius(RADIUS_SMALL)
                .padding(Padding::from_vh(4.0, 8.0)),
            )
            .width(250.px())
            .height(CONTROL_HEIGHT.px()),
            remove,
        ))
        .gap(6.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(SETTINGS_ROW_HEIGHT.px())
    .expand_width()
}

fn ai_settings_input_row(
    label_text: &'static str,
    value: String,
    placeholder: &'static str,
    field: AiSettingField,
) -> impl WidgetView<DesktopReader> {
    sized_box(
        flex_row((
            label(label_text)
                .font(UI_FONT_STACK)
                .text_size(12.0)
                .color(UI_TEXT_SOFT),
            FlexSpacer::Flex(1.0),
            sized_box(
                text_input(value, move |state: &mut DesktopReader, value| {
                    set_ai_setting(state, field, value);
                })
                .placeholder(placeholder)
                .text_color(UI_TEXT)
                .caret_color(UI_ACCENT)
                .background_color(UI_SURFACE_MUTED)
                .border_color(UI_BORDER)
                .border_width(1.0)
                .corner_radius(RADIUS_SMALL)
                .padding(Padding::from_vh(4.0, 8.0)),
            )
            .width(276.px())
            .height(CONTROL_HEIGHT.px()),
        ))
        .gap(10.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(SETTINGS_ROW_HEIGHT.px())
    .expand_width()
    .padding(Padding::horizontal(10.0))
}

fn set_ai_setting(state: &mut DesktopReader, field: AiSettingField, value: String) {
    match field {
        AiSettingField::ProviderName(index) => {
            if let Some(provider) = state.ui.draft_plugin_settings.providers.get_mut(index) {
                provider.name = value;
            }
        }
        AiSettingField::ProviderBaseUrl(index) => {
            if let Some(provider) = state.ui.draft_plugin_settings.providers.get_mut(index) {
                provider.base_url = value;
            }
        }
        AiSettingField::ProviderApiKey(index) => {
            if let Some(provider) = state.ui.draft_plugin_settings.providers.get_mut(index) {
                provider.api_key = value;
            }
        }
        AiSettingField::ProviderModel {
            provider_index,
            model_index,
        } => {
            let updated = state
                .ui
                .draft_plugin_settings
                .providers
                .get_mut(provider_index)
                .and_then(|provider| {
                    let provider_id = provider.id.clone();
                    let model = provider.models.get_mut(model_index)?;
                    let previous = std::mem::replace(model, value.clone());
                    Some((provider_id, previous))
                });
            if let Some((provider_id, previous)) = updated {
                if state.ui.draft_plugin_settings.chat_provider == provider_id
                    && state.ui.draft_plugin_settings.chat_model == previous
                {
                    state.ui.draft_plugin_settings.chat_model.clone_from(&value);
                }
                if state.ui.draft_plugin_settings.translation_provider == provider_id
                    && state.ui.draft_plugin_settings.translation_model == previous
                {
                    state
                        .ui
                        .draft_plugin_settings
                        .translation_model
                        .clone_from(&value);
                }
            }
        }
        AiSettingField::TargetLanguage => {
            state.ui.draft_plugin_settings.target_language = value;
        }
    }
}

fn ai_chat_settings_content(settings: &PluginSettings) -> impl WidgetView<DesktopReader> + use<> {
    portal(
        flex_col((
            settings_section_label("AI Chat 模型"),
            ai_model_choices(settings, AiFeature::Chat),
            prose("AI Chat 会使用这里选中的 Provider 和模型进行书籍问答、检索与解释。")
                .text_size(10.5)
                .text_color(UI_MUTED),
        ))
        .gap(CONTENT_GAP.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::from_vh(
            CONTENT_PADDING_VERTICAL,
            CONTENT_PADDING_HORIZONTAL,
        )),
    )
}

fn translation_settings_content(
    settings: &PluginSettings,
) -> impl WidgetView<DesktopReader> + use<> {
    let target_language = settings.target_language.clone();
    let translation_mode = settings.translation_mode;
    portal(
        flex_col((
            settings_section_label("翻译模型"),
            ai_model_choices(settings, AiFeature::Translation),
            settings_section_label("输出"),
            flex_col((
                ai_settings_input_row(
                    "目标语言",
                    target_language,
                    "简体中文",
                    AiSettingField::TargetLanguage,
                ),
                divider(),
                translation_mode_settings_row(translation_mode),
            ))
            .cross_axis_alignment(CrossAxisAlignment::Fill)
            .background_color(UI_SURFACE)
            .border(UI_BORDER, 1.0)
            .corner_radius(RADIUS_MEDIUM),
            prose("点击阅读器顶部的翻译按钮后，会使用这里的模型、目标语言和显示方式翻译正文。")
                .text_size(10.5)
                .text_color(UI_MUTED),
        ))
        .gap(CONTENT_GAP.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::from_vh(
            CONTENT_PADDING_VERTICAL,
            CONTENT_PADDING_HORIZONTAL,
        )),
    )
}

fn translation_mode_settings_row(mode: TranslationMode) -> impl WidgetView<DesktopReader> {
    sized_box(
        flex_row((
            label("显示方式").text_size(13.0).color(UI_TEXT_SOFT),
            FlexSpacer::Flex(1.0),
            translation_mode_choice("替换", TranslationMode::Replace, mode),
            translation_mode_choice("双行翻译", TranslationMode::Bilingual, mode),
        ))
        .gap(6.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(SETTINGS_ROW_HEIGHT.px())
    .expand_width()
    .padding(Padding::horizontal(12.0))
}

fn translation_mode_choice(
    text: &'static str,
    value: TranslationMode,
    selected: TranslationMode,
) -> impl WidgetView<DesktopReader> {
    let active = value == selected;
    sized_box(
        button(
            label(text)
                .text_size(12.0)
                .weight(if active {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                })
                .color(if active { UI_ACCENT } else { UI_TEXT_SOFT }),
            move |state: &mut DesktopReader| {
                state.ui.draft_plugin_settings.translation_mode = value;
            },
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

fn ai_model_choices(
    settings: &PluginSettings,
    feature: AiFeature,
) -> Box<AnyWidgetView<DesktopReader>> {
    let choices = settings
        .providers
        .iter()
        .flat_map(|provider| {
            provider.models.iter().filter_map(|model| {
                let model = model.trim();
                (!model.is_empty()).then(|| {
                    ai_model_choice_button(
                        provider.id.clone(),
                        &provider.name,
                        model.to_owned(),
                        feature,
                        match feature {
                            AiFeature::Chat => {
                                settings.chat_provider == provider.id
                                    && settings.chat_model.trim() == model
                            }
                            AiFeature::Translation => {
                                settings.translation_provider == provider.id
                                    && settings.translation_model.trim() == model
                            }
                        },
                    )
                })
            })
        })
        .collect::<Vec<_>>();
    if choices.is_empty() {
        sized_box(
            label("请先在 AI 页面为 Provider 添加模型")
                .font(UI_FONT_STACK)
                .text_size(12.0)
                .color(UI_MUTED),
        )
        .height(SETTINGS_ROW_HEIGHT.px())
        .expand_width()
        .background_color(UI_SURFACE)
        .border(UI_BORDER, 1.0)
        .corner_radius(RADIUS_MEDIUM)
        .padding(Padding::horizontal(12.0))
        .boxed()
    } else {
        flex_col(choices)
            .gap(6.px())
            .cross_axis_alignment(CrossAxisAlignment::Fill)
            .boxed()
    }
}

fn ai_model_choice_button(
    provider_id: String,
    provider_name: &str,
    model: String,
    feature: AiFeature,
    active: bool,
) -> impl WidgetView<DesktopReader> {
    let display = format!(
        "{}  /  {}",
        if provider_name.trim().is_empty() {
            "Provider"
        } else {
            provider_name.trim()
        },
        model
    );
    sized_box(
        button(
            flex_row((
                label(display)
                    .font(UI_FONT_STACK)
                    .text_size(12.0)
                    .weight(if active {
                        FontWeight::BOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .color(if active { UI_ACCENT } else { UI_TEXT_SOFT }),
                FlexSpacer::Flex(1.0),
                label(if active { "已选择" } else { "选择" })
                    .font(UI_FONT_STACK)
                    .text_size(10.5)
                    .color(if active { UI_ACCENT } else { UI_MUTED }),
            )),
            move |state: &mut DesktopReader| match feature {
                AiFeature::Chat => {
                    state
                        .ui
                        .draft_plugin_settings
                        .chat_provider
                        .clone_from(&provider_id);
                    state.ui.draft_plugin_settings.chat_model.clone_from(&model);
                }
                AiFeature::Translation => {
                    state
                        .ui
                        .draft_plugin_settings
                        .translation_provider
                        .clone_from(&provider_id);
                    state
                        .ui
                        .draft_plugin_settings
                        .translation_model
                        .clone_from(&model);
                }
            },
        )
        .background_color(if active { UI_ACCENT_SOFT } else { UI_SURFACE })
        .active_background_color(UI_ACCENT_SOFT)
        .border_color(if active { UI_ACCENT_BORDER } else { UI_BORDER })
        .hovered_border_color(UI_ACCENT_BORDER)
        .corner_radius(RADIUS_MEDIUM)
        .padding(Padding::horizontal(12.0)),
    )
    .height(40.px())
    .expand_width()
}

fn plugin_settings_content() -> impl WidgetView<DesktopReader> + use<> {
    let plugin_cards = BUILTIN_PLUGINS
        .into_iter()
        .map(|plugin| {
            flex_row((
                icon_label(Icon::Blocks, 15.0, UI_ACCENT),
                flex_col((
                    label(plugin.name)
                        .font(UI_FONT_STACK)
                        .text_size(12.0)
                        .weight(FontWeight::BOLD)
                        .color(UI_TEXT_SOFT),
                    label(plugin.description)
                        .font(UI_FONT_STACK)
                        .text_size(10.5)
                        .color(UI_MUTED),
                ))
                .gap(2.px())
                .cross_axis_alignment(CrossAxisAlignment::Start)
                .flex(1.0),
                value_badge("已启用"),
            ))
            .gap(9.px())
            .cross_axis_alignment(CrossAxisAlignment::Center)
            .padding(Padding::from_vh(7.0, 10.0))
        })
        .collect::<Vec<_>>();
    portal(
        flex_col((
            label("内置插件")
                .font(UI_FONT_STACK)
                .text_size(12.0)
                .weight(FontWeight::BOLD)
                .color(UI_MUTED),
            flex_col(plugin_cards)
                .cross_axis_alignment(CrossAxisAlignment::Fill)
                .background_color(UI_SURFACE)
                .border(UI_BORDER, 1.0)
                .corner_radius(RADIUS_MEDIUM),
        ))
        .gap(CONTENT_GAP.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::from_vh(
            CONTENT_PADDING_VERTICAL,
            CONTENT_PADDING_HORIZONTAL,
        )),
    )
}

fn settings_value_row(name: &'static str, value: &'static str) -> impl WidgetView<DesktopReader> {
    sized_box(
        flex_row((
            label(name).text_size(13.0).color(UI_TEXT_SOFT),
            FlexSpacer::Flex(1.0),
            value_badge(value),
        ))
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(SETTINGS_ROW_HEIGHT.px())
    .expand_width()
    .padding(Padding::horizontal(12.0))
}

fn spread_settings_row(spread: SpreadMode) -> impl WidgetView<DesktopReader> {
    sized_box(
        flex_row((
            label("分页方式").text_size(13.0).color(UI_TEXT_SOFT),
            FlexSpacer::Flex(1.0),
            spread_choice("单栏", SpreadMode::Single, spread),
            spread_choice("双栏", SpreadMode::Double, spread),
        ))
        .gap(6.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(SETTINGS_ROW_HEIGHT.px())
    .expand_width()
    .padding(Padding::horizontal(12.0))
}

fn spread_choice(
    text: &'static str,
    value: SpreadMode,
    selected: SpreadMode,
) -> impl WidgetView<DesktopReader> {
    let active = value == selected;
    sized_box(
        button(
            label(text)
                .text_size(12.0)
                .weight(if active {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                })
                .color(if active { UI_ACCENT } else { UI_TEXT_SOFT }),
            move |state: &mut DesktopReader| state.ui.draft_spread = value,
        )
        .background_color(if active { UI_ACCENT_SOFT } else { UI_SURFACE })
        .active_background_color(UI_ACCENT_SOFT)
        .border_color(if active { UI_ACCENT_BORDER } else { UI_BORDER })
        .hovered_border_color(UI_ACCENT_BORDER)
        .corner_radius(RADIUS_SMALL)
        .padding(Padding::from_vh(5.0, 9.0)),
    )
    .width(58.px())
    .height(CONTROL_HEIGHT.px())
}

fn value_badge(text: &'static str) -> impl WidgetView<DesktopReader> {
    sized_box(label(text).text_size(12.0).color(UI_TEXT_SOFT))
        .height(CONTROL_HEIGHT.px())
        .background_color(UI_SURFACE)
        .border(UI_BORDER, 1.0)
        .corner_radius(RADIUS_SMALL)
        .padding(Padding::from_vh(5.0, 10.0))
}

fn primary_action_button(
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

fn secondary_action_button(
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

fn animated_scrim(
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

fn transparent_catcher(
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

fn modal_scrim_color(progress: f32) -> Color {
    Color::from_rgb8(0x1f, 0x2d, 0x3d).with_alpha(MODAL_SCRIM_ALPHA * progress)
}

fn ui_color(color: Rgba) -> Color {
    Color::from_rgba8(color.red, color.green, color.blue, color.alpha)
}

fn usage(executable: &str) -> String {
    format!("usage: {executable} [book]")
}

fn decode_cover(bytes: &[u8]) -> Result<ImageData, ::image::ImageError> {
    let pixels = ::image::load_from_memory(bytes)?.into_rgba8();
    let width = pixels.width();
    let height = pixels.height();
    Ok(ImageData {
        data: Blob::new(Arc::new(pixels.into_vec())),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width,
        height,
    })
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn logical_dimension(value: f64) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    value.round().clamp(1.0, f64::from(u32::MAX)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_dimension_rejects_invalid_sizes_and_rounds_pixels() {
        assert_eq!(logical_dimension(f64::NAN), 0);
        assert_eq!(logical_dimension(f64::INFINITY), 0);
        assert_eq!(logical_dimension(-1.0), 0);
        assert_eq!(logical_dimension(0.0), 0);
        assert_eq!(logical_dimension(10.4), 10);
        assert_eq!(logical_dimension(10.6), 11);
    }

    #[test]
    fn motion_reaches_its_target_with_ease_out_timing() {
        let mut motion = Motion::settled(0.0);

        assert!(motion.animate_to(1.0));
        motion.advance(MOTION_DURATION / 2);
        assert!(motion.value > 0.5);
        assert!(motion.is_animating());

        motion.advance(MOTION_DURATION / 2);
        assert!((motion.value - 1.0).abs() <= f32::EPSILON);
        assert!(!motion.is_animating());
    }

    #[test]
    fn motion_can_reverse_without_jumping() {
        let mut motion = Motion::settled(0.0);
        motion.animate_to(1.0);
        motion.advance(MOTION_DURATION / 3);
        let value_before_reverse = motion.value;

        assert!(motion.animate_to(0.0));
        assert!((motion.value - value_before_reverse).abs() <= f32::EPSILON);
        motion.advance(MOTION_DURATION);
        assert!(motion.value.abs() <= f32::EPSILON);
        assert!(!motion.is_visible());
    }

    #[test]
    fn modal_motion_uses_a_gentler_exit_curve() {
        let mut motion =
            Motion::settled_with_curve(1.0, SETTINGS_MOTION_DURATION, MotionCurve::EnterExit);

        motion.animate_to(0.0);
        motion.advance(SETTINGS_MOTION_DURATION / 2);

        assert!((motion.value - 0.75).abs() < f32::EPSILON);
        assert!(motion.is_animating());
    }

    #[test]
    fn toolbar_hide_delay_is_cancelled_when_pointer_returns() {
        let now = Instant::now();
        let mut ui = ReaderUiState {
            sidebar_open: false,
            sidebar_pinned: false,
            sidebar_tab: SidebarTab::Toc,
            toolbar_hovered: false,
            toolbar_hide_at: None,
            overlay: ReaderOverlay::None,
            settings_tab: SettingsTab::Reading,
            draft_spread: SpreadMode::Single,
            draft_typography: ReaderTypography::default(),
            font_picker: None,
            draft_plugin_settings: PluginSettings::default(),
            assistant_panel: None,
            toolbar_motion: Motion::settled_with_duration(0.0, TOOLBAR_MOTION_DURATION),
            sidebar_motion: Motion::settled(0.0),
            menu_motion: Motion::settled(0.0),
            settings_motion: Motion::settled_with_curve(
                0.0,
                SETTINGS_MOTION_DURATION,
                MotionCurve::EnterExit,
            ),
            last_motion_tick: None,
            expanded_toc: HashSet::new(),
        };

        ui.reveal_toolbar(now);
        ui.toolbar_motion.advance(TOOLBAR_MOTION_DURATION);
        ui.schedule_toolbar_hide(now);
        assert_eq!(ui.toolbar_hide_at, Some(now + TOOLBAR_HIDE_DELAY));

        ui.reveal_toolbar(now + TOOLBAR_HIDE_DELAY / 2);
        assert!(ui.toolbar_hide_at.is_none());
        assert!((ui.toolbar_motion.target - 1.0).abs() <= f32::EPSILON);
    }

    #[test]
    fn cjk_font_candidates_keep_readest_defaults_and_filter_latin_families() {
        let available: Arc<[String]> = [
            "Arial".to_owned(),
            "LXGW WenKai".to_owned(),
            "Microsoft YaHei UI".to_owned(),
        ]
        .into();

        let candidates = font_candidates(FontPickerKind::Cjk, &available);

        assert_eq!(candidates[0], "LXGW WenKai GB Screen");
        assert_eq!(
            candidates
                .iter()
                .filter(|family| family.as_str() == "LXGW WenKai")
                .count(),
            1
        );
        assert!(
            candidates
                .iter()
                .any(|family| family == "Microsoft YaHei UI")
        );
        assert!(!candidates.iter().any(|family| family == "Arial"));
    }

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
                .all(|line| { line.chars().map(shelf_title_character_units).sum::<usize>() <= 20 })
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

    #[test]
    fn shelf_metadata_overrides_a_hash_based_parser_title() {
        let shelf_metadata = BookDisplayMetadata {
            title: "情景学习".into(),
            authors: Vec::new(),
        };

        let resolved = resolve_book_display_metadata(
            Some(shelf_metadata.clone()),
            "21f76642e79935732871e58d99d4e7eb4e890a8ae1ed93f859097b655a37e434",
            &[],
        );

        assert_eq!(resolved, shelf_metadata);
    }

    #[test]
    fn parsed_metadata_remains_the_fallback_for_external_files() {
        let authors = vec!["作者".to_owned()];
        let resolved = resolve_book_display_metadata(None, "外部文件", &authors);

        assert_eq!(resolved.title, "外部文件");
        assert_eq!(resolved.authors, authors);
    }

    #[test]
    fn translation_error_notice_auto_dismisses_after_three_seconds() {
        let now = Instant::now();
        let mut translation = TranslationUiState::default();
        translation.show_error("测试错误".into(), now);

        assert_eq!(
            translation.dismiss_at,
            Some(now + NOTICE_AUTO_DISMISS_DELAY)
        );
        assert!(!translation.dismiss_if_due(now + NOTICE_AUTO_DISMISS_DELAY / 2));
        assert_eq!(translation.error.as_deref(), Some("测试错误"));

        assert!(translation.dismiss_if_due(now + NOTICE_AUTO_DISMISS_DELAY));
        assert!(translation.error.is_none());
        assert!(translation.dismiss_at.is_none());
    }

    #[test]
    fn manually_dismissing_translation_notice_cancels_auto_dismiss() {
        let mut translation = TranslationUiState::default();
        translation.show_error("测试错误".into(), Instant::now());

        translation.clear_error();

        assert!(translation.error.is_none());
        assert!(translation.dismiss_at.is_none());
    }
}
