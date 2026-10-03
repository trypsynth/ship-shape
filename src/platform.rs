//! Everything that differs between operating systems. Each platform module provides the same
//! three functions: `asset_name_parts`, `download_dir`, and `install`.

#[cfg(any(target_os = "linux", test))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
mod other;
#[cfg(any(target_os = "windows", test))]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::{asset_name_parts, download_dir, install};
#[cfg(target_os = "macos")]
#[allow(unused_imports, reason = "macOS UI uses cancellable preparation")]
pub use macos::{asset_name_parts, download_dir, install, install_with_cancel, remove_private_download};
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub use other::{asset_name_parts, download_dir, install};
#[cfg(target_os = "windows")]
pub use windows::{asset_name_parts, download_dir, install};

/// What the caller must do after a successful [`install`].
#[allow(dead_code, reason = "each platform constructs only some variants")]
pub enum InstallOutcome {
	/// macOS preparation succeeded; commit on the UI thread before requesting shutdown.
	#[cfg(target_os = "macos")]
	Prepared(macos::PreparedUpdate),
	/// The installer is waiting for this process to exit.
	Exit,
	/// The user must finish the install by hand. Holds the instructions to show.
	ManualStep(String),
}
