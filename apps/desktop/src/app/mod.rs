use std::path::Path;
use std::sync::Arc;

use xilem::core::map_state;
use xilem::masonry::peniko::Blob;
use xilem::{AnyWidgetView, WidgetView};

use crate::library::LocalLibrary;
use crate::reader::{DesktopReader, app_view};
use crate::shelf::{ShelfFeature, view as shelf_view};

pub(crate) struct DesktopApp {
    shelf: ShelfFeature,
    reader: Option<DesktopReader>,
}

impl DesktopApp {
    pub(crate) fn new(library: LocalLibrary, reader_fonts: Arc<[Blob<u8>]>) -> Self {
        Self {
            shelf: ShelfFeature::new(library, reader_fonts),
            reader: None,
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

    if let Some(reader) = state.reader.as_mut() {
        let reader_view = app_view(reader);
        map_state(reader_view, |state: &mut DesktopApp| {
            state.reader.as_mut().expect("reader exists")
        })
        .boxed()
    } else {
        let view = shelf_view(&mut state.shelf);
        map_state(view, |state: &mut DesktopApp| &mut state.shelf).boxed()
    }
}
