use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use rebook_layout::ReaderTypography;
use serde::{Deserialize, Serialize};

use crate::persistence::write_json_atomic;

const SETTINGS_VERSION: u32 = 1;
const SETTINGS_FILE: &str = "reader-settings.json";

pub type PreferencesResult<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum AppLanguage {
    #[default]
    #[serde(rename = "zh-CN")]
    SimplifiedChinese,
    #[serde(rename = "en")]
    English,
}

impl AppLanguage {
    pub(crate) const fn text(
        self,
        simplified_chinese: &'static str,
        english: &'static str,
    ) -> &'static str {
        match self {
            Self::SimplifiedChinese => simplified_chinese,
            Self::English => english,
        }
    }

    pub(crate) const fn translation_target(self) -> &'static str {
        match self {
            Self::SimplifiedChinese => "简体中文",
            Self::English => "English",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ReaderPreferences {
    pub(crate) typography: ReaderTypography,
    pub(crate) language: AppLanguage,
}

#[derive(Serialize, Deserialize)]
struct StoredReaderPreferences {
    version: u32,
    #[serde(default)]
    typography: ReaderTypography,
    #[serde(default)]
    language: AppLanguage,
}

pub(crate) fn load_reader_preferences() -> PreferencesResult<ReaderPreferences> {
    load_from(settings_path()?)
}

pub(crate) fn save_reader_preferences(preferences: &ReaderPreferences) -> PreferencesResult<()> {
    save_to(&settings_path()?, preferences)
}

pub(crate) fn load_app_language() -> PreferencesResult<AppLanguage> {
    Ok(load_reader_preferences()?.language)
}

pub(crate) fn save_app_language(language: AppLanguage) -> PreferencesResult<()> {
    let mut preferences = load_reader_preferences()?;
    preferences.language = language;
    save_reader_preferences(&preferences)
}

fn settings_path() -> io::Result<PathBuf> {
    let project = ProjectDirs::from("com", "Rebook", "Rebook")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "无法确定阅读设置目录"))?;
    Ok(project.config_dir().join(SETTINGS_FILE))
}

fn load_from(path: PathBuf) -> PreferencesResult<ReaderPreferences> {
    if !path.exists() {
        return Ok(ReaderPreferences::default());
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
    Ok(ReaderPreferences {
        typography,
        language: stored.language,
    })
}

fn save_to(path: &Path, preferences: &ReaderPreferences) -> PreferencesResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "阅读设置路径没有父目录"))?;
    fs::create_dir_all(parent)?;
    let mut typography = preferences.typography.clone();
    typography.normalize();
    let stored = StoredReaderPreferences {
        version: SETTINGS_VERSION,
        typography,
        language: preferences.language,
    };
    write_json_atomic(path, &stored)?;
    Ok(())
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

        let preferences = ReaderPreferences {
            typography,
            language: AppLanguage::English,
        };
        save_to(&path, &preferences).unwrap();
        let loaded = load_from(path.clone()).unwrap();

        assert_eq!(loaded.typography.default_font, ReaderDefaultFont::SansSerif);
        assert_eq!(loaded.typography.default_cjk_font, "Microsoft YaHei");
        assert!((loaded.typography.font_size - 18.0).abs() < f32::EPSILON);
        assert!((loaded.typography.minimum_font_size - 9.0).abs() < f32::EPSILON);
        assert_eq!(loaded.typography.font_weight, 600);
        assert_eq!(loaded.language, AppLanguage::English);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn legacy_preferences_default_to_simplified_chinese() {
        let json = r#"{"version":1}"#;
        let stored: StoredReaderPreferences = serde_json::from_str(json).unwrap();
        assert_eq!(stored.language, AppLanguage::SimplifiedChinese);
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
