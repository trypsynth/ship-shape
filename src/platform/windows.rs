#[cfg(target_os = "windows")]
use std::{
	env,
	os::windows::process::CommandExt,
	path::{Path, PathBuf},
	process::{self, Command},
};

#[cfg(target_os = "windows")]
use patois::t;

#[cfg(target_os = "windows")]
use super::InstallOutcome;
use crate::{InstallKind, relaunch::RELAUNCH_ENV};
#[cfg(target_os = "windows")]
use crate::{UpdateError, UpdaterConfig};

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub const fn asset_name_parts(install_kind: InstallKind) -> (&'static str, &'static str) {
	match install_kind {
		InstallKind::Installer => ("_setup", "exe"),
		InstallKind::Portable => ("", "zip"),
	}
}

/// Portable zips land next to the current executable so that the extraction script can overwrite
/// in place. Installers land in the system temp directory.
#[cfg(target_os = "windows")]
pub fn download_dir(config: &UpdaterConfig) -> Result<PathBuf, UpdateError> {
	match config.install_kind {
		InstallKind::Installer => Ok(env::temp_dir()),
		InstallKind::Portable => env::current_exe()
			.map_err(|e| UpdateError::Io(format!("Failed to determine exe path: {e}")))?
			.parent()
			.map(Path::to_path_buf)
			.ok_or_else(|| UpdateError::Io("Failed to get exe directory".to_string())),
	}
}

/// Start a hidden PowerShell script that waits for this process to exit, applies the update,
/// and relaunches the app, marked so it knows an update started it.
#[cfg(target_os = "windows")]
pub fn install(config: &UpdaterConfig, path: &Path) -> Result<InstallOutcome, String> {
	let current_exe = env::current_exe().map_err(|e| format!("{}: {e}", t("Failed to get current exe path")))?;
	let update = path.display().to_string();
	let exe = current_exe.display().to_string();
	let script = match config.install_kind {
		InstallKind::Installer => installer_script(process::id(), &config.installer_args, &update, &exe),
		InstallKind::Portable => {
			let exe_dir = current_exe.parent().unwrap_or(&current_exe);
			zip_update_script(process::id(), &update, &exe_dir.display().to_string(), &exe)
		}
	};
	Command::new("powershell.exe")
		.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command"])
		.arg(&script)
		.creation_flags(CREATE_NO_WINDOW)
		.spawn()
		.map_err(|e| format!("{}: {e}", t("Failed to launch update script")))?;
	Ok(InstallOutcome::Exit)
}

/// Brings the window whose native handle is `hwnd` to the front, returning whether Windows agreed.
///
/// Windows only lets a window take the foreground from the foreground process or one it handed the
/// right to, and a relaunched app is neither: the hidden script above starts it after the old
/// instance has exited and any installer window has come and gone. Sharing input state with the
/// thread that owns the foreground window, for the length of the call, is what lets
/// `SetForegroundWindow` through. Only for a relaunch the user asked for, never an ordinary start.
#[cfg(target_os = "windows")]
pub fn bring_to_front(hwnd: *mut std::ffi::c_void) -> bool {
	use windows::Win32::{
		Foundation::HWND,
		System::Threading::{AttachThreadInput, GetCurrentThreadId},
		UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow},
	};
	if hwnd.is_null() {
		return false;
	}
	let ours = unsafe { GetCurrentThreadId() };
	let foreground = unsafe { GetForegroundWindow() };
	let theirs = if foreground.is_invalid() { 0 } else { unsafe { GetWindowThreadProcessId(foreground, None) } };
	let attached = theirs != 0 && theirs != ours && unsafe { AttachThreadInput(ours, theirs, true) }.as_bool();
	let raised = unsafe { SetForegroundWindow(HWND(hwnd)) }.as_bool();
	if attached {
		let _ = unsafe { AttachThreadInput(ours, theirs, false) };
	}
	raised
}

fn ps_quote(s: &str) -> String {
	format!("'{}'", s.replace('\'', "''"))
}

fn installer_script(pid: u32, installer_args: &[String], installer: &str, current_exe: &str) -> String {
	let arg_clause = if installer_args.is_empty() {
		String::new()
	} else {
		let args = installer_args.iter().map(|a| ps_quote(a)).collect::<Vec<_>>().join(",");
		format!(" -ArgumentList {args}")
	};
	format!(
		"Start-Sleep -Seconds 1; Wait-Process -Id {pid} -ErrorAction SilentlyContinue; Start-Process -FilePath {}{arg_clause} -Wait; $env:{RELAUNCH_ENV}='1'; Start-Process -FilePath {}",
		ps_quote(installer),
		ps_quote(current_exe)
	)
}

fn zip_update_script(pid: u32, zip: &str, dest_dir: &str, current_exe: &str) -> String {
	format!(
		"Start-Sleep -Seconds 1; Wait-Process -Id {pid} -ErrorAction SilentlyContinue; Expand-Archive -Path {zip_q} -DestinationPath {dest_q} -Force; Remove-Item -Path {zip_q} -Force; $env:{RELAUNCH_ENV}='1'; Start-Process {exe_q}",
		zip_q = ps_quote(zip),
		dest_q = ps_quote(dest_dir),
		exe_q = ps_quote(current_exe)
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn asset_name_depends_on_install_kind() {
		assert_eq!(asset_name_parts(InstallKind::Installer), ("_setup", "exe"));
		assert_eq!(asset_name_parts(InstallKind::Portable), ("", "zip"));
	}

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
			r"Start-Sleep -Seconds 1; Wait-Process -Id 42 -ErrorAction SilentlyContinue; Start-Process -FilePath 'C:\Temp\app_setup.exe' -ArgumentList '/S' -Wait; $env:SHIP_SHAPE_RELAUNCHED='1'; Start-Process -FilePath 'C:\App\app.exe'"
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

	/// The installer runs before the marker is set, so only the relaunched app inherits it.
	#[test]
	fn both_scripts_mark_only_the_relaunched_app() {
		let installer = installer_script(7, &[], r"C:\t\s.exe", r"C:\a\a.exe");
		assert!(installer.contains(r"-Wait; $env:SHIP_SHAPE_RELAUNCHED='1'; Start-Process -FilePath 'C:\a\a.exe'"));
		let zip = zip_update_script(7, r"C:\a\a.zip", r"C:\a", r"C:\a\a.exe");
		assert!(zip.contains(r"-Force; $env:SHIP_SHAPE_RELAUNCHED='1'; Start-Process 'C:\a\a.exe'"));
	}
}
