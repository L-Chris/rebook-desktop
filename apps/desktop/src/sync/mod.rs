mod engine;
mod protocol;
mod settings;
mod store;
mod webdav;

pub(crate) use engine::{LocalSyncBook, RemoteBookDownload, SyncReport, run_sync};
pub(crate) use settings::SyncSettings;
pub(crate) use store::SyncStore;

pub(crate) type SyncResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
