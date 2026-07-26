use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use directories::ProjectDirs;
use rebook_publication::SourceRange;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const LEGACY_STORE_VERSION: u32 = 1;
const LEGACY_STORE_FILE: &str = "highlights.json";

pub type HighlightResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredHighlight {
    pub id: String,
    pub book_id: String,
    pub ranges: Vec<SourceRange>,
    pub quote: String,
    pub created_at: u64,
}

impl StoredHighlight {
    pub fn new(book_id: String, ranges: Vec<SourceRange>, quote: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            book_id,
            ranges,
            quote,
            created_at: unix_timestamp_millis(),
        }
    }
}

pub(crate) trait HighlightRepository: Send + Sync {
    fn highlights_for_book(&self, book_id: &str) -> HighlightResult<Vec<StoredHighlight>>;
    fn insert_highlight(&self, highlight: &StoredHighlight) -> HighlightResult<()>;
    fn remove_highlight(&self, id: &str) -> HighlightResult<bool>;
    fn import_legacy_highlight(&self, highlight: &StoredHighlight) -> HighlightResult<()>;
}

pub struct HighlightStore {
    repository: Arc<dyn HighlightRepository>,
}

#[derive(Default, Serialize, Deserialize)]
struct LegacyStoredHighlights {
    version: u32,
    #[serde(default)]
    highlights: Vec<StoredHighlight>,
}

impl HighlightStore {
    pub(crate) fn from_repository(
        repository: impl HighlightRepository + 'static,
    ) -> HighlightResult<Self> {
        let project = ProjectDirs::from("com", "Rebook", "Rebook")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "无法确定高亮数据目录"))?;
        Self::from_repository_at(
            Arc::new(repository),
            &project.data_local_dir().join(LEGACY_STORE_FILE),
        )
    }

    fn from_repository_at(
        repository: Arc<dyn HighlightRepository>,
        legacy_path: &Path,
    ) -> HighlightResult<Self> {
        migrate_legacy(repository.as_ref(), legacy_path)?;
        Ok(Self { repository })
    }

    pub fn for_book(&self, book_id: &str) -> Vec<StoredHighlight> {
        self.repository
            .highlights_for_book(book_id)
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "failed to load highlights from sync store");
                Vec::new()
            })
    }

    pub fn insert(&mut self, highlight: &StoredHighlight) -> HighlightResult<()> {
        self.repository.insert_highlight(highlight)
    }

    pub fn remove(&mut self, id: &str) -> HighlightResult<bool> {
        self.repository.remove_highlight(id)
    }
}

fn migrate_legacy(repository: &dyn HighlightRepository, path: &Path) -> HighlightResult<()> {
    if !path.exists() {
        return Ok(());
    }
    let legacy: LegacyStoredHighlights = serde_json::from_slice(&fs::read(path)?)?;
    if legacy.version != LEGACY_STORE_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("不支持的高亮数据版本：{}", legacy.version),
        )
        .into());
    }
    for highlight in legacy.highlights {
        repository.import_legacy_highlight(&highlight)?;
    }
    Ok(())
}

fn unix_timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use rebook_publication::{SourceAnchor, SourceRange, SpineItemId};

    use super::{HighlightRepository, HighlightResult, HighlightStore, StoredHighlight};

    #[derive(Clone, Default)]
    struct MemoryRepository {
        highlights: Arc<Mutex<Vec<StoredHighlight>>>,
    }

    impl HighlightRepository for MemoryRepository {
        fn highlights_for_book(&self, book_id: &str) -> HighlightResult<Vec<StoredHighlight>> {
            Ok(self
                .highlights
                .lock()
                .unwrap()
                .iter()
                .filter(|highlight| highlight.book_id == book_id)
                .cloned()
                .collect())
        }

        fn insert_highlight(&self, highlight: &StoredHighlight) -> HighlightResult<()> {
            self.highlights.lock().unwrap().push(highlight.clone());
            Ok(())
        }

        fn remove_highlight(&self, id: &str) -> HighlightResult<bool> {
            let mut highlights = self.highlights.lock().unwrap();
            let previous_len = highlights.len();
            highlights.retain(|highlight| highlight.id != id);
            Ok(highlights.len() != previous_len)
        }

        fn import_legacy_highlight(&self, highlight: &StoredHighlight) -> HighlightResult<()> {
            self.insert_highlight(highlight)
        }
    }

    fn new_store(repository: &MemoryRepository) -> HighlightStore {
        HighlightStore {
            repository: Arc::new(repository.clone()),
        }
    }

    #[test]
    fn highlights_round_trip_and_are_scoped_by_book() {
        let repository = MemoryRepository::default();
        let mut store = new_store(&repository);
        let range = SourceRange {
            start: SourceAnchor {
                spine: SpineItemId::new("chapter").unwrap(),
                node: "p1".into(),
                text_offset: 2,
            },
            end: SourceAnchor {
                spine: SpineItemId::new("chapter").unwrap(),
                node: "p1".into(),
                text_offset: 6,
            },
        };
        let highlight = StoredHighlight::new("book-a".into(), vec![range], "text".into());
        let id = highlight.id.clone();
        store.insert(&highlight).unwrap();

        let mut loaded = new_store(&repository);
        assert_eq!(loaded.for_book("book-a").len(), 1);
        assert!(loaded.for_book("book-b").is_empty());
        assert!(loaded.remove(&id).unwrap());
        assert!(new_store(&repository).for_book("book-a").is_empty());
    }
}
