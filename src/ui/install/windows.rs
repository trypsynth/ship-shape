#[cfg(target_os = "windows")]
use std::{
	env,
	os::windows::process::CommandExt,
	path::Path,
	process::{self, Command},
};

#[cfg(target_os = "windows")]
use patois::t;

#[cfg(target_os = "windows")]
use crate::UpdaterConfig;

#[cfg(target_os = "windows")]
use super::{ParentWindow, show_error};

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

#[cfg(target_os = "windows")]
pub(super) fn install(
	config: &UpdaterConfig,
	parent: &ParentWindow,
	path: &Path,
	is_exe: bool,
	is_zip: bool,
	err_title: &str,
) {
	let current_exe = match env::current_exe() {
		Ok(p) => p,
		Err(e) => {
			show_error(parent, err_title, &format!("{}: {e}", t("Failed to get current exe path")));
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
			show_error(parent, err_title, &format!("{}: {e}", t("Failed to launch installer script")));
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
			show_error(parent, err_title, &format!("{}: {e}", t("Failed to launch update script")));
			return;
		}
		process::exit(0);
	} else {
		show_error(parent, err_title, &t("Unknown update file format."));
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
