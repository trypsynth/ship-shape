use std::path::PathBuf;

use patois::t;
use wxdragon::prelude::*;

use crate::{UpdateError, UpdaterConfig};

use super::ParentWindow;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "windows", test))]
mod windows;

/// Launch the platform-specific install flow for a verified, downloaded update file.
pub(super) fn execute_update(config: &UpdaterConfig, parent: &ParentWindow, result: Result<PathBuf, UpdateError>) {
	let err_title = t("Error");
	let path = match result {
		Ok(p) => p,
		Err(e) => {
			show_error(parent, &err_title, &format!("{}: {e}", t("Update failed")));
			return;
		}
	};
	let is_exe = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("exe"));
	let is_zip = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("zip"));
	let is_dmg = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("dmg"));
	#[cfg(not(any(target_os = "windows", target_os = "macos")))]
	{
		let _ = (is_exe, is_zip, is_dmg, config);
		let msg = t("Update downloaded to: %s\nPlease install it manually.").replace("%s", &path.display().to_string());
		let dialog = MessageDialog::builder(parent, &msg, &t("Update Ready"))
			.with_style(MessageDialogStyle::OK | MessageDialogStyle::IconInformation)
			.build();
		dialog.show_modal();
		return;
	}
	#[cfg(target_os = "macos")]
	{
		let _ = (is_exe, is_zip);
		macos::install(config, parent, &path, is_dmg, &err_title);
	}
	#[cfg(target_os = "windows")]
	{
		let _ = is_dmg;
		windows::install(config, parent, &path, is_exe, is_zip, &err_title);
	}
}

pub(super) fn show_error(parent: &ParentWindow, title: &str, msg: &str) {
	let dialog =
		MessageDialog::builder(parent, msg, title).with_style(MessageDialogStyle::OK | MessageDialogStyle::IconError).build();
	dialog.show_modal();
}
