//! Disposable macOS updater demo. It includes the actual installer implementation, without
//! adding a production API for applying arbitrary local files. No network or key bypass is
//! added to the updater. This demo trusts ONLY its own locally generated fixture image.
//! Set `SHIP_SHAPE_DEMO_SIGNING_ID` to an Apple-issued signing identity for automatic installation.
#![allow(dead_code, reason = "the shared platform module has APIs unused by the demo")]

#[cfg(target_os = "macos")]
#[path = "../src/platform/macos.rs"]
mod macos;

#[cfg(target_os = "macos")]
use std::{env, fs, path::Path, process::Command};

#[cfg(target_os = "macos")]
use ship_shape::{InstallKind, UpdateError, UpdaterConfig};

#[cfg(target_os = "macos")]
enum InstallOutcome {
	Prepared(macos::PreparedUpdate),
	ManualStep(String),
}

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
	let args: Vec<String> = env::args().collect();
	if args.get(1).is_some_and(|arg| arg == "--setup") {
		let root = Path::new(args.get(2).ok_or("Pass a NEW destination directory after --setup")?);
		// Refuse reuse or an existing destination. The demo only replaces copies it creates.
		fs::create_dir(root)?;
		let running = root.join("Installed Demo.app");
		let image_root = root.join("image");
		fs::create_dir(&image_root)?;
		make_bundle(&running, "1")?;
		make_bundle(&image_root.join("Update Demo.app"), "2")?;
		let dmg = root.join("demo.dmg");
		let status = Command::new("/usr/bin/hdiutil")
			.args(["create", "-quiet", "-fs", "HFS+", "-srcfolder"])
			.arg(&image_root)
			.arg(&dmg)
			.status()?;
		if !status.success() {
			return Err("Failed to create demo DMG".into());
		}
		println!("Created disposable demo at {}", root.display());
		println!("Run the installed bundle's Contents/MacOS/demo executable with --apply and the demo.dmg path.");
		return Ok(());
	}
	let exe = env::current_exe()?;
	let bundle = exe.parent().and_then(Path::parent).and_then(Path::parent).ok_or("Not in a demo bundle")?;
	if bundle.join("Contents/Resources/fixture-version").is_file() {
		if args.get(1).is_some_and(|arg| arg == "--apply" || arg == "--cancel") {
			let path = Path::new(args.get(2).ok_or("Pass demo.dmg after --apply or --cancel")?);
			// Bind the test harness to the destination and DMG produced by --setup.
			let root = bundle.parent().ok_or("Missing fixture directory")?;
			if bundle.file_name().is_none_or(|name| name != "Installed Demo.app")
				|| path.canonicalize()? != root.join("demo.dmg").canonicalize()?
			{
				return Err("Use only the fixture's Installed Demo.app and demo.dmg".into());
			}
			let config = UpdaterConfig::new("fixture/demo", "demo", "Updater Demo", "unused-fixture-key", "1");
			match macos::install(&config, path)? {
				InstallOutcome::Prepared(update) if args[1] == "--cancel" => {
					drop(update);
					println!("Cancelled prepared update; version 1 must remain installed.");
				}
				InstallOutcome::Prepared(update) => {
					update.commit()?;
					println!("Helper ready. Exiting so it can replace this disposable bundle.");
				}
				InstallOutcome::ManualStep(message) => println!("{message}"),
			}
			return Ok(());
		}
		let version = fs::read_to_string(bundle.join("Contents/Resources/fixture-version"))?;
		if let Some(root) = bundle.parent()
			&& root.join(".demo-headless").is_file()
		{
			fs::write(root.join("launched-version"), &version)?;
			return Ok(());
		}
		let message = format!("Updater demo version {version} launched. This is a disposable test app.");
		Command::new("/usr/bin/osascript")
			.args([
				"-e",
				"on run argv",
				"-e",
				"display dialog (item 1 of argv) buttons {\"OK\"} default button 1",
				"-e",
				"end run",
				&message,
			])
			.status()?;
		return Ok(());
	}
	Err("Use --setup with a new disposable directory first".into())
}

#[cfg(target_os = "macos")]
fn make_bundle(bundle: &Path, version: &str) -> Result<(), Box<dyn std::error::Error>> {
	let macos = bundle.join("Contents/MacOS");
	fs::create_dir_all(&macos)?;
	fs::create_dir_all(bundle.join("Contents/Resources"))?;
	fs::copy(env::current_exe()?, macos.join("demo"))?;
	fs::write(bundle.join("Contents/Resources/fixture-version"), version)?;
	fs::write(
		bundle.join("Contents/Info.plist"),
		format!(
			r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>org.ship-shape.disposable-demo</string>
<key>CFBundleName</key><string>Updater Demo</string>
<key>CFBundleExecutable</key><string>demo</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>{version}</string>
</dict></plist>"#
		),
	)?;
	let identity = env::var("SHIP_SHAPE_DEMO_SIGNING_ID").unwrap_or_else(|_| "-".to_owned());
	if !Command::new("/usr/bin/codesign").args(["--force", "--sign", &identity]).arg(bundle).status()?.success() {
		return Err("Failed to sign fixture".into());
	}
	Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
	eprintln!("This disposable demo requires macOS.");
}
