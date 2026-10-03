use std::{
	env,
	ffi::OsString,
	fs,
	os::unix::fs::MetadataExt,
	path::{Path, PathBuf},
	process::{self, Command, Stdio},
	sync::atomic::{AtomicBool, Ordering},
	thread,
	time::{Duration, Instant, SystemTime},
};

use patois::t;
use tempfile::TempDir;

use super::InstallOutcome;
use crate::{InstallKind, UpdateError, UpdaterConfig};

const STALE_STAGE_AGE: Duration = Duration::from_hours(24);

/// macOS has a single asset kind, a disk image, so `install_kind` is ignored.
pub const fn asset_name_parts(_install_kind: InstallKind) -> (&'static str, &'static str) {
	("", "dmg")
}

#[expect(clippy::unnecessary_wraps, reason = "must match the signature of the other platforms")]
pub fn download_dir(_config: &UpdaterConfig) -> Result<PathBuf, UpdateError> {
	Ok(env::temp_dir())
}

/// A staged update with a helper waiting for an explicit commit. Dropping it cancels the helper
/// and removes the staging directory; accepting it transfers cleanup to the helper.
pub struct PreparedUpdate {
	stage: TempDir,
}

impl PreparedUpdate {
	pub fn commit(self) -> Result<(), String> {
		fs::write(self.stage.path().join("commit"), b"install")
			.map_err(|e| format!("{}: {e}", t("Failed to start installation")))?;
		let _ = self.stage.keep();
		Ok(())
	}
}

/// Prepare an in-place bundle update while the application is still running. A detached helper
/// waits for both an explicit commit and process termination before replacing the bundle.
/// This must only receive a downloaded disk image whose Minisign signature has been verified.
#[allow(dead_code, reason = "UI uses the cancellable entry point; the demo uses install")]
pub fn install(config: &UpdaterConfig, path: &Path) -> Result<InstallOutcome, String> {
	install_with_cancel(config, path, &AtomicBool::new(false))
}

pub fn install_with_cancel(
	config: &UpdaterConfig,
	path: &Path,
	cancelled: &AtomicBool,
) -> Result<InstallOutcome, String> {
	check_cancelled(cancelled)?;
	let current = env::current_exe().map_err(|e| e.to_string())?;
	let Some(bundle) = bundle_for_executable(&current) else {
		return manual_install(config, path);
	};
	if !can_update(&bundle) {
		return manual_install(config, path);
	}
	let Some(parent) = bundle.parent() else {
		return manual_install(config, path);
	};
	// A bundle that no longer verifies, e.g. one modified after install, can still be replaced by hand.
	let Some(team) = signing_team(&bundle).filter(|team| validate_bundle(&bundle, team).is_ok()) else {
		return manual_install(config, path);
	};
	let Ok(stage) = tempfile::Builder::new().prefix(".ship-shape-").tempdir_in(parent) else {
		return manual_install(config, path);
	};
	register_stage(stage.path(), &bundle)?;
	sweep_stale_stages(parent, &bundle, stage.path().metadata().map_err(|e| e.to_string())?.uid(), SystemTime::now());
	let original_stamp = bundle_stamp(&bundle)?;
	let identity = plist_value(&bundle, "CFBundleIdentifier")?;
	let mount = Mount::attach(path)?;
	check_cancelled(cancelled)?;
	let replacement = find_replacement(&mount.point, &identity)?;
	validate_bundle(&replacement, &team)?;
	let staged = stage.path().join("new.app");
	run(Command::new("/usr/bin/ditto").arg(&replacement).arg(&staged), &t("Failed to stage the application"))?;
	check_cancelled(cancelled)?;
	validate_bundle(&staged, &team)?;
	if plist_value(&staged, "CFBundleIdentifier")? != identity {
		return Err(t("The update contains a different application."));
	}
	mount.detach()?;
	check_cancelled(cancelled)?;
	// Failure diagnostics survive cleanup. The helper removes the log after a successful launch.
	let logs = env::var_os("HOME")
		.map(|home| PathBuf::from(home).join("Library/Logs"))
		.ok_or_else(|| t("Could not determine the update log directory."))?;
	fs::create_dir_all(&logs).map_err(|e| e.to_string())?;
	let log = tempfile::Builder::new()
		.prefix("ship-shape-update-")
		.suffix(".log")
		.tempfile_in(logs)
		.map_err(|e| e.to_string())?;
	let (log_file, log_path) = log.keep().map_err(|e| e.to_string())?;
	write_messages(stage.path())?;
	let script_path = stage.path().join("install.sh");
	fs::write(&script_path, include_str!("macos_install.sh")).map_err(|e| e.to_string())?;
	let mut command = Command::new("/bin/sh");
	command
		.arg(&script_path)
		.arg(process::id().to_string())
		.arg(&bundle)
		.arg(stage.path())
		.arg(&log_path)
		.arg(original_stamp)
		.stdin(Stdio::null())
		.stdout(Stdio::from(log_file.try_clone().map_err(|e| e.to_string())?))
		.stderr(Stdio::from(log_file));
	for (name, value) in &config.macos_relaunch_env {
		let mut assignment = OsString::from(name);
		assignment.push("=");
		assignment.push(value);
		command.arg("--env").arg(assignment);
	}
	let mut helper = command.spawn().map_err(|e| format!("{}: {e}", t("Failed to launch update helper")))?;
	let deadline = Instant::now() + Duration::from_secs(5);
	while !stage.path().join("ready").is_file() {
		if helper.try_wait().map_err(|e| e.to_string())?.is_some() || Instant::now() >= deadline {
			let _ = helper.kill();
			let _ = helper.wait();
			return Err(format!("{} {}", t("The update helper did not start. See the log:"), log_path.display()));
		}
		thread::sleep(Duration::from_millis(20));
	}
	// The helper is intentionally independent of the host process. It exits itself on cancellation
	// or timeout; a detached reaper prevents a zombie if this application stays open after cancel.
	thread::spawn(move || {
		let _ = helper.wait();
	});
	Ok(InstallOutcome::Prepared(PreparedUpdate { stage }))
}

fn write_messages(stage: &Path) -> Result<(), String> {
	let messages = [
		t("The application did not quit within 60 seconds. The current app was kept."),
		t("The installation changed while waiting. The current app was kept."),
		t("Could not move the current application. The current app was kept."),
		t("Could not install the update. The previous version was restored."),
		t("Could not restore the previous version. Recover it from:"),
		t("macOS could not launch the update. The previous version is preserved at:"),
	];
	for (index, message) in messages.iter().enumerate() {
		fs::write(stage.join(format!("message-{index}")), message).map_err(|e| e.to_string())?;
	}
	fs::write(stage.join("message-title"), t("Application update failed")).map_err(|e| e.to_string())?;
	fs::write(stage.join("message-log"), t("Log:")).map_err(|e| e.to_string())?;
	Ok(())
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
	if cancelled.load(Ordering::Relaxed) {
		return Err(t("Update cancelled."));
	}
	Ok(())
}

fn bundle_stamp(bundle: &Path) -> Result<String, String> {
	let metadata = fs::symlink_metadata(bundle).map_err(|e| e.to_string())?;
	Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
}

fn manual_install(config: &UpdaterConfig, path: &Path) -> Result<InstallOutcome, String> {
	run(Command::new("/usr/bin/open").arg(path), &t("Failed to open disk image"))?;
	Ok(InstallOutcome::ManualStep(
		t("The update has been downloaded and its disk image opened. Quit %s and drag the new version into Applications to finish installing.")
			.replace("%s", &config.app_display_name),
	))
}

fn bundle_for_executable(exe: &Path) -> Option<PathBuf> {
	let macos = exe.parent()?;
	let contents = macos.parent()?;
	let bundle = contents.parent()?;
	(macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && bundle.extension()? == "app")
		.then(|| bundle.to_path_buf())
}

fn can_update(bundle: &Path) -> bool {
	!bundle.components().any(|part| part.as_os_str() == "AppTranslocation")
		&& !fs::symlink_metadata(bundle).is_ok_and(|meta| meta.file_type().is_symlink())
		&& Command::new("/bin/test").arg("-w").arg(bundle).status().is_ok_and(|s| s.success())
}

fn plist_value(bundle: &Path, key: &str) -> Result<String, String> {
	let output = Command::new("/usr/libexec/PlistBuddy")
		.args(["-c", &format!("Print :{key}")])
		.arg(bundle.join("Contents/Info.plist"))
		.output()
		.map_err(|e| e.to_string())?;
	let value = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
	if !output.status.success() || value.trim().is_empty() {
		return Err(format!("{}: {key}", t("Invalid application metadata")));
	}
	Ok(value.trim().to_owned())
}

fn find_replacement(root: &Path, identity: &str) -> Result<PathBuf, String> {
	let mut matches = Vec::new();
	for entry in fs::read_dir(root).map_err(|e| e.to_string())? {
		let entry = entry.map_err(|e| e.to_string())?;
		if entry.file_type().map_err(|e| e.to_string())?.is_dir()
			&& entry.path().extension().is_some_and(|ext| ext == "app")
			&& plist_value(&entry.path(), "CFBundleIdentifier").is_ok_and(|id| id == identity)
		{
			matches.push(entry.path());
		}
	}
	if matches.len() != 1 {
		return Err(t("The disk image must contain exactly one matching application."));
	}
	Ok(matches.remove(0))
}

fn signing_team(bundle: &Path) -> Option<String> {
	let output = Command::new("/usr/bin/codesign").args(["--display", "--verbose=4"]).arg(bundle).output().ok()?;
	if !output.status.success() {
		return None;
	}
	let metadata = String::from_utf8(output.stderr).ok()?;
	let team = metadata.lines().find_map(|line| line.strip_prefix("TeamIdentifier="))?;
	// Apple Team IDs contain ten ASCII letters/digits. This also rejects "not set" and
	// keeps the value safe to embed in the native code requirement language.
	(team.len() == 10 && team.bytes().all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit()))
		.then(|| team.to_owned())
}

fn validate_bundle(bundle: &Path, team: &str) -> Result<(), String> {
	validate_bundle_integrity(bundle)?;
	// Checking TeamIdentifier metadata alone could trust a self-signed certificate. Require
	// an Apple-issued signing chain and the running app's team in the certificate itself.
	let requirement = format!("anchor apple generic and certificate leaf[subject.OU] = \"{team}\"");
	run(
		Command::new("/usr/bin/codesign")
			.args(["--verify", "--deep", "--strict"])
			.arg(format!("-R={requirement}"))
			.arg(bundle),
		&t("The update must be signed by the application's developer team"),
	)
}

fn validate_bundle_integrity(bundle: &Path) -> Result<(), String> {
	let name = plist_value(bundle, "CFBundleExecutable")?;
	if Path::new(&name).file_name().is_none_or(|n| n != name.as_str()) {
		return Err(t("Invalid application executable."));
	}
	let exe = bundle.join("Contents/MacOS").join(name);
	let resolved = exe.canonicalize().map_err(|e| e.to_string())?;
	if !resolved.starts_with(bundle.canonicalize().map_err(|e| e.to_string())?) || !resolved.is_file() {
		return Err(t("Invalid application executable."));
	}
	run(Command::new("/bin/test").arg("-x").arg(exe), &t("Invalid application executable"))?;
	run(
		Command::new("/usr/bin/codesign").args(["--verify", "--deep", "--strict"]).arg(bundle),
		&t("Application signature verification failed"),
	)
}

fn register_stage(stage: &Path, bundle: &Path) -> Result<(), String> {
	let bundle = bundle.canonicalize().map_err(|e| e.to_string())?;
	fs::write(stage.join("owner-bundle"), bundle.as_os_str().as_encoded_bytes()).map_err(|e| e.to_string())?;
	fs::write(stage.join("host-pid"), process::id().to_string()).map_err(|e| e.to_string())
}

/// Only remove abandoned stages belonging to this app and user. Recovery backups are kept.
fn sweep_stale_stages(parent: &Path, bundle: &Path, owner: u32, now: SystemTime) {
	let Ok(bundle) = bundle.canonicalize() else { return };
	let Ok(entries) = fs::read_dir(parent) else { return };
	for entry in entries.flatten() {
		if !entry.file_name().to_string_lossy().starts_with(".ship-shape-") {
			continue;
		}
		let stage = entry.path();
		let Ok(metadata) = fs::symlink_metadata(&stage) else { continue };
		if !metadata.is_dir() || metadata.uid() != owner {
			continue;
		}
		match fs::symlink_metadata(stage.join("old.app")) {
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
			_ => continue,
		}
		let marker = stage.join("owner-bundle");
		let Ok(metadata) = fs::symlink_metadata(&marker) else { continue };
		if !metadata.is_file() || metadata.uid() != owner {
			continue;
		}
		let old_enough = metadata
			.modified()
			.ok()
			.and_then(|time| now.duration_since(time).ok())
			.is_some_and(|age| age >= STALE_STAGE_AGE);
		if !old_enough || !fs::read(&marker).is_ok_and(|path| path == bundle.as_os_str().as_encoded_bytes()) {
			continue;
		}
		if stage_process_alive(&stage.join("host-pid"), owner, false)
			|| stage_process_alive(&stage.join("helper-pid"), owner, true)
		{
			continue;
		}
		let _ = fs::remove_dir_all(stage);
	}
}

fn stage_process_alive(path: &Path, owner: u32, optional: bool) -> bool {
	let metadata = match fs::symlink_metadata(path) {
		Ok(metadata) => metadata,
		Err(error) if optional && error.kind() == std::io::ErrorKind::NotFound => return false,
		Err(_) => return true,
	};
	if !metadata.is_file() || metadata.uid() != owner {
		return true;
	}
	let Some(pid) =
		fs::read_to_string(path).ok().and_then(|text| text.trim().parse::<i32>().ok()).filter(|pid| *pid > 0)
	else {
		return true;
	};
	// Fail closed if process inspection itself cannot run. A reused PID only delays cleanup.
	Command::new("/bin/kill").args(["-0", &pid.to_string()]).output().map_or(true, |output| output.status.success())
}

fn run(command: &mut Command, message: &str) -> Result<(), String> {
	let output = command.output().map_err(|e| format!("{message}: {e}"))?;
	if !output.status.success() {
		return Err(format!("{}: {}", message, String::from_utf8_lossy(&output.stderr).trim()));
	}
	Ok(())
}

struct Mount {
	point: PathBuf,
	_directory: TempDir,
	attached: bool,
}

impl Mount {
	fn attach(dmg: &Path) -> Result<Self, String> {
		let directory = tempfile::Builder::new().prefix("ship-shape-mount-").tempdir().map_err(|e| e.to_string())?;
		let point = directory.path().join("image");
		fs::create_dir(&point).map_err(|e| e.to_string())?;
		// A failed attach can still partially mount a volume; Drop attempts detach.
		let mount = Self { point, _directory: directory, attached: true };
		run(
			Command::new("/usr/bin/hdiutil")
				.args(["attach", "-readonly", "-nobrowse", "-noautoopen", "-mountpoint"])
				.arg(&mount.point)
				.arg(dmg),
			&t("Failed to mount disk image"),
		)?;
		Ok(mount)
	}

	fn detach(mut self) -> Result<(), String> {
		run(Command::new("/usr/bin/hdiutil").arg("detach").arg(&self.point), &t("Failed to unmount disk image"))?;
		self.attached = false;
		Ok(())
	}
}

impl Drop for Mount {
	fn drop(&mut self) {
		if self.attached {
			let _ = Command::new("/usr/bin/hdiutil").args(["detach", "-force"]).arg(&self.point).output();
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn asset_is_always_a_dmg() {
		assert_eq!(asset_name_parts(InstallKind::Installer), ("", "dmg"));
		assert_eq!(asset_name_parts(InstallKind::Portable), ("", "dmg"));
	}

	#[test]
	fn discovers_only_bundle_executables() {
		assert_eq!(
			bundle_for_executable(Path::new("/tmp/O'Brien App.app/Contents/MacOS/app")),
			Some(PathBuf::from("/tmp/O'Brien App.app"))
		);
		assert!(bundle_for_executable(Path::new("/tmp/app")).is_none());
		assert!(bundle_for_executable(Path::new("/tmp/App.app/Other/MacOS/app")).is_none());
	}

	#[test]
	fn translocated_bundle_is_never_updated() {
		assert!(!can_update(Path::new("/private/var/AppTranslocation/id/d/App.app")));
	}
}

/// Only delete directories created by our download pipeline, never a caller's arbitrary DMG.
pub fn remove_private_download(path: &Path) {
	if let Some(parent) = path.parent()
		&& parent.parent() == Some(env::temp_dir().as_path())
		&& parent.file_name().is_some_and(|name| name.to_string_lossy().starts_with("ship-shape-download-"))
	{
		let _ = fs::remove_dir_all(parent);
	}
}

#[cfg(test)]
#[path = "macos_tests.rs"]
mod integration_tests;
