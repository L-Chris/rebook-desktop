use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rebook_formats::{BookFormat, open_file as open_publication_file};
use rebook_layout::{LayoutViewport, ReaderStyle};
use rebook_publication::{BookSource, SourceRange};
use rebook_reader::{PageDirection, ReaderSelection, ReaderSession, ReaderSnapshot, ReaderTextHit};
use xilem::Color;
use xilem::masonry::peniko::{Blob, ImageData};

use crate::async_task::{TaskResult, TaskSlot};
use crate::highlights::{HighlightStore, StoredHighlight};
use crate::library::LibraryBook;
use crate::plugins::{
    BlockTranslation, BookSearchResult, ChatResponse, ChatTurn, PluginSettings, RewriteBookSource,
    TranslationBlockInput, TranslationBookSource,
};
use crate::preferences::{self, AppLanguage, ReaderPreferences};
use crate::sync::{SyncSettings, SyncStore};
use crate::ui::decode_image;

const INITIAL_WIDTH: u32 = 1200;
const INITIAL_HEIGHT: u32 = 800;
const MOTION_DURATION: Duration = Duration::from_millis(180);
const TOOLBAR_MOTION_DURATION: Duration = Duration::from_millis(200);
const TOOLBAR_HIDE_DELAY: Duration = Duration::from_millis(500);
const NOTICE_AUTO_DISMISS_DELAY: Duration = Duration::from_secs(3);
const MOTION_EPSILON: f32 = 0.001;
const SEARCH_MARK_COLOR: Color = Color::from_rgba8(250, 204, 21, 89);
const ASSISTANT_MARK_COLOR: Color = Color::from_rgba8(245, 158, 11, 56);

mod assistant;
mod assistant_view;
mod interaction;
mod navigation;
pub(super) mod render;
mod settings_controller;
mod ui_controller;
mod view;

use render::{PageSceneKey, PageSceneLayers};
pub(super) use view::app_view;

pub(super) fn open_reader(
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
        .and_then(|bytes| decode_image(bytes).ok());
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
    let highlight_store = HighlightStore::from_repository(local_store.clone())?;
    let highlights = highlight_store.for_book(&book_id);
    let viewport = LayoutViewport::new(INITIAL_WIDTH, INITIAL_HEIGHT)?;
    let reader_preferences = preferences::load_reader_preferences().unwrap_or_else(|error| {
        tracing::warn!(%error, "failed to load reader preferences; using defaults");
        ReaderPreferences::default()
    });
    let style = ReaderStyle {
        spread: reader_preferences.spread,
        typography: reader_preferences.typography.clone(),
        ..ReaderStyle::default()
    };
    let sync_settings = SyncSettings::load_default().unwrap_or_else(|error| {
        tracing::warn!(%error, "failed to load WebDAV settings; using defaults");
        SyncSettings::new_device()
    });
    let sync_password = sync_settings.load_password().unwrap_or_else(|error| {
        tracing::warn!(%error, "failed to load WebDAV credential");
        String::new()
    });
    let mut reader =
        ReaderSession::open_with_fonts(Arc::clone(&source), viewport, style, reader_fonts)?;
    let progress_store = Some(local_store);
    if let Some(store) = &progress_store
        && let Some(progress) = store.load_progress(&book_id)?
        && let Err(error) = reader.restore_locator(&progress.locator)
    {
        tracing::warn!(%error, "failed to restore durable reading locator");
    }
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
            language: reader_preferences.language,
            sync_settings,
            sync_password,
        },
    ))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct BookDisplayMetadata {
    pub(super) title: String,
    pub(super) authors: Vec<String>,
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

pub(super) struct DesktopReader {
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
    language: AppLanguage,
    sync_settings: SyncSettings,
    sync_password: String,
    search: SearchUiState,
    chat: ChatUiState,
    translation: TranslationUiState,
    ui: ReaderUiState,
    canvas_size: Option<(u32, u32)>,
    scene_revision: u64,
    page_scenes: HashMap<PageSceneKey, Arc<PageSceneLayers>>,
    page_scene_lru: VecDeque<PageSceneKey>,
    pending_page_turn: Option<PageDirection>,
    settings_requested: bool,
    error: Option<String>,
    pub(super) exit_requested: bool,
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
    language: AppLanguage,
    sync_settings: SyncSettings,
    sync_password: String,
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
    response_language: String,
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
    assistant_panel: Option<AssistantPanel>,
    toolbar_motion: Motion,
    sidebar_motion: Motion,
    menu_motion: Motion,
    last_motion_tick: Option<Instant>,
    expanded_toc: HashSet<String>,
}

impl ReaderUiState {
    fn is_animating(&self) -> bool {
        self.toolbar_motion.is_animating()
            || self.sidebar_motion.is_animating()
            || self.menu_motion.is_animating()
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
        self.menu_motion.is_visible()
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
            language,
            sync_settings,
            sync_password,
        } = resources;
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
                assistant_panel: None,
                toolbar_motion: Motion::settled_with_duration(0.0, TOOLBAR_MOTION_DURATION),
                sidebar_motion: Motion::settled(1.0),
                menu_motion: Motion::settled(0.0),
                last_motion_tick: None,
                expanded_toc,
            },
            plugin_settings,
            language,
            sync_settings,
            sync_password,
            canvas_size: None,
            scene_revision: 0,
            page_scenes: HashMap::new(),
            page_scene_lru: VecDeque::new(),
            pending_page_turn: None,
            settings_requested: false,
            error,
            exit_requested: false,
        }
    }
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
    use super::{
        BookDisplayMetadata, HashSet, Instant, MOTION_DURATION, Motion, NOTICE_AUTO_DISMISS_DELAY,
        ReaderOverlay, ReaderUiState, SidebarTab, TOOLBAR_HIDE_DELAY, TOOLBAR_MOTION_DURATION,
        TranslationUiState, logical_dimension, resolve_book_display_metadata,
    };

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
            assistant_panel: None,
            toolbar_motion: Motion::settled_with_duration(0.0, TOOLBAR_MOTION_DURATION),
            sidebar_motion: Motion::settled(0.0),
            menu_motion: Motion::settled(0.0),
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
