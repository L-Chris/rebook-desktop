use rebook_layout::ReaderTypography;

use crate::plugins::PluginSettings;
use crate::preferences::{self, AppLanguage, ReaderPreferences};
use crate::sync::SyncSettings;

use super::{DesktopReader, FollowUp, ReaderOverlay, SnapshotEffects};

impl DesktopReader {
    pub(in crate::reader) fn open_settings(&mut self) {
        self.cancel_text_selection();
        let style = self.reader.style();
        self.ui.draft_spread = style.spread;
        self.ui.draft_typography = style.typography;
        self.ui.font_picker = None;
        self.ui
            .draft_plugin_settings
            .clone_from(&self.plugin_settings);
        self.ui.draft_language = self.language;
        self.ui.draft_sync_settings.clone_from(&self.sync_settings);
        self.ui.draft_sync_password.clear();
        self.set_overlay(ReaderOverlay::Settings);
    }

    pub(in crate::reader) fn apply_settings(&mut self) {
        let mut plugin_settings = self.ui.draft_plugin_settings.clone();
        plugin_settings.normalize();
        let language = self.ui.draft_language;
        let mut sync_settings = self.ui.draft_sync_settings.clone();
        sync_settings.normalize();
        if sync_settings.enabled
            && let Err(error) = sync_settings.validate()
        {
            self.error = Some(format!(
                "{}: {error}",
                language.text("云盘设置无效", "Invalid cloud settings")
            ));
            return;
        }
        let translation_backend_changed = self.plugin_settings.translation_provider
            != plugin_settings.translation_provider
            || self.plugin_settings.translation_model != plugin_settings.translation_model
            || self.plugin_settings.target_language != plugin_settings.target_language
            || self.plugin_settings.providers != plugin_settings.providers
            || (self.language != language
                && plugin_settings.target_language == crate::plugins::TARGET_LANGUAGE_INTERFACE);
        let mut typography = self.ui.draft_typography.clone();
        typography.normalize();
        if let Err(error) =
            self.persist_draft_settings(&plugin_settings, &typography, &sync_settings, language)
        {
            self.error = Some(error);
            return;
        }
        let mut style = self.reader.style();
        style.spread = self.ui.draft_spread;
        style.typography = typography;
        if let Err(error) = self
            .translation_source
            .set_mode(plugin_settings.translation_mode)
            .and_then(|()| {
                if translation_backend_changed {
                    self.translation_source.clear()
                } else {
                    Ok(())
                }
            })
        {
            self.error = Some(format!(
                "{}: {error}",
                language.text("应用翻译设置失败", "Failed to apply translation settings")
            ));
            return;
        }
        let result = self.reader.set_style(style);
        match result {
            Ok(snapshot) => {
                self.plugin_settings = plugin_settings;
                self.language = language;
                self.sync_settings = sync_settings;
                if !self.ui.draft_sync_password.is_empty() {
                    self.sync_password.clone_from(&self.ui.draft_sync_password);
                }
                self.ui.draft_sync_password.clear();
                self.translation.clear_error();
                if translation_backend_changed {
                    self.translation.task.cancel();
                }
                self.apply_snapshot(
                    snapshot,
                    SnapshotEffects {
                        translation: FollowUp::Run,
                        ..SnapshotEffects::static_content_change()
                    },
                );
                self.close_overlay();
            }
            Err(error) => {
                self.error = Some(format!(
                    "{}: {error}",
                    language.text("应用阅读设置失败", "Failed to apply reading settings")
                ));
            }
        }
    }

    fn persist_draft_settings(
        &self,
        plugin_settings: &PluginSettings,
        typography: &ReaderTypography,
        sync_settings: &SyncSettings,
        language: AppLanguage,
    ) -> Result<(), String> {
        plugin_settings.save_default().map_err(|error| {
            format!(
                "{}: {error}",
                language.text("保存插件设置失败", "Failed to save plugin settings")
            )
        })?;
        preferences::save_reader_preferences(&ReaderPreferences {
            typography: typography.clone(),
            language,
        })
        .map_err(|error| {
            format!(
                "{}: {error}",
                language.text("保存通用设置失败", "Failed to save general settings")
            )
        })?;
        sync_settings.save_default().map_err(|error| {
            format!(
                "{}: {error}",
                language.text("保存云盘设置失败", "Failed to save cloud settings")
            )
        })?;
        if !self.ui.draft_sync_password.is_empty() {
            sync_settings
                .save_password(&self.ui.draft_sync_password)
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
}
