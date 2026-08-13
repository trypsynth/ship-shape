#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::{
	cell::RefCell,
	env,
	path::PathBuf,
	process::{self, Command},
	sync::{
		Arc,
		atomic::{AtomicBool, AtomicU64, Ordering},
	},
	thread,
	time::Duration,
};

use patois::t;
use pulldown_cmark::{Event, Parser, TagEnd};
use wxdragon::{ffi, prelude::*, window::WxWidget};

use crate::{UpdateChannel, UpdateCheckOutcome, UpdateError, UpdaterConfig, check_for_updates, download_update_file};

thread_local! {
	static ACTIVE_PROGRESS: RefCell<Option<ProgressDialog>> = const { RefCell::new(None) };
}

/// Guards against a second update-check flow (silent startup check, manual "Check for
/// Updates", or an impatient double-click while a download is stuck) from starting while
/// one is already running. Without this, two concurrent downloads race on the same temp
/// file and a completing stale check can silently destroy the progress dialog belonging to
/// a newer one.
static UPDATE_CHECK_ACTIVE: AtomicBool = AtomicBool::new(false);

struct ParentWindow {
	handle: *mut ffi::wxd_Window_t,
}

impl WxWidget for ParentWindow {
	fn handle_ptr(&self) -> *mut ffi::wxd_Window_t {
		self.handle
	}
}

/// Convert markdown to plain text suitable for display in a read-only `TextCtrl`.
///
/// Preserves paragraph breaks, headings, list items, and inline code; strips all other markdown syntax.
#[must_use]
pub fn markdown_to_text(markdown: &str) -> String {
	let mut text = String::new();
	let parser = Parser::new(markdown);
	for event in parser {
		match event {
			Event::Text(s) | Event::Code(s) => text.push_str(&s),
			Event::End(TagEnd::Paragraph | TagEnd::Heading(_)) => text.push_str("\n\n"),
			Event::End(TagEnd::Item) => text.push('\n'),
			_ => {}
		}
	}
	text.trim().to_string()
}

/// Show the "update available" dialog and return `true` if the user accepted.
///
/// `app_display_name` appears in the body label (e.g. `"A new version of My App is available."`).
pub fn show_update_dialog(parent: &dyn WxWidget, new_version: &str, changelog: &str, app_display_name: &str) -> bool {
	const PADDING: i32 = 10;
	let title = t("Update to %s").replace("%s", new_version);
	let dialog = Dialog::builder(parent, &title).build();
	let panel = Panel::builder(&dialog).build();
	let label = t("A new version of %s is available. Here's what's new:").replace("%s", app_display_name);
	let message = StaticText::builder(&panel).with_label(&label).build();
	let changelog_ctrl = TextCtrl::builder(&panel)
		.with_value(changelog)
		.with_style(TextCtrlStyle::MultiLine | TextCtrlStyle::ReadOnly | TextCtrlStyle::Rich2)
		.with_size(Size::new(500, 300))
		.build();
	let yes_label = t("&Yes");
	let no_label = t("&No");
	let yes_button = Button::builder(&panel).with_id(ID_OK).with_label(&yes_label).build();
	let no_button = Button::builder(&panel).with_id(ID_CANCEL).with_label(&no_label).build();
	dialog.set_escape_id(ID_CANCEL);
	dialog.set_affirmative_id(ID_OK);
	let content_sizer = BoxSizer::builder(Orientation::Vertical).build();
	content_sizer.add(&message, 0, SizerFlag::All, PADDING);
	content_sizer.add(
		&changelog_ctrl,
		1,
		SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
		PADDING,
	);
	let button_sizer = BoxSizer::builder(Orientation::Horizontal).build();
	button_sizer.add_stretch_spacer(1);
	button_sizer.add(&yes_button, 0, SizerFlag::Right, PADDING);
	button_sizer.add(&no_button, 0, SizerFlag::Right, PADDING);
	content_sizer.add_sizer(&button_sizer, 0, SizerFlag::Expand | SizerFlag::All, 0);
	panel.set_sizer(content_sizer, true);
	let dialog_sizer = BoxSizer::builder(Orientation::Vertical).build();
	dialog_sizer.add(&panel, 1, SizerFlag::Expand, 0);
	dialog.set_sizer_and_fit(dialog_sizer, true);
	dialog.centre();
	dialog.raise();
	changelog_ctrl.set_focus();
	dialog.show_modal() == ID_OK
}

/// Spawn a background thread that checks for updates and drives the entire update UI flow:
/// update-available dialog -> progress dialog -> download + verify -> launch installer/extractor.
///
/// `window_handle` must be `frame.handle_ptr() as usize` and must remain valid for the lifetime of the update flow. `silent` suppresses the "you're up to date" and error dialogs while still showing the update dialog when one is found.
///
/// If an update check or download is already in progress, this is a no-op: it is safe to call
/// from both a silent startup check and a user-triggered menu action without risking two
/// concurrent downloads fighting over the same temp file and progress dialog.
pub fn run_update_check(
	config: Arc<UpdaterConfig>,
	window_handle: usize,
	current_version: &str,
	current_commit: &str,
	is_installer: bool,
	channel: UpdateChannel,
	silent: bool,
) {
	if UPDATE_CHECK_ACTIVE.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
		return;
	}
	let version = current_version.to_string();
	let commit = current_commit.to_string();
	thread::spawn(move || {
		let outcome = check_for_updates(&config, &version, &commit, is_installer, channel);
		wxdragon::call_after(Box::new(move || {
			present_update_result(config, window_handle, outcome, silent, &version);
		}));
	});
}

fn present_update_result(
	config: Arc<UpdaterConfig>,
	window_handle: usize,
	outcome: Result<UpdateCheckOutcome, UpdateError>,
	silent: bool,
	current_version: &str,
) {
	let handle = window_handle as *mut ffi::wxd_Window_t;
	let parent = ParentWindow { handle };
	match outcome {
		Ok(UpdateCheckOutcome::UpdateAvailable(result)) => {
			let latest_version =
				if result.latest_version.is_empty() { current_version.to_string() } else { result.latest_version };
			let plain_notes = markdown_to_text(&result.release_notes);
			let release_notes =
				if plain_notes.trim().is_empty() { t("No release notes provided.") } else { plain_notes };
			if !show_update_dialog(&parent, &latest_version, &release_notes, &config.app_display_name)
				|| result.download_url.is_empty()
			{
				UPDATE_CHECK_ACTIVE.store(false, Ordering::SeqCst);
				return;
			}
			let download_url = result.download_url;
			let signature_url = result.signature_url;
			let progress_title = t("%s Update").replace("%s", &config.app_display_name);
			let downloading_msg = t("Downloading update...");
			let progress = ProgressDialog::builder(&parent, &progress_title, &downloading_msg, 100)
				.with_style(
					ProgressDialogStyle::AutoHide
						| ProgressDialogStyle::AppModal
						| ProgressDialogStyle::RemainingTime
						| ProgressDialogStyle::CanAbort,
				)
				.build();
			ACTIVE_PROGRESS.with(|p| {
				*p.borrow_mut() = Some(progress);
			});
			let downloaded = Arc::new(AtomicU64::new(0));
			let total = Arc::new(AtomicU64::new(0));
			let is_running = Arc::new(AtomicBool::new(true));
			let cancelled = Arc::new(AtomicBool::new(false));
			// Heartbeat thread: updates the progress dialog from the main thread every 200 ms.
			let hb_downloaded = downloaded.clone();
			let hb_total = total.clone();
			let hb_is_running = is_running.clone();
			let hb_cancelled = cancelled.clone();
			thread::spawn(move || {
				while hb_is_running.load(Ordering::Relaxed) && !hb_cancelled.load(Ordering::Relaxed) {
					let d = hb_downloaded.load(Ordering::Relaxed);
					let t = hb_total.load(Ordering::Relaxed);
					let hb_cancelled_c = hb_cancelled.clone();
					wxdragon::call_after(Box::new(move || {
						ACTIVE_PROGRESS.with(|p| {
							let keep_going = {
								let borrow = p.borrow();
								if let Some(dialog) = borrow.as_ref() {
									if let Some(percent) =
										d.saturating_mul(100).checked_div(t).and_then(|v| i32::try_from(v).ok())
									{
										dialog.update(percent, None)
									} else {
										dialog.pulse(None)
									}
								} else {
									return;
								}
							};
							if !keep_going {
								// Signal the download thread to abort immediately instead of
								// letting the transfer run to completion (or its 10-minute
								// timeout) unattended in the background.
								hb_cancelled_c.store(true, Ordering::Relaxed);
								if let Some(dialog) = p.borrow().as_ref() {
									dialog.update(100, None);
								}
								*p.borrow_mut() = None;
							}
						});
					}));
					thread::sleep(Duration::from_millis(200));
				}
			});
			// Download thread.
			let d_downloaded = downloaded;
			let d_total = total;
			let d_is_running = is_running;
			let d_cancelled = cancelled;
			thread::spawn(move || {
				let res = download_update_file(&config, &download_url, &signature_url, &d_cancelled, |d, t| {
					d_downloaded.store(d, Ordering::Relaxed);
					d_total.store(t, Ordering::Relaxed);
				});
				d_is_running.store(false, Ordering::Relaxed);
				wxdragon::call_after(Box::new(move || {
					ACTIVE_PROGRESS.with(|p| {
						*p.borrow_mut() = None;
					});
					if !d_cancelled.load(Ordering::Relaxed) {
						execute_update(&config, window_handle, res);
					}
					UPDATE_CHECK_ACTIVE.store(false, Ordering::SeqCst);
				}));
			});
		}
		Ok(UpdateCheckOutcome::UpToDate(ver)) => {
			if !silent {
				let msg = if ver.trim().is_empty() {
					t("No updates available.")
				} else {
					t("No updates available. Latest version: %s").replace("%s", &ver)
				};
				let title = t("Info");
				let dialog = MessageDialog::builder(&parent, &msg, &title)
					.with_style(
						MessageDialogStyle::OK | MessageDialogStyle::IconInformation | MessageDialogStyle::Centre,
					)
					.build();
				dialog.show_modal();
			}
			UPDATE_CHECK_ACTIVE.store(false, Ordering::SeqCst);
		}
		Err(e) => {
			if !silent {
				let err_title = t("Error");
				let (msg, title) = match &e {
					UpdateError::VerificationError(m) => (
						t("Security verification failed. The update might have been tampered with: %s")
							.replace("%s", m),
						t("Security Error"),
					),
					_ => (e.to_string(), err_title),
				};
				let dialog = MessageDialog::builder(&parent, &msg, &title)
					.with_style(MessageDialogStyle::OK | MessageDialogStyle::IconError | MessageDialogStyle::Centre)
					.build();
				dialog.show_modal();
			}
			UPDATE_CHECK_ACTIVE.store(false, Ordering::SeqCst);
		}
	}
}

#[cfg(any(target_os = "windows", test))]
fn ps_quote(s: &str) -> String {
	format!("'{}'", s.replace('\'', "''"))
}

#[cfg(any(target_os = "windows", test))]
fn installer_script(pid: u32, installer_args: &[String], installer: &str, current_exe: &str) -> String {
	let arg_clause = if installer_args.is_empty() {
		String::new()
	} else {
		let args = installer_args.iter().map(|a| ps_quote(a)).collect::<Vec<_>>().join(",");
		format!(" -ArgumentList {args}")
	};
	format!(
		"Start-Sleep -Seconds 1; Wait-Process -Id {pid} -ErrorAction SilentlyContinue; Start-Process -FilePath {}{arg_clause} -Wait; Start-Process -FilePath {}",
		ps_quote(installer),
		ps_quote(current_exe)
	)
}

#[cfg(any(target_os = "windows", test))]
fn zip_update_script(pid: u32, zip: &str, dest_dir: &str, current_exe: &str) -> String {
	format!(
		"Start-Sleep -Seconds 1; Wait-Process -Id {pid} -ErrorAction SilentlyContinue; Expand-Archive -Path {zip_q} -DestinationPath {dest_q} -Force; Remove-Item -Path {zip_q} -Force; Start-Process {exe_q}",
		zip_q = ps_quote(zip),
		dest_q = ps_quote(dest_dir),
		exe_q = ps_quote(current_exe)
	)
}

fn execute_update(config: &UpdaterConfig, window_handle: usize, result: Result<PathBuf, UpdateError>) {
	let handle = window_handle as *mut ffi::wxd_Window_t;
	let parent = ParentWindow { handle };
	let err_title = t("Error");
	let path = match result {
		Ok(p) => p,
		Err(e) => {
			let msg = format!("{}: {e}", t("Update failed"));
			let dialog = MessageDialog::builder(&parent, &msg, &err_title)
				.with_style(MessageDialogStyle::OK | MessageDialogStyle::IconError)
				.build();
			dialog.show_modal();
			return;
		}
	};
	let is_exe = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("exe"));
	let is_zip = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("zip"));
	#[cfg(not(target_os = "windows"))]
	{
		let _ = (is_exe, is_zip, config);
		let msg = t("Update downloaded to: %s\nPlease install it manually.").replace("%s", &path.display().to_string());
		let ready_title = t("Update Ready");
		let dialog = MessageDialog::builder(&parent, &msg, &ready_title)
			.with_style(MessageDialogStyle::OK | MessageDialogStyle::IconInformation)
			.build();
		dialog.show_modal();
		return;
	}
	#[cfg(target_os = "windows")]
	{
		let current_exe = match env::current_exe() {
			Ok(p) => p,
			Err(e) => {
				let msg = format!("{}: {e}", t("Failed to get current exe path"));
				let dialog = MessageDialog::builder(&parent, &msg, &err_title)
					.with_style(MessageDialogStyle::OK | MessageDialogStyle::IconError)
					.build();
				dialog.show_modal();
				return;
			}
		};
		if is_exe {
			let script = installer_script(
				process::id(),
				&config.installer_args,
				&path.display().to_string(),
				&current_exe.display().to_string(),
			);
			if let Err(e) = Command::new("powershell.exe")
				.arg("-NoProfile")
				.arg("-ExecutionPolicy")
				.arg("Bypass")
				.arg("-Command")
				.arg(&script)
				.creation_flags(0x0800_0000) // CREATE_NO_WINDOW
				.spawn()
			{
				let msg = format!("{}: {e}", t("Failed to launch installer script"));
				let dialog = MessageDialog::builder(&parent, &msg, &err_title)
					.with_style(MessageDialogStyle::OK | MessageDialogStyle::IconError)
					.build();
				dialog.show_modal();
				return;
			}
			process::exit(0);
		} else if is_zip {
			let exe_dir = current_exe.parent().unwrap_or(&current_exe);
			let script = zip_update_script(
				process::id(),
				&path.display().to_string(),
				&exe_dir.display().to_string(),
				&current_exe.display().to_string(),
			);
			if let Err(e) = Command::new("powershell.exe")
				.arg("-NoProfile")
				.arg("-ExecutionPolicy")
				.arg("Bypass")
				.arg("-Command")
				.arg(&script)
				.creation_flags(0x0800_0000) // CREATE_NO_WINDOW
				.spawn()
			{
				let msg = format!("{}: {e}", t("Failed to launch update script"));
				let dialog = MessageDialog::builder(&parent, &msg, &err_title)
					.with_style(MessageDialogStyle::OK | MessageDialogStyle::IconError)
					.build();
				dialog.show_modal();
				return;
			}
			process::exit(0);
		} else {
			let msg = t("Unknown update file format.");
			let dialog = MessageDialog::builder(&parent, &msg, &err_title)
				.with_style(MessageDialogStyle::OK | MessageDialogStyle::IconError)
				.build();
			dialog.show_modal();
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn ps_quote_wraps_in_single_quotes() {
		assert_eq!(ps_quote(r"C:\Temp\app_setup.exe"), r"'C:\Temp\app_setup.exe'");
	}

	#[test]
	fn ps_quote_doubles_embedded_single_quotes() {
		assert_eq!(ps_quote(r"C:\Users\O'Brien\app.exe"), r"'C:\Users\O''Brien\app.exe'");
	}

	#[test]
	fn installer_script_uses_configured_args() {
		let script = installer_script(42, &["/S".to_string()], r"C:\Temp\app_setup.exe", r"C:\App\app.exe");
		assert_eq!(
			script,
			r"Start-Sleep -Seconds 1; Wait-Process -Id 42 -ErrorAction SilentlyContinue; Start-Process -FilePath 'C:\Temp\app_setup.exe' -ArgumentList '/S' -Wait; Start-Process -FilePath 'C:\App\app.exe'"
		);
	}

	#[test]
	fn installer_script_quotes_each_arg() {
		let args = ["/S".to_string(), r"/D=C:\Program Files\App".to_string()];
		let script = installer_script(1, &args, r"C:\t\s.exe", r"C:\a\a.exe");
		assert!(script.contains(r"-ArgumentList '/S','/D=C:\Program Files\App' -Wait"));
	}

	#[test]
	fn installer_script_omits_argument_list_when_empty() {
		let script = installer_script(7, &[], r"C:\t\s.exe", r"C:\a\a.exe");
		assert!(!script.contains("-ArgumentList"));
		assert!(script.contains(r"Start-Process -FilePath 'C:\t\s.exe' -Wait"));
	}

	#[test]
	fn installer_script_escapes_quotes_in_paths() {
		let script = installer_script(7, &[], r"C:\Users\O'Brien\s.exe", r"C:\Users\O'Brien\a.exe");
		assert!(script.contains(r"'C:\Users\O''Brien\s.exe'"));
		assert!(script.contains(r"'C:\Users\O''Brien\a.exe'"));
	}

	#[test]
	fn zip_script_escapes_quotes_in_paths() {
		let script =
			zip_update_script(7, r"C:\Users\O'Brien\app.zip", r"C:\Users\O'Brien", r"C:\Users\O'Brien\app.exe");
		assert!(
			script.contains(
				r"Expand-Archive -Path 'C:\Users\O''Brien\app.zip' -DestinationPath 'C:\Users\O''Brien' -Force"
			)
		);
		assert!(script.contains(r"Remove-Item -Path 'C:\Users\O''Brien\app.zip' -Force"));
		assert!(script.contains(r"Start-Process 'C:\Users\O''Brien\app.exe'"));
	}
}
