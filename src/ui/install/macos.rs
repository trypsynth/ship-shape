use std::{path::Path, process::Command};

use patois::t;
use wxdragon::prelude::*;

use crate::UpdaterConfig;

use super::{ParentWindow, show_error};

/// Mount the downloaded disk image and prompt the user to finish installing by hand.
///
/// There's no way to self-replace a running, Gatekeeper-checked `.app` bundle the way the
/// Windows install script does, so this stops at opening the mounted image; dragging the app
/// into Applications is left to the user.
pub(super) fn install(config: &UpdaterConfig, parent: &ParentWindow, path: &Path, is_dmg: bool, err_title: &str) {
	if !is_dmg {
		show_error(parent, err_title, &t("Unknown update file format."));
		return;
	}
	// `open` mounts the disk image and shows it in a Finder window, the same as
	// double-clicking it.
	if let Err(e) = Command::new("open").arg(path).spawn() {
		show_error(parent, err_title, &format!("{}: {e}", t("Failed to open disk image")));
		return;
	}
	let msg = t(
		"The update has been downloaded and its disk image opened. Quit %s and drag the new version into Applications to finish installing.",
	)
	.replace("%s", &config.app_display_name);
	let dialog = MessageDialog::builder(parent, &msg, &t("Update Ready"))
		.with_style(MessageDialogStyle::OK | MessageDialogStyle::IconInformation)
		.build();
	dialog.show_modal();
}
