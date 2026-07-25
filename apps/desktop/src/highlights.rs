use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use directories::ProjectDirs;
use rebook_publication::{Rgba, SourceRange};
use serde::{Deserialize, Serialize};

const STORE_VERSION: u32 = 1;
const STORE_FILE: &str = "highlights.json";
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

pub type HighlightResult<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HighlightColor {
    #[default]
    Yellow,
    Green,
    Blue,
    Pink,
}

impl HighlightColor {
    pub const ALL: [Self; 4] = [Self::Yellow, Self::Green, Self::Blue, Self::Pink];

    pub const fn rgba(self) -> Rgba {
        match self {
            Self::Yellow => Rgba {
                red: 250,
                green: 204,
                blue: 21,
                alpha: 92,
            },
            Self::Green => Rgba {
                red: 74,
                green: 222,
                blue: 128,
                alpha: 80,
            },
            Self::Blue => Rgba {
                red: 96,
                green: 165,
                blue: 250,
                alpha: 82,
            },
            Self::Pink => Rgba {
                red: 244,
                green: 114,
                blue: 182,
                alpha: 78,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredHighlight {
    pub id: String,
    pub book_id: String,
    pub ranges: Vec<SourceRange>,
    pub quote: String,
    pub color: HighlightColor,
    pub created_at: u64,
}

impl StoredHighlight {
    pub fn new(
        book_id: String,
        ranges: Vec<SourceRange>,
        quote: String,
        color: HighlightColor,
    ) -> Self {
        let created_at = unix_timestamp_millis();
        let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        Self {
            id: format!("{}-{created_at}-{sequence}", std::process::id()),
            book_id,
            ranges,
            quote,
            color,
            created_at,
        }
    }
}

pub struct HighlightStore {
    path: PathBuf,
    highlights: Vec<StoredHighlight>,
}

#[derive(Default, Serialize, Deserialize)]
struct StoredHighlights {
    version: u32,
    #[serde(default)]
    highlights: Vec<StoredHighlight>,
}

impl HighlightStore {
    pub fn load_default() -> HighlightResult<Self> {
        let project = ProjectDirs::from("com", "Rebook", "Rebook")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "无法确定高亮数据目录"))?;
        Self::load_from(project.data_local_dir().join(STORE_FILE))
    }

    fn load_from(path: PathBuf) -> HighlightResult<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let stored = if path.exists() {
            serde_json::from_slice::<StoredHighlights>(&fs::read(&path)?)?
        } else {
            StoredHighlights {
                version: STORE_VERSION,
                highlights: Vec::new(),
            }
        };
        if stored.version != STORE_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("不支持的高亮数据版本：{}", stored.version),
            )
            .into());
        }
        Ok(Self {
            path,
            highlights: stored.highlights,
        })
    }

    pub fn for_book(&self, book_id: &str) -> Vec<StoredHighlight> {
        let mut highlights = self
            .highlights
            .iter()
            .filter(|highlight| highlight.book_id == book_id)
            .cloned()
            .collect::<Vec<_>>();
        highlights.sort_by_key(|highlight| std::cmp::Reverse(highlight.created_at));
        highlights
    }

    pub fn insert(&mut self, highlight: StoredHighlight) -> HighlightResult<()> {
        self.highlights.push(highlight);
        if let Err(error) = self.persist() {
            self.highlights.pop();
            return Err(error);
        }
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> HighlightResult<bool> {
        let Some(index) = self
            .highlights
            .iter()
            .position(|highlight| highlight.id == id)
        else {
            return Ok(false);
        };
        let removed = self.highlights.remove(index);
        if let Err(error) = self.persist() {
            self.highlights.insert(index, removed);
            return Err(error);
        }
        Ok(true)
    }

    fn persist(&self) -> HighlightResult<()> {
        let stored = StoredHighlights {
            version: STORE_VERSION,
            highlights: self.highlights.clone(),
        };
        fs::write(&self.path, serde_json::to_vec_pretty(&stored)?)?;
        Ok(())
    }
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
            "rebook-highlights-{}-{}.json",
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
        let highlight = StoredHighlight::new(
            "book-a".into(),
            vec![range],
            "text".into(),
            HighlightColor::Yellow,
        );
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
