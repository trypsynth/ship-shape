# ship-shape

Auto-updater library for wxDragon desktop apps on Windows, macOS, and Linux. It checks GitHub
Releases for a newer version, shows the update dialogs, downloads and verifies the update,
installs it, and relaunches the app.

## Features

- Stable (semver tag) and dev (rolling commit) release channels on GitHub Releases
- Minisign signature verification before anything is installed
- Update, download, and error dialogs, with every message translatable through patois
- Platform-specific install flows:
  - Windows: a hidden PowerShell script runs the installer or extracts the zip, then relaunches
    the app
  - macOS: privately mounts the verified `.dmg`, stages and validates its matching app bundle,
    then replaces and relaunches writable installations; protected, mounted-image, or translocated
    installations retain manual DMG instructions
  - Linux: a shell script that extracts the tarball or replaces the running `.AppImage`, then
    relaunches the app, the same self-updating flow as Windows
  - Other platforms: downloads the file and tells the user where it is
- Quits through the app's normal close so it saves its state, with a watchdog so a stalled
  shutdown cannot hold up the update
- Brings the relaunched app to the front on Windows, where it would otherwise open behind
  whatever the user was doing

## Usage

```toml
[dependencies]
ship-shape = "0.5.0"
```

```rust
use std::sync::Arc;
use ship_shape::{InstallKind, UpdateChannel, UpdaterConfig, ui::{self, CheckTrigger}};

let config = Arc::new(
    UpdaterConfig::new(
        "owner/repo",
        "myapp",
        "My App",
        "RWQ...minisign-public-key...",
        env!("CARGO_PKG_VERSION"),
    )
    .with_commit(env!("MY_APP_COMMIT_HASH"))
    .with_install_kind(InstallKind::Portable),
);
ui::run_update_check(config, &frame, UpdateChannel::Stable, CheckTrigger::Manual);
```

Use `CheckTrigger::Automatic` for a background check, such as one at startup: only an available
update shows a dialog. `CheckTrigger::Manual` is for when the user asks, and also reports that the
app is up to date or that the check failed. Starting a check while another is running does nothing.

Once your main window is first shown at startup, let a relaunch after an update come to the front:

```rust
frame.show(true);
ui::bring_to_front_after_update(&frame);
```

## Publishing releases

Each release carries one asset per platform and install kind, named after `app_name`:

| Platform | `InstallKind::Installer` | `InstallKind::Portable` |
| --- | --- | --- |
| Windows | `{app_name}_setup.exe` | `{app_name}.zip` |
| Linux | `{app_name}.AppImage` | `{app_name}.tar.gz` |
| macOS | `{app_name}.dmg` | `{app_name}.dmg` |

`with_asset_suffix` inserts a suffix before the extension, so `-arm64` looks for
`{app_name}-arm64.zip` and `{app_name}_setup-arm64.exe`, for releases that publish a build per
architecture. `with_installer_asset_suffix` overrides it for the installer alone, for one installer
that serves every architecture. Names match regardless of case.

Every asset needs its minisign signature beside it, named `{asset}.minisig`, which
`minisign -Sm myapp_setup.exe` produces. Pass the public key, the second line of `minisign.pub`, to
`UpdaterConfig::new`. A download whose signature does not verify is never installed.

Windows installers run with `/silent`, which suits Inno Setup; NSIS installers need
`with_installer_args(["/S"])`. The Linux installer flow only works when the app runs from an
AppImage, whose `APPIMAGE` variable says which file to replace.

### Stable channel

`UpdateChannel::Stable` compares `current_version` against the tag of the repository's latest
release, which GitHub never picks from drafts or pre-releases. Tags are semver, with or without a
leading `v`. A pre-release build such as `1.1.0-beta.1` is offered `1.1.0` once it is out. The
update dialog shows the release's notes, converted from Markdown to plain text.

### Dev channel

`UpdateChannel::Dev` compares the running build's commit, set with `with_commit`, against a
rolling release tagged `latest`, usually a pre-release that CI replaces on every push. Its notes
must list commits newest first, one per line, as a short hash of seven characters and a message:

```text
- 1a2b3c4 Fix the thing
- 5d6e7f8 Add the other thing
```

The update dialog shows only the commits newer than the running build, or every line when the
running commit is no longer in the list. Assets are named as on the stable channel.

## Quitting for an update

When an update is ready to install, the app has to quit so the helper can replace it.
`run_update_check` closes `parent`'s top-level window the normal way, so its close handler runs
and saves whatever it saves, then ends the event loop. To do more first, such as announcing the
restart, use `ui::run_update_check_with_exit_handler` and quit from the handler. Either way, the
handler runs on the UI thread only after the update is ready, and ship-shape ends the process
itself if it is still running 15 seconds later, because the update cannot install until it has.
Simply hiding the main window is not enough.

## Coming back to the front

Windows does not let a relaunched app take the foreground: its window opens behind whatever the
user was doing. The Windows helper marks the app it relaunches, and
`ui::bring_to_front_after_update(&window)` uses that mark to bring the window forward. On an
ordinary start, and on other platforms, it does nothing. `ship_shape::relaunched_after_update()`
answers whether this start was an update's relaunch, for apps that want to do something else with
it; the mark is removed the first time it is read, so programs the app starts do not inherit it.

## macOS installation

Updates replace the running app at its existing location, including custom folders and renamed
bundles. The DMG must contain exactly one top-level `.app` matching the running bundle identifier,
with a valid code signature and executable. Both the running app and its replacement must carry
the same Apple developer Team ID, verified against an Apple-issued signing chain. Unsigned and
ad-hoc signed development builds use the manual flow, as does an installed app whose signature no
longer verifies. Mounted images and Gatekeeper-translocated apps are not modified. No
administrator privileges are requested.

Mounting, validation, copying, and the helper handshake run off the UI thread. Canceling before
shutdown drops the staged update. The detached helper waits up to 60 seconds for the app to exit,
then swaps bundles using same-filesystem renames. A failed replacement restores the old bundle;
a failed restore or launch preserves the backup and reports its location. Failure logs survive
cleanup under `~/Library/Logs/ship-shape-update-*.log`; successful and canceled updates remove
their logs. A successful Launch Services request does not detect subsequent application crashes.
The app quits as described in [Quitting for an update](#quitting-for-an-update), well within the
helper's 60 seconds.

`UpdaterConfig::with_macos_relaunch_env(name, value)` preserves an explicit environment override
when launching through macOS `open`, such as an isolated settings directory. It is ignored on
other platforms; values are passed as arguments and never interpolated into shell code.

On the next automatic update, stages older than 24 hours are swept only when they belong to the
same bundle and user and neither the host nor helper process is alive. Unregistered stages,
symlinks, and stages containing `old.app` recovery backups are preserved. This avoids deleting
another app's staging area, an active update, or the only recoverable copy after a failed swap.

## Upgrading from 0.4

- `run_update_check` no longer exits the process outright when an update is ready. It closes
  `parent`'s top-level window normally and ends the event loop, so the app's close handler runs.
  An app whose close handler only hides the window should quit from
  `run_update_check_with_exit_handler` instead, or the watchdog ends it 15 seconds later.
- Exit handlers passed to `run_update_check_with_exit_handler` get the same 15-second watchdog.
- Call `ui::bring_to_front_after_update(&frame)` after first showing the main window.
- Error dialogs are translated, and the details inside `UpdateError` variants are now in the
  user's language. `UpdateError`'s `Display` prefixes stay English, for logs.
- A pre-release build is offered the release it leads up to.

## Development

Each platform's asset names, download folder, and install flow live in one file under
`src/platform/`. To add a platform, add a file there and select it in `src/platform.rs`.

The download window comes from the `progress` module of
[wx-utils](https://github.com/trypsynth/wx-utils). Its gauge stays below 100 until verification
and preparation finish. Native progress updates can yield to the event loop, so completion waits
for the active Update/Pulse call to return before destroying the dialog or calling the shutdown
handler. wx-utils' regression tests cover this ordering without requiring a graphical session.
wx-utils shows one progress window at a time, so a download that starts while the application
shows another one waits for it.

Run `cargo test` on macOS for coverage of cancellation, helper timeouts, replacement and rollback,
failed restore and launch, competing destination changes, protected locations, bundle selection,
signature tampering, and paths with spaces and apostrophes. Tests substitute launch commands and
alerts and only modify temporary fixture apps. For a read-only signer check with an existing
release, set `SHIP_SHAPE_TEST_SIGNED_APP` to its app path and run
`cargo test developer_signed_release_accepts_only_its_team -- --ignored`.

The `macos_update_demo` example additionally exercises actual DMG mounting and Launch Services;
testing a signed, notarized release establishes the Gatekeeper path. From a repository checkout,
build it with `cargo build --example macos_update_demo` and run it with
`--setup /tmp/ship-shape-demo`, using a new directory. Then run the generated
`Installed Demo.app/Contents/MacOS/demo` executable with `--cancel` or `--apply`, followed by the
fixture's `demo.dmg` path. Applying must display version 2; canceling must retain version 1. For
automatic installation, set `SHIP_SHAPE_DEMO_SIGNING_ID` to an Apple-issued signing identity when
running `--setup`; without it, fixtures are ad-hoc signed and demonstrate the manual fallback
instead. The demo trusts only its generated local fixture and keeps the production signer checks.
It is a repository test harness, excluded from the published crate package.

## License

MIT
