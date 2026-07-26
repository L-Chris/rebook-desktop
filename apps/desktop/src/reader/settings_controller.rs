use crate::preferences;

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
        self.set_overlay(ReaderOverlay::Settings);
    }

    pub(in crate::reader) fn apply_settings(&mut self) {
        let mut plugin_settings = self.ui.draft_plugin_settings.clone();
        plugin_settings.normalize();
        let translation_backend_changed = self.plugin_settings.translation_provider
            != plugin_settings.translation_provider
            || self.plugin_settings.translation_model != plugin_settings.translation_model
            || self.plugin_settings.target_language != plugin_settings.target_language
            || self.plugin_settings.providers != plugin_settings.providers;
        if let Err(error) = plugin_settings.save_default() {
            self.error = Some(format!("保存插件设置失败：{error}"));
            return;
        }
        let mut typography = self.ui.draft_typography.clone();
        typography.normalize();
        if let Err(error) = preferences::save_reader_typography(&typography) {
            self.error = Some(format!("保存字体设置失败：{error}"));
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
            self.error = Some(format!("应用翻译设置失败：{error}"));
            return;
        }
        let result = self.reader.set_style(style);
        match result {
            Ok(snapshot) => {
                self.plugin_settings = plugin_settings;
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
            Err(error) => self.error = Some(format!("应用阅读设置失败：{error}")),
        }
    }
}
