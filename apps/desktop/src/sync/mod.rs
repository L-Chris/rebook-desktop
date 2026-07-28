mod engine;
mod protocol;
mod settings;
mod store;
mod webdav;

pub(crate) use engine::{LocalSyncBook, SyncReport, run_sync};
pub(crate) use settings::{CloudProviderKind, SYNC_INTERVAL_OPTIONS, SyncSettings};
pub(crate) use store::SyncStore;

pub(crate) type SyncResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
