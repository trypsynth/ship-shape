#[cfg(target_os = "linux")]
use std::{
	env, fs,
	path::{Path, PathBuf},
	process::{self, Command},
};

#[cfg(target_os = "linux")]
use patois::t;

#[cfg(target_os = "linux")]
use super::InstallOutcome;
use crate::InstallKind;
#[cfg(target_os = "linux")]
use crate::{UpdateError, UpdaterConfig};

/// Portable is a `.tar.gz` extracted over the app folder, the same shape as the Windows zip.
/// The installer is a standalone `.AppImage` that replaces itself in place.
pub const fn asset_name_parts(install_kind: InstallKind) -> (&'static str, &'static str) {
	match install_kind {
		InstallKind::Installer => ("", "AppImage"),
		InstallKind::Portable => ("", "tar.gz"),
	}
}

/// The tar.gz lands next to the current executable so the extraction script can overwrite it in
/// place. The `AppImage` lands in a per-user cache directory rather than the shared system temp
/// directory, since a predictable path under `/tmp` could be squatted by another local user and
/// break the update for everyone else; it only replaces the running `AppImage` once its signature
/// is verified.
///
/// For [`InstallKind::Installer`] this also checks that the app is actually running from an
/// `AppImage`, so a misconfigured build fails before spending a download on an update it can
/// never apply, rather than after.
#[cfg(target_os = "linux")]
pub fn download_dir(config: &UpdaterConfig) -> Result<PathBuf, UpdateError> {
	match config.install_kind {
		InstallKind::Installer => {
			env::var("APPIMAGE")
				.map_err(|_| UpdateError::Io("Not running from an AppImage; cannot self-update.".to_string()))?;
			cache_dir(config)
		}
		InstallKind::Portable => env::current_exe()
			.map_err(|e| UpdateError::Io(format!("Failed to determine exe path: {e}")))?
			.parent()
			.map(Path::to_path_buf)
			.ok_or_else(|| UpdateError::Io("Failed to get exe directory".to_string())),
	}
}

/// A per-user cache directory (`$XDG_CACHE_HOME/{app_name}`, falling back to
/// `$HOME/.cache/{app_name}`) for the downloaded `AppImage`, created if it doesn't exist yet.
#[cfg(target_os = "linux")]
fn cache_dir(config: &UpdaterConfig) -> Result<PathBuf, UpdateError> {
	let base = env::var_os("XDG_CACHE_HOME")
		.map(PathBuf::from)
		.or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
		.ok_or_else(|| {
			UpdateError::Io("Could not determine a cache directory: neither XDG_CACHE_HOME nor HOME is set".to_string())
		})?;
	let dir = base.join(&config.app_name);
	fs::create_dir_all(&dir).map_err(|e| UpdateError::Io(format!("Failed to create cache directory: {e}")))?;
	Ok(dir)
}

/// Start a detached shell script that waits for this process to exit, applies the update, and
/// relaunches the app.
#[cfg(target_os = "linux")]
pub fn install(config: &UpdaterConfig, path: &Path) -> Result<InstallOutcome, String> {
	let update = path.display().to_string();
	let script = match config.install_kind {
		InstallKind::Installer => {
			// Set by the AppImage runtime to the path of the `.AppImage` file itself; unlike on
			// Windows there's no separate installer step, so the update just overwrites that
			// file. `current_exe` would instead resolve to the temporary FUSE mount the runtime
			// extracts itself to, which disappears when the AppImage exits.
			let current_appimage =
				env::var("APPIMAGE").map_err(|_| t("Not running from an AppImage; cannot self-update."))?;
			installer_script(process::id(), &update, &current_appimage)
		}
		InstallKind::Portable => {
			let current_exe =
				env::current_exe().map_err(|e| format!("{}: {e}", t("Failed to get current exe path")))?;
			let exe_dir = current_exe.parent().unwrap_or(&current_exe);
			let exe = current_exe.display().to_string();
			targz_update_script(process::id(), &update, &exe_dir.display().to_string(), &exe)
		}
	};
	Command::new("sh")
		.arg("-c")
		.arg(&script)
		.spawn()
		.map_err(|e| format!("{}: {e}", t("Failed to launch update script")))?;
	Ok(InstallOutcome::Exit)
}

fn sh_quote(s: &str) -> String {
	format!("'{}'", s.replace('\'', r"'\''"))
}

fn wait_clause(pid: u32) -> String {
	format!("sleep 1; while kill -0 {pid} 2>/dev/null; do sleep 0.2; done")
}

fn installer_script(pid: u32, new_appimage: &str, current_appimage: &str) -> String {
	// `;` rather than `&&` between steps, like the Windows scripts: if `chmod`/`mv` fails (for
	// example the AppImage lives somewhere the user can't write), the old AppImage is still
	// there under `current_appimage` and still gets relaunched, instead of leaving the app
	// closed with no indication why.
	format!(
		"{wait}; chmod +x {new_q}; mv -f {new_q} {cur_q}; nohup {cur_q} >/dev/null 2>&1 &",
		wait = wait_clause(pid),
		new_q = sh_quote(new_appimage),
		cur_q = sh_quote(current_appimage),
	)
}

fn targz_update_script(pid: u32, targz: &str, dest_dir: &str, current_exe: &str) -> String {
	// See `installer_script`: `;` so a failed extraction still relaunches the existing exe
	// rather than leaving the app closed.
	format!(
		"{wait}; tar -xzf {targz_q} -C {dest_q}; rm -f {targz_q}; nohup {exe_q} >/dev/null 2>&1 &",
		wait = wait_clause(pid),
		targz_q = sh_quote(targz),
		dest_q = sh_quote(dest_dir),
		exe_q = sh_quote(current_exe),
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn asset_name_depends_on_install_kind() {
		assert_eq!(asset_name_parts(InstallKind::Installer), ("", "AppImage"));
		assert_eq!(asset_name_parts(InstallKind::Portable), ("", "tar.gz"));
	}

	#[cfg(target_os = "linux")]
	#[test]
	fn cache_dir_prefers_xdg_cache_home_and_creates_it() {
		let temp = env::temp_dir().join("ship-shape-cache-dir-test");
		// SAFETY: no other test in this crate reads or writes XDG_CACHE_HOME.
		unsafe { env::set_var("XDG_CACHE_HOME", &temp) };
		let config = UpdaterConfig::new("o/r", "myapp", "My App", "key", "1.0.0");
		let dir = cache_dir(&config).unwrap();
		assert_eq!(dir, temp.join("myapp"));
		assert!(dir.is_dir());
		// SAFETY: see above.
		unsafe { env::remove_var("XDG_CACHE_HOME") };
		let _ = fs::remove_dir_all(&temp);
	}

	#[test]
	fn sh_quote_wraps_in_single_quotes() {
		assert_eq!(sh_quote("/tmp/app.tar.gz"), "'/tmp/app.tar.gz'");
	}

	#[test]
	fn sh_quote_escapes_embedded_single_quotes() {
		assert_eq!(sh_quote("/home/o'brien/app.AppImage"), r"'/home/o'\''brien/app.AppImage'");
	}

	#[test]
	fn installer_script_waits_replaces_and_relaunches() {
		let script = installer_script(42, "/tmp/app.AppImage", "/home/user/App.AppImage");
		assert_eq!(
			script,
			"sleep 1; while kill -0 42 2>/dev/null; do sleep 0.2; done; chmod +x '/tmp/app.AppImage'; mv -f '/tmp/app.AppImage' '/home/user/App.AppImage'; nohup '/home/user/App.AppImage' >/dev/null 2>&1 &"
		);
	}

	#[test]
	fn installer_script_uses_semicolons_so_a_failed_step_still_relaunches() {
		let script = installer_script(1, "/tmp/app.AppImage", "/opt/App.AppImage");
		assert!(!script.contains("&&"));
	}

	#[test]
	fn targz_script_extracts_cleans_up_and_relaunches() {
		let script = targz_update_script(7, "/tmp/app.tar.gz", "/opt/app", "/opt/app/app");
		assert_eq!(
			script,
			"sleep 1; while kill -0 7 2>/dev/null; do sleep 0.2; done; tar -xzf '/tmp/app.tar.gz' -C '/opt/app'; rm -f '/tmp/app.tar.gz'; nohup '/opt/app/app' >/dev/null 2>&1 &"
		);
	}

	#[test]
	fn targz_script_uses_semicolons_so_a_failed_step_still_relaunches() {
		let script = targz_update_script(1, "/tmp/app.tar.gz", "/opt/app", "/opt/app/app");
		assert!(!script.contains("&&"));
	}

	#[test]
	fn scripts_escape_quotes_in_paths() {
		let script = installer_script(7, "/tmp/o'brien.AppImage", "/home/o'brien/App.AppImage");
		assert!(script.contains(r"'/tmp/o'\''brien.AppImage'"));
		assert!(script.contains(r"'/home/o'\''brien/App.AppImage'"));
	}
}
