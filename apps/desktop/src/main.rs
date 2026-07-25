//! Native e-book reader: parser -> reading IR -> page layout -> display list -> Xilem/Vello.

mod highlights;
mod library;
mod plugins;
mod pointer_button;
mod reader_canvas;
mod vello_bridge;

use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use highlights::{HighlightColor, HighlightStore, StoredHighlight};
use library::{LibraryBook, LocalLibrary};
use lucide_icons::{Icon, LUCIDE_FONT_BYTES};
use plugins::{
    BUILTIN_PLUGINS, BookSearchResult, ChatCommand, ChatCommandResolution, ChatResponse, ChatRole,
    ChatTurn, PluginSettings, RewriteBookSource, chat_command_suggestions, chat_with_book,
    resolve_chat_command, search_book, translate_text,
};
use pointer_button::button;
use reader_canvas::{ReaderCanvasAction, reader_canvas};
use rebook_formats::{BookFormat, open_file as open_publication_file};
use rebook_layout::{LayoutViewport, ReaderFontFamily, ReaderStyle, SpreadMode};
use rebook_publication::{BookSource, PublicationUrl, Rgba, SourceRange};
use rebook_reader::{
    NavigationOutcome, PageDirection, ReaderSelection, ReaderSession, ReaderSnapshot,
    ReaderTextHit, TocViewItem,
};
use vello_bridge::XilemVelloScene;
use xilem::core::{fork, map_state};
use xilem::masonry::kurbo::Size;
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
const SETTINGS_WIDTH: f64 = 640.0;
const SETTINGS_HEIGHT: f64 = 460.0;
const SHELF_CARD_WIDTH: f64 = 144.0;
const SHELF_COVER_HEIGHT: f64 = 216.0;
const SHELF_COLUMNS: usize = 4;
const SHELF_TITLE_MAX_DISPLAY_UNITS: usize = 18;
const PAGE_SCENE_CACHE_CAPACITY: usize = 32;
const MOTION_DURATION: Duration = Duration::from_millis(180);
const TOOLBAR_MOTION_DURATION: Duration = Duration::from_millis(200);
const TOOLBAR_HIDE_DELAY: Duration = Duration::from_millis(500);
const MOTION_FRAME_INTERVAL: Duration = Duration::from_millis(16);
const SIDEBAR_SCRIM_ALPHA: f32 = 0.28;
const MODAL_SCRIM_ALPHA: f32 = 0.35;
const MOTION_EPSILON: f32 = 0.001;
const SELECTION_TOOLBAR_WIDTH: f64 = 276.0;
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

    let library =
        LocalLibrary::load_default().map_err(|error| io::Error::other(error.to_string()))?;
    let mut state = DesktopApp::new(library);
    if let LaunchMode::Open(path) = launch {
        state.open_book(&path);
    }
    let window = WindowOptions::new("Rebook")
        .with_initial_inner_size(xilem::winit::dpi::LogicalSize::new(
            INITIAL_WIDTH,
            INITIAL_HEIGHT,
        ))
        .with_min_inner_size(xilem::winit::dpi::LogicalSize::new(720_u32, 520_u32));
    Xilem::new_simple(state, root_view, window)
        .with_font(LUCIDE_FONT_BYTES.to_vec())
        .run_in(EventLoop::with_user_event())?;
    Ok(())
}

fn open_reader(path: &Path) -> Result<DesktopReader, Box<dyn std::error::Error>> {
    let started = Instant::now();
    let publication = open_publication_file(path)?;
    let format = publication.format();
    let cover = publication
        .cover_bytes()
        .and_then(|bytes| decode_cover(bytes).ok());
    let canonical_source = publication.source();
    let book_id = canonical_source.book().id.to_string();
    let rewrite_source = Arc::new(RewriteBookSource::new(canonical_source));
    let source: Arc<dyn BookSource> = rewrite_source.clone();
    let highlight_store = HighlightStore::load_default()?;
    let highlights = highlight_store.for_book(&book_id);
    let plugin_settings = PluginSettings::load_default().unwrap_or_else(|error| {
        tracing::warn!(%error, "failed to load plugin settings; using defaults");
        PluginSettings::default()
    });
    let viewport = LayoutViewport::new(INITIAL_WIDTH, INITIAL_HEIGHT)?;
    let reader = ReaderSession::open(Arc::clone(&source), viewport, ReaderStyle::default())?;
    tracing::debug!(
        elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
        "opened book"
    );
    Ok(DesktopReader::new(
        reader,
        DesktopReaderResources {
            source,
            rewrite_source,
            cover,
            format,
            book_id,
            highlight_store,
            highlights,
            plugin_settings,
        },
    ))
}

enum LaunchMode {
    Shelf,
    Open(PathBuf),
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
}

struct ShelfState {
    library: LocalLibrary,
    covers: HashMap<String, ImageData>,
    query: String,
    notice: Option<String>,
    error: Option<String>,
}

impl DesktopApp {
    fn new(library: LocalLibrary) -> Self {
        Self {
            shelf: ShelfState::new(library),
            reader: None,
        }
    }

    fn open_book(&mut self, path: &Path) {
        match open_reader(path) {
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
                self.shelf.covers.remove(id);
                self.shelf.notice = Some("已从本地书架移除".into());
                self.shelf.error = None;
            }
            Ok(false) => self.shelf.error = Some("书籍已不在本地书架中".into()),
            Err(error) => self.shelf.error = Some(format!("移除失败：{error}")),
        }
    }
}

impl ShelfState {
    fn new(library: LocalLibrary) -> Self {
        let mut state = Self {
            library,
            covers: HashMap::new(),
            query: String::new(),
            notice: None,
            error: None,
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
    snapshot: ReaderSnapshot,
    cover: Option<ImageData>,
    format: BookFormat,
    book_id: String,
    highlight_store: HighlightStore,
    highlights: Vec<StoredHighlight>,
    selection_anchor: Option<ReaderTextHit>,
    selection: Option<ReaderSelection>,
    selected_highlight_id: Option<String>,
    focused_range: Option<SourceRange>,
    plugin_settings: PluginSettings,
    search: SearchUiState,
    chat: ChatUiState,
    translation: TranslationUiState,
    ui: ReaderUiState,
    canvas_size: Option<(u32, u32)>,
    scene_revision: u64,
    page_scenes: HashMap<PageSceneKey, Arc<Scene>>,
    page_scene_lru: VecDeque<PageSceneKey>,
    error: Option<String>,
    exit_requested: bool,
}

struct DesktopReaderResources {
    source: Arc<dyn BookSource>,
    rewrite_source: Arc<RewriteBookSource>,
    cover: Option<ImageData>,
    format: BookFormat,
    book_id: String,
    highlight_store: HighlightStore,
    highlights: Vec<StoredHighlight>,
    plugin_settings: PluginSettings,
}

#[derive(Clone)]
struct SearchTaskRequest {
    id: u64,
    source: Arc<dyn BookSource>,
    query: String,
}

#[derive(Debug)]
struct SearchTaskMessage {
    id: u64,
    result: Result<Vec<BookSearchResult>, String>,
}

#[derive(Default)]
struct SearchUiState {
    query: String,
    results: Vec<BookSearchResult>,
    status: String,
    pending: Option<SearchTaskRequest>,
    next_request_id: u64,
}

#[derive(Clone)]
struct ChatTaskRequest {
    id: u64,
    source: Arc<dyn BookSource>,
    settings: PluginSettings,
    history: Vec<ChatTurn>,
    question: String,
    current_section: usize,
}

#[derive(Debug)]
struct ChatTaskMessage {
    id: u64,
    result: Result<ChatResponse, String>,
}

#[derive(Default)]
struct ChatUiState {
    input: String,
    messages: Vec<ChatTurn>,
    error: Option<String>,
    pending: Option<ChatTaskRequest>,
    next_request_id: u64,
}

#[derive(Clone)]
struct TranslationTaskRequest {
    id: u64,
    settings: PluginSettings,
    text: String,
}

#[derive(Debug)]
struct TranslationTaskMessage {
    id: u64,
    result: Result<String, String>,
}

#[derive(Default)]
struct TranslationUiState {
    source_text: String,
    translated_text: String,
    source_range: Option<SourceRange>,
    error: Option<String>,
    pending: Option<TranslationTaskRequest>,
    next_request_id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct PageSceneKey {
    section: usize,
    segment: usize,
    page: usize,
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
    Plugins,
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
    Translation,
}

#[derive(Clone, Copy, Debug)]
struct Motion {
    value: f32,
    start: f32,
    target: f32,
    elapsed: Duration,
    duration: Duration,
}

impl Motion {
    const fn settled(value: f32) -> Self {
        Self::settled_with_duration(value, MOTION_DURATION)
    }

    const fn settled_with_duration(value: f32, duration: Duration) -> Self {
        Self {
            value,
            start: value,
            target: value,
            elapsed: Duration::ZERO,
            duration,
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
        let eased = 1.0 - (1.0 - progress).powi(3);
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
    draft_font_family: ReaderFontFamily,
    draft_font_size: f32,
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
            cover,
            format,
            book_id,
            highlight_store,
            highlights,
            plugin_settings,
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
            snapshot,
            cover,
            format,
            book_id,
            highlight_store,
            highlights,
            selection_anchor: None,
            selection: None,
            selected_highlight_id: None,
            focused_range: None,
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
                draft_font_family: draft_style.font_family,
                draft_font_size: draft_style.font_size,
                draft_plugin_settings: plugin_settings.clone(),
                assistant_panel: None,
                toolbar_motion: Motion::settled_with_duration(0.0, TOOLBAR_MOTION_DURATION),
                sidebar_motion: Motion::settled(1.0),
                menu_motion: Motion::settled(0.0),
                settings_motion: Motion::settled(0.0),
                last_motion_tick: None,
                expanded_toc,
            },
            plugin_settings,
            canvas_size: None,
            scene_revision: 0,
            page_scenes: HashMap::new(),
            page_scene_lru: VecDeque::new(),
            error,
            exit_requested: false,
        }
    }

    fn request_exit(&mut self) {
        self.exit_requested = true;
    }

    fn begin_text_selection(&mut self, x: f32, y: f32) {
        match self.reader.hit_test_current_spread(x, y, true) {
            Ok(anchor) => {
                self.selection_anchor = anchor;
                self.selection = None;
                self.selected_highlight_id = None;
                self.invalidate_page_scenes();
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
                self.invalidate_page_scenes();
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
            return;
        }

        self.selection_anchor = None;
        self.selection = None;
        self.invalidate_page_scenes();
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
        self.selection_anchor = None;
        if self.selection.take().is_some() {
            self.invalidate_page_scenes();
        }
    }

    fn create_highlight(&mut self, color: HighlightColor) {
        let Some(selection) = self.selection.clone() else {
            return;
        };
        let highlight = StoredHighlight::new(
            self.book_id.clone(),
            selection.ranges,
            selection.text,
            color,
        );
        match self.highlight_store.insert(highlight.clone()) {
            Ok(()) => {
                self.highlights.insert(0, highlight);
                self.selection_anchor = None;
                self.selection = None;
                self.selected_highlight_id = None;
                self.invalidate_page_scenes();
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
                self.invalidate_page_scenes();
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
                self.install_snapshot(result.snapshot);
                self.selected_highlight_id = Some(id.to_owned());
                self.selection_anchor = None;
                self.selection = None;
                self.invalidate_page_scenes();
                self.prefetch();
                self.error = None;
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
        if self.search.pending.is_some() {
            return;
        }
        let query = self.search.query.trim().to_owned();
        if query.is_empty() {
            self.search.status = "请输入搜索内容".into();
            return;
        }
        let id = self.search.next_request_id;
        self.search.next_request_id = self.search.next_request_id.wrapping_add(1);
        self.search.status = "正在搜索…".into();
        self.search.results.clear();
        self.focused_range = None;
        self.search.pending = Some(SearchTaskRequest {
            id,
            source: Arc::clone(&self.source),
            query,
        });
        self.invalidate_page_scenes();
    }

    fn complete_search(&mut self, message: SearchTaskMessage) {
        if self.search.pending.as_ref().map(|request| request.id) != Some(message.id) {
            return;
        }
        self.search.pending = None;
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
                self.install_snapshot(navigation.snapshot);
                self.focused_range = Some(result.range.clone());
                self.selection_anchor = None;
                self.selection = None;
                self.selected_highlight_id = None;
                self.invalidate_page_scenes();
                self.prefetch();
                self.error = None;
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
        if raw.is_empty() || self.chat.pending.is_some() {
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
        if self.chat.pending.is_none() {
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
        self.cancel_text_selection();
        self.ui.assistant_panel = Some(AssistantPanel::Chat);
        self.queue_chat(question, None);
    }

    fn queue_chat(&mut self, question: String, display_content: Option<String>) {
        if let Err(error) = self.plugin_settings.validate_ai() {
            self.chat.error = Some(error);
            self.ui.assistant_panel = Some(AssistantPanel::Chat);
            return;
        }
        let id = self.chat.next_request_id;
        self.chat.next_request_id = self.chat.next_request_id.wrapping_add(1);
        let history = self.chat.messages.clone();
        self.chat.messages.push(ChatTurn {
            role: ChatRole::User,
            content: question.clone(),
            display_content,
        });
        self.chat.error = None;
        self.chat.pending = Some(ChatTaskRequest {
            id,
            source: Arc::clone(&self.source),
            settings: self.plugin_settings.clone(),
            history,
            question,
            current_section: self.snapshot.location.section_index,
        });
    }

    fn complete_chat(&mut self, message: ChatTaskMessage) {
        if self.chat.pending.as_ref().map(|request| request.id) != Some(message.id) {
            return;
        }
        self.chat.pending = None;
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
                            self.install_snapshot(snapshot);
                            self.selection_anchor = None;
                            self.selection = None;
                            self.selected_highlight_id = None;
                            self.focused_range = None;
                            self.invalidate_page_scenes();
                            self.prefetch();
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
        if self.chat.pending.is_none() {
            self.chat.messages.clear();
            self.chat.error = None;
        }
    }

    fn translate_selection(&mut self) {
        let Some(selection) = self.selection.clone() else {
            return;
        };
        if let Err(error) = self.plugin_settings.validate_ai() {
            self.translation.error = Some(error);
            self.ui.assistant_panel = Some(AssistantPanel::Translation);
            return;
        }
        if self.translation.pending.is_some() {
            return;
        }
        let text = selection.text.trim().to_owned();
        let id = self.translation.next_request_id;
        self.translation.next_request_id = self.translation.next_request_id.wrapping_add(1);
        self.translation.source_text.clone_from(&text);
        self.translation.translated_text.clear();
        self.translation.source_range = selection.ranges.first().cloned();
        self.translation.error = None;
        self.translation.pending = Some(TranslationTaskRequest {
            id,
            settings: self.plugin_settings.clone(),
            text,
        });
        self.cancel_text_selection();
        self.ui.assistant_panel = Some(AssistantPanel::Translation);
    }

    fn retry_translation(&mut self) {
        if self.translation.pending.is_some() || self.translation.source_text.trim().is_empty() {
            return;
        }
        if let Err(error) = self.plugin_settings.validate_ai() {
            self.translation.error = Some(error);
            return;
        }
        let id = self.translation.next_request_id;
        self.translation.next_request_id = self.translation.next_request_id.wrapping_add(1);
        self.translation.error = None;
        self.translation.pending = Some(TranslationTaskRequest {
            id,
            settings: self.plugin_settings.clone(),
            text: self.translation.source_text.clone(),
        });
    }

    fn go_to_translation_source(&mut self) {
        let Some(range) = self.translation.source_range.clone() else {
            return;
        };
        match self.reader.go_to_source(&range.start) {
            Ok(navigation) => {
                self.install_snapshot(navigation.snapshot);
                self.focused_range = Some(range);
                self.invalidate_page_scenes();
                self.prefetch();
                self.error = None;
            }
            Err(error) => self.translation.error = Some(format!("原文定位失败：{error}")),
        }
    }

    fn complete_translation(&mut self, message: TranslationTaskMessage) {
        if self.translation.pending.as_ref().map(|request| request.id) != Some(message.id) {
            return;
        }
        self.translation.pending = None;
        match message.result {
            Ok(content) => {
                self.translation.translated_text = content;
                self.translation.error = None;
            }
            Err(error) => self.translation.error = Some(error),
        }
    }

    fn turn_page(&mut self, direction: PageDirection) {
        let previous_section = self.snapshot.location.section_index;
        let previous_segment = self.snapshot.location.segment_index;
        let result = self.reader.turn_page(direction);
        match result {
            Ok(result) => {
                let moved = result.outcome == NavigationOutcome::Moved;
                let section_changed = result.snapshot.location.section_index != previous_section;
                let segment_changed = result.snapshot.location.segment_index != previous_segment;
                self.install_snapshot(result.snapshot);
                self.selection_anchor = None;
                self.selection = None;
                self.selected_highlight_id = None;
                self.error = None;
                if moved {
                    self.bump_scene_revision();
                }
                if moved && (section_changed || segment_changed) {
                    self.prefetch();
                }
            }
            Err(error) => self.error = Some(format!("翻页失败：{error}")),
        }
    }

    fn open_settings(&mut self) {
        self.cancel_text_selection();
        let style = self.reader.style();
        self.ui.draft_spread = style.spread;
        self.ui.draft_font_family = style.font_family;
        self.ui.draft_font_size = style.font_size;
        self.ui
            .draft_plugin_settings
            .clone_from(&self.plugin_settings);
        self.set_overlay(ReaderOverlay::Settings);
    }

    fn apply_settings(&mut self) {
        let plugin_settings = self.ui.draft_plugin_settings.clone();
        if let Err(error) = plugin_settings.save_default() {
            self.error = Some(format!("保存插件设置失败：{error}"));
            return;
        }
        let mut style = self.reader.style();
        style.spread = self.ui.draft_spread;
        style.font_family = self.ui.draft_font_family;
        style.font_size = self.ui.draft_font_size;
        let result = self.reader.set_style(style);
        match result {
            Ok(snapshot) => {
                self.plugin_settings = plugin_settings;
                self.install_snapshot(snapshot);
                self.selection_anchor = None;
                self.selection = None;
                self.close_overlay();
                self.invalidate_page_scenes();
                self.prefetch();
            }
            Err(error) => self.error = Some(format!("应用阅读设置失败：{error}")),
        }
    }

    fn go_to(&mut self, target: &PublicationUrl) {
        let result = self.reader.go_to_href(target);
        match result {
            Ok(result) => {
                self.install_snapshot(result.snapshot);
                self.selection_anchor = None;
                self.selection = None;
                self.selected_highlight_id = None;
                self.bump_scene_revision();
                self.error = None;
                self.prefetch();
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
                self.install_snapshot(snapshot);
                self.selection_anchor = None;
                self.selection = None;
                self.canvas_size = Some((width, height));
                self.invalidate_page_scenes();
                self.prefetch();
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

    fn page_scene(&mut self) -> Arc<Scene> {
        let key = PageSceneKey {
            section: self.snapshot.location.section_index,
            segment: self.snapshot.location.segment_index,
            page: self.snapshot.location.page_index,
        };
        if let Some(scene) = self.page_scenes.get(&key).cloned() {
            self.touch_page_scene(key);
            return scene;
        }

        let mut scene = Scene::new();
        {
            let mut bridge = XilemVelloScene::new(&mut scene);
            match self.reader.current_spread() {
                Ok(spread) => {
                    spread.primary.paint_background(&mut bridge);
                    for highlight in &self.highlights {
                        spread.primary.paint_source_ranges(
                            &mut bridge,
                            &highlight.ranges,
                            ui_color(highlight.color.rgba()),
                            0.0,
                        );
                    }
                    if let Some(range) = &self.focused_range {
                        spread.primary.paint_source_ranges(
                            &mut bridge,
                            std::slice::from_ref(range),
                            Color::from_rgba8(59, 130, 246, 96),
                            0.0,
                        );
                    }
                    if let Some(selection) = &self.selection {
                        spread.primary.paint_source_ranges(
                            &mut bridge,
                            &selection.ranges,
                            Color::from_rgba8(96, 165, 250, 88),
                            0.0,
                        );
                    }
                    spread.primary.paint_content_at(&mut bridge, 0.0);
                    if let Some(secondary) = spread.secondary {
                        for highlight in &self.highlights {
                            secondary.paint_source_ranges(
                                &mut bridge,
                                &highlight.ranges,
                                ui_color(highlight.color.rgba()),
                                spread.secondary_offset_x,
                            );
                        }
                        if let Some(range) = &self.focused_range {
                            secondary.paint_source_ranges(
                                &mut bridge,
                                std::slice::from_ref(range),
                                Color::from_rgba8(59, 130, 246, 96),
                                spread.secondary_offset_x,
                            );
                        }
                        if let Some(selection) = &self.selection {
                            secondary.paint_source_ranges(
                                &mut bridge,
                                &selection.ranges,
                                Color::from_rgba8(96, 165, 250, 88),
                                spread.secondary_offset_x,
                            );
                        }
                        secondary.paint_content_at(&mut bridge, spread.secondary_offset_x);
                    }
                }
                Err(error) => {
                    self.error = Some(format!("组合双页失败：{error}"));
                    self.reader.current_page().paint(&mut bridge);
                }
            }
        }
        let scene = Arc::new(scene);
        self.page_scenes.insert(key, Arc::clone(&scene));
        self.touch_page_scene(key);
        while self.page_scenes.len() > PAGE_SCENE_CACHE_CAPACITY {
            let Some(oldest) = self.page_scene_lru.pop_front() else {
                break;
            };
            if oldest != key {
                self.page_scenes.remove(&oldest);
            }
        }
        scene
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

        if sidebar_was_animating && !self.ui.sidebar_motion.is_animating() {
            // Reader layout is deliberately held stable during the slide. Trigger one
            // final canvas draw so the EPUB is reflowed only once at the settled width.
            self.bump_scene_revision();
        }
        if !self.ui.needs_motion_tick() {
            self.ui.last_motion_tick = None;
        }
    }
}

fn root_view(state: &mut DesktopApp) -> Box<AnyWidgetView<DesktopApp>> {
    if state
        .reader
        .as_ref()
        .is_some_and(|reader| reader.exit_requested)
    {
        state.reader = None;
    }

    if let Some(reader) = state.reader.as_mut() {
        let reader_view = app_view(reader);
        map_state(reader_view, |state: &mut DesktopApp| {
            state.reader.as_mut().expect("reader exists")
        })
        .boxed()
    } else {
        shelf_view(state).boxed()
    }
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
    let notice = state.shelf.notice.clone().map(|message| {
        sized_box(label(message).text_size(13.0).color(UI_ACCENT))
            .expand_width()
            .background_color(UI_ACCENT_SOFT)
            .border(UI_ACCENT_BORDER, 1.0)
            .corner_radius(10.0)
            .padding(Padding::from_vh(10.0, 14.0))
    });
    let error = state.shelf.error.clone().map(|message| {
        sized_box(
            label(message)
                .text_size(13.0)
                .color(Color::from_rgb8(0xb4, 0x23, 0x18)),
        )
        .expand_width()
        .background_color(Color::from_rgb8(0xfe, 0xf3, 0xf2))
        .border(Color::from_rgb8(0xfe, 0xcd, 0xca), 1.0)
        .corner_radius(10.0)
        .padding(Padding::from_vh(10.0, 14.0))
    });
    let content: Box<AnyWidgetView<DesktopApp>> = if books.is_empty() && !query.is_empty() {
        sized_box(
            flex_col((
                FlexSpacer::Fixed(96.px()),
                icon_label_for_app(Icon::Search, 30.0, UI_MUTED),
                label("没有匹配的书籍").text_size(14.0).color(UI_MUTED),
            ))
            .gap(14.px())
            .cross_axis_alignment(CrossAxisAlignment::Center),
        )
        .expand_width()
        .boxed()
    } else {
        shelf_grid(state, books, query.is_empty()).boxed()
    };

    sized_box(
        flex_col((
            shelf_toolbar(state.shelf.query.clone(), book_count),
            divider_for_app(),
            portal(
                flex_col((notice, error, content))
                    .gap(12.px())
                    .cross_axis_alignment(CrossAxisAlignment::Fill)
                    .padding(Padding::from_vh(24.0, 28.0)),
            )
            .flex(1.0),
        ))
        .cross_axis_alignment(CrossAxisAlignment::Fill),
    )
    .expand()
    .background_color(UI_BACKGROUND)
}

fn shelf_toolbar(query: String, book_count: usize) -> impl WidgetView<DesktopApp> {
    let search = sized_box(
        flex_row((
            icon_label_for_app(Icon::Search, 16.0, UI_MUTED),
            text_input(query, |state: &mut DesktopApp, value| {
                state.shelf.query = value;
            })
            .placeholder(format!("搜索 {book_count} 本书"))
            .text_color(UI_TEXT)
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

fn shelf_grid(
    state: &DesktopApp,
    books: Vec<LibraryBook>,
    include_import: bool,
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

    let mut rows = Vec::new();
    let mut cards = cards.into_iter();
    loop {
        let mut row = cards.by_ref().take(SHELF_COLUMNS).collect::<Vec<_>>();
        if row.is_empty() {
            break;
        }
        while row.len() < SHELF_COLUMNS {
            row.push(
                sized_box(label(""))
                    .width(SHELF_CARD_WIDTH.px())
                    .height(1.px())
                    .boxed(),
            );
        }
        rows.push(
            flex_row(row)
                .gap(24.px())
                .cross_axis_alignment(CrossAxisAlignment::Start),
        );
    }

    flex_col(rows)
        .gap(28.px())
        .cross_axis_alignment(CrossAxisAlignment::Start)
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
            shelf_book_status(available, book.format),
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
        icon_label_for_app(Icon::BookOpen, 20.0, Color::from_rgba8(255, 255, 255, 150)),
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
            icon_label_for_app(Icon::Trash2, 14.0, Color::WHITE),
            move |state: &mut DesktopApp| {
                let confirmed = rfd::MessageDialog::new()
                    .set_title("从书架移除")
                    .set_description(format!(
                        "确定要移除《{title}》吗？本地书架中的副本将被删除。"
                    ))
                    .set_level(rfd::MessageLevel::Warning)
                    .set_buttons(rfd::MessageButtons::YesNo)
                    .show();
                if confirmed == rfd::MessageDialogResult::Yes {
                    state.remove_book(&id);
                }
            },
        )
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

fn shelf_book_status(available: bool, format: BookFormat) -> impl WidgetView<DesktopApp> {
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
        icon_label_for_app(icon, 12.0, color),
        label(text).text_size(11.5).color(color),
        FlexSpacer::Flex(1.0),
        label(format.label()).text_size(11.5).color(UI_MUTED),
    ))
    .gap(5.px())
    .cross_axis_alignment(CrossAxisAlignment::Center)
}

fn import_card() -> impl WidgetView<DesktopApp> {
    sized_box(
        flex_col((
            sized_box(
                button(
                    icon_label_for_app(Icon::Plus, 46.0, UI_MUTED),
                    import_with_dialog,
                )
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
        button(icon_label_for_app(icon, 17.0, UI_TEXT_SOFT), callback)
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

fn icon_label_for_app(icon: Icon, size: f32, color: Color) -> impl WidgetView<DesktopApp> {
    label(char::from(icon).to_string())
        .font("lucide")
        .text_size(size)
        .color(color)
}

fn divider_for_app() -> impl WidgetView<DesktopApp> {
    sized_box(label(""))
        .height(1.px())
        .expand_width()
        .background_color(UI_BORDER)
}

fn import_with_dialog(state: &mut DesktopApp) {
    let Some(paths) = rfd::FileDialog::new()
        .add_filter(
            "电子书（EPUB / Kindle / FB2 / CBZ）",
            &["epub", "mobi", "azw", "azw3", "fb2", "fbz", "cbz", "zip"],
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
    let animations_running = state.ui.needs_motion_tick();
    let search_request = state.search.pending.clone();
    let chat_request = state.chat.pending.clone();
    let translation_request = state.translation.pending.clone();
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
                |state: &mut DesktopReader, now| state.advance_motion(now),
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
                        let result = xilem::tokio::task::spawn_blocking(move || {
                            search_book(request.source.as_ref(), &request.query, 100)
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
                        let result = chat_with_book(
                            request.source,
                            request.settings,
                            request.history,
                            request.question,
                            request.current_section,
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
                        let result = translate_text(request.settings, request.text).await;
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
            state.reader.book().metadata.title.clone(),
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
    let selection_layer = selection_toolbar(state.selection.as_ref(), state.canvas_size)
        .alignment(UnitPoint::TOP_LEFT);
    let pages = sized_box(flex_col((
        reader_view(state.scene_revision, reader_background).flex(1.0),
        progress_bar(progress),
    )))
    .expand();

    sized_box(zstack((
        pages,
        selection_layer,
        menu_scrim,
        toolbar_layer,
        menu_layer,
    )))
    .expand()
    .background_color(reader_background)
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
    let color_buttons = HighlightColor::ALL
        .into_iter()
        .map(|color| highlight_color_button(color).boxed())
        .collect::<Vec<_>>();

    let toolbar = sized_box(
        flex_row((
            icon_label(Icon::Highlighter, 16.0, UI_MUTED),
            flex_row(color_buttons)
                .gap(6.px())
                .cross_axis_alignment(CrossAxisAlignment::Center),
            sized_box(label(""))
                .width(1.px())
                .height(22.px())
                .background_color(UI_BORDER),
            icon_button(Icon::Languages, false, DesktopReader::translate_selection),
            icon_button(
                Icon::MessageCircleQuestion,
                false,
                DesktopReader::explain_selection,
            ),
            sized_box(label(""))
                .width(1.px())
                .height(22.px())
                .background_color(UI_BORDER),
            icon_button(Icon::X, false, DesktopReader::cancel_text_selection),
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

fn highlight_color_button(color: HighlightColor) -> impl WidgetView<DesktopReader> {
    let rgba = color.rgba();
    let swatch = Color::from_rgb8(rgba.red, rgba.green, rgba.blue);
    sized_box(
        button(label(""), move |state: &mut DesktopReader| {
            state.create_highlight(color);
        })
        .background_color(swatch)
        .active_background_color(swatch.with_alpha(0.72))
        .border_color(Color::from_rgba8(31, 45, 61, 36))
        .hovered_border_color(UI_TEXT_SOFT)
        .border_width(1.0)
        .corner_radius(8.0)
        .padding(0.0),
    )
    .width(26.px())
    .height(26.px())
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
            sidebar_book_summary(cover, title, author, format),
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
    let busy = state.search.pending.is_some();
    let status = state.search.status.clone();
    let active_range = state.focused_range.clone();
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
    let color = highlight.color;
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
                .background_color(highlight_swatch_color(color))
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

fn highlight_swatch_color(color: HighlightColor) -> Color {
    let rgba = color.rgba();
    Color::from_rgb8(rgba.red, rgba.green, rgba.blue)
}

fn sidebar_book_metadata(state: &DesktopReader) -> (String, String) {
    let title = state.reader.book().metadata.title.clone();
    let author = if state.reader.book().metadata.authors.is_empty() {
        "未知作者".to_owned()
    } else {
        state.reader.book().metadata.authors.join(" / ")
    };
    (title, author)
}

fn sidebar_book_summary(
    cover: Option<ImageData>,
    title: String,
    author: String,
    format: BookFormat,
) -> impl WidgetView<DesktopReader> {
    flex_row((
        sidebar_book_cover(cover, format),
        flex_col((
            prose(title)
                .text_size(13.5)
                .weight(FontWeight::BOLD)
                .text_color(UI_TEXT),
            label(author).text_size(11.5).color(UI_MUTED),
        ))
        .gap(4.px())
        .cross_axis_alignment(CrossAxisAlignment::Start)
        .flex(1.0),
    ))
    .gap(12.px())
    .cross_axis_alignment(CrossAxisAlignment::Center)
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
    let content: Box<AnyWidgetView<DesktopReader>> = match state.ui.assistant_panel {
        Some(AssistantPanel::Chat) => chat_panel(state).boxed(),
        Some(AssistantPanel::Translation) => translation_panel(state).boxed(),
        None => sized_box(label("")).width(0.px()).height(0.px()).boxed(),
    };
    sized_box(content)
        .width(ASSISTANT_PANEL_WIDTH.px())
        .expand_height()
        .background_color(UI_SIDEBAR)
        .border(UI_BORDER, 1.0)
}

fn chat_panel(state: &DesktopReader) -> impl WidgetView<DesktopReader> + use<> {
    let input = state.chat.input.clone();
    let busy = state.chat.pending.is_some();
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
        |error| {
            sized_box(
                prose(error.clone())
                    .text_size(11.5)
                    .text_color(Color::from_rgb8(0xb9, 0x1c, 0x1c)),
            )
            .background_color(Color::from_rgb8(0xfe, 0xf2, 0xf2))
            .border(Color::from_rgb8(0xfe, 0xca, 0xca), 1.0)
            .corner_radius(8.0)
            .padding(Padding::from_vh(7.0, 9.0))
            .boxed()
        },
    );
    let command_menu = chat_command_menu(&input, busy);
    let composer = chat_composer(input, busy);

    flex_col((
        assistant_panel_header(Icon::MessageCircle, "AI 对话", true),
        divider(),
        conversation.flex(1.0),
        error,
        command_menu,
        composer,
    ))
    .gap(8.px())
    .cross_axis_alignment(CrossAxisAlignment::Fill)
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
        .corner_radius(10.0)
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
    .height(44.px())
    .background_color(UI_SURFACE)
    .border(UI_BORDER, 1.0)
    .corner_radius(12.0)
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
    .corner_radius(10.0)
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

fn translation_panel(state: &DesktopReader) -> impl WidgetView<DesktopReader> + use<> {
    let busy = state.translation.pending.is_some();
    let source_text = state.translation.source_text.clone();
    let has_source = !source_text.is_empty();
    let translated_text = state.translation.translated_text.clone();
    let target_language = state.plugin_settings.target_language.clone();
    let content: Box<AnyWidgetView<DesktopReader>> = if has_source {
        portal(
            flex_col((
                translation_text_card("原文", source_text, UI_SURFACE_MUTED),
                translation_text_card(
                    &format!("译文 · {target_language}"),
                    if busy {
                        "正在翻译…".into()
                    } else if translated_text.is_empty() {
                        "尚无译文".into()
                    } else {
                        translated_text
                    },
                    UI_SURFACE,
                ),
            ))
            .gap(10.px())
            .cross_axis_alignment(CrossAxisAlignment::Fill)
            .padding(12.0),
        )
        .boxed()
    } else {
        flex_col((
            icon_label(Icon::Languages, 28.0, UI_MUTED),
            label("选择正文后开始翻译")
                .font(UI_FONT_STACK)
                .text_size(13.0)
                .weight(FontWeight::BOLD)
                .color(UI_TEXT_SOFT),
            prose("拖选一段文字，然后点击浮动工具栏中的翻译按钮。原文不会被修改。")
                .text_size(12.0)
                .text_color(UI_MUTED),
        ))
        .gap(8.px())
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .main_axis_alignment(MainAxisAlignment::Center)
        .padding(24.0)
        .boxed()
    };
    let error: Box<AnyWidgetView<DesktopReader>> = state.translation.error.as_ref().map_or_else(
        || sized_box(label("")).width(0.px()).height(0.px()).boxed(),
        |error| {
            sized_box(
                prose(error.clone())
                    .text_size(11.5)
                    .text_color(Color::from_rgb8(0xb9, 0x1c, 0x1c)),
            )
            .background_color(Color::from_rgb8(0xfe, 0xf2, 0xf2))
            .border(Color::from_rgb8(0xfe, 0xca, 0xca), 1.0)
            .corner_radius(8.0)
            .padding(Padding::from_vh(7.0, 9.0))
            .boxed()
        },
    );
    let actions: Box<AnyWidgetView<DesktopReader>> = if has_source {
        flex_row((
            secondary_action_button("定位原文", DesktopReader::go_to_translation_source),
            FlexSpacer::Flex(1.0),
            secondary_action_button(
                if busy { "翻译中…" } else { "重新翻译" },
                DesktopReader::retry_translation,
            ),
        ))
        .gap(8.px())
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .boxed()
    } else {
        sized_box(label("")).width(0.px()).height(0.px()).boxed()
    };
    flex_col((
        assistant_panel_header(Icon::Languages, "翻译", false),
        divider(),
        content.flex(1.0),
        error,
        actions,
    ))
    .gap(8.px())
    .cross_axis_alignment(CrossAxisAlignment::Fill)
    .padding(Padding::from_vh(6.0, 10.0))
}

fn translation_text_card(
    title: &str,
    text: String,
    background: Color,
) -> impl WidgetView<DesktopReader> + use<> {
    flex_col((
        label(title.to_owned())
            .font(UI_FONT_STACK)
            .text_size(11.0)
            .weight(FontWeight::BOLD)
            .color(UI_MUTED),
        prose(text).text_size(12.5).text_color(UI_TEXT_SOFT),
    ))
    .gap(6.px())
    .cross_axis_alignment(CrossAxisAlignment::Start)
    .background_color(background)
    .border(UI_BORDER, 1.0)
    .corner_radius(10.0)
    .padding(Padding::from_vh(10.0, 11.0))
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
        left,
        FlexSpacer::Flex(1.0),
        label(title)
            .text_size(13.5)
            .weight(FontWeight::BOLD)
            .color(UI_TEXT),
        FlexSpacer::Flex(1.0),
        icon_button(Icon::Search, false, DesktopReader::open_search),
        icon_button(
            Icon::Languages,
            assistant_panel == Some(AssistantPanel::Translation),
            |state: &mut DesktopReader| {
                state.toggle_assistant_panel(AssistantPanel::Translation);
            },
        ),
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

fn icon_label(icon: Icon, size: f32, color: Color) -> impl WidgetView<DesktopReader> {
    label(char::from(icon).to_string())
        .font("lucide")
        .text_size(size)
        .color(color)
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
    let scale = 0.97 + 0.03 * f64::from(progress);
    let offset = 12.0 * f64::from(1.0 - progress);
    let dialog_transform =
        Affine::scale_about(scale, (SETTINGS_WIDTH / 2.0, SETTINGS_HEIGHT / 2.0))
            .then_translate((0.0, offset).into());
    sized_box(zstack((
        animated_scrim(modal_scrim_color(progress), DesktopReader::close_overlay),
        sized_box(settings_dialog(state))
            .width(SETTINGS_WIDTH.px())
            .height(SETTINGS_HEIGHT.px())
            .background_color(UI_SURFACE)
            .border(UI_BORDER, 1.0)
            .corner_radius(18.0)
            .transform(dialog_transform),
    )))
    .expand()
}

fn settings_dialog(state: &DesktopReader) -> impl WidgetView<DesktopReader> + use<> {
    settings_content(state)
}

fn settings_content(state: &DesktopReader) -> impl WidgetView<DesktopReader> + use<> {
    let spread = state.ui.draft_spread;
    let font_family = state.ui.draft_font_family;
    let font_size = state.ui.draft_font_size;
    let tab = state.ui.settings_tab;
    let title = match tab {
        SettingsTab::Reading => "阅读",
        SettingsTab::Font => "字体",
        SettingsTab::Plugins => "插件",
    };
    let body: Box<AnyWidgetView<DesktopReader>> = match tab {
        SettingsTab::Reading => reading_settings_content(spread).boxed(),
        SettingsTab::Font => font_settings_content(font_family, font_size).boxed(),
        SettingsTab::Plugins => {
            plugin_settings_content(state.ui.draft_plugin_settings.clone()).boxed()
        }
    };

    flex_row((
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
                .padding(Padding::from_vh(12.0, 10.0)),
                settings_tab_button("阅读", SettingsTab::Reading, tab),
                settings_tab_button("字体", SettingsTab::Font, tab),
                settings_tab_button("插件", SettingsTab::Plugins, tab),
                FlexSpacer::Flex(1.0),
            ))
            .gap(4.px())
            .cross_axis_alignment(CrossAxisAlignment::Fill)
            .padding(10.0),
        )
        .width(146.px())
        .expand_height()
        .background_color(UI_SURFACE_MUTED),
        flex_col((
            flex_row((
                label(title)
                    .font(UI_FONT_STACK)
                    .text_size(16.0)
                    .weight(FontWeight::BOLD)
                    .color(UI_TEXT),
                FlexSpacer::Flex(1.0),
                icon_button(Icon::X, false, DesktopReader::close_overlay),
            ))
            .cross_axis_alignment(CrossAxisAlignment::Center)
            .padding(Padding::from_vh(10.0, 16.0)),
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
            .height(56.px())
            .expand_width()
            .padding(Padding::horizontal(16.0)),
        ))
        .flex(1.0),
    ))
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
            move |state: &mut DesktopReader| state.ui.settings_tab = value,
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
        .corner_radius(8.0)
        .padding(Padding::horizontal(12.0)),
    )
    .height(38.px())
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
        .corner_radius(12.0),
    ))
    .gap(10.px())
    .cross_axis_alignment(CrossAxisAlignment::Fill)
    .padding(Padding::from_vh(18.0, 20.0))
}

fn font_settings_content(
    font_family: ReaderFontFamily,
    font_size: f32,
) -> impl WidgetView<DesktopReader> {
    flex_col((
        label("字体类型")
            .font(UI_FONT_STACK)
            .text_size(12.0)
            .weight(FontWeight::BOLD)
            .color(UI_MUTED),
        sized_box(
            flex_col((
                flex_row((
                    font_choice("衬线", ReaderFontFamily::Serif, font_family),
                    font_choice("无衬线", ReaderFontFamily::SansSerif, font_family),
                    font_choice("等宽", ReaderFontFamily::Monospace, font_family),
                ))
                .gap(8.px()),
                flex_row((
                    font_choice("微软雅黑", ReaderFontFamily::MicrosoftYaHei, font_family),
                    font_choice("宋体", ReaderFontFamily::SimSun, font_family),
                    font_choice("楷体", ReaderFontFamily::KaiTi, font_family),
                ))
                .gap(8.px()),
            ))
            .gap(8.px()),
        )
        .background_color(UI_SURFACE)
        .border(UI_BORDER, 1.0)
        .corner_radius(12.0)
        .padding(12.0),
        flex_row((
            label("字号")
                .font(UI_FONT_STACK)
                .text_size(13.0)
                .color(UI_TEXT_SOFT),
            FlexSpacer::Flex(1.0),
            font_size_choice(14.0, font_size),
            font_size_choice(16.0, font_size),
            font_size_choice(18.0, font_size),
            font_size_choice(20.0, font_size),
            font_size_choice(22.0, font_size),
        ))
        .gap(6.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
        sized_box(
            flex_col((
                label("字体预览")
                    .font(UI_FONT_STACK)
                    .text_size(11.0)
                    .color(UI_MUTED),
                label("阅读让思想抵达更远的地方  Reading 0123")
                    .font(font_family.css_stack())
                    .text_size(font_size.min(20.0))
                    .color(UI_TEXT),
            ))
            .gap(6.px())
            .cross_axis_alignment(CrossAxisAlignment::Start),
        )
        .background_color(UI_SURFACE_MUTED)
        .border(UI_BORDER, 1.0)
        .corner_radius(12.0)
        .padding(Padding::from_vh(12.0, 14.0)),
    ))
    .gap(10.px())
    .cross_axis_alignment(CrossAxisAlignment::Fill)
    .padding(Padding::from_vh(14.0, 20.0))
}

#[derive(Clone, Copy)]
enum PluginSettingField {
    BaseUrl,
    ApiKey,
    ChatModel,
    TranslationModel,
    TargetLanguage,
}

fn plugin_settings_content(settings: PluginSettings) -> impl WidgetView<DesktopReader> + use<> {
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
                .corner_radius(12.0),
            label("OpenAI 兼容服务")
                .font(UI_FONT_STACK)
                .text_size(12.0)
                .weight(FontWeight::BOLD)
                .color(UI_MUTED),
            flex_col((
                plugin_settings_input_row(
                    "API 地址",
                    settings.base_url,
                    "https://api.openai.com/v1",
                    PluginSettingField::BaseUrl,
                ),
                divider(),
                plugin_settings_input_row(
                    "API Key（仅本次会话）",
                    settings.api_key,
                    "sk-… 或 REBOOK_AI_API_KEY",
                    PluginSettingField::ApiKey,
                ),
                divider(),
                plugin_settings_input_row(
                    "对话模型",
                    settings.chat_model,
                    "gpt-4o-mini",
                    PluginSettingField::ChatModel,
                ),
                divider(),
                plugin_settings_input_row(
                    "翻译模型",
                    settings.translation_model,
                    "gpt-4o-mini",
                    PluginSettingField::TranslationModel,
                ),
                divider(),
                plugin_settings_input_row(
                    "目标语言",
                    settings.target_language,
                    "简体中文",
                    PluginSettingField::TargetLanguage,
                ),
            ))
            .cross_axis_alignment(CrossAxisAlignment::Fill)
            .background_color(UI_SURFACE)
            .border(UI_BORDER, 1.0)
            .corner_radius(12.0),
            prose("API Key 只保存在当前运行内存中，不会写入 plugins.json；也可以通过 REBOOK_AI_API_KEY 环境变量提供。")
                .text_size(10.5)
                .text_color(UI_MUTED),
        ))
        .gap(10.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::from_vh(14.0, 20.0)),
    )
}

fn plugin_settings_input_row(
    label_text: &'static str,
    value: String,
    placeholder: &'static str,
    field: PluginSettingField,
) -> impl WidgetView<DesktopReader> {
    sized_box(
        flex_row((
            label(label_text)
                .font(UI_FONT_STACK)
                .text_size(12.0)
                .color(UI_TEXT_SOFT),
            FlexSpacer::Flex(1.0),
            sized_box(
                text_input(value, move |state: &mut DesktopReader, value| match field {
                    PluginSettingField::BaseUrl => {
                        state.ui.draft_plugin_settings.base_url = value;
                    }
                    PluginSettingField::ApiKey => {
                        state.ui.draft_plugin_settings.api_key = value;
                    }
                    PluginSettingField::ChatModel => {
                        state.ui.draft_plugin_settings.chat_model = value;
                    }
                    PluginSettingField::TranslationModel => {
                        state.ui.draft_plugin_settings.translation_model = value;
                    }
                    PluginSettingField::TargetLanguage => {
                        state.ui.draft_plugin_settings.target_language = value;
                    }
                })
                .placeholder(placeholder)
                .text_color(UI_TEXT)
                .background_color(UI_SURFACE_MUTED)
                .border_color(UI_BORDER)
                .border_width(1.0)
                .corner_radius(7.0)
                .padding(Padding::horizontal(8.0)),
            )
            .width(250.px())
            .height(34.px()),
        ))
        .gap(10.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(48.px())
    .expand_width()
    .padding(Padding::horizontal(12.0))
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
    .height(64.px())
    .expand_width()
    .padding(Padding::horizontal(16.0))
}

fn font_choice(
    text: &'static str,
    value: ReaderFontFamily,
    selected: ReaderFontFamily,
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
            move |state: &mut DesktopReader| state.ui.draft_font_family = value,
        )
        .background_color(if active { UI_ACCENT_SOFT } else { UI_SURFACE })
        .active_background_color(UI_ACCENT_SOFT)
        .border_color(if active { UI_ACCENT_BORDER } else { UI_BORDER })
        .hovered_border_color(UI_ACCENT_BORDER)
        .corner_radius(8.0)
        .padding(Padding::from_vh(6.0, 8.0)),
    )
    .width(104.px())
    .height(34.px())
}

fn font_size_choice(value: f32, selected: f32) -> impl WidgetView<DesktopReader> {
    let active = (value - selected).abs() < f32::EPSILON;
    let text = format!("{value:.0}");
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
            move |state: &mut DesktopReader| state.ui.draft_font_size = value,
        )
        .background_color(if active { UI_ACCENT_SOFT } else { UI_SURFACE })
        .active_background_color(UI_ACCENT_SOFT)
        .border_color(if active { UI_ACCENT_BORDER } else { UI_BORDER })
        .hovered_border_color(UI_ACCENT_BORDER)
        .corner_radius(8.0)
        .padding(0.0),
    )
    .width(42.px())
    .height(32.px())
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
    .height(64.px())
    .expand_width()
    .padding(Padding::horizontal(16.0))
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
        .corner_radius(8.0)
        .padding(Padding::from_vh(6.0, 10.0)),
    )
    .width(62.px())
    .height(34.px())
}

fn value_badge(text: &'static str) -> impl WidgetView<DesktopReader> {
    sized_box(label(text).text_size(12.0).color(UI_TEXT_SOFT))
        .height(34.px())
        .background_color(UI_SURFACE)
        .border(UI_BORDER, 1.0)
        .corner_radius(8.0)
        .padding(Padding::from_vh(7.0, 12.0))
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
        .corner_radius(8.0)
        .padding(Padding::from_vh(7.0, 14.0)),
    )
    .height(36.px())
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
            .corner_radius(8.0)
            .padding(Padding::from_vh(7.0, 12.0)),
    )
    .height(36.px())
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

fn divider() -> impl WidgetView<DesktopReader> {
    sized_box(label(""))
        .height(1.px())
        .expand_width()
        .background_color(UI_BORDER)
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
            draft_font_family: ReaderFontFamily::Serif,
            draft_font_size: 16.0,
            draft_plugin_settings: PluginSettings::default(),
            assistant_panel: None,
            toolbar_motion: Motion::settled_with_duration(0.0, TOOLBAR_MOTION_DURATION),
            sidebar_motion: Motion::settled(0.0),
            menu_motion: Motion::settled(0.0),
            settings_motion: Motion::settled(0.0),
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
    fn shelf_search_matches_title_author_and_source_file() {
        let book = LibraryBook {
            id: "book-id".into(),
            title: "系统之美".into(),
            authors: vec!["Donella Meadows".into()],
            file_name: "thinking-in-systems.epub".into(),
            format: BookFormat::Epub,
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
}
