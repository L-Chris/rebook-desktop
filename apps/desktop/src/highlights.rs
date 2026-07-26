use std::fs;
use std::io;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use directories::ProjectDirs;
use rebook_publication::SourceRange;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::sync::{SyncSettings, SyncStore};

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

pub struct HighlightStore {
    store: SyncStore,
}

#[derive(Default, Serialize, Deserialize)]
struct LegacyStoredHighlights {
    version: u32,
    #[serde(default)]
    highlights: Vec<StoredHighlight>,
}

impl HighlightStore {
    pub fn load_default() -> HighlightResult<Self> {
        let project = ProjectDirs::from("com", "Rebook", "Rebook")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "无法确定高亮数据目录"))?;
        let settings = SyncSettings::load_default()?;
        let store = SyncStore::open_default(settings.device_id)?;
        migrate_legacy(&store, &project.data_local_dir().join(LEGACY_STORE_FILE))?;
        Ok(Self { store })
    }

    #[cfg(test)]
    fn load_from(path: PathBuf) -> HighlightResult<Self> {
        let store = SyncStore::open_at(path, "test-device")?;
        Ok(Self { store })
    }

    pub fn for_book(&self, book_id: &str) -> Vec<StoredHighlight> {
        self.store.annotations_for_book(book_id).map_or_else(
            |error| {
                tracing::warn!(%error, "failed to load highlights from sync store");
                Vec::new()
            },
            |annotations| {
                annotations
                    .into_iter()
                    .map(|annotation| StoredHighlight {
                        id: annotation.id,
                        book_id: annotation.book_id,
                        ranges: annotation.ranges,
                        quote: annotation.quote,
                        created_at: annotation.created_at,
                    })
                    .collect()
            },
        )
    }

    pub fn insert(&mut self, highlight: StoredHighlight) -> HighlightResult<()> {
        self.store.create_annotation(
            highlight.id,
            highlight.book_id,
            highlight.ranges,
            highlight.quote,
            highlight.created_at,
        )?;
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> HighlightResult<bool> {
        self.store.delete_annotation(id)
    }
}

fn migrate_legacy(store: &SyncStore, path: &Path) -> HighlightResult<()> {
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
        store.import_legacy_annotation(
            highlight.id,
            highlight.book_id,
            highlight.ranges,
            highlight.quote,
            highlight.created_at,
        )?;
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
    use super::*;
    use rebook_publication::{SourceAnchor, SpineItemId};

    #[test]
    fn highlights_round_trip_and_are_scoped_by_book() {
        let path = std::env::temp_dir().join(format!(
            "rebook-highlights-{}-{}.sqlite3",
            std::process::id(),
            unix_timestamp_millis()
        ));
        let mut store = HighlightStore::load_from(path.clone()).unwrap();
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
        store.insert(highlight).unwrap();

        let mut loaded = HighlightStore::load_from(path.clone()).unwrap();
        assert_eq!(loaded.for_book("book-a").len(), 1);
        assert!(loaded.for_book("book-b").is_empty());
        assert!(loaded.remove(&id).unwrap());
        assert!(
            HighlightStore::load_from(path.clone())
                .unwrap()
                .for_book("book-a")
                .is_empty()
        );

        fs::remove_file(path).unwrap();
    }
}
