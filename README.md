# ship-shape

Auto-updater library for wxDragon desktop apps, supporting Windows, macOS, and Linux.

## Features

- Check for stable (semver) or dev (commit hash) updates from GitHub Releases
- Download with progress callback
- Minisign signature verification before applying
- `ui` module: update/progress dialogs, plus platform-specific install flows:
  - Windows: PowerShell install/extract scripts that relaunch the app afterward
  - macOS: downloads and mounts a signed, notarized `.dmg` for the user to drag into
    Applications; the app must be quit and relaunched manually
  - Linux: a shell script that extracts the tarball or replaces the running `.AppImage`, then
    relaunches the app, the same self-updating flow as Windows
  - Other platforms: downloads the file and tells the user where it is

On macOS the expected release asset is `{app_name}.dmg` (`install_kind` is ignored there,
since there's only one asset kind). Windows and Linux keep the same `InstallKind` distinction:
`{app_name}.zip` / `{app_name}.tar.gz` for `InstallKind::Portable`, and `{app_name}_setup.exe` /
`{app_name}.AppImage` for `InstallKind::Installer`. The Linux installer flow requires the running
process to be inside an AppImage (it reads the `APPIMAGE` environment variable the AppImage
runtime sets); there's no separate install step to run, so the update just replaces that file.

Each platform's asset names, download folder, and install flow live in one file under
`src/platform/`. To add a platform, add a file there and select it in `src/platform.rs`.

## Usage

```toml
[dependencies]
ship-shape = "0.3.0"
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

## Upgrading from 0.2

- `UpdaterConfig::new` takes the current version instead of the user agent. The user agent
  now defaults to `"{app_name}/{version}"`; override it with `with_user_agent`.
- The commit hash and `is_installer` moved into the config: `with_commit` and
  `with_install_kind`.
- `check_for_updates(config, channel)` and
  `ui::run_update_check(config, &frame, channel, trigger)` lost their other arguments.
  `silent: true` is now `CheckTrigger::Automatic`.
- `UpdateError` variants dropped their `Error` suffix (`HttpError` is now `Http(u16)`), gained
  `Io`, and the enum is `#[non_exhaustive]`.

## License

MIT
