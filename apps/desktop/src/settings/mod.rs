use std::sync::Arc;
use std::time::{Duration, Instant};

use rebook_layout::{LayoutEngine, ReaderTypography, SpreadMode};
use xilem::core::fork;
use xilem::masonry::peniko::Blob;
use xilem::masonry::properties::types::AsUnit;
use xilem::view::{label, sized_box, task};
use xilem::{AnyWidgetView, WidgetView};

use crate::plugins::PluginSettings;
use crate::preferences::{self, AppLanguage, ReaderPreferences};
use crate::sync::SyncSettings;

mod view;

const MOTION_DURATION: Duration = Duration::from_millis(180);
const MOTION_FRAME_INTERVAL: Duration = Duration::from_millis(16);
const MOTION_EPSILON: f32 = 0.001;

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
    font_picker: Option<FontPickerKind>,
    draft_plugin_settings: PluginSettings,
    draft_language: AppLanguage,
    draft_sync_settings: SyncSettings,
    draft_sync_password: String,
    available_font_families: Arc<[String]>,
    applied: AppliedSettings,
    revision: u64,
    error: Option<String>,
    motion: Motion,
    last_motion_tick: Option<Instant>,
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
        let mut layout = LayoutEngine::with_fonts(reader_fonts.iter().cloned());
        let available_font_families = layout.available_font_families().into();
        let applied = AppliedSettings {
            spread: preferences.spread,
            typography: preferences.typography,
            plugin_settings,
            language: preferences.language,
            sync_settings,
            sync_password,
        };
        Self {
            settings_tab: SettingsTab::General,
            draft_spread: applied.spread,
            draft_typography: applied.typography.clone(),
            font_picker: None,
            draft_plugin_settings: applied.plugin_settings.clone(),
            draft_language: applied.language,
            draft_sync_settings: applied.sync_settings.clone(),
            draft_sync_password: String::new(),
            available_font_families,
            applied,
            revision: 0,
            error: None,
            motion: Motion::settled(0.0),
            last_motion_tick: None,
        }
    }

    pub(crate) fn open(&mut self) {
        self.settings_tab = SettingsTab::General;
        self.draft_spread = self.applied.spread;
        self.draft_typography.clone_from(&self.applied.typography);
        self.font_picker = None;
        self.draft_plugin_settings
            .clone_from(&self.applied.plugin_settings);
        self.draft_language = self.applied.language;
        self.draft_sync_settings
            .clone_from(&self.applied.sync_settings);
        self.draft_sync_password.clear();
        self.error = None;
        if self.motion.animate_to(1.0) {
            self.last_motion_tick = Some(Instant::now());
        }
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn applied(&self) -> &AppliedSettings {
        &self.applied
    }

    fn close_overlay(&mut self) {
        if self.motion.animate_to(0.0) {
            self.last_motion_tick = Some(Instant::now());
        }
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

    fn advance_motion(&mut self, now: Instant) {
        let delta = self
            .last_motion_tick
            .replace(now)
            .map_or(Duration::ZERO, |last| now.saturating_duration_since(last));
        self.motion.advance(delta);
        if !self.motion.is_animating() {
            self.last_motion_tick = None;
        }
    }
}

pub(crate) fn settings_view(state: &mut SettingsFeature) -> Box<AnyWidgetView<SettingsFeature>> {
    let progress = state.motion.value.clamp(0.0, 1.0);
    let layer: Box<AnyWidgetView<SettingsFeature>> = if state.motion.is_visible() {
        view::settings_overlay(state, progress).boxed()
    } else {
        sized_box(label("")).width(0.px()).height(0.px()).boxed()
    };
    fork(
        layer,
        state.motion.is_animating().then(|| {
            task(
                |proxy| async move {
                    let mut interval = xilem::tokio::time::interval(MOTION_FRAME_INTERVAL);
                    interval.set_missed_tick_behavior(xilem::tokio::time::MissedTickBehavior::Skip);
                    loop {
                        interval.tick().await;
                        if proxy.message(Instant::now()).is_err() {
                            break;
                        }
                    }
                },
                |state: &mut SettingsFeature, now| state.advance_motion(now),
            )
        }),
    )
    .boxed()
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
            language.text("保存插件设置失败", "Failed to save plugin settings")
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
    General,
    Reading,
    Font,
    Cloud,
    Ai,
    AiChat,
    Translation,
    Plugins,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FontPickerKind {
    Cjk,
    Serif,
    SansSerif,
    Monospace,
}

impl FontPickerKind {
    const fn title(self, language: AppLanguage) -> &'static str {
        match self {
            Self::Cjk => language.text("中文字体", "CJK font"),
            Self::Serif => language.text("衬线字体", "Serif font"),
            Self::SansSerif => language.text("无衬线字体", "Sans-serif font"),
            Self::Monospace => language.text("等宽字体", "Monospace font"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Motion {
    value: f32,
    start: f32,
    target: f32,
    elapsed: Duration,
}

impl Motion {
    const fn settled(value: f32) -> Self {
        Self {
            value,
            start: value,
            target: value,
            elapsed: Duration::ZERO,
        }
    }

    fn animate_to(&mut self, target: f32) -> bool {
        if (self.target - target).abs() <= MOTION_EPSILON {
            return false;
        }
        self.start = self.value;
        self.target = target;
        self.elapsed = Duration::ZERO;
        true
    }

    fn advance(&mut self, delta: Duration) {
        if !self.is_animating() {
            return;
        }
        self.elapsed = self.elapsed.saturating_add(delta);
        let progress = (self.elapsed.as_secs_f32() / MOTION_DURATION.as_secs_f32()).min(1.0);
        let eased = if self.target < self.start {
            progress.powi(2)
        } else {
            1.0 - (1.0 - progress).powi(3)
        };
        self.value = self.start + (self.target - self.start) * eased;
        if progress >= 1.0 {
            self.value = self.target;
            self.start = self.target;
            self.elapsed = Duration::ZERO;
        }
    }

    fn is_animating(self) -> bool {
        (self.value - self.target).abs() > MOTION_EPSILON
    }

    fn is_visible(self) -> bool {
        self.value > MOTION_EPSILON
    }
}

#[cfg(test)]
mod tests {
    use super::view::font_candidates;
    use super::*;

    #[test]
    fn settings_motion_keeps_the_layer_alive_during_exit() {
        let mut motion = Motion::settled(1.0);
        motion.animate_to(0.0);
        motion.advance(MOTION_DURATION / 2);
        assert!(motion.is_visible());
        assert!(motion.is_animating());
        motion.advance(MOTION_DURATION / 2);
        assert!(!motion.is_visible());
    }

    #[test]
    fn cjk_font_candidates_keep_reader_defaults_and_filter_latin_families() {
        let available: Arc<[String]> = [
            "Arial".to_owned(),
            "LXGW WenKai".to_owned(),
            "Microsoft YaHei UI".to_owned(),
        ]
        .into();

        let candidates = font_candidates(FontPickerKind::Cjk, &available);

        assert_eq!(candidates[0], "LXGW WenKai GB Screen");
        assert_eq!(
            candidates
                .iter()
                .filter(|family| family.as_str() == "LXGW WenKai")
                .count(),
            1
        );
        assert!(
            candidates
                .iter()
                .any(|family| family == "Microsoft YaHei UI")
        );
        assert!(!candidates.iter().any(|family| family == "Arial"));
    }
}
