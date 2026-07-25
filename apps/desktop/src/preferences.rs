use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use rebook_layout::ReaderTypography;
use serde::{Deserialize, Serialize};

const SETTINGS_VERSION: u32 = 1;
const SETTINGS_FILE: &str = "reader-settings.json";

pub type PreferencesResult<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Serialize, Deserialize)]
struct StoredReaderPreferences {
    version: u32,
    #[serde(default)]
    typography: ReaderTypography,
}

pub fn load_reader_typography() -> PreferencesResult<ReaderTypography> {
    load_from(settings_path()?)
}

pub fn save_reader_typography(typography: &ReaderTypography) -> PreferencesResult<()> {
    save_to(&settings_path()?, typography)
}

fn settings_path() -> io::Result<PathBuf> {
    let project = ProjectDirs::from("com", "Rebook", "Rebook")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "无法确定阅读设置目录"))?;
    Ok(project.config_dir().join(SETTINGS_FILE))
}

fn load_from(path: PathBuf) -> PreferencesResult<ReaderTypography> {
    if !path.exists() {
        return Ok(ReaderTypography::default());
    }
    let stored: StoredReaderPreferences = serde_json::from_slice(&fs::read(path)?)?;
    if stored.version != SETTINGS_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("不支持的阅读设置版本：{}", stored.version),
        )
        .into());
    }
    let mut typography = stored.typography;
    typography.normalize();
    Ok(typography)
}

fn save_to(path: &Path, typography: &ReaderTypography) -> PreferencesResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "阅读设置路径没有父目录"))?;
    fs::create_dir_all(parent)?;
    let mut typography = typography.clone();
    typography.normalize();
    let stored = StoredReaderPreferences {
        version: SETTINGS_VERSION,
        typography,
    };
    persist_json(path, &serde_json::to_vec_pretty(&stored)?)?;
    Ok(())
}

fn persist_json(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, bytes)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rebook_layout::ReaderDefaultFont;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn typography_round_trips_and_normalizes() {
        let path = test_path();
        let typography = ReaderTypography {
            default_font: ReaderDefaultFont::SansSerif,
            default_cjk_font: "  Microsoft YaHei  ".into(),
            serif_font: "Literata".into(),
            sans_serif_font: "Noto Sans".into(),
            monospace_font: "Fira Code".into(),
            font_size: 18.0,
            minimum_font_size: 9.0,
            font_weight: 550,
        };

        save_to(&path, &typography).unwrap();
        let loaded = load_from(path.clone()).unwrap();

        assert_eq!(loaded.default_font, ReaderDefaultFont::SansSerif);
        assert_eq!(loaded.default_cjk_font, "Microsoft YaHei");
        assert!((loaded.font_size - 18.0).abs() < f32::EPSILON);
        assert!((loaded.minimum_font_size - 9.0).abs() < f32::EPSILON);
        assert_eq!(loaded.font_weight, 600);
        fs::remove_file(path).unwrap();
    }

    fn test_path() -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "rebook-reader-settings-{}-{timestamp}.json",
            std::process::id()
        ))
    }
}
