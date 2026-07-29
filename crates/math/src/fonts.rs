use std::num::NonZeroUsize;
use std::sync::LazyLock;

use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Style, Weight};
use lru::LruCache;
use parking_lot::Mutex;

#[derive(Hash, PartialEq, Eq, Clone)]
struct MeasureKey {
    text: String,
    font_size_bits: u32,
    is_code: bool,
    is_bold: bool,
    is_italic: bool,
    max_width_bits: Option<u32>,
}

pub trait TextMeasure {
    fn measure_text(
        &mut self,
        text: &str,
        font_size: f32,
        is_code: bool,
        is_bold: bool,
        is_italic: bool,
        max_width: Option<f32>,
    ) -> (f32, f32);
}

/// Global font system with thread-safe LRU cache.
/// This singleton ensures:
/// - Single `FontSystem` instance (expensive to create)
/// - Shared cache across all measurements
/// - Automatic LRU eviction to manage memory
static GLOBAL_FONT_SYSTEM: LazyLock<GlobalFontSystem> = LazyLock::new(GlobalFontSystem::new);

struct GlobalFontSystem {
    font_system: Mutex<FontSystem>,
    cache: Mutex<LruCache<MeasureKey, (f32, f32)>>,
}

impl GlobalFontSystem {
    fn new() -> Self {
        // Cache up to ~10MB of text measurements (estimated ~100 bytes per entry average)
        const CACHE_CAPACITY: usize = 100_000;
        Self {
            font_system: Mutex::new(FontSystem::new()),
            cache: Mutex::new(LruCache::new(NonZeroUsize::new(CACHE_CAPACITY).unwrap())),
        }
    }

    fn measure_text(
        &self,
        text: &str,
        font_size: f32,
        is_code: bool,
        is_bold: bool,
        is_italic: bool,
        max_width: Option<f32>,
    ) -> (f32, f32) {
        let key = MeasureKey {
            text: text.to_string(),
            font_size_bits: font_size.to_bits(),
            is_code,
            is_bold,
            is_italic,
            max_width_bits: max_width.map(f32::to_bits),
        };

        // Check cache first
        {
            let mut cache = self.cache.lock();
            if let Some(&cached) = cache.get(&key) {
                return cached;
            }
        }

        // Perform measurement with font_system lock
        let measured = {
            let mut font_system = self.font_system.lock();
            measure_text_impl(
                &mut font_system,
                text,
                font_size,
                is_code,
                is_bold,
                is_italic,
                max_width,
            )
        };

        // Store in cache
        {
            let mut cache = self.cache.lock();
            cache.put(key, measured);
        }

        measured
    }

    #[allow(dead_code)]
    #[inline]
    fn cache_len(&self) -> usize {
        self.cache.lock().len()
    }
}

fn measure_text_impl(
    font_system: &mut FontSystem,
    text: &str,
    font_size: f32,
    is_code: bool,
    is_bold: bool,
    is_italic: bool,
    max_width: Option<f32>,
) -> (f32, f32) {
    let line_height = font_size * 1.2;
    let mut buffer = Buffer::new(
        font_system,
        Metrics {
            font_size,
            line_height,
        },
    );

    buffer.set_size(font_system, max_width, None);

    let attrs = Attrs::new()
        .family(if is_code {
            Family::Monospace
        } else {
            Family::SansSerif
        })
        .weight(if is_bold {
            Weight::BOLD
        } else {
            Weight::NORMAL
        })
        .style(if is_italic {
            Style::Italic
        } else {
            Style::Normal
        });

    buffer.set_text(font_system, text, &attrs, Shaping::Advanced, None);

    let mut total_width: f32 = 0.0;
    let mut total_height: f32 = 0.0;

    for run in buffer.layout_runs() {
        total_width = total_width.max(run.line_w);
        total_height += run.line_height;
    }

    (total_width, total_height)
}

/// Text measurement using the global font system with LRU cache.
/// Multiple instances share the same underlying font system and cache.
pub struct CosmicTextMeasure;

impl CosmicTextMeasure {
    pub fn new() -> Result<Self, String> {
        // Initialize the global system (no-op if already initialized)
        let _ = &*GLOBAL_FONT_SYSTEM;
        Ok(Self)
    }

    /// Returns the number of entries currently in the global cache.
    #[allow(dead_code)]
    pub fn cache_size() -> usize {
        GLOBAL_FONT_SYSTEM.cache_len()
    }
}

impl TextMeasure for CosmicTextMeasure {
    fn measure_text(
        &mut self,
        text: &str,
        font_size: f32,
        is_code: bool,
        is_bold: bool,
        is_italic: bool,
        max_width: Option<f32>,
    ) -> (f32, f32) {
        GLOBAL_FONT_SYSTEM.measure_text(text, font_size, is_code, is_bold, is_italic, max_width)
    }
}

impl Default for CosmicTextMeasure {
    fn default() -> Self {
        Self::new().expect("Failed to initialize font system")
    }
}
