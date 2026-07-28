mod application;
mod gpu;

pub(crate) use application::run;

pub(crate) enum UserEvent {
    RepaintAfter(std::time::Duration),
    ShelfSync(crate::shelf::SyncTaskMessage),
    ReaderSearch(crate::reader::SearchTaskMessage),
    ReaderChat(crate::reader::ChatTaskMessage),
    ReaderTranslation(crate::reader::TranslationTaskMessage),
    ReaderTocTranslation(crate::reader::TocTranslationTaskMessage),
}
