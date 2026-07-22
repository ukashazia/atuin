#![deny(unsafe_code)]

pub mod backend;
pub mod database;
pub mod entry;
pub mod store;
pub mod watcher;

pub use backend::{ArboardBackend, ClipboardBackend, ClipboardError};
pub use database::{ClipboardDatabase, SearchOptions};
pub use entry::{ClipboardEntry, ClipboardId};
pub use store::{CLIPBOARD_TAG, ClipboardRecord, ClipboardStore, MaterializedEvent};
pub use watcher::{CaptureSettings, ClipboardWatcher, ObserveOutcome};
