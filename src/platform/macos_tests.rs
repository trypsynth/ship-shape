//! Exercise the actual helper script with disposable directories. Only launch/report commands
//! and fault injection are substituted, so tests cannot open apps or display desktop alerts.
use std::{
	fs,
	path::PathBuf,
	process::{Child, Command},
	thread,
	time::{Duration, Instant, SystemTime},
};

use tempfile::TempDir;

use super::{
	PreparedUpdate, STALE_STAGE_AGE, bundle_stamp, check_cancelled, find_replacement, register_stage, signing_team,
	sweep_stale_stages, validate_bundle, validate_bundle_integrity,
};

struct Fixture {
	root: TempDir,
	bundle: PathBuf,
	stage: PathBuf,
}

impl Fixture {
	fn new() -> Self {
		let root = tempfile::Builder::new().prefix("ship-shape-O'Brien test-").tempdir().unwrap();
		let bundle = root.path().join("Current App.app");
		let stage = root.path().join("stage");
		fs::create_dir(&bundle).unwrap();
		fs::create_dir(&stage).unwrap();
		super::write_messages(&stage).unwrap();
		fs::create_dir(stage.join("new.app")).unwrap();
		fs::write(bundle.join("version"), "old").unwrap();
		fs::write(stage.join("new.app/version"), "new").unwrap();
		fs::write(root.path().join("log"), "diagnostic").unwrap();
		Self { root, bundle, stage }
	}

	fn launch(&self, pid: u32, commit: bool, fault: &str) -> Child {
		let mut script = include_str!("macos_install.sh").to_string();
		// Replace desktop alerts with a successful noninteractive command.
		script = script.replace("/usr/bin/osascript -", "/usr/bin/true");
		let launcher = self.root.path().join("launch.sh");
		fs::write(
			&launcher,
			if fault == "launch" {
				"exit 1\n"
			} else {
				"printf '%s\\n' \"$@\" > \"$(dirname \"$0\")/launch-args\"\nexit 0\n"
			},
		)
		.unwrap();
		script = script.replace("/usr/bin/open -n", &format!("/bin/sh {}", quote(launcher.to_str().unwrap())));
		if fault == "rename" || fault == "restore" {
			let renamer = self.root.path().join("rename.sh");
			let count = self.root.path().join("renames");
			let restore = if fault == "restore" { " -o \"$n\" -eq 3" } else { "" };
			fs::write(&renamer, format!("count={}\nn=0\n[ ! -f \"$count\" ] || n=$(cat \"$count\")\nn=$((n+1))\nprintf '%s' \"$n\" > \"$count\"\n[ \"$n\" -ne 2{restore} ] || exit 1\n/usr/bin/perl -e 'rename $ARGV[0], $ARGV[1] or die $!' \"$1\" \"$2\"\n", quote(count.to_str().unwrap()))).unwrap();
			// Use an explicit condition for the restore failure (both calls 2 and 3 fail).
			if fault == "restore" {
				let contents = fs::read_to_string(&renamer)
					.unwrap()
					.replace("[ \"$n\" -ne 2 -o \"$n\" -eq 3 ]", "[ \"$n\" -eq 1 ]");
				fs::write(&renamer, contents).unwrap();
			}
			script = script.replace(
				"/usr/bin/perl -e 'rename $ARGV[0], $ARGV[1] or die \"rename: $!\\n\"'",
				&format!("/bin/sh {}", quote(renamer.to_str().unwrap())),
			);
		}
		if fault == "timeout" {
			script = script.replace("-ge 300", "-ge 3").replace("/bin/sleep 0.2", "/bin/sleep 0.01");
		}
		let stamp = bundle_stamp(&self.bundle).unwrap();
		if fault == "changed" {
			fs::rename(&self.bundle, self.root.path().join("another-installer-backup.app")).unwrap();
			fs::create_dir(&self.bundle).unwrap();
			fs::write(self.bundle.join("version"), "other").unwrap();
		}
		let script_path = self.root.path().join("helper.sh");
		fs::write(&script_path, script).unwrap();
		if commit {
			fs::write(self.stage.join("commit"), "yes").unwrap();
		}
		Command::new("/bin/sh")
			.arg(script_path)
			.arg(pid.to_string())
			.arg(&self.bundle)
			.arg(&self.stage)
			.arg(self.root.path().join("log"))
			.arg(stamp)
			.arg("--env")
			.arg("TEST_CONFIG_PATH=O'Brien test/settings")
			.spawn()
			.unwrap()
	}

	fn version(&self) -> String {
		fs::read_to_string(self.bundle.join("version")).unwrap()
	}
}

fn quote(path: &str) -> String {
	format!("'{}'", path.replace('\'', "'\\''"))
}

fn wait(mut child: Child) -> bool {
	let deadline = Instant::now() + Duration::from_secs(5);
	loop {
		if let Some(status) = child.try_wait().unwrap() {
			return status.success();
		}
		if Instant::now() > deadline {
			child.kill().unwrap();
			child.wait().unwrap();
			panic!("helper did not exit");
		}
		thread::sleep(Duration::from_millis(10));
	}
}

#[test]
fn helper_replaces_and_cleans_up_with_quoted_paths() {
	let fixture = Fixture::new();
	assert!(wait(fixture.launch(u32::MAX, true, "")));
	assert_eq!(fixture.version(), "new");
	assert!(!fixture.stage.exists());
	assert!(!fixture.root.path().join("log").exists());
	let args = fs::read_to_string(fixture.root.path().join("launch-args")).unwrap();
	assert!(args.ends_with("--env\nTEST_CONFIG_PATH=O'Brien test/settings\n"));
}

#[test]
fn helper_restores_old_bundle_after_install_failure() {
	let fixture = Fixture::new();
	assert!(!wait(fixture.launch(u32::MAX, true, "rename")));
	assert_eq!(fixture.version(), "old");
	assert!(!fixture.stage.exists());
	assert!(fixture.root.path().join("log").exists());
}

#[test]
fn helper_preserves_backup_when_restore_fails() {
	let fixture = Fixture::new();
	assert!(!wait(fixture.launch(u32::MAX, true, "restore")));
	assert_eq!(fs::read_to_string(fixture.stage.join("old.app/version")).unwrap(), "old");
	assert_eq!(fs::read_to_string(fixture.stage.join("new.app/version")).unwrap(), "new");
}

#[test]
fn helper_preserves_backup_when_launch_fails() {
	let fixture = Fixture::new();
	assert!(!wait(fixture.launch(u32::MAX, true, "launch")));
	assert_eq!(fixture.version(), "new");
	assert_eq!(fs::read_to_string(fixture.stage.join("old.app/version")).unwrap(), "old");
	assert!(fixture.root.path().join("log").exists());
}

#[test]
fn helper_does_not_replace_an_installation_changed_by_another_installer() {
	let fixture = Fixture::new();
	assert!(!wait(fixture.launch(u32::MAX, true, "changed")));
	assert_eq!(fixture.version(), "other");
}

#[test]
fn helper_times_out_without_commit_or_host_exit() {
	for commit in [false, true] {
		let fixture = Fixture::new();
		assert!(!wait(fixture.launch(std::process::id(), commit, "timeout")));
		assert_eq!(fixture.version(), "old");
		assert!(!fixture.stage.exists());
	}
}

#[test]
fn dropping_prepared_update_cancels_before_replacement() {
	let fixture = Fixture::new();
	let child = fixture.launch(u32::MAX, false, "");
	let deadline = Instant::now() + Duration::from_secs(2);
	while !fixture.stage.join("ready").exists() {
		assert!(Instant::now() < deadline);
		thread::sleep(Duration::from_millis(10));
	}
	fs::remove_dir_all(&fixture.stage).unwrap();
	assert!(wait(child));
	assert_eq!(fixture.version(), "old");
	assert!(!fixture.root.path().join("log").exists());
	let stage = tempfile::tempdir().unwrap();
	let path = stage.path().to_owned();
	drop(PreparedUpdate { stage });
	assert!(!path.exists());
}

fn metadata_bundle(root: &std::path::Path, name: &str, identity: &str, executable: &str) -> PathBuf {
	let bundle = root.join(name);
	fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
	fs::write(bundle.join("Contents/Info.plist"), format!("<plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>{identity}</string><key>CFBundleExecutable</key><string>{executable}</string></dict></plist>")).unwrap();
	bundle
}

#[test]
fn replacement_requires_exactly_one_matching_bundle_and_ignores_symlinks() {
	let root = tempfile::tempdir().unwrap();
	assert!(find_replacement(root.path(), "correct.id").is_err());
	metadata_bundle(root.path(), "Wrong.app", "wrong.id", "app");
	let first = metadata_bundle(root.path(), "First.app", "correct.id", "app");
	std::os::unix::fs::symlink(&first, root.path().join("Link.app")).unwrap();
	assert_eq!(find_replacement(root.path(), "correct.id").unwrap(), first);
	metadata_bundle(root.path(), "Duplicate.app", "correct.id", "app");
	assert!(find_replacement(root.path(), "correct.id").is_err());
}

#[test]
fn rejects_missing_escaping_and_unsigned_executables() {
	use std::os::unix::fs::PermissionsExt;
	let root = tempfile::tempdir().unwrap();
	let bundle = metadata_bundle(root.path(), "Missing.app", "id", "app");
	assert!(validate_bundle(&bundle, "TESTTEAM01").is_err());
	let bundle = metadata_bundle(root.path(), "Escaping.app", "id", "../../outside");
	assert!(validate_bundle(&bundle, "TESTTEAM01").is_err());
	let bundle = metadata_bundle(root.path(), "Unsigned.app", "id", "app");
	let executable = bundle.join("Contents/MacOS/app");
	fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
	fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
	assert!(validate_bundle(&bundle, "TESTTEAM01").is_err());
}

#[test]
fn cancellation_stops_preparation() {
	assert!(check_cancelled(&std::sync::atomic::AtomicBool::new(true)).is_err());
}

#[test]
fn protected_and_symlinked_bundles_use_manual_fallback() {
	use std::os::unix::fs::PermissionsExt;
	let root = tempfile::tempdir().unwrap();
	let bundle = metadata_bundle(root.path(), "Protected.app", "id", "app");
	fs::set_permissions(&bundle, fs::Permissions::from_mode(0o555)).unwrap();
	assert!(!super::can_update(&bundle));
	fs::set_permissions(&bundle, fs::Permissions::from_mode(0o755)).unwrap();
	let link = root.path().join("Link.app");
	std::os::unix::fs::symlink(&bundle, &link).unwrap();
	assert!(!super::can_update(&link));
}

#[test]
fn signed_bundle_passes_but_tampering_fails() {
	let root = tempfile::tempdir().unwrap();
	let bundle = metadata_bundle(root.path(), "Signed.app", "id", "app");
	fs::create_dir(bundle.join("Contents/Resources")).unwrap();
	let resource = bundle.join("Contents/Resources/content.txt");
	fs::write(&resource, "original").unwrap();
	// A Mach-O executable, copied into a disposable fixture, avoids signing shell scripts.
	fs::copy("/usr/bin/true", bundle.join("Contents/MacOS/app")).unwrap();
	let status = Command::new("/usr/bin/codesign").args(["--force", "--sign", "-"]).arg(&bundle).output().unwrap();
	assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stderr));
	validate_bundle_integrity(&bundle).unwrap();
	assert!(signing_team(&bundle).is_none());
	let error = validate_bundle(&bundle, "TESTTEAM01").unwrap_err();
	assert!(error.contains("developer team"), "{error}");
	fs::write(&resource, "tampered").unwrap();
	let error = validate_bundle_integrity(&bundle).unwrap_err();
	assert!(error.contains("signature verification failed"), "{error}");
}

#[test]
fn unsigned_host_has_no_signing_team() {
	let root = tempfile::tempdir().unwrap();
	let bundle = metadata_bundle(root.path(), "Unsigned.app", "id", "app");
	fs::copy("/usr/bin/true", bundle.join("Contents/MacOS/app")).unwrap();
	assert!(signing_team(&bundle).is_none());
}

#[test]
#[ignore = "requires SHIP_SHAPE_TEST_SIGNED_APP pointing to an Apple Developer-signed app"]
fn developer_signed_release_accepts_only_its_team() {
	let bundle = PathBuf::from(std::env::var_os("SHIP_SHAPE_TEST_SIGNED_APP").expect("provide a signed app path"));
	let team = signing_team(&bundle).expect("the fixture must have an Apple developer team");
	validate_bundle(&bundle, &team).unwrap();
	let other = if team == "TESTTEAM01" { "TESTTEAM02" } else { "TESTTEAM01" };
	let error = validate_bundle(&bundle, other).unwrap_err();
	assert!(error.contains("developer team"), "{error}");
}

#[test]
fn stale_stage_cleanup_preserves_live_helpers_backups_and_other_apps() {
	use std::os::unix::fs::MetadataExt;
	let root = tempfile::tempdir().unwrap();
	let bundle = metadata_bundle(root.path(), "Current.app", "id", "app");
	let other = metadata_bundle(root.path(), "Other.app", "other.id", "app");
	let make_stage = |name: &str, app: &std::path::Path| {
		let stage = root.path().join(format!(".ship-shape-{name}"));
		fs::create_dir(&stage).unwrap();
		register_stage(&stage, app).unwrap();
		fs::write(stage.join("host-pid"), "999999999").unwrap();
		fs::create_dir(stage.join("new.app")).unwrap();
		stage
	};
	let abandoned = make_stage("abandoned", &bundle);
	let live_host = make_stage("host", &bundle);
	fs::write(live_host.join("host-pid"), std::process::id().to_string()).unwrap();
	let live_helper = make_stage("helper", &bundle);
	fs::write(live_helper.join("helper-pid"), std::process::id().to_string()).unwrap();
	let recovery = make_stage("recovery", &bundle);
	fs::create_dir(recovery.join("old.app")).unwrap();
	let unrelated = make_stage("other", &other);
	let malformed = make_stage("malformed", &bundle);
	fs::write(malformed.join("host-pid"), "-1").unwrap();
	let unregistered = root.path().join(".ship-shape-unregistered");
	fs::create_dir(&unregistered).unwrap();
	let symlink = root.path().join(".ship-shape-link");
	std::os::unix::fs::symlink(&abandoned, &symlink).unwrap();
	let owner = abandoned.metadata().unwrap().uid();
	sweep_stale_stages(root.path(), &bundle, owner, SystemTime::now());
	assert!(abandoned.exists(), "recent stages must not be swept");
	sweep_stale_stages(root.path(), &bundle, owner.wrapping_add(1), SystemTime::now() + STALE_STAGE_AGE * 2);
	assert!(abandoned.exists(), "stages owned by another user must not be swept");
	sweep_stale_stages(root.path(), &bundle, owner, SystemTime::now() + STALE_STAGE_AGE * 2);
	assert!(!abandoned.exists());
	for stage in [live_host, live_helper, recovery, unrelated, malformed, unregistered] {
		assert!(stage.exists(), "must preserve {}", stage.display());
	}
	assert!(fs::symlink_metadata(symlink).unwrap().file_type().is_symlink());
}
