use std::path::Path;
use std::sync::Arc;

use xilem::core::map_state;
use xilem::masonry::peniko::Blob;
use xilem::view::{sized_box, zstack};
use xilem::{AnyWidgetView, WidgetView};

use crate::library::LocalLibrary;
use crate::reader::{DesktopReader, app_view};
use crate::settings::{SettingsFeature, settings_view};
use crate::shelf::{ShelfFeature, view as shelf_view};

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

    fn promote_opened_reader(&mut self) {
        if self.reader.is_none() {
            self.reader = self.shelf.take_opened_reader();
        }
    }
}

pub(crate) fn root_view(state: &mut DesktopApp) -> Box<AnyWidgetView<DesktopApp>> {
    if state
        .reader
        .as_ref()
        .is_some_and(|reader| reader.exit_requested)
    {
        state.reader = None;
        state.shelf.resume();
    }
    state.promote_opened_reader();

    let settings_requested = state.reader.as_mut().map_or_else(
        || state.shelf.take_settings_request(),
        DesktopReader::take_settings_request,
    );
    if settings_requested {
        state.settings.open();
    }

    let settings_revision = state.settings.revision();
    if settings_revision != state.applied_settings_revision {
        let applied = state.settings.applied().clone();
        state.shelf.apply_global_settings(&applied);
        if let Some(reader) = state.reader.as_mut() {
            reader.apply_global_settings(&applied);
        }
        state.applied_settings_revision = settings_revision;
    }

    let page: Box<AnyWidgetView<DesktopApp>> = if let Some(reader) = state.reader.as_mut() {
        let reader_view = app_view(reader);
        map_state(reader_view, |state: &mut DesktopApp| {
            state.reader.as_mut().expect("reader exists")
        })
        .boxed()
    } else {
        let view = shelf_view(&mut state.shelf);
        map_state(view, |state: &mut DesktopApp| &mut state.shelf).boxed()
    };
    let overlay = settings_view(&mut state.settings);
    let overlay = map_state(overlay, |state: &mut DesktopApp| &mut state.settings).boxed();
    sized_box(zstack((page, overlay))).expand().boxed()
}
