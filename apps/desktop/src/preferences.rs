use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use rebook_layout::{ReaderTypography, SpreadMode};
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum AppTheme {
    #[default]
    #[serde(rename = "light")]
    Light,
    #[serde(rename = "dark")]
    Dark,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReaderPreferences {
    pub(crate) typography: ReaderTypography,
    pub(crate) language: AppLanguage,
    pub(crate) spread: SpreadMode,
    pub(crate) theme: AppTheme,
}

impl Default for ReaderPreferences {
    fn default() -> Self {
        Self {
            typography: ReaderTypography::default(),
            language: AppLanguage::default(),
            spread: SpreadMode::Double,
            theme: AppTheme::default(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct StoredReaderPreferences {
    version: u32,
    #[serde(default)]
    typography: ReaderTypography,
    #[serde(default)]
    language: AppLanguage,
    #[serde(default)]
    theme: AppTheme,
    #[serde(default = "default_spread")]
    spread: StoredSpreadMode,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum StoredSpreadMode {
    Single,
    #[default]
    Double,
}

impl From<StoredSpreadMode> for SpreadMode {
    fn from(value: StoredSpreadMode) -> Self {
        match value {
            StoredSpreadMode::Single => Self::Single,
            StoredSpreadMode::Double => Self::Double,
        }
    }
}

impl From<SpreadMode> for StoredSpreadMode {
    fn from(value: SpreadMode) -> Self {
        match value {
            SpreadMode::Single => Self::Single,
            SpreadMode::Double => Self::Double,
        }
    }
}

const fn default_spread() -> StoredSpreadMode {
    StoredSpreadMode::Double
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
        spread: stored.spread.into(),
        theme: stored.theme,
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
        spread: preferences.spread.into(),
        theme: preferences.theme,
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
            spread: SpreadMode::Single,
            theme: AppTheme::Dark,
        };
        save_to(&path, &preferences).unwrap();
        let loaded = load_from(path.clone()).unwrap();

        assert_eq!(loaded.typography.default_font, ReaderDefaultFont::SansSerif);
        assert_eq!(loaded.typography.default_cjk_font, "Microsoft YaHei");
        assert!((loaded.typography.font_size - 18.0).abs() < f32::EPSILON);
        assert!((loaded.typography.minimum_font_size - 9.0).abs() < f32::EPSILON);
        assert_eq!(loaded.typography.font_weight, 600);
        assert_eq!(loaded.language, AppLanguage::English);
        assert_eq!(loaded.spread, SpreadMode::Single);
        assert_eq!(loaded.theme, AppTheme::Dark);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn legacy_preferences_default_to_simplified_chinese() {
        let json = r#"{"version":1}"#;
        let stored: StoredReaderPreferences = serde_json::from_str(json).unwrap();
        assert_eq!(stored.language, AppLanguage::SimplifiedChinese);
        assert!(matches!(stored.spread, StoredSpreadMode::Double));
        assert_eq!(stored.theme, AppTheme::Light);
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
