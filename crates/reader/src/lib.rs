//! Reader session with section, layout, and display-list caches.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;

use rebook_layout::{LayoutEngine, LayoutError, LayoutViewport, ReaderStyle};
use rebook_publication::{Book, BookSource, PublicationError, PublicationUrl};
use rebook_renderer::{DisplayListCompiler, PageDisplayList};
use thiserror::Error;

const PREFETCH_DISTANCE: usize = 2;
const DEFAULT_SECTION_CACHE_CAPACITY: usize = PREFETCH_DISTANCE * 2 + 1;

/// Direction requested by keyboard, pointer, or command navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageDirection {
    Next,
    Previous,
}

/// Result of moving through cached pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTurn {
    Moved {
        section_index: usize,
        page_index: usize,
    },
    Boundary {
        section_index: usize,
        page_index: usize,
    },
}

/// Stable current position exposed to the application shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReaderLocation {
    pub section_index: usize,
    pub page_index: usize,
    pub page_count: usize,
}

struct CachedSection {
    pages: Vec<PageDisplayList>,
}

struct PrefetchRequest {
    index: usize,
    viewport: LayoutViewport,
    style: ReaderStyle,
    generation: u64,
}

struct PrefetchResult {
    index: usize,
    generation: u64,
    section: Result<Arc<CachedSection>, ReaderError>,
}

/// Single-owner reader orchestration. The parser and renderer communicate only
/// through the publication and layout IR crates.
pub struct ReaderSession {
    source: Arc<dyn BookSource>,
    layout_engine: LayoutEngine,
    display_compiler: DisplayListCompiler,
    viewport: LayoutViewport,
    style: ReaderStyle,
    cache_capacity: usize,
    cache: HashMap<usize, Arc<CachedSection>>,
    lru: VecDeque<usize>,
    prefetch_requests: Sender<PrefetchRequest>,
    prefetch_results: Receiver<PrefetchResult>,
    prefetch_inflight: HashSet<usize>,
    prefetch_failures: HashMap<usize, ReaderError>,
    prefetch_generation: Arc<AtomicU64>,
    current_section: usize,
    current_page: usize,
}

impl ReaderSession {
    /// Opens the first section and compiles its pages once.
    pub fn open(
        source: Arc<dyn BookSource>,
        viewport: LayoutViewport,
        style: ReaderStyle,
    ) -> Result<Self, ReaderError> {
        if source.book().sections.is_empty() {
            return Err(ReaderError::EmptyBook);
        }
        let prefetch_generation = Arc::new(AtomicU64::new(0));
        let (prefetch_requests, prefetch_results) =
            spawn_prefetch_worker(Arc::clone(&source), Arc::clone(&prefetch_generation))?;
        let mut session = Self {
            source,
            layout_engine: LayoutEngine::new(),
            display_compiler: DisplayListCompiler,
            viewport,
            style,
            cache_capacity: DEFAULT_SECTION_CACHE_CAPACITY,
            cache: HashMap::new(),
            lru: VecDeque::new(),
            prefetch_requests,
            prefetch_results,
            prefetch_inflight: HashSet::new(),
            prefetch_failures: HashMap::new(),
            prefetch_generation,
            current_section: 0,
            current_page: 0,
        };
        session.ensure_section(0)?;
        Ok(session)
    }

    pub fn book(&self) -> &Book {
        self.source.book()
    }

    pub fn viewport(&self) -> LayoutViewport {
        self.viewport
    }

    pub fn style(&self) -> ReaderStyle {
        self.style
    }

    pub fn location(&self) -> ReaderLocation {
        ReaderLocation {
            section_index: self.current_section,
            page_index: self.current_page,
            page_count: self.current_page_count(),
        }
    }

    /// Returns the compiled display list for the current page.
    ///
    /// # Panics
    ///
    /// Panics if the reader's internal invariant is broken and the current
    /// section or page is missing from the cache.
    pub fn current_page(&self) -> &PageDisplayList {
        &self
            .cache
            .get(&self.current_section)
            .expect("current section must remain cached")
            .pages[self.current_page]
    }

    /// Resolves a publication URL to its spine section, ignoring a fragment
    /// until source anchors retain authored element IDs.
    pub fn section_index_for_href(&self, href: &PublicationUrl) -> Option<usize> {
        let resource = href.resource_url();
        self.source
            .book()
            .sections
            .iter()
            .position(|section| section.href.resource_url() == resource)
    }

    /// Navigates to the beginning of a spine section.
    pub fn go_to_section(&mut self, index: usize) -> Result<PageTurn, ReaderError> {
        self.poll_prefetch()?;
        if index >= self.source.book().sections.len() {
            return Err(ReaderError::SectionOutOfBounds(index));
        }
        self.ensure_section(index)?;
        self.current_section = index;
        self.current_page = 0;
        self.touch(index);
        Ok(self.moved())
    }

    /// Navigates a TOC or link target to the beginning of its containing section.
    pub fn go_to_href(&mut self, href: &PublicationUrl) -> Result<PageTurn, ReaderError> {
        let index = self
            .section_index_for_href(href)
            .ok_or_else(|| ReaderError::NavigationTargetNotFound(href.to_string()))?;
        self.go_to_section(index)
    }

    /// Moves in constant time while pages are cached. Section boundaries compile
    /// only the destination section, never the previous one again.
    pub fn turn_page(&mut self, direction: PageDirection) -> Result<PageTurn, ReaderError> {
        self.poll_prefetch()?;
        match direction {
            PageDirection::Next => self.next_page(),
            PageDirection::Previous => self.previous_page(),
        }
    }

    /// Queues a small chapter window around the current position for background
    /// parsing, pagination, and display-list compilation. Looking two chapters
    /// ahead keeps short, single-page chapters from outrunning the worker.
    /// This method never performs chapter layout on the caller thread.
    pub fn prefetch_adjacent(&mut self) -> Result<(), ReaderError> {
        self.poll_prefetch()?;
        let section_count = self.source.book().sections.len();
        for distance in 1..=PREFETCH_DISTANCE {
            if let Some(index) = self.current_section.checked_add(distance)
                && index < section_count
            {
                self.queue_prefetch(index)?;
            }
        }
        for distance in 1..=PREFETCH_DISTANCE {
            if let Some(index) = self.current_section.checked_sub(distance) {
                self.queue_prefetch(index)?;
            }
        }
        self.touch(self.current_section);
        Ok(())
    }

    /// Blocks until all currently queued prefetch work has been collected.
    /// Intended for diagnostics and deterministic tests, not interactive shells.
    pub fn wait_for_prefetch(&mut self) -> Result<(), ReaderError> {
        while !self.prefetch_inflight.is_empty() {
            let result = self
                .prefetch_results
                .recv()
                .map_err(|_| ReaderError::PrefetchWorkerStopped)?;
            self.install_prefetch(result);
        }
        if let Some(index) = self.prefetch_failures.keys().next().copied()
            && let Some(error) = self.prefetch_failures.remove(&index)
        {
            return Err(error);
        }
        Ok(())
    }

    /// Invalidates layout/display caches while preserving approximate progress
    /// inside the active section.
    pub fn resize(&mut self, viewport: LayoutViewport) -> Result<(), ReaderError> {
        if self.viewport == viewport {
            return Ok(());
        }
        let old_count = self.current_page_count();
        let fraction = page_fraction(self.current_page, old_count);
        self.viewport = viewport;
        self.invalidate_layout(fraction)
    }

    pub fn set_style(&mut self, style: ReaderStyle) -> Result<(), ReaderError> {
        if self.style == style {
            return Ok(());
        }
        let fraction = page_fraction(self.current_page, self.current_page_count());
        self.style = style;
        self.invalidate_layout(fraction)
    }

    pub fn cached_section_count(&self) -> usize {
        self.cache.len()
    }

    fn next_page(&mut self) -> Result<PageTurn, ReaderError> {
        if self.current_page + 1 < self.current_page_count() {
            self.current_page += 1;
            return Ok(self.moved());
        }
        let next = self.current_section + 1;
        if next >= self.source.book().sections.len() {
            return Ok(self.boundary());
        }
        self.ensure_section(next)?;
        self.current_section = next;
        self.current_page = 0;
        self.touch(next);
        Ok(self.moved())
    }

    fn previous_page(&mut self) -> Result<PageTurn, ReaderError> {
        if self.current_page > 0 {
            self.current_page -= 1;
            return Ok(self.moved());
        }
        let Some(previous) = self.current_section.checked_sub(1) else {
            return Ok(self.boundary());
        };
        self.ensure_section(previous)?;
        self.current_section = previous;
        self.current_page = self.current_page_count().saturating_sub(1);
        self.touch(previous);
        Ok(self.moved())
    }

    fn ensure_section(&mut self, index: usize) -> Result<(), ReaderError> {
        if self.cache.contains_key(&index) {
            self.touch(index);
            return Ok(());
        }
        if let Some(error) = self.prefetch_failures.remove(&index) {
            return Err(error);
        }
        if self.prefetch_inflight.contains(&index) {
            self.wait_for_section(index)?;
            if self.cache.contains_key(&index) {
                self.touch(index);
                return Ok(());
            }
        }
        let section = compile_section(
            self.source.as_ref(),
            index,
            self.viewport,
            self.style,
            &mut self.layout_engine,
            &self.display_compiler,
        )?;
        self.cache.insert(index, Arc::new(section));
        self.touch(index);
        self.evict();
        Ok(())
    }

    fn invalidate_layout(&mut self, fraction: f32) -> Result<(), ReaderError> {
        self.prefetch_generation.fetch_add(1, Ordering::Release);
        self.prefetch_inflight.clear();
        self.prefetch_failures.clear();
        self.cache.clear();
        self.lru.clear();
        self.ensure_section(self.current_section)?;
        let count = self.current_page_count();
        self.current_page = page_for_fraction(fraction, count);
        Ok(())
    }

    fn queue_prefetch(&mut self, index: usize) -> Result<(), ReaderError> {
        if self.cache.contains_key(&index) || self.prefetch_inflight.contains(&index) {
            return Ok(());
        }
        self.prefetch_requests
            .send(PrefetchRequest {
                index,
                viewport: self.viewport,
                style: self.style,
                generation: self.prefetch_generation.load(Ordering::Acquire),
            })
            .map_err(|_| ReaderError::PrefetchWorkerStopped)?;
        self.prefetch_inflight.insert(index);
        Ok(())
    }

    fn poll_prefetch(&mut self) -> Result<(), ReaderError> {
        loop {
            match self.prefetch_results.try_recv() {
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

    fn wait_for_section(&mut self, index: usize) -> Result<(), ReaderError> {
        while self.prefetch_inflight.contains(&index) {
            let result = self
                .prefetch_results
                .recv()
                .map_err(|_| ReaderError::PrefetchWorkerStopped)?;
            self.install_prefetch(result);
        }
        if let Some(error) = self.prefetch_failures.remove(&index) {
            return Err(error);
        }
        Ok(())
    }

    fn install_prefetch(&mut self, result: PrefetchResult) {
        self.prefetch_inflight.remove(&result.index);
        if result.generation != self.prefetch_generation.load(Ordering::Acquire) {
            return;
        }
        let section = match result.section {
            Ok(section) => section,
            Err(error) => {
                self.prefetch_failures.insert(result.index, error);
                return;
            }
        };
        if self.cache.insert(result.index, section).is_none() {
            self.touch(result.index);
            self.evict();
        }
        self.touch(self.current_section);
    }

    fn current_page_count(&self) -> usize {
        self.cache
            .get(&self.current_section)
            .map_or(0, |section| section.pages.len())
    }

    fn touch(&mut self, index: usize) {
        self.lru.retain(|cached| *cached != index);
        self.lru.push_back(index);
    }

    fn evict(&mut self) {
        while self.cache.len() > self.cache_capacity {
            let Some(candidate) = self.lru.pop_front() else {
                break;
            };
            if candidate == self.current_section {
                self.lru.push_back(candidate);
                continue;
            }
            self.cache.remove(&candidate);
        }
    }

    fn moved(&self) -> PageTurn {
        PageTurn::Moved {
            section_index: self.current_section,
            page_index: self.current_page,
        }
    }

    fn boundary(&self) -> PageTurn {
        PageTurn::Boundary {
            section_index: self.current_section,
            page_index: self.current_page,
        }
    }
}

fn spawn_prefetch_worker(
    source: Arc<dyn BookSource>,
    active_generation: Arc<AtomicU64>,
) -> Result<(Sender<PrefetchRequest>, Receiver<PrefetchResult>), ReaderError> {
    let (request_sender, request_receiver) = mpsc::channel::<PrefetchRequest>();
    let (result_sender, result_receiver) = mpsc::channel::<PrefetchResult>();
    thread::Builder::new()
        .name("rebook-prefetch".into())
        .spawn(move || {
            let mut layout_engine = LayoutEngine::new();
            let display_compiler = DisplayListCompiler;
            while let Ok(request) = request_receiver.recv() {
                if active_generation.load(Ordering::Acquire) != request.generation {
                    continue;
                }
                let section = compile_section(
                    source.as_ref(),
                    request.index,
                    request.viewport,
                    request.style,
                    &mut layout_engine,
                    &display_compiler,
                )
                .map(Arc::new);
                if active_generation.load(Ordering::Acquire) != request.generation {
                    continue;
                }
                if result_sender
                    .send(PrefetchResult {
                        index: request.index,
                        generation: request.generation,
                        section,
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .map_err(ReaderError::PrefetchWorkerStart)?;
    Ok((request_sender, result_receiver))
}

fn compile_section(
    source: &dyn BookSource,
    index: usize,
    viewport: LayoutViewport,
    style: ReaderStyle,
    layout_engine: &mut LayoutEngine,
    display_compiler: &DisplayListCompiler,
) -> Result<CachedSection, ReaderError> {
    let section = source.parse_section(index)?;
    let layout = layout_engine.layout_section(source, &section, viewport, style)?;
    let pages = layout
        .pages
        .iter()
        .map(|page| display_compiler.compile(page))
        .collect();
    Ok(CachedSection { pages })
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
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use rebook_layout::ReaderFontFamily;
    use rebook_publication::{
        Block, BlockStyle, Inline, Metadata, PublicationId, PublicationUrl, Resource, Section,
        SpineItem, SpineItemId, TextBlock, TextBlockKind, TextRun, TextStyle,
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
                descriptors.push(SpineItem {
                    id: id.clone(),
                    href: href.clone(),
                    media_type: "application/xhtml+xml".into(),
                    linear: true,
                    properties: Vec::new(),
                });
                sections.push(Section {
                    id,
                    href,
                    blocks: vec![Block::Text(TextBlock {
                        kind: TextBlockKind::Paragraph,
                        content: vec![Inline::Text(TextRun {
                            text: text.clone(),
                            style: TextStyle::default(),
                            link: None,
                        })],
                        style: BlockStyle::default(),
                        source: None,
                    })],
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

    #[test]
    fn cached_page_turns_and_boundaries_do_not_reparse() {
        let source = CountingSource::new(&["缓存翻页测试。".repeat(600)]);
        let mut reader =
            ReaderSession::open(source.clone(), viewport(600, 400), ReaderStyle::default())
                .unwrap();
        let page_count = reader.location().page_count;
        assert!(page_count > 2);
        assert_eq!(source.parse_count(0), 1);

        assert!(matches!(
            reader.turn_page(PageDirection::Previous).unwrap(),
            PageTurn::Boundary { .. }
        ));
        for _ in 1..page_count {
            assert!(matches!(
                reader.turn_page(PageDirection::Next).unwrap(),
                PageTurn::Moved { .. }
            ));
        }
        assert!(matches!(
            reader.turn_page(PageDirection::Next).unwrap(),
            PageTurn::Boundary { .. }
        ));
        assert_eq!(source.parse_count(0), 1);
    }

    #[test]
    fn chapter_window_prefetch_makes_short_section_switches_cache_only() {
        let source = CountingSource::new(&["第一章".into(), "第二章".into(), "第三章".into()]);
        let mut reader =
            ReaderSession::open(source.clone(), viewport(600, 400), ReaderStyle::default())
                .unwrap();

        reader.prefetch_adjacent().unwrap();
        reader.wait_for_prefetch().unwrap();
        assert_eq!(reader.cached_section_count(), 3);
        assert_eq!(source.parse_count(1), 1);
        assert_eq!(source.parse_count(2), 1);
        reader.turn_page(PageDirection::Next).unwrap();
        assert_eq!(reader.location().section_index, 1);
        assert_eq!(source.parse_count(1), 1);

        reader.prefetch_adjacent().unwrap();
        reader.wait_for_prefetch().unwrap();
        assert_eq!(reader.cached_section_count(), 3);
        assert_eq!(source.parse_count(2), 1);
        reader.turn_page(PageDirection::Next).unwrap();
        assert_eq!(reader.location().section_index, 2);
        assert_eq!(source.parse_count(2), 1);
    }

    #[test]
    fn toc_href_navigation_resolves_fragments_and_uses_the_section_cache() {
        let source = CountingSource::new(&[
            "第一章".repeat(100),
            "第二章".repeat(100),
            "第三章".repeat(100),
        ]);
        let mut reader =
            ReaderSession::open(source.clone(), viewport(600, 400), ReaderStyle::default())
                .unwrap();
        reader.prefetch_adjacent().unwrap();
        reader.wait_for_prefetch().unwrap();

        let target = PublicationUrl::parse("section-1.xhtml#part-2").unwrap();
        assert_eq!(reader.section_index_for_href(&target), Some(1));
        reader.go_to_href(&target).unwrap();

        assert_eq!(reader.location().section_index, 1);
        assert_eq!(reader.location().page_index, 0);
        assert_eq!(source.parse_count(1), 1);
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
        assert_eq!(source.parse_count(0), 2);
        assert_eq!(reader.cached_section_count(), 1);
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
        style.font_family = ReaderFontFamily::SansSerif;
        reader.set_style(style).unwrap();

        let location = reader.location();
        let new_fraction = page_fraction(location.page_index, location.page_count);
        let one_page = page_fraction(1, location.page_count);
        assert!((new_fraction - old_fraction).abs() <= one_page);
        assert_eq!(reader.style().font_family, ReaderFontFamily::SansSerif);
        assert_eq!(source.parse_count(0), 2);
        assert_eq!(reader.cached_section_count(), 1);
    }
}
