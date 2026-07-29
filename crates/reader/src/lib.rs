//! Reader session with section, layout, and display-list caches.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::{self, JoinHandle};

use rebook_layout::{
    LayoutEngine, LayoutError, LayoutViewport, PageItem, ReaderFontBlob, ReaderStyle,
};
use rebook_publication::{
    Block, Book, BookSource, Inline, LocatorV1, PublicationError, PublicationUrl, Section,
    SourceAnchor, SourceRange, TextBlock, TextRun, TocEntry,
};
use rebook_renderer::{DisplayListCompiler, PageDisplayList, PageTextHit};
use thiserror::Error;

const PREFETCH_DISTANCE: usize = 2;
const DEFAULT_SEGMENT_CACHE_CAPACITY: usize = PREFETCH_DISTANCE * 2 + 3;
const FRAGMENT_TEXT_BUDGET: usize = 4_096;
const FRAGMENT_BLOCK_BUDGET: usize = 64;

/// Direction requested by keyboard, pointer, or command navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageDirection {
    Next,
    Previous,
}

/// Stable current position exposed to the application shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReaderLocation {
    pub section_index: usize,
    pub segment_index: usize,
    pub segment_count: usize,
    pub page_index: usize,
    pub page_count: usize,
}

/// Resolved random-access destination in the current pagination generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReaderPosition {
    pub section_index: usize,
    pub segment_index: usize,
    pub page_index: usize,
}

/// A pointer-resolved text position tied to the current pagination generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReaderTextHit {
    position: ReaderPosition,
    region_index: usize,
    byte_index: usize,
}

/// Page-coordinate rectangle used to paint a native selection and anchor its
/// floating action toolbar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReaderSelectionRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Durable source ranges plus transient geometry for the active native text
/// selection. Each range belongs to one source-backed text block.
#[derive(Debug, Clone, PartialEq)]
pub struct ReaderSelection {
    pub ranges: Vec<SourceRange>,
    pub text: String,
    pub rects: Vec<ReaderSelectionRect>,
}

/// One source-backed text fragment retained on a logical page in the current
/// visible spread. The source range remains stable while `position` identifies
/// the page that supplied the visible quote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReaderVisibleTextFragment {
    pub position: ReaderPosition,
    pub range: SourceRange,
    pub text: String,
}

/// One visual reader surface assembled from adjacent logical pages. In double
/// mode the secondary page may come from the next layout segment or authored
/// spine section.
pub struct ReaderSpread {
    pub primary: Arc<PageDisplayList>,
    pub secondary: Option<Arc<PageDisplayList>>,
    pub primary_offset_x: f32,
    pub secondary_offset_x: f32,
}

/// Flattened, presentation-ready table-of-contents item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocViewItem {
    pub id: String,
    pub label: String,
    pub target: Option<PublicationUrl>,
    pub depth: usize,
    pub ancestors: Vec<String>,
    pub has_children: bool,
}

/// Complete reader state after a command has been applied.
#[derive(Debug, Clone, PartialEq)]
pub struct ReaderSnapshot {
    pub location: ReaderLocation,
    pub total_progression: f64,
    pub active_toc_id: Option<String>,
    pub active_toc_path: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationOutcome {
    Moved,
    Boundary,
}

/// Navigation always returns the resulting state, including at book boundaries.
#[derive(Debug, Clone, PartialEq)]
pub struct NavigationResult {
    pub outcome: NavigationOutcome,
    pub snapshot: ReaderSnapshot,
}

/// Result of an interactive navigation attempt. A pending result means the
/// destination is being prepared by the background pagination worker and the
/// caller should retry without blocking its event loop.
#[derive(Debug, Clone, PartialEq)]
pub enum NavigationAttempt {
    Ready(NavigationResult),
    Pending,
}

enum PositionAttempt {
    Ready(Option<ReaderPosition>),
    Pending,
}

struct CachedSegment {
    section: Arc<PreparedSection>,
    pages: Vec<Arc<PageDisplayList>>,
    anchor_pages: HashMap<String, usize>,
    visible_pages: usize,
    continuation_offset_x: f32,
}

struct PreparedSection {
    fragments: Vec<ContentFragment>,
    segments: Vec<LayoutSegment>,
    anchor_segments: HashMap<String, usize>,
}

struct ContentFragment {
    blocks: Vec<Block>,
    anchors: Vec<rebook_publication::SectionAnchor>,
}

struct LayoutSegment {
    fragment_range: Range<usize>,
}

struct SectionRepository {
    source: Arc<dyn BookSource>,
    sections: Vec<SectionSlot>,
}

struct SectionSlot {
    state: Mutex<SectionSlotState>,
    ready: Condvar,
}

enum SectionSlotState {
    Empty,
    Loading,
    Ready(Weak<PreparedSection>),
}

impl SectionRepository {
    fn new(source: Arc<dyn BookSource>) -> Self {
        let section_count = source.book().sections.len();
        Self {
            source,
            sections: (0..section_count)
                .map(|_| SectionSlot {
                    state: Mutex::new(SectionSlotState::Empty),
                    ready: Condvar::new(),
                })
                .collect(),
        }
    }

    fn get(&self, index: usize) -> Option<Arc<PreparedSection>> {
        let slot = self.sections.get(index)?;
        let state = slot.state.lock().ok()?;
        match &*state {
            SectionSlotState::Ready(section) => section.upgrade(),
            SectionSlotState::Empty | SectionSlotState::Loading => None,
        }
    }

    fn load(&self, index: usize) -> Result<Arc<PreparedSection>, ReaderError> {
        let slot = self
            .sections
            .get(index)
            .ok_or(ReaderError::SectionOutOfBounds(index))?;
        loop {
            let mut state = slot
                .state
                .lock()
                .map_err(|_| ReaderError::SectionRepositoryPoisoned)?;
            match &*state {
                SectionSlotState::Ready(section) => {
                    if let Some(section) = section.upgrade() {
                        return Ok(section);
                    }
                    *state = SectionSlotState::Loading;
                }
                SectionSlotState::Empty => *state = SectionSlotState::Loading,
                SectionSlotState::Loading => {
                    drop(
                        slot.ready
                            .wait(state)
                            .map_err(|_| ReaderError::SectionRepositoryPoisoned)?,
                    );
                    continue;
                }
            }
            drop(state);

            let parsed = self.source.parse_section(index).map(prepare_section);
            let mut state = slot
                .state
                .lock()
                .map_err(|_| ReaderError::SectionRepositoryPoisoned)?;
            match parsed {
                Ok(section) => {
                    let section = Arc::new(section);
                    *state = SectionSlotState::Ready(Arc::downgrade(&section));
                    slot.ready.notify_all();
                    return Ok(section);
                }
                Err(error) => {
                    *state = SectionSlotState::Empty;
                    slot.ready.notify_all();
                    return Err(error.into());
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SegmentKey {
    section_index: usize,
    segment_index: usize,
}

struct PrefetchRequest {
    key: SegmentKey,
    viewport: LayoutViewport,
    style: ReaderStyle,
    generation: u64,
}

struct PrefetchResult {
    key: SegmentKey,
    generation: u64,
    segment: Result<Arc<CachedSegment>, ReaderError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PrefetchKey {
    generation: u64,
    segment: SegmentKey,
}

struct PrefetchWorker {
    requests: Option<Sender<PrefetchRequest>>,
    results: Mutex<Receiver<PrefetchResult>>,
    active_generation: Arc<AtomicU64>,
    cancelled: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl PrefetchWorker {
    fn spawn(
        source: Arc<dyn BookSource>,
        repository: Arc<SectionRepository>,
        fonts: Arc<[ReaderFontBlob]>,
    ) -> Result<Self, ReaderError> {
        let (request_sender, request_receiver) = mpsc::channel::<PrefetchRequest>();
        let (result_sender, result_receiver) = mpsc::channel::<PrefetchResult>();
        let active_generation = Arc::new(AtomicU64::new(0));
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_generation = Arc::clone(&active_generation);
        let worker_cancelled = Arc::clone(&cancelled);
        let handle = thread::Builder::new()
            .name("rebook-prefetch".into())
            .spawn(move || {
                let mut layout_engine = LayoutEngine::with_fonts(fonts.iter().cloned());
                let display_compiler = DisplayListCompiler;
                while let Ok(request) = request_receiver.recv() {
                    if worker_cancelled.load(Ordering::Acquire) {
                        break;
                    }
                    if worker_generation.load(Ordering::Acquire) != request.generation {
                        continue;
                    }
                    let segment = repository
                        .load(request.key.section_index)
                        .and_then(|section| {
                            compile_segment(
                                source.as_ref(),
                                section,
                                request.key,
                                request.viewport,
                                &request.style,
                                &mut layout_engine,
                                &display_compiler,
                            )
                            .map(Arc::new)
                        });
                    if worker_cancelled.load(Ordering::Acquire) {
                        break;
                    }
                    if worker_generation.load(Ordering::Acquire) != request.generation {
                        continue;
                    }
                    if result_sender
                        .send(PrefetchResult {
                            key: request.key,
                            generation: request.generation,
                            segment,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(ReaderError::PrefetchWorkerStart)?;
        Ok(Self {
            requests: Some(request_sender),
            results: Mutex::new(result_receiver),
            active_generation,
            cancelled,
            handle: Some(handle),
        })
    }

    fn generation(&self) -> u64 {
        self.active_generation.load(Ordering::Acquire)
    }

    fn invalidate(&self) -> u64 {
        self.active_generation.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn send(&self, request: PrefetchRequest) -> Result<(), ReaderError> {
        self.requests
            .as_ref()
            .ok_or(ReaderError::PrefetchWorkerStopped)?
            .send(request)
            .map_err(|_| ReaderError::PrefetchWorkerStopped)
    }

    fn recv(&self) -> Result<PrefetchResult, ReaderError> {
        self.results
            .lock()
            .map_err(|_| ReaderError::PrefetchWorkerStopped)?
            .recv()
            .map_err(|_| ReaderError::PrefetchWorkerStopped)
    }

    fn try_recv(&self) -> Result<PrefetchResult, TryRecvError> {
        self.results
            .lock()
            .map_or(Err(TryRecvError::Disconnected), |results| {
                results.try_recv()
            })
    }
}

impl Drop for PrefetchWorker {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.requests.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Single-owner reader orchestration. The parser and renderer communicate only
/// through the publication and layout IR crates.
pub struct ReaderSession {
    source: Arc<dyn BookSource>,
    repository: Arc<SectionRepository>,
    fonts: Arc<[ReaderFontBlob]>,
    layout_engine: LayoutEngine,
    display_compiler: DisplayListCompiler,
    viewport: LayoutViewport,
    style: ReaderStyle,
    toc_items: Arc<[TocViewItem]>,
    cache_capacity: usize,
    cache: HashMap<SegmentKey, Arc<CachedSegment>>,
    lru: VecDeque<SegmentKey>,
    prefetch_worker: PrefetchWorker,
    prefetch_inflight: HashSet<PrefetchKey>,
    prefetch_failures: HashMap<SegmentKey, ReaderError>,
    current_section: usize,
    current_segment: usize,
    current_page: usize,
}

impl ReaderSession {
    /// Opens the first section and compiles its pages once.
    pub fn open(
        source: Arc<dyn BookSource>,
        viewport: LayoutViewport,
        style: ReaderStyle,
    ) -> Result<Self, ReaderError> {
        Self::open_with_fonts(source, viewport, style, Arc::default())
    }

    /// Opens a reader with application-provided fonts registered in both the
    /// foreground and background pagination engines.
    pub fn open_with_fonts(
        source: Arc<dyn BookSource>,
        viewport: LayoutViewport,
        style: ReaderStyle,
        fonts: Arc<[ReaderFontBlob]>,
    ) -> Result<Self, ReaderError> {
        if source.book().sections.is_empty() {
            return Err(ReaderError::EmptyBook);
        }
        let toc_items = flatten_toc(&source.book().table_of_contents).into();
        let repository = Arc::new(SectionRepository::new(Arc::clone(&source)));
        let prefetch_worker = PrefetchWorker::spawn(
            Arc::clone(&source),
            Arc::clone(&repository),
            Arc::clone(&fonts),
        )?;
        let mut session = Self {
            source,
            repository,
            layout_engine: LayoutEngine::with_fonts(fonts.iter().cloned()),
            fonts,
            display_compiler: DisplayListCompiler,
            viewport,
            style,
            toc_items,
            cache_capacity: DEFAULT_SEGMENT_CACHE_CAPACITY,
            cache: HashMap::new(),
            lru: VecDeque::new(),
            prefetch_worker,
            prefetch_inflight: HashSet::new(),
            prefetch_failures: HashMap::new(),
            current_section: 0,
            current_segment: 0,
            current_page: 0,
        };
        session.ensure_segment(SegmentKey {
            section_index: 0,
            segment_index: 0,
        })?;
        Ok(session)
    }

    pub fn book(&self) -> &Book {
        self.source.book()
    }

    pub fn viewport(&self) -> LayoutViewport {
        self.viewport
    }

    pub fn style(&self) -> ReaderStyle {
        self.style.clone()
    }

    pub fn available_font_families(&mut self) -> Vec<String> {
        self.layout_engine.available_font_families()
    }

    pub fn toc_items(&self) -> &[TocViewItem] {
        &self.toc_items
    }

    pub fn location(&self) -> ReaderLocation {
        let segment_count = self.current_section_data().segments.len();
        ReaderLocation {
            section_index: self.current_section,
            segment_index: self.current_segment,
            segment_count,
            page_index: self.current_page,
            page_count: self.current_page_count(),
        }
    }

    pub fn snapshot(&self) -> ReaderSnapshot {
        let location = self.location();
        let active_toc = active_toc_item_for_location(
            &self.toc_items,
            location.section_index,
            location.segment_index,
            location.page_index,
            |target| self.position_for_href(target),
        );
        let (active_toc_id, active_toc_path) = active_toc.map_or_else(
            || (None, Vec::new()),
            |item| {
                let mut path = item.ancestors.clone();
                if item.has_children {
                    path.push(item.id.clone());
                }
                (Some(item.id.clone()), path)
            },
        );
        ReaderSnapshot {
            location,
            total_progression: total_progression(location, self.source.book().sections.len()),
            active_toc_id,
            active_toc_path,
        }
    }

    /// Captures a durable, versioned locator for the first visible content.
    #[allow(clippy::cast_precision_loss)]
    pub fn current_locator(&self) -> LocatorV1 {
        let location = self.location();
        let segment_count = location.segment_count.max(1);
        let page_progression = if location.page_count <= 1 {
            0.0
        } else {
            location.page_index as f64 / (location.page_count - 1) as f64
        };
        let progression = (location.segment_index as f64 + page_progression) / segment_count as f64;
        let section = &self.source.book().sections[location.section_index];
        LocatorV1 {
            version: LocatorV1::VERSION,
            publication_id: self.source.book().id.clone(),
            href: section.href.clone(),
            progression: Some(progression.clamp(0.0, 1.0)),
            total_progression: Some(self.snapshot().total_progression),
            position: None,
            source: self.current_page().leading_source_range(),
            partial_cfi: None,
            text: None,
        }
    }

    /// Restores a durable locator, preferring source anchors over layout-relative
    /// progression so typography and viewport changes do not move the reader.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss
    )]
    pub fn restore_locator(
        &mut self,
        locator: &LocatorV1,
    ) -> Result<NavigationResult, ReaderError> {
        locator.validate()?;
        if locator.publication_id != self.source.book().id {
            return Err(ReaderError::NavigationTargetNotFound(
                locator.publication_id.to_string(),
            ));
        }
        if let Some(source) = &locator.source
            && let Ok(result) = self.go_to_source(&source.start)
        {
            return Ok(result);
        }

        let (section_index, progression) =
            if let Some(index) = self.section_index_for_href(&locator.href) {
                (index, locator.progression.unwrap_or(0.0))
            } else if let Some(total) = locator.total_progression {
                let section_count = self.source.book().sections.len();
                let scaled = total.clamp(0.0, 1.0) * section_count as f64;
                let index = (scaled.floor() as usize).min(section_count.saturating_sub(1));
                (index, if total >= 1.0 { 1.0 } else { scaled.fract() })
            } else {
                return self.go_to_href(&locator.href);
            };
        let section = self.repository.load(section_index)?;
        let segment_count = section.segments.len().max(1);
        let scaled = progression.clamp(0.0, 1.0) * segment_count as f64;
        let segment_index = if progression >= 1.0 {
            segment_count - 1
        } else {
            (scaled.floor() as usize).min(segment_count - 1)
        };
        let key = SegmentKey {
            section_index,
            segment_index,
        };
        self.ensure_segment(key)?;
        let page_count = self
            .cache
            .get(&key)
            .map_or(1, |segment| segment.pages.len().max(1));
        let within_segment = if progression >= 1.0 {
            1.0
        } else {
            scaled.fract()
        };
        let page_index = if page_count <= 1 {
            0
        } else {
            (within_segment * (page_count - 1) as f64).round() as usize
        };
        self.install_position(ReaderPosition {
            section_index,
            segment_index,
            page_index,
        });
        Ok(self.moved())
    }

    /// Returns the compiled display list for the current page.
    ///
    /// # Panics
    ///
    /// Panics if the reader's internal invariant is broken and the current
    /// section or page is missing from the cache.
    pub fn current_page(&self) -> &PageDisplayList {
        self.cache
            .get(&self.current_key())
            .expect("current layout segment must remain cached")
            .pages[self.current_page]
            .as_ref()
    }

    /// Returns the logical pages visible in the current reader viewport.
    /// Adjacent content is resolved across layout-segment and spine-section
    /// boundaries so those implementation boundaries never create a blank
    /// right page.
    pub fn current_spread(&mut self) -> Result<ReaderSpread, ReaderError> {
        self.poll_prefetch()?;
        let position = self.current_position();
        let (primary, visible_pages, secondary_offset_x) = self
            .cache
            .get(&self.current_key())
            .and_then(|segment| {
                segment.pages.get(self.current_page).map(|page| {
                    (
                        Arc::clone(page),
                        segment.visible_pages,
                        segment.continuation_offset_x,
                    )
                })
            })
            .ok_or(ReaderError::PageOutOfBounds(position))?;
        let secondary = if visible_pages > 1 {
            self.next_position(position)?
                .map(|position| self.page_at(position))
                .transpose()?
        } else {
            None
        };
        let (primary_offset_x, secondary_offset_x) = resolve_spread_offsets(
            &primary,
            secondary.as_deref(),
            secondary_offset_x,
            self.style.column_gap == 0.0,
        );
        Ok(ReaderSpread {
            primary,
            secondary,
            primary_offset_x,
            secondary_offset_x,
        })
    }

    /// Returns the authored spine sections represented by the currently
    /// visible logical pages, in visual order and without duplicates.
    pub fn current_spread_section_indices(&mut self) -> Result<Vec<usize>, ReaderError> {
        let mut indices = Vec::with_capacity(self.current_visible_pages());
        for (position, _, _) in self.current_spread_pages()? {
            if indices.last().copied() != Some(position.section_index) {
                indices.push(position.section_index);
            }
        }
        Ok(indices)
    }

    /// Returns the source-backed text actually retained on the currently
    /// visible logical pages. In double-page mode this includes both pages in
    /// visual order and excludes text outside the displayed spread.
    pub fn current_visible_text_fragments(
        &mut self,
    ) -> Result<Vec<ReaderVisibleTextFragment>, ReaderError> {
        let mut fragments = Vec::new();
        for (position, page, _) in self.current_spread_pages()? {
            for region_index in 0..page.text_region_count() {
                let Some(visible_range) = page.text_region_visible_range(region_index) else {
                    continue;
                };
                let Some(fragment) = page.selection_fragment(region_index, visible_range) else {
                    continue;
                };
                if fragment.quote.trim().is_empty() {
                    continue;
                }
                fragments.push(ReaderVisibleTextFragment {
                    position,
                    range: fragment.range,
                    text: fragment.quote,
                });
            }
        }
        Ok(fragments)
    }

    /// Resolves a canvas point against the currently visible logical pages.
    /// Exact hits start a selection; nearest hits extend an active drag through
    /// whitespace in the same page or across a two-page spread.
    pub fn hit_test_current_spread(
        &mut self,
        x: f32,
        y: f32,
        exact: bool,
    ) -> Result<Option<ReaderTextHit>, ReaderError> {
        let pages = self.current_spread_pages()?;
        if pages.is_empty() {
            return Ok(None);
        }
        if exact {
            return Ok(pages.iter().find_map(|(position, page, offset_x)| {
                page.hit_test_text(x - *offset_x, y, true)
                    .map(|hit| reader_text_hit(*position, hit))
            }));
        }
        let page_index = usize::from(pages.len() > 1 && x >= pages[1].2);
        let (position, page, offset_x) = &pages[page_index];
        Ok(page
            .hit_test_text(x - *offset_x, y, false)
            .map(|hit| reader_text_hit(*position, hit)))
    }

    /// Builds a source-backed selection between two pointer hits. Native
    /// selections are intentionally bounded to the currently visible spread;
    /// the returned per-block ranges remain stable after repagination.
    pub fn selection_between(
        &mut self,
        anchor: &ReaderTextHit,
        focus: &ReaderTextHit,
    ) -> Result<Option<ReaderSelection>, ReaderError> {
        let pages = self.current_spread_pages()?;
        let Some(anchor_page) = pages
            .iter()
            .position(|(position, _, _)| *position == anchor.position)
        else {
            return Ok(None);
        };
        let Some(focus_page) = pages
            .iter()
            .position(|(position, _, _)| *position == focus.position)
        else {
            return Ok(None);
        };
        let anchor_order = (anchor_page, anchor.region_index, anchor.byte_index);
        let focus_order = (focus_page, focus.region_index, focus.byte_index);
        let (start, end, start_page, end_page) = if anchor_order <= focus_order {
            (anchor, focus, anchor_page, focus_page)
        } else {
            (focus, anchor, focus_page, anchor_page)
        };

        let mut ranges = Vec::new();
        let mut quote = String::new();
        let mut rects = Vec::new();
        for (page_index, (_, page, offset_x)) in
            pages.iter().enumerate().take(end_page + 1).skip(start_page)
        {
            let first_region = if page_index == start_page {
                start.region_index
            } else {
                0
            };
            let last_region = if page_index == end_page {
                end.region_index
            } else {
                page.text_region_count().saturating_sub(1)
            };
            for region_index in first_region..=last_region {
                let Some(visible) = page.text_region_visible_range(region_index) else {
                    continue;
                };
                let byte_start = if page_index == start_page && region_index == start.region_index {
                    start.byte_index
                } else {
                    visible.start
                };
                let byte_end = if page_index == end_page && region_index == end.region_index {
                    end.byte_index
                } else {
                    visible.end
                };
                let Some(fragment) = page.selection_fragment(region_index, byte_start..byte_end)
                else {
                    continue;
                };
                append_selection_quote(&mut quote, &fragment.quote);
                push_source_range(&mut ranges, fragment.range);
                rects.extend(fragment.rects.into_iter().map(|rect| ReaderSelectionRect {
                    x: logical_coordinate(rect.x0) + *offset_x,
                    y: logical_coordinate(rect.y0),
                    width: logical_coordinate(rect.width()),
                    height: logical_coordinate(rect.height()),
                }));
            }
        }
        if ranges.is_empty() || quote.trim().is_empty() || rects.is_empty() {
            return Ok(None);
        }
        Ok(Some(ReaderSelection {
            ranges,
            text: quote,
            rects,
        }))
    }

    /// Returns whether a canvas point falls inside the resolved geometry for a
    /// set of durable source ranges on the current spread.
    pub fn source_ranges_contain_point(
        &mut self,
        ranges: &[SourceRange],
        x: f32,
        y: f32,
    ) -> Result<bool, ReaderError> {
        Ok(self
            .current_spread_pages()?
            .iter()
            .any(|(_, page, offset_x)| page.source_ranges_contain_point(ranges, x - *offset_x, y)))
    }

    /// Navigates to a durable source anchor, resolving its page again under
    /// the current viewport and reader style.
    pub fn go_to_source(&mut self, anchor: &SourceAnchor) -> Result<NavigationResult, ReaderError> {
        let section_index = self
            .source
            .book()
            .sections
            .iter()
            .position(|section| section.id == anchor.spine)
            .ok_or_else(|| ReaderError::NavigationTargetNotFound(anchor.node.clone()))?;
        let section = self.repository.load(section_index)?;
        let fragment_index = section
            .fragments
            .iter()
            .position(|fragment| {
                fragment.blocks.iter().any(|block| {
                    block_source(block).is_some_and(|range| source_range_contains(range, anchor))
                })
            })
            .unwrap_or(0);
        let segment_index = section
            .segments
            .iter()
            .position(|segment| segment.fragment_range.contains(&fragment_index))
            .unwrap_or(0);
        let key = SegmentKey {
            section_index,
            segment_index,
        };
        self.ensure_segment(key)?;
        let page_index = self
            .cache
            .get(&key)
            .and_then(|segment| {
                segment
                    .pages
                    .iter()
                    .position(|page| page.contains_source_anchor(anchor))
            })
            .unwrap_or(0);
        self.install_position(ReaderPosition {
            section_index,
            segment_index,
            page_index,
        });
        Ok(self.moved())
    }

    /// Resolves a publication URL to its containing spine section.
    pub fn section_index_for_href(&self, href: &PublicationUrl) -> Option<usize> {
        let resource = href.resource_url();
        self.source
            .book()
            .sections
            .iter()
            .position(|section| section.href.resource_url() == resource)
    }

    /// Resolves a publication URL to the layout segment and page containing its
    /// authored anchor.
    ///
    /// A missing or unknown fragment falls back to the beginning of the section,
    /// matching [`Self::go_to_href`]. Page indexes are available for compiled
    /// segments; the current segment is always compiled.
    pub fn position_for_href(&self, href: &PublicationUrl) -> Option<ReaderPosition> {
        let section_index = self.section_index_for_href(href)?;
        let section = self.repository.get(section_index);
        let segment_index = href
            .fragment()
            .and_then(|fragment| {
                section
                    .as_ref()
                    .and_then(|section| section.anchor_segments.get(fragment))
            })
            .copied()
            .unwrap_or(0);
        let key = SegmentKey {
            section_index,
            segment_index,
        };
        let page_index = href
            .fragment()
            .and_then(|fragment| {
                self.cache
                    .get(&key)
                    .and_then(|cached| cached.anchor_pages.get(fragment))
            })
            .copied()
            .unwrap_or(0);
        Some(ReaderPosition {
            section_index,
            segment_index,
            page_index,
        })
    }

    /// Navigates to the beginning of a spine section.
    pub fn go_to_section(&mut self, index: usize) -> Result<NavigationResult, ReaderError> {
        self.poll_prefetch()?;
        if index >= self.source.book().sections.len() {
            return Err(ReaderError::SectionOutOfBounds(index));
        }
        let key = SegmentKey {
            section_index: index,
            segment_index: 0,
        };
        self.ensure_segment(key)?;
        self.current_section = index;
        self.current_segment = 0;
        self.current_page = 0;
        self.touch(key);
        Ok(self.moved())
    }

    /// Navigates a TOC or link target to its authored anchor when available.
    pub fn go_to_href(&mut self, href: &PublicationUrl) -> Result<NavigationResult, ReaderError> {
        let index = self
            .section_index_for_href(href)
            .ok_or_else(|| ReaderError::NavigationTargetNotFound(href.to_string()))?;
        let section = self.repository.load(index)?;
        let segment_index = href
            .fragment()
            .and_then(|fragment| section.anchor_segments.get(fragment))
            .copied()
            .unwrap_or(0);
        let key = SegmentKey {
            section_index: index,
            segment_index,
        };
        self.ensure_segment(key)?;
        self.current_section = index;
        self.current_segment = segment_index;
        self.current_page = href
            .fragment()
            .and_then(|fragment| {
                self.cache
                    .get(&key)
                    .and_then(|cached| cached.anchor_pages.get(fragment))
            })
            .copied()
            .unwrap_or(0);
        self.touch(key);
        Ok(self.moved())
    }

    /// Moves in constant time while pages are cached. Section boundaries compile
    /// only the destination section, never the previous one again.
    pub fn turn_page(&mut self, direction: PageDirection) -> Result<NavigationResult, ReaderError> {
        self.poll_prefetch()?;
        match direction {
            PageDirection::Next => self.next_page(),
            PageDirection::Previous => self.previous_page(),
        }
    }

    /// Attempts to turn a page without parsing, laying out, or waiting on the
    /// caller thread. When the destination is not cached, this queues exactly
    /// the required segment and returns [`NavigationAttempt::Pending`].
    pub fn try_turn_page(
        &mut self,
        direction: PageDirection,
    ) -> Result<NavigationAttempt, ReaderError> {
        self.poll_prefetch()?;
        match direction {
            PageDirection::Next => self.try_next_page(),
            PageDirection::Previous => self.try_previous_page(),
        }
    }

    /// Queues a small layout-segment window around the current position for background
    /// pagination and display-list compilation. Crossing forward over an
    /// authored section boundary queues the start of the following sections.
    /// This method never performs layout on the caller thread.
    pub fn prefetch_adjacent(&mut self) -> Result<(), ReaderError> {
        self.poll_prefetch()?;
        let section_count = self.source.book().sections.len();
        let segment_count = self.current_section_data().segments.len();
        // A double spread advances by two logical pages. Queue the whole next
        // spread plus the normal lookahead so a turn never leaves its second
        // page for synchronous layout on the UI thread.
        let forward_distance = PREFETCH_DISTANCE + self.current_visible_pages().saturating_sub(1);
        for distance in 1..=forward_distance {
            if let Some(segment_index) = self.current_segment.checked_add(distance)
                && segment_index < segment_count
            {
                self.queue_prefetch(SegmentKey {
                    section_index: self.current_section,
                    segment_index,
                })?;
            } else {
                let overflow = self.current_segment + distance - segment_count;
                let section_index = self.current_section + overflow + 1;
                if section_index < section_count {
                    self.queue_prefetch(SegmentKey {
                        section_index,
                        segment_index: 0,
                    })?;
                }
            }
        }
        for distance in 1..=PREFETCH_DISTANCE {
            if let Some(segment_index) = self.current_segment.checked_sub(distance) {
                self.queue_prefetch(SegmentKey {
                    section_index: self.current_section,
                    segment_index,
                })?;
            }
        }
        self.touch(self.current_key());
        Ok(())
    }

    /// Blocks until all currently queued prefetch work has been collected.
    /// Intended for diagnostics and deterministic tests, not interactive shells.
    pub fn wait_for_prefetch(&mut self) -> Result<(), ReaderError> {
        let generation = self.prefetch_worker.generation();
        while self
            .prefetch_inflight
            .iter()
            .any(|key| key.generation == generation)
        {
            let result = self.prefetch_worker.recv()?;
            self.install_prefetch(result);
        }
        if let Some(key) = self.prefetch_failures.keys().next().copied()
            && let Some(error) = self.prefetch_failures.remove(&key)
        {
            return Err(error);
        }
        Ok(())
    }

    /// Invalidates layout/display caches while preserving approximate progress
    /// inside the active section.
    pub fn resize(&mut self, viewport: LayoutViewport) -> Result<ReaderSnapshot, ReaderError> {
        if self.viewport == viewport {
            return Ok(self.snapshot());
        }
        let old_count = self.current_page_count();
        let fraction = page_fraction(self.current_page, old_count);
        self.viewport = viewport;
        self.invalidate_layout(fraction)?;
        Ok(self.snapshot())
    }

    pub fn set_style(&mut self, style: ReaderStyle) -> Result<ReaderSnapshot, ReaderError> {
        if self.style == style {
            return Ok(self.snapshot());
        }
        let fraction = page_fraction(self.current_page, self.current_page_count());
        self.style = style;
        self.invalidate_layout(fraction)?;
        Ok(self.snapshot())
    }

    /// Reparses the publication through its current source layer, invalidates
    /// every parsed/layout cache, and preserves approximate progress inside the
    /// active section. This is used by non-persistent document overlays such as
    /// AI-assisted block rewrites.
    pub fn refresh_source(&mut self) -> Result<ReaderSnapshot, ReaderError> {
        let fraction = page_fraction(self.current_page, self.current_page_count());
        let repository = Arc::new(SectionRepository::new(Arc::clone(&self.source)));
        let section = repository.load(self.current_section)?;
        let segment_index = self
            .current_segment
            .min(section.segments.len().saturating_sub(1));
        let key = SegmentKey {
            section_index: self.current_section,
            segment_index,
        };
        let segment = compile_segment(
            self.source.as_ref(),
            section,
            key,
            self.viewport,
            &self.style,
            &mut self.layout_engine,
            &self.display_compiler,
        )?;
        let prefetch_worker = PrefetchWorker::spawn(
            Arc::clone(&self.source),
            Arc::clone(&repository),
            Arc::clone(&self.fonts),
        )?;

        self.repository = repository;
        self.prefetch_worker = prefetch_worker;
        self.prefetch_inflight.clear();
        self.prefetch_failures.clear();
        self.cache.clear();
        self.lru.clear();
        self.current_segment = segment_index;
        self.cache.insert(key, Arc::new(segment));
        self.touch(key);
        self.current_page = page_for_fraction(fraction, self.current_page_count());
        Ok(self.snapshot())
    }

    pub fn cached_segment_count(&self) -> usize {
        self.cache.len()
    }

    fn current_spread_pages(
        &mut self,
    ) -> Result<Vec<(ReaderPosition, Arc<PageDisplayList>, f32)>, ReaderError> {
        let position = self.current_position();
        let spread = self.current_spread()?;
        let mut pages = vec![(position, spread.primary, spread.primary_offset_x)];
        if let Some(secondary) = spread.secondary
            && let Some(position) = self.next_position(position)?
        {
            pages.push((position, secondary, spread.secondary_offset_x));
        }
        Ok(pages)
    }

    fn next_page(&mut self) -> Result<NavigationResult, ReaderError> {
        let mut destination = self.current_position();
        for _ in 0..self.current_visible_pages() {
            let Some(next) = self.next_position(destination)? else {
                return Ok(self.boundary());
            };
            destination = next;
        }
        self.install_position(destination);
        Ok(self.moved())
    }

    fn previous_page(&mut self) -> Result<NavigationResult, ReaderError> {
        let original = self.current_position();
        let mut destination = original;
        for _ in 0..self.current_visible_pages() {
            let Some(previous) = self.previous_position(destination)? else {
                break;
            };
            destination = previous;
        }
        if destination == original {
            return Ok(self.boundary());
        }
        self.install_position(destination);
        Ok(self.moved())
    }

    fn try_next_page(&mut self) -> Result<NavigationAttempt, ReaderError> {
        let mut destination = self.current_position();
        for _ in 0..self.current_visible_pages() {
            match self.try_next_position(destination)? {
                PositionAttempt::Ready(Some(next)) => destination = next,
                PositionAttempt::Ready(None) => {
                    return Ok(NavigationAttempt::Ready(self.boundary()));
                }
                PositionAttempt::Pending => return Ok(NavigationAttempt::Pending),
            }
        }
        if !self.try_spread_ready_at(destination)? {
            return Ok(NavigationAttempt::Pending);
        }
        self.install_position(destination);
        Ok(NavigationAttempt::Ready(self.moved()))
    }

    fn try_previous_page(&mut self) -> Result<NavigationAttempt, ReaderError> {
        let original = self.current_position();
        let mut destination = original;
        for _ in 0..self.current_visible_pages() {
            match self.try_previous_position(destination)? {
                PositionAttempt::Ready(Some(previous)) => destination = previous,
                PositionAttempt::Ready(None) => break,
                PositionAttempt::Pending => return Ok(NavigationAttempt::Pending),
            }
        }
        if destination == original {
            return Ok(NavigationAttempt::Ready(self.boundary()));
        }
        if !self.try_spread_ready_at(destination)? {
            return Ok(NavigationAttempt::Pending);
        }
        self.install_position(destination);
        Ok(NavigationAttempt::Ready(self.moved()))
    }

    fn try_spread_ready_at(&mut self, position: ReaderPosition) -> Result<bool, ReaderError> {
        let key = SegmentKey {
            section_index: position.section_index,
            segment_index: position.segment_index,
        };
        if !self.try_ensure_segment(key)? {
            return Ok(false);
        }
        let visible_pages = self
            .cache
            .get(&key)
            .expect("ready layout segment must remain cached")
            .visible_pages;
        if visible_pages <= 1 {
            return Ok(true);
        }
        Ok(matches!(
            self.try_next_position(position)?,
            PositionAttempt::Ready(_)
        ))
    }

    fn try_next_position(
        &mut self,
        position: ReaderPosition,
    ) -> Result<PositionAttempt, ReaderError> {
        let key = SegmentKey {
            section_index: position.section_index,
            segment_index: position.segment_index,
        };
        if !self.try_ensure_segment(key)? {
            return Ok(PositionAttempt::Pending);
        }
        let segment = self
            .cache
            .get(&key)
            .expect("ready layout segment must remain cached");
        if position.page_index + 1 < segment.pages.len() {
            return Ok(PositionAttempt::Ready(Some(ReaderPosition {
                page_index: position.page_index + 1,
                ..position
            })));
        }
        let next_key = if position.segment_index + 1 < segment.section.segments.len() {
            SegmentKey {
                section_index: position.section_index,
                segment_index: position.segment_index + 1,
            }
        } else {
            let section_index = position.section_index + 1;
            if section_index >= self.source.book().sections.len() {
                return Ok(PositionAttempt::Ready(None));
            }
            SegmentKey {
                section_index,
                segment_index: 0,
            }
        };
        if !self.try_ensure_segment(next_key)? {
            return Ok(PositionAttempt::Pending);
        }
        Ok(PositionAttempt::Ready(Some(ReaderPosition {
            section_index: next_key.section_index,
            segment_index: next_key.segment_index,
            page_index: 0,
        })))
    }

    fn try_previous_position(
        &mut self,
        position: ReaderPosition,
    ) -> Result<PositionAttempt, ReaderError> {
        if position.page_index > 0 {
            return Ok(PositionAttempt::Ready(Some(ReaderPosition {
                page_index: position.page_index - 1,
                ..position
            })));
        }
        let previous_key = if let Some(segment_index) = position.segment_index.checked_sub(1) {
            SegmentKey {
                section_index: position.section_index,
                segment_index,
            }
        } else {
            let Some(section_index) = position.section_index.checked_sub(1) else {
                return Ok(PositionAttempt::Ready(None));
            };
            let first_key = SegmentKey {
                section_index,
                segment_index: 0,
            };
            if !self.try_ensure_segment(first_key)? {
                return Ok(PositionAttempt::Pending);
            }
            let segment_index = self
                .cache
                .get(&first_key)
                .expect("ready layout segment must remain cached")
                .section
                .segments
                .len()
                .saturating_sub(1);
            SegmentKey {
                section_index,
                segment_index,
            }
        };
        if !self.try_ensure_segment(previous_key)? {
            return Ok(PositionAttempt::Pending);
        }
        let page_index = self
            .cache
            .get(&previous_key)
            .expect("ready layout segment must remain cached")
            .pages
            .len()
            .saturating_sub(1);
        Ok(PositionAttempt::Ready(Some(ReaderPosition {
            section_index: previous_key.section_index,
            segment_index: previous_key.segment_index,
            page_index,
        })))
    }

    fn next_position(
        &mut self,
        position: ReaderPosition,
    ) -> Result<Option<ReaderPosition>, ReaderError> {
        let key = SegmentKey {
            section_index: position.section_index,
            segment_index: position.segment_index,
        };
        self.ensure_segment(key)?;
        let segment = self
            .cache
            .get(&key)
            .expect("ensured layout segment must remain cached");
        if position.page_index + 1 < segment.pages.len() {
            return Ok(Some(ReaderPosition {
                page_index: position.page_index + 1,
                ..position
            }));
        }
        let next_key = if position.segment_index + 1 < segment.section.segments.len() {
            SegmentKey {
                section_index: position.section_index,
                segment_index: position.segment_index + 1,
            }
        } else {
            let section_index = position.section_index + 1;
            if section_index >= self.source.book().sections.len() {
                return Ok(None);
            }
            SegmentKey {
                section_index,
                segment_index: 0,
            }
        };
        self.ensure_segment(next_key)?;
        Ok(Some(ReaderPosition {
            section_index: next_key.section_index,
            segment_index: next_key.segment_index,
            page_index: 0,
        }))
    }

    fn previous_position(
        &mut self,
        position: ReaderPosition,
    ) -> Result<Option<ReaderPosition>, ReaderError> {
        if position.page_index > 0 {
            return Ok(Some(ReaderPosition {
                page_index: position.page_index - 1,
                ..position
            }));
        }
        let previous_key = if let Some(segment_index) = position.segment_index.checked_sub(1) {
            SegmentKey {
                section_index: position.section_index,
                segment_index,
            }
        } else {
            let Some(section_index) = position.section_index.checked_sub(1) else {
                return Ok(None);
            };
            let section = self.repository.load(section_index)?;
            SegmentKey {
                section_index,
                segment_index: section.segments.len().saturating_sub(1),
            }
        };
        self.ensure_segment(previous_key)?;
        let page_index = self
            .cache
            .get(&previous_key)
            .expect("ensured layout segment must remain cached")
            .pages
            .len()
            .saturating_sub(1);
        Ok(Some(ReaderPosition {
            section_index: previous_key.section_index,
            segment_index: previous_key.segment_index,
            page_index,
        }))
    }

    fn page_at(&self, position: ReaderPosition) -> Result<Arc<PageDisplayList>, ReaderError> {
        self.cache
            .get(&SegmentKey {
                section_index: position.section_index,
                segment_index: position.segment_index,
            })
            .and_then(|segment| segment.pages.get(position.page_index))
            .cloned()
            .ok_or(ReaderError::PageOutOfBounds(position))
    }

    fn current_position(&self) -> ReaderPosition {
        ReaderPosition {
            section_index: self.current_section,
            segment_index: self.current_segment,
            page_index: self.current_page,
        }
    }

    fn current_visible_pages(&self) -> usize {
        self.cache
            .get(&self.current_key())
            .map_or(1, |segment| segment.visible_pages.max(1))
    }

    fn install_position(&mut self, position: ReaderPosition) {
        self.current_section = position.section_index;
        self.current_segment = position.segment_index;
        self.current_page = position.page_index;
        self.touch(self.current_key());
    }

    fn ensure_segment(&mut self, key: SegmentKey) -> Result<(), ReaderError> {
        if self.cache.contains_key(&key) {
            self.touch(key);
            return Ok(());
        }
        if let Some(error) = self.prefetch_failures.remove(&key) {
            return Err(error);
        }
        let prefetch_key = PrefetchKey {
            generation: self.prefetch_worker.generation(),
            segment: key,
        };
        if self.prefetch_inflight.contains(&prefetch_key) {
            self.wait_for_segment(key)?;
            if self.cache.contains_key(&key) {
                self.touch(key);
                return Ok(());
            }
        }
        let section = self.repository.load(key.section_index)?;
        let segment = compile_segment(
            self.source.as_ref(),
            section,
            key,
            self.viewport,
            &self.style,
            &mut self.layout_engine,
            &self.display_compiler,
        )?;
        self.cache.insert(key, Arc::new(segment));
        self.touch(key);
        self.evict();
        Ok(())
    }

    fn try_ensure_segment(&mut self, key: SegmentKey) -> Result<bool, ReaderError> {
        if self.cache.contains_key(&key) {
            self.touch(key);
            return Ok(true);
        }
        if let Some(error) = self.prefetch_failures.remove(&key) {
            return Err(error);
        }
        let prefetch_key = PrefetchKey {
            generation: self.prefetch_worker.generation(),
            segment: key,
        };
        if !self.prefetch_inflight.contains(&prefetch_key) {
            self.queue_prefetch(key)?;
        }
        Ok(false)
    }

    fn invalidate_layout(&mut self, fraction: f32) -> Result<(), ReaderError> {
        self.prefetch_worker.invalidate();
        self.prefetch_inflight.clear();
        self.prefetch_failures.clear();
        let current_section = Arc::clone(self.current_section_data());
        self.cache.clear();
        self.lru.clear();
        let key = self.current_key();
        let segment = compile_segment(
            self.source.as_ref(),
            current_section,
            key,
            self.viewport,
            &self.style,
            &mut self.layout_engine,
            &self.display_compiler,
        )?;
        self.cache.insert(key, Arc::new(segment));
        self.touch(key);
        let count = self.current_page_count();
        self.current_page = page_for_fraction(fraction, count);
        Ok(())
    }

    fn queue_prefetch(&mut self, segment: SegmentKey) -> Result<(), ReaderError> {
        let key = PrefetchKey {
            generation: self.prefetch_worker.generation(),
            segment,
        };
        if self.cache.contains_key(&segment) || self.prefetch_inflight.contains(&key) {
            return Ok(());
        }
        self.prefetch_worker.send(PrefetchRequest {
            key: segment,
            viewport: self.viewport,
            style: self.style.clone(),
            generation: key.generation,
        })?;
        self.prefetch_inflight.insert(key);
        Ok(())
    }

    fn poll_prefetch(&mut self) -> Result<(), ReaderError> {
        loop {
            match self.prefetch_worker.try_recv() {
                Ok(result) => self.install_prefetch(result),
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Disconnected) if self.prefetch_inflight.is_empty() => {
                    return Ok(());
                }
                Err(TryRecvError::Disconnected) => {
                    return Err(ReaderError::PrefetchWorkerStopped);
                }
            }
        }
    }

    fn wait_for_segment(&mut self, segment: SegmentKey) -> Result<(), ReaderError> {
        let key = PrefetchKey {
            generation: self.prefetch_worker.generation(),
            segment,
        };
        while self.prefetch_inflight.contains(&key) {
            let result = self.prefetch_worker.recv()?;
            self.install_prefetch(result);
        }
        if let Some(error) = self.prefetch_failures.remove(&segment) {
            return Err(error);
        }
        Ok(())
    }

    fn install_prefetch(&mut self, result: PrefetchResult) {
        let key = PrefetchKey {
            generation: result.generation,
            segment: result.key,
        };
        self.prefetch_inflight.remove(&key);
        if result.generation != self.prefetch_worker.generation() {
            return;
        }
        let segment = match result.segment {
            Ok(segment) => segment,
            Err(error) => {
                self.prefetch_failures.insert(result.key, error);
                return;
            }
        };
        if self.cache.insert(result.key, segment).is_none() {
            self.touch(result.key);
            self.evict();
        }
        self.touch(self.current_key());
    }

    fn current_page_count(&self) -> usize {
        self.cache
            .get(&self.current_key())
            .map_or(0, |segment| segment.pages.len())
    }

    fn current_key(&self) -> SegmentKey {
        SegmentKey {
            section_index: self.current_section,
            segment_index: self.current_segment,
        }
    }

    fn current_section_data(&self) -> &Arc<PreparedSection> {
        &self
            .cache
            .get(&self.current_key())
            .expect("current layout segment must remain cached")
            .section
    }

    fn touch(&mut self, key: SegmentKey) {
        self.lru.retain(|cached| *cached != key);
        self.lru.push_back(key);
    }

    fn evict(&mut self) {
        while self.cache.len() > self.cache_capacity {
            let Some(candidate) = self.lru.pop_front() else {
                break;
            };
            if candidate == self.current_key() {
                self.lru.push_back(candidate);
                continue;
            }
            self.cache.remove(&candidate);
        }
    }

    fn moved(&self) -> NavigationResult {
        NavigationResult {
            outcome: NavigationOutcome::Moved,
            snapshot: self.snapshot(),
        }
    }

    fn boundary(&self) -> NavigationResult {
        NavigationResult {
            outcome: NavigationOutcome::Boundary,
            snapshot: self.snapshot(),
        }
    }
}

fn compile_segment(
    source: &dyn BookSource,
    section: Arc<PreparedSection>,
    key: SegmentKey,
    viewport: LayoutViewport,
    style: &ReaderStyle,
    layout_engine: &mut LayoutEngine,
    display_compiler: &DisplayListCompiler,
) -> Result<CachedSegment, ReaderError> {
    let segment =
        section
            .segments
            .get(key.segment_index)
            .ok_or(ReaderError::SegmentOutOfBounds {
                section: key.section_index,
                segment: key.segment_index,
            })?;
    let fragments = section.fragments[segment.fragment_range.clone()]
        .iter()
        .map(|fragment| fragment.blocks.as_slice())
        .collect::<Vec<_>>();
    let layout = layout_engine.layout_fragments(source, &fragments, viewport, style)?;
    let visible_pages = layout.visible_pages;
    let continuation_offset_x = layout.continuation_offset_x;
    let anchor_pages = section.fragments[segment.fragment_range.clone()]
        .iter()
        .flat_map(|fragment| &fragment.anchors)
        .filter_map(|anchor| {
            layout
                .pages
                .iter()
                .position(|page| {
                    page.items.iter().any(|item| {
                        let source = match item {
                            PageItem::Text(placement) => placement.source.as_ref(),
                            PageItem::Image(placement) => placement.source.as_ref(),
                            PageItem::Separator(_) => None,
                        };
                        source.is_some_and(|range| source_range_contains(range, &anchor.source))
                    })
                })
                .map(|page| (anchor.fragment.clone(), page))
        })
        .collect();
    let pages = layout
        .pages
        .iter()
        .map(|page| Arc::new(display_compiler.compile(page)))
        .collect();
    Ok(CachedSegment {
        section,
        pages,
        anchor_pages,
        visible_pages,
        continuation_offset_x,
    })
}

fn prepare_section(section: Section) -> PreparedSection {
    let Section {
        blocks, anchors, ..
    } = section;
    let mut block_groups = Vec::<Vec<Block>>::new();
    let mut current = Vec::new();
    let mut current_text = 0_usize;

    let flush =
        |current: &mut Vec<Block>, current_text: &mut usize, block_groups: &mut Vec<Vec<Block>>| {
            if !current.is_empty() {
                block_groups.push(std::mem::take(current));
                *current_text = 0;
            }
        };

    for block in blocks {
        let pieces = match block {
            Block::Text(block) => split_text_block(block)
                .into_iter()
                .map(Block::Text)
                .collect::<Vec<_>>(),
            block => vec![block],
        };
        for piece in pieces {
            let text_len = block_text_len(&piece);
            if !current.is_empty()
                && (current.len() >= FRAGMENT_BLOCK_BUDGET
                    || current_text.saturating_add(text_len) > FRAGMENT_TEXT_BUDGET)
            {
                flush(&mut current, &mut current_text, &mut block_groups);
            }
            current_text = current_text.saturating_add(text_len);
            current.push(piece);
            if current.len() >= FRAGMENT_BLOCK_BUDGET || current_text >= FRAGMENT_TEXT_BUDGET {
                flush(&mut current, &mut current_text, &mut block_groups);
            }
        }
    }
    flush(&mut current, &mut current_text, &mut block_groups);
    if block_groups.is_empty() {
        block_groups.push(Vec::new());
    }

    let mut fragments = block_groups
        .into_iter()
        .map(|blocks| ContentFragment {
            blocks,
            anchors: Vec::new(),
        })
        .collect::<Vec<_>>();
    let mut anchor_segments = HashMap::new();
    for anchor in anchors {
        let fragment_index = fragments
            .iter()
            .position(|fragment| {
                fragment.blocks.iter().any(|block| {
                    block_source(block)
                        .is_some_and(|range| source_range_contains(range, &anchor.source))
                })
            })
            .unwrap_or(0);
        anchor_segments.insert(anchor.fragment.clone(), 0);
        fragments[fragment_index].anchors.push(anchor);
    }

    // An authored spine section must be paginated as one continuous flow.
    // Splitting its fragments into independently paginated layout segments
    // turns every internal cache boundary into an implicit page break and can
    // leave a large unused area at the bottom of the preceding page.
    let segments = vec![LayoutSegment {
        fragment_range: 0..fragments.len(),
    }];

    PreparedSection {
        fragments,
        segments,
        anchor_segments,
    }
}

fn split_text_block(block: TextBlock) -> Vec<TextBlock> {
    let content_len = inline_content_len(&block.content);
    if content_len <= FRAGMENT_TEXT_BUDGET {
        return vec![block];
    }

    let TextBlock {
        kind,
        content,
        style,
        source,
    } = block;
    let content_parts = split_inline_content(content);
    let part_count = content_parts.len();
    let mut source_offset = 0_usize;
    content_parts
        .into_iter()
        .enumerate()
        .map(|(index, (content, length))| {
            let mut part_style = style;
            let part_kind = if index > 0
                && matches!(kind, rebook_publication::TextBlockKind::ListItem { .. })
            {
                rebook_publication::TextBlockKind::Paragraph
            } else {
                kind
            };
            if index > 0 {
                part_style.margin_before = 0.0;
                part_style.indent = 0.0;
            }
            if index + 1 < part_count {
                part_style.margin_after = 0.0;
            }
            let part_source = source
                .as_ref()
                .map(|range| slice_source_range(range, source_offset, source_offset + length));
            source_offset += length;
            TextBlock {
                kind: part_kind,
                content,
                style: part_style,
                source: part_source,
            }
        })
        .collect()
}

fn split_inline_content(content: Vec<Inline>) -> Vec<(Vec<Inline>, usize)> {
    let mut parts = Vec::new();
    let mut current = Vec::new();
    let mut current_len = 0_usize;

    let flush = |current: &mut Vec<Inline>,
                 current_len: &mut usize,
                 parts: &mut Vec<(Vec<Inline>, usize)>| {
        if !current.is_empty() {
            parts.push((std::mem::take(current), *current_len));
            *current_len = 0;
        }
    };

    for inline in content {
        match inline {
            Inline::Break => {
                if current_len == FRAGMENT_TEXT_BUDGET {
                    flush(&mut current, &mut current_len, &mut parts);
                }
                current.push(Inline::Break);
                current_len += 1;
            }
            Inline::Text(run) => {
                let TextRun { text, style, link } = run;
                let mut remaining = text.as_str();
                while !remaining.is_empty() {
                    if current_len == FRAGMENT_TEXT_BUDGET {
                        flush(&mut current, &mut current_len, &mut parts);
                    }
                    let capacity = FRAGMENT_TEXT_BUDGET - current_len;
                    let split_at = byte_index_after_chars(remaining, capacity);
                    let (slice, rest) = remaining.split_at(split_at);
                    current.push(Inline::Text(TextRun {
                        text: slice.to_owned(),
                        style,
                        link: link.clone(),
                    }));
                    current_len += slice.chars().count();
                    remaining = rest;
                }
            }
        }
    }
    flush(&mut current, &mut current_len, &mut parts);
    parts
}

fn byte_index_after_chars(text: &str, count: usize) -> usize {
    text.char_indices()
        .nth(count)
        .map_or(text.len(), |(index, _)| index)
}

fn slice_source_range(range: &SourceRange, start: usize, end: usize) -> SourceRange {
    if range.start.spine == range.end.spine && range.start.node == range.end.node {
        let offset = |value: usize| {
            range
                .start
                .text_offset
                .saturating_add(u64::try_from(value).unwrap_or(u64::MAX))
                .min(range.end.text_offset)
        };
        SourceRange {
            start: SourceAnchor {
                spine: range.start.spine.clone(),
                node: range.start.node.clone(),
                text_offset: offset(start),
            },
            end: SourceAnchor {
                spine: range.end.spine.clone(),
                node: range.end.node.clone(),
                text_offset: offset(end),
            },
        }
    } else {
        range.clone()
    }
}

fn inline_content_len(content: &[Inline]) -> usize {
    content
        .iter()
        .map(|inline| match inline {
            Inline::Text(run) => run.text.chars().count(),
            Inline::Break => 1,
        })
        .sum()
}

fn block_text_len(block: &Block) -> usize {
    match block {
        Block::Text(block) => inline_content_len(&block.content),
        Block::Image(_) | Block::Separator | Block::PageBreak => 0,
    }
}

fn block_source(block: &Block) -> Option<&SourceRange> {
    match block {
        Block::Text(block) => block.source.as_ref(),
        Block::Image(block) => block.source.as_ref(),
        Block::Separator | Block::PageBreak => None,
    }
}

fn source_range_contains(range: &SourceRange, anchor: &SourceAnchor) -> bool {
    if range.start.spine != anchor.spine || range.start.node != anchor.node {
        return false;
    }
    if range.start.spine != range.end.spine || range.start.node != range.end.node {
        return range.start == *anchor;
    }
    anchor.text_offset >= range.start.text_offset
        && (anchor.text_offset < range.end.text_offset
            || (range.start.text_offset == range.end.text_offset
                && anchor.text_offset == range.start.text_offset))
}

fn reader_text_hit(position: ReaderPosition, hit: PageTextHit) -> ReaderTextHit {
    ReaderTextHit {
        position,
        region_index: hit.region_index,
        byte_index: hit.byte_index,
    }
}

fn append_selection_quote(output: &mut String, value: &str) {
    if value.is_empty() {
        return;
    }
    if !output.is_empty()
        && output
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric)
        && value.chars().next().is_some_and(char::is_alphanumeric)
    {
        output.push(' ');
    }
    output.push_str(value);
}

fn push_source_range(ranges: &mut Vec<SourceRange>, range: SourceRange) {
    if let Some(previous) = ranges.last_mut()
        && previous.end.spine == range.start.spine
        && previous.end.node == range.start.node
        && previous.end.text_offset >= range.start.text_offset
    {
        if range.end.text_offset > previous.end.text_offset {
            previous.end = range.end;
        }
        return;
    }
    ranges.push(range);
}

fn flatten_toc(entries: &[TocEntry]) -> Vec<TocViewItem> {
    fn append(
        entries: &[TocEntry],
        depth: usize,
        ancestors: &[String],
        items: &mut Vec<TocViewItem>,
    ) {
        for (index, entry) in entries.iter().enumerate() {
            let id = ancestors
                .last()
                .map_or_else(|| index.to_string(), |parent| format!("{parent}/{index}"));
            items.push(TocViewItem {
                id: id.clone(),
                label: entry.label.clone(),
                target: entry.href.clone(),
                depth,
                ancestors: ancestors.to_vec(),
                has_children: !entry.children.is_empty(),
            });
            let mut child_ancestors = ancestors.to_vec();
            child_ancestors.push(id);
            append(&entry.children, depth + 1, &child_ancestors, items);
        }
    }

    let mut items = Vec::new();
    append(entries, 0, &[], &mut items);
    items
}

fn active_toc_item_for_location(
    items: &[TocViewItem],
    current_section: usize,
    current_segment: usize,
    current_page: usize,
    mut resolve: impl FnMut(&PublicationUrl) -> Option<ReaderPosition>,
) -> Option<&TocViewItem> {
    let mut first_in_current_section = None;
    let mut best = None;

    for (order, item) in items.iter().enumerate() {
        let Some(position) = item.target.as_ref().and_then(&mut resolve) else {
            continue;
        };
        let ReaderPosition {
            section_index,
            segment_index,
            page_index,
        } = position;
        if section_index == current_section && first_in_current_section.is_none() {
            first_in_current_section = Some(item);
        }
        if section_index > current_section
            || (section_index == current_section
                && (segment_index, page_index) > (current_segment, current_page))
        {
            continue;
        }
        let key = (section_index, segment_index, page_index, order);
        if best.is_none_or(|(best_key, _)| key > best_key) {
            best = Some((key, item));
        }
    }

    best.map(|(_, item)| item).or(first_in_current_section)
}

fn total_progression(location: ReaderLocation, section_count: usize) -> f64 {
    let to_f64 = |value: usize| f64::from(u32::try_from(value).unwrap_or(u32::MAX));
    let section_count = to_f64(section_count.max(1));
    let segment_count = to_f64(location.segment_count.max(1));
    let page_count = to_f64(location.page_count.max(1));
    let segment_progress = (to_f64(location.segment_index)
        + to_f64(location.page_index + 1) / page_count)
        / segment_count;
    ((to_f64(location.section_index) + segment_progress) / section_count).clamp(0.0, 1.0)
}

// Spread bounds use kurbo's f64 geometry while reader composition uses bounded
// logical f32 coordinates.
#[allow(clippy::cast_possible_truncation)]
fn resolve_spread_offsets(
    primary: &PageDisplayList,
    secondary: Option<&PageDisplayList>,
    default_secondary_offset_x: f32,
    compact_images: bool,
) -> (f32, f32) {
    let Some(secondary) = secondary.filter(|_| compact_images) else {
        return (0.0, default_secondary_offset_x);
    };
    let (Some(primary_bounds), Some(secondary_bounds)) =
        (primary.image_bounds(), secondary.image_bounds())
    else {
        return (0.0, default_secondary_offset_x);
    };
    let viewport_width = f64::from(primary.width());
    let primary_offset_x = f64::midpoint(
        viewport_width - primary_bounds.x0 - primary_bounds.x1 - secondary_bounds.x1,
        secondary_bounds.x0,
    );
    let secondary_offset_x = primary_bounds.x1 + primary_offset_x - secondary_bounds.x0;
    if !primary_offset_x.is_finite() || !secondary_offset_x.is_finite() {
        return (0.0, default_secondary_offset_x);
    }
    (primary_offset_x as f32, secondary_offset_x as f32)
}

// Renderer geometry uses f64 (kurbo), while pointer events and the reader's
// public logical-pixel geometry use f32. Page coordinates are viewport-bounded,
// so the conversion cannot overflow and only discards unused sub-pixel precision.
#[allow(clippy::cast_possible_truncation)]
fn logical_coordinate(value: f64) -> f32 {
    debug_assert!(value.is_finite());
    debug_assert!((f64::from(f32::MIN)..=f64::from(f32::MAX)).contains(&value));
    value as f32
}

// Page counts are bounded by the pages that fit in memory, so they remain far
// below f32's exact-integer limit. These conversions intentionally map the
// discrete page index to and from a normalized viewport-resize progress value.
#[allow(clippy::cast_precision_loss)]
fn page_fraction(page: usize, count: usize) -> f32 {
    if count <= 1 {
        0.0
    } else {
        page as f32 / (count - 1) as f32
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn page_for_fraction(fraction: f32, count: usize) -> usize {
    if count <= 1 {
        0
    } else {
        (fraction.clamp(0.0, 1.0) * (count - 1) as f32).round() as usize
    }
}

#[derive(Debug, Error)]
pub enum ReaderError {
    #[error("publication has no readable sections")]
    EmptyBook,
    #[error("section index is outside the reading order: {0}")]
    SectionOutOfBounds(usize),
    #[error("layout segment {segment} is outside section {section}")]
    SegmentOutOfBounds { section: usize, segment: usize },
    #[error("logical page is outside the compiled reader cache: {0:?}")]
    PageOutOfBounds(ReaderPosition),
    #[error("navigation target is not in the reading order: {0}")]
    NavigationTargetNotFound(String),
    #[error(transparent)]
    Publication(#[from] PublicationError),
    #[error(transparent)]
    Layout(#[from] LayoutError),
    #[error("failed to start the section prefetch worker: {0}")]
    PrefetchWorkerStart(std::io::Error),
    #[error("section prefetch worker stopped unexpectedly")]
    PrefetchWorkerStopped,
    #[error("parsed section repository lock is poisoned")]
    SectionRepositoryPoisoned,
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use rebook_layout::{ReaderDefaultFont, SpreadMode};
    use rebook_publication::{
        Block, BlockStyle, Inline, Metadata, PublicationId, PublicationUrl, Resource, Section,
        SectionAnchor, SourceAnchor, SourceRange, SpineItem, SpineItemId, TextBlock, TextBlockKind,
        TextRun, TextStyle, TocEntry,
    };

    use super::*;

    struct CountingSource {
        book: Book,
        sections: Vec<Section>,
        parse_counts: Vec<AtomicUsize>,
        background_delay: Duration,
    }

    impl CountingSource {
        fn new(texts: &[String]) -> Arc<Self> {
            Self::with_background_delay(texts, Duration::ZERO)
        }

        fn with_background_delay(texts: &[String], background_delay: Duration) -> Arc<Self> {
            let mut descriptors = Vec::with_capacity(texts.len());
            let mut sections = Vec::with_capacity(texts.len());
            for (index, text) in texts.iter().enumerate() {
                let id = SpineItemId::new(format!("section-{index}")).unwrap();
                let href = PublicationUrl::parse(&format!("section-{index}.xhtml")).unwrap();
                let text_len = u64::try_from(text.chars().count()).unwrap();
                descriptors.push(SpineItem {
                    id: id.clone(),
                    href: href.clone(),
                    media_type: "application/xhtml+xml".into(),
                    linear: true,
                    properties: Vec::new(),
                });
                sections.push(Section {
                    id: id.clone(),
                    href,
                    blocks: vec![Block::Text(TextBlock {
                        kind: TextBlockKind::Paragraph,
                        content: vec![Inline::Text(TextRun {
                            text: text.clone(),
                            style: TextStyle::default(),
                            link: None,
                        })],
                        style: BlockStyle::default(),
                        source: Some(SourceRange {
                            start: SourceAnchor {
                                spine: id.clone(),
                                node: "paragraph-0".into(),
                                text_offset: 0,
                            },
                            end: SourceAnchor {
                                spine: id.clone(),
                                node: "paragraph-0".into(),
                                text_offset: text_len,
                            },
                        }),
                    })],
                    anchors: Vec::new(),
                });
            }

            Arc::new(Self {
                book: Book {
                    id: PublicationId::new("reader-test").unwrap(),
                    metadata: Metadata::default(),
                    cover: None,
                    sections: descriptors,
                    table_of_contents: Vec::new(),
                },
                parse_counts: (0..sections.len()).map(|_| AtomicUsize::new(0)).collect(),
                sections,
                background_delay,
            })
        }

        fn parse_count(&self, index: usize) -> usize {
            self.parse_counts[index].load(Ordering::Relaxed)
        }
    }

    impl BookSource for CountingSource {
        fn book(&self) -> &Book {
            &self.book
        }

        fn parse_section(&self, index: usize) -> Result<Section, PublicationError> {
            if index > 0 {
                thread::sleep(self.background_delay);
            }
            let section =
                self.sections.get(index).cloned().ok_or_else(|| {
                    PublicationError::ResourceNotFound(format!("section {index}"))
                })?;
            self.parse_counts[index].fetch_add(1, Ordering::Relaxed);
            Ok(section)
        }

        fn resource(&self, href: &PublicationUrl) -> Result<Resource, PublicationError> {
            Err(PublicationError::ResourceNotFound(href.to_string()))
        }
    }

    fn viewport(width: u32, height: u32) -> LayoutViewport {
        LayoutViewport::new(width, height).unwrap()
    }

    fn image_page(image_x: f32) -> PageDisplayList {
        DisplayListCompiler.compile(&rebook_layout::PageLayout {
            viewport: viewport(1_200, 700),
            background: rebook_publication::Rgba::BLACK,
            items: vec![rebook_layout::PageItem::Image(
                rebook_layout::ImagePlacement {
                    image: rebook_layout::RasterImage {
                        width: 400,
                        height: 600,
                        pixels: vec![255; 400 * 600 * 4].into(),
                    },
                    x: image_x,
                    y: 0.0,
                    width: 400.0,
                    height: 600.0,
                    source: None,
                    text_layer: None,
                    replacement: None,
                },
            )],
        })
    }

    #[test]
    fn compact_image_spread_touches_and_centers_page_edges() {
        let primary = image_page(150.0);
        let secondary = image_page(150.0);
        let (primary_offset, secondary_offset) =
            resolve_spread_offsets(&primary, Some(&secondary), 600.0, true);
        let primary_bounds = primary.image_bounds().unwrap();
        let secondary_bounds = secondary.image_bounds().unwrap();
        let primary_left = primary_bounds.x0 + f64::from(primary_offset);
        let primary_right = primary_bounds.x1 + f64::from(primary_offset);
        let secondary_left = secondary_bounds.x0 + f64::from(secondary_offset);
        let secondary_right = secondary_bounds.x1 + f64::from(secondary_offset);

        assert!((primary_right - secondary_left).abs() < f64::EPSILON);
        assert!(((primary_left + secondary_right) / 2.0 - 600.0).abs() < f64::EPSILON);
    }

    #[test]
    fn cached_page_turns_and_boundaries_do_not_reparse() {
        let source = CountingSource::new(&["缓存翻页测试。".repeat(600)]);
        let mut reader =
            ReaderSession::open(source.clone(), viewport(600, 400), ReaderStyle::default())
                .unwrap();
        assert!(reader.location().page_count > 2);
        assert_eq!(source.parse_count(0), 1);

        assert!(matches!(
            reader.turn_page(PageDirection::Previous).unwrap().outcome,
            NavigationOutcome::Boundary
        ));
        let mut moved = 0;
        loop {
            let result = reader.turn_page(PageDirection::Next).unwrap();
            if result.outcome == NavigationOutcome::Boundary {
                break;
            }
            moved += 1;
            assert!(moved < 10_000);
        }
        assert!(moved > 2);
        assert_eq!(source.parse_count(0), 1);
    }

    #[test]
    fn native_selection_round_trips_to_source_ranges_and_geometry() {
        let source = CountingSource::new(&["selectable native reader text".into()]);
        let mut reader =
            ReaderSession::open(source, viewport(600, 400), ReaderStyle::default()).unwrap();
        let selected_source = SourceRange {
            start: SourceAnchor {
                spine: SpineItemId::new("section-0").unwrap(),
                node: "paragraph-0".into(),
                text_offset: 0,
            },
            end: SourceAnchor {
                spine: SpineItemId::new("section-0").unwrap(),
                node: "paragraph-0".into(),
                text_offset: 10,
            },
        };
        let rect = reader
            .current_page()
            .source_rects(std::slice::from_ref(&selected_source))[0];
        let y = logical_coordinate(rect.center().y);
        let anchor = reader
            .hit_test_current_spread(logical_coordinate(rect.x0) + 0.1, y, true)
            .unwrap()
            .unwrap();
        let focus = reader
            .hit_test_current_spread(logical_coordinate(rect.x1) - 0.1, y, true)
            .unwrap()
            .unwrap();
        let selection = reader.selection_between(&anchor, &focus).unwrap().unwrap();

        assert!(!selection.text.trim().is_empty());
        assert!(!selection.ranges.is_empty());
        assert!(!selection.rects.is_empty());
        assert!(
            reader
                .source_ranges_contain_point(
                    &selection.ranges,
                    selection.rects[0].x + selection.rects[0].width / 2.0,
                    selection.rects[0].y + selection.rects[0].height / 2.0,
                )
                .unwrap()
        );
    }

    #[test]
    fn visible_text_fragments_follow_the_current_page() {
        let source = CountingSource::new(&["visible page text ".repeat(1_200)]);
        let mut style = ReaderStyle::default();
        style.spread = SpreadMode::Single;
        let mut reader = ReaderSession::open(source, viewport(600, 400), style).unwrap();

        let first = reader.current_visible_text_fragments().unwrap();
        assert!(!first.is_empty());
        assert!(first.iter().all(|fragment| {
            fragment.position
                == ReaderPosition {
                    section_index: reader.location().section_index,
                    segment_index: reader.location().segment_index,
                    page_index: reader.location().page_index,
                }
        }));
        let first_ranges = first
            .iter()
            .map(|fragment| fragment.range.clone())
            .collect::<Vec<_>>();

        assert_eq!(
            reader.turn_page(PageDirection::Next).unwrap().outcome,
            NavigationOutcome::Moved
        );
        let second = reader.current_visible_text_fragments().unwrap();
        assert!(!second.is_empty());
        assert!(second.iter().all(|fragment| {
            fragment.position
                == ReaderPosition {
                    section_index: reader.location().section_index,
                    segment_index: reader.location().segment_index,
                    page_index: reader.location().page_index,
                }
        }));
        assert_ne!(
            first_ranges,
            second
                .iter()
                .map(|fragment| fragment.range.clone())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn durable_source_navigation_resolves_the_page_after_pagination() {
        let source = CountingSource::new(&["navigation target ".repeat(900)]);
        let mut reader =
            ReaderSession::open(source, viewport(600, 400), ReaderStyle::default()).unwrap();
        let anchor = SourceAnchor {
            spine: SpineItemId::new("section-0").unwrap(),
            node: "paragraph-0".into(),
            text_offset: 8_000,
        };

        reader.go_to_source(&anchor).unwrap();
        assert!(reader.location().page_index > 0);
        assert!(reader.current_page().contains_source_anchor(&anchor));
    }

    #[test]
    fn durable_locator_restores_after_viewport_repagination() {
        let source = CountingSource::new(&["durable locator ".repeat(1_200)]);
        let mut first =
            ReaderSession::open(source.clone(), viewport(600, 400), ReaderStyle::default())
                .unwrap();
        first
            .go_to_source(&SourceAnchor {
                spine: SpineItemId::new("section-0").unwrap(),
                node: "paragraph-0".into(),
                text_offset: 9_000,
            })
            .unwrap();
        let locator = first.current_locator();
        let source_anchor = locator.source.as_ref().unwrap().start.clone();

        let mut restored =
            ReaderSession::open(source, viewport(820, 620), ReaderStyle::default()).unwrap();
        restored.restore_locator(&locator).unwrap();

        assert!(
            restored
                .current_page()
                .contains_source_anchor(&source_anchor)
        );
        assert_eq!(
            restored.current_locator().publication_id,
            locator.publication_id
        );
    }

    #[test]
    fn oversized_text_block_is_split_into_stable_source_ranged_fragments() {
        let source = CountingSource::new(&["a".repeat(FRAGMENT_TEXT_BUDGET * 2 + 17)]);
        let mut section = source.sections[0].clone();
        let spine = section.id.clone();
        section.blocks[0] = Block::Text(TextBlock {
            kind: TextBlockKind::Paragraph,
            content: vec![Inline::Text(TextRun {
                text: "a".repeat(FRAGMENT_TEXT_BUDGET * 2 + 17),
                style: TextStyle::default(),
                link: None,
            })],
            style: BlockStyle::default(),
            source: Some(SourceRange {
                start: SourceAnchor {
                    spine: spine.clone(),
                    node: "n0".into(),
                    text_offset: 0,
                },
                end: SourceAnchor {
                    spine,
                    node: "n0".into(),
                    text_offset: u64::try_from(FRAGMENT_TEXT_BUDGET * 2 + 17).unwrap(),
                },
            }),
        });

        let prepared = prepare_section(section);

        assert_eq!(prepared.fragments.len(), 3);
        assert_eq!(prepared.segments.len(), 1);
        assert_eq!(block_text_len(&prepared.fragments[0].blocks[0]), 4_096);
        assert_eq!(block_text_len(&prepared.fragments[1].blocks[0]), 4_096);
        assert_eq!(block_text_len(&prepared.fragments[2].blocks[0]), 17);
        let ranges = prepared
            .fragments
            .iter()
            .map(|fragment| block_source(&fragment.blocks[0]).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(ranges[0].start.text_offset, 0);
        assert_eq!(ranges[0].end.text_offset, 4_096);
        assert_eq!(ranges[1].start.text_offset, 4_096);
        assert_eq!(ranges[1].end.text_offset, 8_192);
        assert_eq!(ranges[2].start.text_offset, 8_192);
        assert_eq!(ranges[2].end.text_offset, 8_209);
    }

    #[test]
    fn content_fragment_boundaries_never_commit_partial_pages() {
        let text_len = FRAGMENT_TEXT_BUDGET * 3 + 100;
        let source = CountingSource::new(&["a".repeat(text_len)]);
        let mut section = source.sections[0].clone();
        let spine = section.id.clone();
        let Block::Text(block) = &mut section.blocks[0] else {
            unreachable!();
        };
        block.source = Some(SourceRange {
            start: SourceAnchor {
                spine: spine.clone(),
                node: "n0".into(),
                text_offset: 0,
            },
            end: SourceAnchor {
                spine,
                node: "n0".into(),
                text_offset: u64::try_from(text_len).unwrap(),
            },
        });
        let prepared = prepare_section(section);
        let segment = &prepared.segments[0];
        let fragments = prepared.fragments[segment.fragment_range.clone()]
            .iter()
            .map(|fragment| fragment.blocks.as_slice())
            .collect::<Vec<_>>();

        let layout = LayoutEngine::new()
            .layout_fragments(
                source.as_ref(),
                &fragments,
                viewport(600, 50_000),
                &ReaderStyle::default(),
            )
            .unwrap();

        assert_eq!(prepared.fragments.len(), 4);
        assert_eq!(prepared.segments.len(), 1);
        assert_eq!(layout.pages.len(), 1);
        let ranges = layout.pages[0]
            .items
            .iter()
            .filter_map(|item| match item {
                PageItem::Text(placement) => placement.source.as_ref(),
                PageItem::Image(placement) => placement.source.as_ref(),
                PageItem::Separator(_) => None,
            })
            .collect::<Vec<_>>();
        assert!(
            ranges
                .iter()
                .any(|range| range.end.text_offset == u64::try_from(FRAGMENT_TEXT_BUDGET).unwrap())
        );
        assert!(ranges.iter().any(|range| {
            range.start.text_offset == u64::try_from(FRAGMENT_TEXT_BUDGET).unwrap()
        }));
        assert!(
            ranges
                .iter()
                .any(|range| { range.end.text_offset == u64::try_from(text_len).unwrap() })
        );
    }

    #[test]
    fn continued_list_item_does_not_repeat_its_marker() {
        let parts = split_text_block(TextBlock {
            kind: TextBlockKind::ListItem {
                ordered: true,
                ordinal: 7,
            },
            content: vec![Inline::Text(TextRun {
                text: "item ".repeat(FRAGMENT_TEXT_BUDGET),
                style: TextStyle::default(),
                link: None,
            })],
            style: BlockStyle {
                indent: 24.0,
                ..BlockStyle::default()
            },
            source: None,
        });

        assert!(parts.len() > 1);
        assert!(matches!(
            parts[0].kind,
            TextBlockKind::ListItem {
                ordered: true,
                ordinal: 7
            }
        ));
        assert!(
            parts[1..]
                .iter()
                .all(|part| part.kind == TextBlockKind::Paragraph)
        );
        assert!(parts[1..].iter().all(|part| part.style.indent == 0.0));
    }

    #[test]
    fn page_turns_across_content_fragments_do_not_reparse_the_authored_section() {
        let source = CountingSource::new(&["long text ".repeat(FRAGMENT_TEXT_BUDGET)]);
        let mut reader =
            ReaderSession::open(source.clone(), viewport(600, 400), ReaderStyle::default())
                .unwrap();
        assert_eq!(reader.location().segment_count, 1);
        assert!(reader.location().page_count > 10);

        for _ in 0..10 {
            assert_eq!(
                reader.turn_page(PageDirection::Next).unwrap().outcome,
                NavigationOutcome::Moved
            );
        }
        assert_eq!(reader.location().segment_index, 0);
        assert_eq!(reader.location().page_index, 10);
        assert_eq!(source.parse_count(0), 1);

        assert_eq!(
            reader.turn_page(PageDirection::Previous).unwrap().outcome,
            NavigationOutcome::Moved
        );
        assert_eq!(reader.location().segment_index, 0);
        assert_eq!(reader.location().page_index, 9);
        assert_eq!(source.parse_count(0), 1);
    }

    #[test]
    fn double_spread_composes_adjacent_pages_from_one_continuous_section() {
        let source = CountingSource::new(&["long text ".repeat(FRAGMENT_TEXT_BUDGET)]);
        let mut reader = ReaderSession::open(
            source,
            viewport(1_200, 700),
            ReaderStyle {
                spread: rebook_layout::SpreadMode::Double,
                ..ReaderStyle::default()
            },
        )
        .unwrap();
        assert_eq!(reader.location().segment_count, 1);
        assert!(reader.current_page_count() > 2);
        reader.current_page = reader.current_page_count() - 2;

        let next = reader
            .next_position(reader.current_position())
            .unwrap()
            .expect("another logical page should follow");
        assert_eq!(next.section_index, 0);
        assert_eq!(next.segment_index, 0);
        let spread = reader.current_spread().unwrap();
        assert!(spread.secondary.is_some());
    }

    #[test]
    fn segment_window_prefetch_makes_short_section_switches_cache_only() {
        let source = CountingSource::new(&["第一章".into(), "第二章".into(), "第三章".into()]);
        let mut reader =
            ReaderSession::open(source.clone(), viewport(600, 400), ReaderStyle::default())
                .unwrap();

        reader.prefetch_adjacent().unwrap();
        reader.wait_for_prefetch().unwrap();
        assert_eq!(reader.cached_segment_count(), 3);
        assert_eq!(source.parse_count(1), 1);
        assert_eq!(source.parse_count(2), 1);
        reader.turn_page(PageDirection::Next).unwrap();
        assert_eq!(reader.location().section_index, 1);
        assert_eq!(source.parse_count(1), 1);

        reader.prefetch_adjacent().unwrap();
        reader.wait_for_prefetch().unwrap();
        assert_eq!(reader.cached_segment_count(), 3);
        assert_eq!(source.parse_count(2), 1);
        reader.turn_page(PageDirection::Next).unwrap();
        assert_eq!(reader.location().section_index, 2);
        assert_eq!(source.parse_count(2), 1);
    }

    #[test]
    fn double_spread_composes_across_section_boundaries_without_repeating_pages() {
        let source = CountingSource::new(&[
            "left page".into(),
            "right page".into(),
            "next spread".into(),
        ]);
        let mut reader = ReaderSession::open(
            source.clone(),
            viewport(1_200, 700),
            ReaderStyle {
                spread: rebook_layout::SpreadMode::Double,
                ..ReaderStyle::default()
            },
        )
        .unwrap();

        let spread = reader.current_spread().unwrap();
        assert!(spread.primary.command_count() > 0);
        assert!(
            spread
                .secondary
                .as_ref()
                .is_some_and(|page| page.command_count() > 0)
        );
        assert_eq!(source.parse_count(1), 1);
        assert_eq!(reader.current_spread_section_indices().unwrap(), [0, 1]);

        assert_eq!(
            reader.turn_page(PageDirection::Next).unwrap().outcome,
            NavigationOutcome::Moved
        );
        assert_eq!(reader.location().section_index, 2);
        assert_eq!(
            reader.turn_page(PageDirection::Previous).unwrap().outcome,
            NavigationOutcome::Moved
        );
        assert_eq!(reader.location().section_index, 0);
    }

    #[test]
    fn double_spread_prefetches_every_page_needed_by_the_next_spread() {
        let source = CountingSource::new(&[
            "page one".into(),
            "page two".into(),
            "page three".into(),
            "page four".into(),
            "page five".into(),
        ]);
        let mut reader = ReaderSession::open(
            source.clone(),
            viewport(1_200, 700),
            ReaderStyle {
                spread: rebook_layout::SpreadMode::Double,
                ..ReaderStyle::default()
            },
        )
        .unwrap();

        reader.prefetch_adjacent().unwrap();
        reader.wait_for_prefetch().unwrap();

        assert_eq!(source.parse_count(1), 1);
        assert_eq!(source.parse_count(2), 1);
        assert_eq!(source.parse_count(3), 1);
    }

    #[test]
    fn toc_href_navigation_resolves_segments_and_reuses_parsed_sections() {
        let mut source = CountingSource::new(&[
            "第一章".repeat(100),
            "第二章".repeat(100),
            "第三章".repeat(100),
        ]);
        let target_section = &mut Arc::get_mut(&mut source).unwrap().sections[1];
        let spine = target_section.id.clone();
        let source_range = |node: &str, length: u64| SourceRange {
            start: SourceAnchor {
                spine: spine.clone(),
                node: node.to_owned(),
                text_offset: 0,
            },
            end: SourceAnchor {
                spine: spine.clone(),
                node: node.to_owned(),
                text_offset: length,
            },
        };
        target_section.blocks = vec![
            Block::Text(TextBlock {
                kind: TextBlockKind::Paragraph,
                content: vec![Inline::Text(TextRun {
                    text: "目标之前的长正文。".repeat(2_000),
                    style: TextStyle::default(),
                    link: None,
                })],
                style: BlockStyle::default(),
                source: Some(source_range("n0", 18_000)),
            }),
            Block::Text(TextBlock {
                kind: TextBlockKind::Heading(2),
                content: vec![Inline::Text(TextRun {
                    text: "目录目标".to_owned(),
                    style: TextStyle::default(),
                    link: None,
                })],
                style: BlockStyle::default(),
                source: Some(source_range("n1", 4)),
            }),
        ];
        target_section.anchors = vec![SectionAnchor {
            fragment: "part-2".to_owned(),
            source: source_range("n1", 4).start,
        }];
        let mut reader =
            ReaderSession::open(source.clone(), viewport(600, 400), ReaderStyle::default())
                .unwrap();
        reader.prefetch_adjacent().unwrap();
        reader.wait_for_prefetch().unwrap();

        let target = PublicationUrl::parse("section-1.xhtml#part-2").unwrap();
        assert_eq!(reader.section_index_for_href(&target), Some(1));
        let target_location = reader.position_for_href(&target).unwrap();
        assert_eq!(target_location.section_index, 1);
        assert_eq!(target_location.segment_index, 0);
        reader.go_to_href(&target).unwrap();
        let resolved_location = reader.position_for_href(&target).unwrap();

        assert_eq!(reader.location().section_index, 1);
        assert_eq!(
            reader.location().segment_index,
            target_location.segment_index
        );
        assert_eq!(reader.location().page_index, resolved_location.page_index);
        assert!(reader.location().page_index > 0);
        assert_eq!(source.parse_count(1), 1);
    }

    #[test]
    fn distant_anchor_navigation_resolves_within_continuous_section_layout() {
        let mut source = CountingSource::new(&["placeholder".into()]);
        let source_mut = Arc::get_mut(&mut source).unwrap();
        let spine = source_mut.sections[0].id.clone();
        let preceding_text_len = FRAGMENT_TEXT_BUDGET * 6 + 100;
        let source_range = |node: &str, length: usize| SourceRange {
            start: SourceAnchor {
                spine: spine.clone(),
                node: node.into(),
                text_offset: 0,
            },
            end: SourceAnchor {
                spine: spine.clone(),
                node: node.into(),
                text_offset: u64::try_from(length).unwrap(),
            },
        };
        source_mut.sections[0].blocks = vec![
            Block::Text(TextBlock {
                kind: TextBlockKind::Paragraph,
                content: vec![Inline::Text(TextRun {
                    text: "a ".repeat(preceding_text_len / 2),
                    style: TextStyle::default(),
                    link: None,
                })],
                style: BlockStyle::default(),
                source: Some(source_range("n0", preceding_text_len)),
            }),
            Block::Text(TextBlock {
                kind: TextBlockKind::Heading(2),
                content: vec![Inline::Text(TextRun {
                    text: "Target".into(),
                    style: TextStyle::default(),
                    link: None,
                })],
                style: BlockStyle::default(),
                source: Some(source_range("n1", 6)),
            }),
        ];
        source_mut.sections[0].anchors = vec![SectionAnchor {
            fragment: "target".into(),
            source: source_range("n1", 6).start,
        }];
        let mut reader =
            ReaderSession::open(source.clone(), viewport(600, 400), ReaderStyle::default())
                .unwrap();
        let target = PublicationUrl::parse("section-0.xhtml#target").unwrap();
        let target_position = reader.position_for_href(&target).unwrap();
        assert_eq!(target_position.segment_index, 0);
        assert!(target_position.page_index > 0);

        reader.go_to_href(&target).unwrap();

        assert_eq!(reader.location().segment_index, 0);
        assert_eq!(reader.location().page_index, target_position.page_index);
        assert!(reader.cache.contains_key(&SegmentKey {
            section_index: 0,
            segment_index: 0,
        }));
        assert_eq!(reader.cache.len(), 1);
        assert_eq!(source.parse_count(0), 1);
    }

    #[test]
    fn toc_and_total_progression_advance_across_page_boundaries() {
        let mut source = CountingSource::new(&["placeholder".into()]);
        let source_mut = Arc::get_mut(&mut source).unwrap();
        let spine = source_mut.sections[0].id.clone();
        let preceding_text_len = FRAGMENT_TEXT_BUDGET * 4 + 100;
        let source_range = |node: &str, length: u64| SourceRange {
            start: SourceAnchor {
                spine: spine.clone(),
                node: node.into(),
                text_offset: 0,
            },
            end: SourceAnchor {
                spine: spine.clone(),
                node: node.into(),
                text_offset: length,
            },
        };
        source_mut.sections[0].blocks = vec![
            Block::Text(TextBlock {
                kind: TextBlockKind::Paragraph,
                content: vec![Inline::Text(TextRun {
                    text: "a ".repeat(preceding_text_len / 2),
                    style: TextStyle::default(),
                    link: None,
                })],
                style: BlockStyle::default(),
                source: Some(source_range(
                    "n0",
                    u64::try_from(preceding_text_len).unwrap(),
                )),
            }),
            Block::Text(TextBlock {
                kind: TextBlockKind::Heading(2),
                content: vec![Inline::Text(TextRun {
                    text: "Later".into(),
                    style: TextStyle::default(),
                    link: None,
                })],
                style: BlockStyle::default(),
                source: Some(source_range("n1", 5)),
            }),
        ];
        source_mut.sections[0].anchors = vec![SectionAnchor {
            fragment: "later".into(),
            source: source_range("n1", 5).start,
        }];
        source_mut.book.table_of_contents = vec![
            TocEntry {
                label: "Start".into(),
                href: Some(PublicationUrl::parse("section-0.xhtml").unwrap()),
                children: Vec::new(),
            },
            TocEntry {
                label: "Later".into(),
                href: Some(PublicationUrl::parse("section-0.xhtml#later").unwrap()),
                children: Vec::new(),
            },
        ];
        let mut reader =
            ReaderSession::open(source.clone(), viewport(600, 400), ReaderStyle::default())
                .unwrap();
        assert_eq!(reader.snapshot().active_toc_id.as_deref(), Some("0"));
        let mut previous_progress = reader.snapshot().total_progression;

        for _ in 0..100 {
            let result = reader.turn_page(PageDirection::Next).unwrap();
            assert!(result.snapshot.total_progression > previous_progress);
            previous_progress = result.snapshot.total_progression;
            if result.snapshot.active_toc_id.as_deref() == Some("1") {
                assert_eq!(result.snapshot.location.segment_index, 0);
                assert!(result.snapshot.location.page_index > 0);
                assert_eq!(source.parse_count(0), 1);
                return;
            }
        }
        panic!("reader did not reach the later fragment TOC anchor");
    }

    #[test]
    fn adjacent_prefetch_never_blocks_the_caller_thread() {
        let source = CountingSource::with_background_delay(
            &["第一章".into(), "第二章".into()],
            Duration::from_millis(300),
        );
        let mut reader =
            ReaderSession::open(source.clone(), viewport(600, 400), ReaderStyle::default())
                .unwrap();

        let started = Instant::now();
        reader.prefetch_adjacent().unwrap();
        assert!(started.elapsed() < Duration::from_millis(100));

        reader.wait_for_prefetch().unwrap();
        assert_eq!(source.parse_count(1), 1);
    }

    #[test]
    fn interactive_page_turn_waits_in_background_instead_of_blocking() {
        let blocking_source = CountingSource::with_background_delay(
            &["first".into(), "second".into()],
            Duration::from_millis(300),
        );
        let mut blocking_reader =
            ReaderSession::open(blocking_source, viewport(600, 400), ReaderStyle::default())
                .unwrap();
        let blocking_started = Instant::now();
        let blocking_result = blocking_reader.turn_page(PageDirection::Next).unwrap();
        let blocking_elapsed = blocking_started.elapsed();
        assert_eq!(blocking_result.outcome, NavigationOutcome::Moved);
        assert!(blocking_elapsed >= Duration::from_millis(250));

        let source = CountingSource::with_background_delay(
            &["first".into(), "second".into()],
            Duration::from_millis(300),
        );
        let mut reader =
            ReaderSession::open(source, viewport(600, 400), ReaderStyle::default()).unwrap();
        let nonblocking_started = Instant::now();
        let attempt = reader.try_turn_page(PageDirection::Next).unwrap();
        let nonblocking_elapsed = nonblocking_started.elapsed();

        assert_eq!(attempt, NavigationAttempt::Pending);
        assert!(nonblocking_elapsed < Duration::from_millis(100));
        assert!(blocking_elapsed > nonblocking_elapsed * 2);
        assert_eq!(reader.location().section_index, 0);

        reader.wait_for_prefetch().unwrap();
        let attempt = reader.try_turn_page(PageDirection::Next).unwrap();
        let NavigationAttempt::Ready(result) = attempt else {
            panic!("prefetched destination should be ready");
        };
        assert_eq!(result.outcome, NavigationOutcome::Moved);
        assert_eq!(result.snapshot.location.section_index, 1);
    }

    #[test]
    fn background_section_parse_does_not_block_snapshot_updates() {
        let mut source = CountingSource::with_background_delay(
            &["first".into(), "second".into()],
            Duration::from_millis(300),
        );
        Arc::get_mut(&mut source).unwrap().book.table_of_contents = vec![TocEntry {
            label: "Second".into(),
            href: Some(PublicationUrl::parse("section-1.xhtml").unwrap()),
            children: Vec::new(),
        }];
        let mut reader =
            ReaderSession::open(source, viewport(600, 400), ReaderStyle::default()).unwrap();
        reader.prefetch_adjacent().unwrap();
        thread::sleep(Duration::from_millis(30));

        let started = Instant::now();
        let _snapshot = reader.snapshot();

        assert!(started.elapsed() < Duration::from_millis(100));
        reader.wait_for_prefetch().unwrap();
    }

    #[test]
    fn resize_rebuilds_layout_and_preserves_approximate_progress() {
        let source = CountingSource::new(&["调整窗口后保持阅读进度。".repeat(600)]);
        let mut reader =
            ReaderSession::open(source.clone(), viewport(600, 400), ReaderStyle::default())
                .unwrap();
        let old_count = reader.location().page_count;
        assert!(old_count > 4);
        for _ in 0..old_count / 2 {
            reader.turn_page(PageDirection::Next).unwrap();
        }
        let old_fraction = page_fraction(reader.location().page_index, old_count);

        reader.resize(viewport(500, 300)).unwrap();

        let location = reader.location();
        let new_fraction = page_fraction(location.page_index, location.page_count);
        let one_page = page_fraction(1, location.page_count);
        assert!((new_fraction - old_fraction).abs() <= one_page);
        assert_eq!(source.parse_count(0), 1);
        assert_eq!(reader.cached_segment_count(), 1);
    }

    #[test]
    fn font_family_change_rebuilds_layout_and_preserves_approximate_progress() {
        let source = CountingSource::new(&["字体切换后保持阅读进度。".repeat(600)]);
        let mut reader =
            ReaderSession::open(source.clone(), viewport(600, 400), ReaderStyle::default())
                .unwrap();
        let old_count = reader.location().page_count;
        assert!(old_count > 4);
        for _ in 0..old_count / 2 {
            reader.turn_page(PageDirection::Next).unwrap();
        }
        let old_fraction = page_fraction(reader.location().page_index, old_count);

        let mut style = reader.style();
        style.typography.default_font = ReaderDefaultFont::SansSerif;
        reader.set_style(style).unwrap();

        let location = reader.location();
        let new_fraction = page_fraction(location.page_index, location.page_count);
        let one_page = page_fraction(1, location.page_count);
        assert!((new_fraction - old_fraction).abs() <= one_page);
        assert_eq!(
            reader.style().typography.default_font,
            ReaderDefaultFont::SansSerif
        );
        assert_eq!(source.parse_count(0), 1);
        assert_eq!(reader.cached_segment_count(), 1);
    }

    #[test]
    fn source_refresh_reparses_and_preserves_approximate_progress() {
        let source = CountingSource::new(&["派生正文刷新后保持阅读进度。".repeat(600)]);
        let mut reader =
            ReaderSession::open(source.clone(), viewport(600, 400), ReaderStyle::default())
                .unwrap();
        let old_count = reader.location().page_count;
        for _ in 0..old_count / 2 {
            reader.turn_page(PageDirection::Next).unwrap();
        }
        let old_fraction = page_fraction(reader.location().page_index, old_count);

        reader.refresh_source().unwrap();

        let location = reader.location();
        let new_fraction = page_fraction(location.page_index, location.page_count);
        let one_page = page_fraction(1, location.page_count);
        assert!((new_fraction - old_fraction).abs() <= one_page);
        assert_eq!(source.parse_count(0), 2);
        assert_eq!(reader.cached_segment_count(), 1);
    }

    #[test]
    fn toc_items_preserve_reading_order_depth_and_ancestry() {
        let first_target = PublicationUrl::parse("text/chapter-1.xhtml").unwrap();
        let child_target = PublicationUrl::parse("text/chapter-1.xhtml#part-1").unwrap();
        let items = flatten_toc(&[
            TocEntry {
                label: "第一章".into(),
                href: Some(first_target.clone()),
                children: vec![TocEntry {
                    label: "第一节".into(),
                    href: Some(child_target.clone()),
                    children: Vec::new(),
                }],
            },
            TocEntry {
                label: "第二章".into(),
                href: None,
                children: Vec::new(),
            },
        ]);

        assert_eq!(items.len(), 3);
        assert_eq!((items[0].label.as_str(), items[0].depth), ("第一章", 0));
        assert_eq!(items[0].target.as_ref(), Some(&first_target));
        assert!(items[0].has_children);
        assert!(items[0].ancestors.is_empty());
        assert_eq!((items[1].label.as_str(), items[1].depth), ("第一节", 1));
        assert_eq!(items[1].target.as_ref(), Some(&child_target));
        assert_eq!(items[1].ancestors, ["0"]);
        assert_eq!((items[2].label.as_str(), items[2].depth), ("第二章", 0));
        assert!(items[2].target.is_none());
    }

    #[test]
    fn active_toc_follows_the_nearest_preceding_segment_page() {
        let items = vec![
            TocViewItem {
                id: "previous".into(),
                label: "Previous".into(),
                target: Some(PublicationUrl::parse("section-0.xhtml#previous").unwrap()),
                depth: 0,
                ancestors: Vec::new(),
                has_children: false,
            },
            TocViewItem {
                id: "chapter".into(),
                label: "Chapter".into(),
                target: Some(PublicationUrl::parse("section-1.xhtml#chapter").unwrap()),
                depth: 0,
                ancestors: Vec::new(),
                has_children: true,
            },
            TocViewItem {
                id: "subsection".into(),
                label: "Subsection".into(),
                target: Some(PublicationUrl::parse("section-1.xhtml#subsection").unwrap()),
                depth: 1,
                ancestors: vec!["chapter".into()],
                has_children: false,
            },
            TocViewItem {
                id: "future".into(),
                label: "Future".into(),
                target: Some(PublicationUrl::parse("section-2.xhtml#future").unwrap()),
                depth: 0,
                ancestors: Vec::new(),
                has_children: false,
            },
        ];
        let position = |section_index, segment_index, page_index| ReaderPosition {
            section_index,
            segment_index,
            page_index,
        };
        let resolve = |target: &PublicationUrl| match (target.path(), target.fragment()) {
            ("section-0.xhtml", _) => Some(position(0, 0, 4)),
            ("section-1.xhtml", Some("chapter")) => Some(position(1, 1, 2)),
            ("section-1.xhtml", Some("subsection")) => Some(position(1, 2, 0)),
            ("section-2.xhtml", _) => Some(position(2, 0, 0)),
            _ => None,
        };

        assert_eq!(
            active_toc_item_for_location(&items, 1, 0, 1, resolve)
                .unwrap()
                .id,
            "previous"
        );
        assert_eq!(
            active_toc_item_for_location(&items, 1, 1, 2, resolve)
                .unwrap()
                .id,
            "chapter"
        );
        assert_eq!(
            active_toc_item_for_location(&items, 1, 2, 0, resolve)
                .unwrap()
                .id,
            "subsection"
        );
    }

    #[test]
    fn snapshot_owns_active_toc_state_and_progression() {
        let mut source = CountingSource::new(&["正文".repeat(600)]);
        Arc::get_mut(&mut source).unwrap().book.table_of_contents = vec![TocEntry {
            label: "Chapter".into(),
            href: Some(PublicationUrl::parse("section-0.xhtml").unwrap()),
            children: vec![TocEntry {
                label: "Child".into(),
                href: Some(PublicationUrl::parse("section-0.xhtml").unwrap()),
                children: Vec::new(),
            }],
        }];
        let reader =
            ReaderSession::open(source, viewport(600, 400), ReaderStyle::default()).unwrap();

        let snapshot = reader.snapshot();
        assert_eq!(snapshot.active_toc_id.as_deref(), Some("0/0"));
        assert_eq!(snapshot.active_toc_path, ["0"]);
        assert!(snapshot.total_progression > 0.0);
        assert!(snapshot.total_progression <= 1.0);
    }

    #[test]
    fn stale_prefetch_result_cannot_clear_current_generation_request() {
        let source = CountingSource::new(&["第一章".into(), "第二章".into()]);
        let mut reader =
            ReaderSession::open(source, viewport(600, 400), ReaderStyle::default()).unwrap();
        let stale_generation = reader.prefetch_worker.generation();
        let current_generation = reader.prefetch_worker.invalidate();
        let segment = SegmentKey {
            section_index: 1,
            segment_index: 0,
        };
        let current_key = PrefetchKey {
            generation: current_generation,
            segment,
        };
        reader.prefetch_inflight.insert(current_key);
        let section = Arc::clone(reader.current_section_data());

        reader.install_prefetch(PrefetchResult {
            key: segment,
            generation: stale_generation,
            segment: Ok(Arc::new(CachedSegment {
                section,
                pages: Vec::new(),
                anchor_pages: HashMap::new(),
                visible_pages: 1,
                continuation_offset_x: 0.0,
            })),
        });

        assert!(reader.prefetch_inflight.contains(&current_key));
    }

    #[test]
    fn dropping_reader_joins_worker_and_releases_source() {
        let source = CountingSource::new(&["正文".into()]);
        let weak = Arc::downgrade(&source);
        let reader =
            ReaderSession::open(source.clone(), viewport(600, 400), ReaderStyle::default())
                .unwrap();
        drop(source);

        assert!(weak.upgrade().is_some());
        drop(reader);
        assert!(weak.upgrade().is_none());
    }
}
