use peniko::Blob;
use rebook_layout::{LayoutEngine, ReaderTypography, SpreadMode};

use crate::plugins::PluginSettings;
use crate::preferences::{self, AppLanguage, ReaderPreferences};
use crate::sync::SyncSettings;

mod egui_view;

pub(crate) use egui_view::settings_overlay;

#[derive(Clone)]
pub(crate) struct AppliedSettings {
    pub(crate) spread: SpreadMode,
    pub(crate) typography: ReaderTypography,
    pub(crate) plugin_settings: PluginSettings,
    pub(crate) language: AppLanguage,
    pub(crate) sync_settings: SyncSettings,
    pub(crate) sync_password: String,
}

pub(crate) struct SettingsFeature {
    settings_tab: SettingsTab,
    draft_spread: SpreadMode,
    draft_typography: ReaderTypography,
    draft_plugin_settings: PluginSettings,
    draft_language: AppLanguage,
    draft_sync_settings: SyncSettings,
    draft_sync_password: String,
    available_font_families: Vec<String>,
    applied: AppliedSettings,
    revision: u64,
    error: Option<String>,
    open: bool,
}

impl SettingsFeature {
    pub(crate) fn new(reader_fonts: &[Blob<u8>]) -> Self {
        let preferences = preferences::load_reader_preferences().unwrap_or_else(|error| {
            tracing::warn!(%error, "failed to load reader preferences; using defaults");
            ReaderPreferences::default()
        });
        let plugin_settings = PluginSettings::load_default().unwrap_or_else(|error| {
            tracing::warn!(%error, "failed to load plugin settings; using defaults");
            PluginSettings::default()
        });
        let sync_settings = SyncSettings::load_default().unwrap_or_else(|error| {
            tracing::warn!(%error, "failed to load WebDAV settings; using defaults");
            SyncSettings::new_device()
        });
        let sync_password = sync_settings.load_password().unwrap_or_else(|error| {
            tracing::warn!(%error, "failed to load WebDAV credential");
            String::new()
        });
        let available_font_families =
            LayoutEngine::with_fonts(reader_fonts.iter().cloned()).available_font_families();
        let applied = AppliedSettings {
            spread: preferences.spread,
            typography: preferences.typography,
            plugin_settings,
            language: preferences.language,
            sync_settings,
            sync_password,
        };
        Self {
            settings_tab: SettingsTab::Reading,
            draft_spread: applied.spread,
            draft_typography: applied.typography.clone(),
            draft_plugin_settings: applied.plugin_settings.clone(),
            draft_language: applied.language,
            draft_sync_settings: applied.sync_settings.clone(),
            draft_sync_password: String::new(),
            available_font_families,
            applied,
            revision: 0,
            error: None,
            open: false,
        }
    }

    pub(crate) fn open(&mut self) {
        self.settings_tab = SettingsTab::Reading;
        self.draft_spread = self.applied.spread;
        self.draft_typography.clone_from(&self.applied.typography);
        self.draft_plugin_settings
            .clone_from(&self.applied.plugin_settings);
        self.draft_language = self.applied.language;
        self.draft_sync_settings
            .clone_from(&self.applied.sync_settings);
        self.draft_sync_password.clear();
        self.error = None;
        self.open = true;
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn applied(&self) -> &AppliedSettings {
        &self.applied
    }

    fn close_overlay(&mut self) {
        self.open = false;
    }

    fn apply_settings(&mut self) {
        let mut plugin_settings = self.draft_plugin_settings.clone();
        plugin_settings.normalize();
        let mut typography = self.draft_typography.clone();
        typography.normalize();
        let mut sync_settings = self.draft_sync_settings.clone();
        sync_settings.normalize();
        let language = self.draft_language;
        if sync_settings.enabled
            && let Err(error) = sync_settings.validate()
        {
            self.error = Some(format!(
                "{}: {error}",
                language.text("云盘设置无效", "Invalid cloud settings")
            ));
            return;
        }
        if let Err(error) = persist_settings(
            self.draft_spread,
            &typography,
            &plugin_settings,
            language,
            &sync_settings,
            &self.draft_sync_password,
        ) {
            self.error = Some(error);
            return;
        }
        let sync_password = if self.draft_sync_password.is_empty() {
            self.applied.sync_password.clone()
        } else {
            self.draft_sync_password.clone()
        };
        self.applied = AppliedSettings {
            spread: self.draft_spread,
            typography,
            plugin_settings,
            language,
            sync_settings,
            sync_password,
        };
        self.draft_sync_password.clear();
        self.error = None;
        self.revision = self.revision.wrapping_add(1);
        self.close_overlay();
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }
}

fn persist_settings(
    spread: SpreadMode,
    typography: &ReaderTypography,
    plugin_settings: &PluginSettings,
    language: AppLanguage,
    sync_settings: &SyncSettings,
    sync_password: &str,
) -> Result<(), String> {
    plugin_settings.save_default().map_err(|error| {
        format!(
            "{}: {error}",
            language.text("保存 AI 设置失败", "Failed to save AI settings")
        )
    })?;
    preferences::save_reader_preferences(&ReaderPreferences {
        typography: typography.clone(),
        language,
        spread,
    })
    .map_err(|error| {
        format!(
            "{}: {error}",
            language.text("保存阅读设置失败", "Failed to save reader settings")
        )
    })?;
    sync_settings.save_default().map_err(|error| {
        format!(
            "{}: {error}",
            language.text("保存云盘设置失败", "Failed to save cloud settings")
        )
    })?;
    if !sync_password.is_empty() {
        sync_settings
            .save_password(sync_password)
            .map_err(|error| {
                format!(
                    "{}: {error}",
                    language.text(
                        "保存 Windows 凭据失败",
                        "Failed to save the Windows credential"
                    )
                )
            })?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SettingsTab {
    #[default]
    Reading,
    Font,
    Ai,
    AiChat,
    Translation,
    Cloud,
}
