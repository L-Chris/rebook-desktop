use std::path::Path;
use std::sync::Arc;

use peniko::Blob;
use vello::Scene;

use crate::library::LocalLibrary;
use crate::platform::UserEvent;
use crate::reader::{
    ChatStreamMessage, ChatTaskMessage, SearchTaskMessage, TocTranslationTaskMessage,
    TranslationTaskMessage,
};
use crate::reader::{DesktopReader, ReaderFramePlan, ReaderPageTexture};
use crate::settings::{SettingsFeature, settings_overlay};
use crate::shelf::{ShelfFeature, SyncTaskMessage};

pub(crate) struct DesktopApp {
    shelf: ShelfFeature,
    reader: Option<DesktopReader>,
    settings: SettingsFeature,
    applied_settings_revision: u64,
}

impl DesktopApp {
    pub(crate) fn new(library: LocalLibrary, reader_fonts: Arc<[Blob<u8>]>) -> Self {
        let settings = SettingsFeature::new(&reader_fonts);
        Self {
            shelf: ShelfFeature::new(library, reader_fonts),
            reader: None,
            settings,
            applied_settings_revision: 0,
        }
    }

    pub(crate) fn open_book(&mut self, path: &Path) {
        self.shelf.open_book(path);
        self.promote_opened_reader();
    }

    pub(crate) fn ui(
        &mut self,
        ui: &mut egui::Ui,
        page_texture: Option<ReaderPageTexture>,
    ) -> Option<ReaderFramePlan> {
        self.reconcile_state();
        let interaction_blocked = self.settings.is_open();
        let plan = if let Some(reader) = self.reader.as_mut() {
            Some(reader.ui(ui, page_texture, interaction_blocked))
        } else {
            self.shelf.ui(ui);
            None
        };

        let settings_requested = self.reader.as_mut().map_or_else(
            || self.shelf.take_settings_request(),
            DesktopReader::take_settings_request,
        );
        if settings_requested {
            self.settings.open();
        }
        settings_overlay(ui.ctx(), &mut self.settings);
        self.apply_settings_if_changed();
        plan
    }

    pub(crate) fn reader_scene(&mut self) -> Option<Arc<Scene>> {
        self.reader.as_mut().map(DesktopReader::page_scene)
    }

    pub(crate) fn spawn_pending_tasks(
        &mut self,
        runtime: &tokio::runtime::Runtime,
        proxy: &winit::event_loop::EventLoopProxy<UserEvent>,
    ) {
        self.shelf.spawn_pending_tasks(runtime, proxy);
        if let Some(reader) = self.reader.as_mut() {
            reader.spawn_pending_tasks(runtime, proxy);
        }
    }

    pub(crate) fn complete_shelf_sync(&mut self, message: SyncTaskMessage) {
        self.shelf.complete_sync(message);
    }

    pub(crate) fn complete_reader_search(&mut self, message: SearchTaskMessage) {
        if let Some(reader) = self.reader.as_mut() {
            reader.complete_search(message);
        }
    }

    pub(crate) fn complete_reader_chat(&mut self, message: ChatTaskMessage) {
        if let Some(reader) = self.reader.as_mut() {
            reader.complete_chat(message);
        }
    }

    pub(crate) fn update_reader_chat_stream(&mut self, message: ChatStreamMessage) {
        if let Some(reader) = self.reader.as_mut() {
            reader.update_chat_stream(message);
        }
    }

    pub(crate) fn complete_reader_translation(&mut self, message: TranslationTaskMessage) {
        if let Some(reader) = self.reader.as_mut() {
            reader.complete_translation(message);
        }
    }

    pub(crate) fn complete_reader_toc_translation(&mut self, message: TocTranslationTaskMessage) {
        if let Some(reader) = self.reader.as_mut() {
            reader.complete_toc_translation(message);
        }
    }

    pub(crate) fn log_reader_diagnostics(&self, event: &'static str, focused: Option<bool>) {
        if let Some(reader) = self.reader.as_ref() {
            reader.log_diagnostic_snapshot(event, focused);
        } else {
            crate::diagnostics::log(
                event,
                &[
                    crate::diagnostics::Field::Text("screen", "shelf"),
                    crate::diagnostics::Field::Text(
                        "focus",
                        match focused {
                            Some(true) => "true",
                            Some(false) => "false",
                            None => "unknown",
                        },
                    ),
                ],
            );
        }
    }

    fn reconcile_state(&mut self) {
        if self
            .reader
            .as_ref()
            .is_some_and(|reader| reader.exit_requested)
        {
            self.reader = None;
            self.shelf.resume();
        }
        self.promote_opened_reader();
        self.apply_settings_if_changed();
    }

    fn promote_opened_reader(&mut self) {
        if self.reader.is_none() {
            self.reader = self.shelf.take_opened_reader();
        }
    }

    fn apply_settings_if_changed(&mut self) {
        let revision = self.settings.revision();
        if revision == self.applied_settings_revision {
            return;
        }
        let applied = self.settings.applied().clone();
        self.shelf.apply_global_settings(&applied);
        if let Some(reader) = self.reader.as_mut() {
            reader.apply_global_settings(&applied);
        }
        self.applied_settings_revision = revision;
    }
}
