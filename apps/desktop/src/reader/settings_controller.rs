use crate::settings::AppliedSettings;

use super::{DesktopReader, FollowUp, SnapshotEffects};

impl DesktopReader {
    pub(in crate::reader) fn request_settings(&mut self) {
        self.cancel_text_selection();
        self.close_overlay();
        self.settings_requested = true;
    }

    pub(crate) fn take_settings_request(&mut self) -> bool {
        std::mem::take(&mut self.settings_requested)
    }

    pub(crate) fn apply_global_settings(&mut self, settings: &AppliedSettings) {
        let plugin_settings = settings.plugin_settings.clone();
        let language = settings.language;
        let translation_backend_changed = self.plugin_settings.translation_provider
            != plugin_settings.translation_provider
            || self.plugin_settings.translation_model != plugin_settings.translation_model
            || self.plugin_settings.target_language != plugin_settings.target_language
            || self.plugin_settings.providers != plugin_settings.providers
            || (self.language != language
                && plugin_settings.target_language == crate::plugins::TARGET_LANGUAGE_INTERFACE);

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

        let mut style = self.reader.style();
        style.spread = settings.spread;
        style.typography.clone_from(&settings.typography);
        match self.reader.set_style(style) {
            Ok(snapshot) => {
                self.plugin_settings = plugin_settings;
                self.language = language;
                self.sync_settings.clone_from(&settings.sync_settings);
                self.sync_password.clone_from(&settings.sync_password);
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
            }
            Err(error) => {
                self.error = Some(format!(
                    "{}: {error}",
                    language.text("应用阅读设置失败", "Failed to apply reading settings")
                ));
            }
        }
    }
}
