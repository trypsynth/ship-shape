# ship-shape

Auto-updater library for wxDragon desktop apps, supporting Windows and macOS.

## Features

- Check for stable (semver) or dev (commit hash) updates from GitHub Releases
- Download with progress callback
- Minisign signature verification before applying
- Optional `ui` feature: update/progress dialogs, plus platform-specific install flows:
  - Windows: PowerShell install/extract scripts that relaunch the app afterward
  - macOS: downloads and mounts a signed, notarized `.dmg` for the user to drag into
    Applications; the app must be quit and relaunched manually

On macOS the expected release asset is `{app_name}.dmg` (`is_installer` is ignored there,
since there's only one asset kind). Windows keeps the existing `{app_name}.zip` /
`{app_name}_setup.exe` distinction.

## Usage

```toml
[dependencies]
ship-shape = { version = "0.1.2", features = ["ui"] }
```

```rust
use std::sync::Arc;
use ship_shape::{UpdaterConfig, UpdateChannel, ui};

let config = Arc::new(UpdaterConfig::new(
    "owner/repo",
    "myapp",
    "My App",
    "RWQ...minisign-public-key...",
    format!("myapp/{}", env!("CARGO_PKG_VERSION")),
));
ui::run_update_check(
    config,
    frame.handle_ptr() as usize,
    env!("CARGO_PKG_VERSION"),
    env!("MY_APP_COMMIT_HASH"),
    is_installer,
    UpdateChannel::Stable,
    false,
);
```

## License

MIT
