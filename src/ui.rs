use std::{
	process, ptr,
	sync::{
		Arc,
		atomic::{AtomicBool, AtomicU64, Ordering},
	},
	thread,
	time::Duration,
};

use patois::t;
use wxdragon::{ffi, prelude::*, window::WxWidget};

pub use self::markdown::markdown_to_text;
use crate::{
	UpdateAvailableResult, UpdateChannel, UpdateCheckOutcome, UpdateError, UpdaterConfig, check_for_updates,
	download_update_file,
	platform::{self, InstallOutcome},
};

mod markdown;
mod progress;

use progress::{ProgressLifecycle, download_percent};

const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

thread_local! {
	static ACTIVE_PROGRESS: ProgressLifecycle<ProgressDialog> = const { ProgressLifecycle::new() };
}

/// Guards against a second update-check flow (silent startup check, manual "Check for
/// Updates", or an impatient double-click while a download is stuck) from starting while
/// one is already running. Without this, two concurrent downloads race on the same temp
/// file and a completing stale check can silently destroy the progress dialog belonging to
/// a newer one.
static UPDATE_CHECK_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Proof that this flow owns [`UPDATE_CHECK_ACTIVE`]. Releases it when dropped, so every exit
/// path of the flow frees the next check.
struct ActiveCheck;

impl ActiveCheck {
	fn acquire() -> Option<Self> {
		UPDATE_CHECK_ACTIVE.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).ok().map(|_| Self)
	}
}

impl Drop for ActiveCheck {
	fn drop(&mut self) {
		UPDATE_CHECK_ACTIVE.store(false, Ordering::SeqCst);
	}
}

/// A window handle that can cross threads. It is only turned back into a pointer on the main
/// thread, inside `call_after` callbacks.
#[derive(Clone, Copy)]
struct ParentWindow(usize);

impl ParentWindow {
	fn new(window: &dyn WxWidget) -> Self {
		Self(window.handle_ptr().expose_provenance())
	}
}

impl WxWidget for ParentWindow {
	fn handle_ptr(&self) -> *mut ffi::wxd_Window_t {
		ptr::with_exposed_provenance_mut(self.0)
	}
}

/// Who started an update check. Decides which results are worth a dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckTrigger {
	/// A background check, for example at startup. Only an available update shows a dialog.
	Automatic,
	/// The user asked for the check. "Up to date" and errors also show a dialog.
	Manual,
}

#[derive(Default)]
struct DownloadProgress {
	downloaded: AtomicU64,
	total: AtomicU64,
	finished: AtomicBool,
	#[cfg(target_os = "macos")]
	preparing: AtomicBool,
	cancelled: AtomicBool,
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
/// `parent` must stay alive for the whole update flow, since every dialog is shown on top of it.
///
/// If an update check or download is already in progress, this is a no-op: it is safe to call
/// from both a silent startup check and a user-triggered menu action without risking two
/// concurrent downloads fighting over the same temp file and progress dialog.
pub fn run_update_check(
	config: Arc<UpdaterConfig>,
	parent: &dyn WxWidget,
	channel: UpdateChannel,
	trigger: CheckTrigger,
) {
	run_update_check_with_exit_handler(config, parent, channel, trigger, || process::exit(0));
}

/// Like [`run_update_check`], but calls `on_exit` on the UI thread when an installer is ready.
///
/// The handler must save application state and terminate the process. macOS installation is
/// cancelled if the host does not exit within 60 seconds. It is not called on failure or cancel.
pub fn run_update_check_with_exit_handler(
	config: Arc<UpdaterConfig>,
	parent: &dyn WxWidget,
	channel: UpdateChannel,
	trigger: CheckTrigger,
	on_exit: impl FnOnce() + Send + 'static,
) {
	let on_exit: Box<dyn FnOnce() + Send> = Box::new(on_exit);
	let Some(check) = ActiveCheck::acquire() else {
		return;
	};
	let parent = ParentWindow::new(parent);
	thread::spawn(move || {
		let outcome = check_for_updates(&config, channel);
		wxdragon::call_after(Box::new(move || match outcome {
			Ok(UpdateCheckOutcome::UpdateAvailable(result)) => offer_update(config, check, parent, result, on_exit),
			Ok(UpdateCheckOutcome::UpToDate(version)) if trigger == CheckTrigger::Manual => {
				show_up_to_date(parent, &version);
			}
			Err(e) if trigger == CheckTrigger::Manual => show_check_error(parent, &e),
			Ok(UpdateCheckOutcome::UpToDate(_)) | Err(_) => {}
		}));
		// call_after only enqueues; an otherwise idle event loop may not drain
		// the queue until the next natural message.
		wxdragon::wake_up_idle();
	});
}

fn offer_update(
	config: Arc<UpdaterConfig>,
	check: ActiveCheck,
	parent: ParentWindow,
	result: UpdateAvailableResult,
	on_exit: Box<dyn FnOnce() + Send>,
) {
	let plain_notes = markdown_to_text(&result.release_notes);
	let release_notes = if plain_notes.is_empty() { t("No release notes provided.") } else { plain_notes };
	if show_update_dialog(&parent, &result.latest_version, &release_notes, &config.app_display_name) {
		start_download(config, check, parent, result, on_exit);
	}
}

fn start_download(
	config: Arc<UpdaterConfig>,
	check: ActiveCheck,
	parent: ParentWindow,
	result: UpdateAvailableResult,
	on_exit: Box<dyn FnOnce() + Send>,
) {
	let title = t("%s Update").replace("%s", &config.app_display_name);
	let progress_dialog = ProgressDialog::builder(&parent, &title, &t("Downloading update..."), 100)
		.with_style(
			ProgressDialogStyle::AutoHide
				| ProgressDialogStyle::AppModal
				| ProgressDialogStyle::RemainingTime
				| ProgressDialogStyle::CanAbort,
		)
		.build();
	ACTIVE_PROGRESS.with(|p| p.set(progress_dialog));
	let progress = Arc::new(DownloadProgress::default());
	spawn_progress_heartbeat(Arc::clone(&progress));
	thread::spawn(move || {
		let download =
			download_update_file(&config, &result.download_url, &result.signature_url, &progress.cancelled, |d, t| {
				progress.downloaded.store(d, Ordering::Relaxed);
				progress.total.store(t, Ordering::Relaxed);
			});
		#[cfg(target_os = "macos")]
		let download = if progress.cancelled.load(Ordering::Relaxed) {
			if let Ok(ref path) = download {
				platform::remove_private_download(path);
			}
			Err(t("Update cancelled."))
		} else {
			download.map_err(|e| format!("{}: {e}", t("Update failed"))).and_then(|path| {
				progress.preparing.store(true, Ordering::Relaxed);
				let result = platform::install_with_cancel(&config, &path, &progress.cancelled);
				// A manual fallback still needs its DMG. Automatic preparation has copied the
				// bundle, so its private verified download can be removed.
				if !matches!(result, Ok(InstallOutcome::ManualStep(_))) {
					platform::remove_private_download(&path);
				}
				result
			})
		};
		progress.finished.store(true, Ordering::Relaxed);
		wxdragon::call_after(Box::new(move || {
			ACTIVE_PROGRESS.with(|p| {
				p.finish(Box::new(move || {
					ACTIVE_PROGRESS.with(ProgressLifecycle::clear);
					if !progress.cancelled.load(Ordering::Relaxed) {
						#[cfg(not(target_os = "macos"))]
						let download = download
							.map_err(|e| format!("{}: {e}", t("Update failed")))
							.and_then(|path| platform::install(&config, &path));
						install_update(parent, download, on_exit);
					}
					drop(check);
				}));
			});
		}));
		wxdragon::wake_up_idle();
	});
}

/// Updates the progress dialog from the main thread every [`PROGRESS_INTERVAL`] until the
/// download finishes or is cancelled.
fn spawn_progress_heartbeat(progress: Arc<DownloadProgress>) {
	thread::spawn(move || {
		while !progress.finished.load(Ordering::Relaxed) && !progress.cancelled.load(Ordering::Relaxed) {
			let downloaded = progress.downloaded.load(Ordering::Relaxed);
			let total = progress.total.load(Ordering::Relaxed);
			let progress = Arc::clone(&progress);
			wxdragon::call_after(Box::new(move || update_progress_dialog(&progress, downloaded, total)));
			wxdragon::wake_up_idle();
			thread::sleep(PROGRESS_INTERVAL);
		}
	});
}

fn update_progress_dialog(progress: &DownloadProgress, downloaded: u64, total: u64) {
	ACTIVE_PROGRESS.with(|p| {
		let Some(update) = p.begin_update() else {
			return;
		};
		let dialog = update.dialog();
		#[cfg(target_os = "macos")]
		let preparing = progress.preparing.load(Ordering::Relaxed);
		#[cfg(not(target_os = "macos"))]
		let preparing = false;
		let keep_going = if preparing {
			dialog.pulse(Some(&t("Preparing update...")))
		} else {
			download_percent(downloaded, total)
				.map_or_else(|| dialog.pulse(None), |percent| dialog.update(percent, None))
		};
		if !keep_going {
			progress.cancelled.store(true, Ordering::Relaxed);
			p.clear();
		}
		// Dropping `update` invokes any completion queued during the native event-loop yield,
		// after Update/Pulse returns and its reference to the dialog is released.
	});
}

fn install_update(parent: ParentWindow, outcome: Result<InstallOutcome, String>, on_exit: Box<dyn FnOnce() + Send>) {
	match outcome {
		Ok(InstallOutcome::Exit) => on_exit(),
		#[cfg(target_os = "macos")]
		Ok(InstallOutcome::Prepared(update)) => match update.commit() {
			Ok(()) => on_exit(),
			Err(msg) => show_message(parent, &msg, &t("Error"), MessageDialogStyle::IconError),
		},
		Ok(InstallOutcome::ManualStep(msg)) => {
			show_message(parent, &msg, &t("Update Ready"), MessageDialogStyle::IconInformation);
		}
		Err(msg) => show_message(parent, &msg, &t("Error"), MessageDialogStyle::IconError),
	}
}

fn show_up_to_date(parent: ParentWindow, version: &str) {
	let msg = if version.trim().is_empty() {
		t("No updates available.")
	} else {
		t("No updates available. Latest version: %s").replace("%s", version)
	};
	show_message(parent, &msg, &t("Info"), MessageDialogStyle::IconInformation);
}

fn show_check_error(parent: ParentWindow, err: &UpdateError) {
	let (msg, title) = match err {
		UpdateError::Verification(m) => (
			t("Security verification failed. The update might have been tampered with: %s").replace("%s", m),
			t("Security Error"),
		),
		_ => (err.to_string(), t("Error")),
	};
	show_message(parent, &msg, &title, MessageDialogStyle::IconError);
}

fn show_message(parent: ParentWindow, msg: &str, title: &str, icon: MessageDialogStyle) {
	let dialog = MessageDialog::builder(&parent, msg, title)
		.with_style(MessageDialogStyle::OK | icon | MessageDialogStyle::Centre)
		.build();
	dialog.show_modal();
}
