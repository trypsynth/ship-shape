mod check;
mod config;
mod download;
mod error;
mod github;
mod http;
mod platform;
mod relaunch;
pub mod ui;
mod version;

pub use check::{UpdateAvailableResult, UpdateChannel, UpdateCheckOutcome, check_for_updates};
pub use config::{InstallKind, UpdaterConfig};
pub use download::download_update_file;
pub use error::UpdateError;
pub use relaunch::relaunched_after_update;
