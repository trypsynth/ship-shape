# ship-shape

Auto-updater library for wxDragon desktop apps on Windows only, for now.

## Features

- Check for stable (semver) or dev (commit hash) updates from GitHub Releases
- Download with progress callback
- Minisign signature verification before applying
- Optional `ui` feature: update/progress dialogs and PowerShell install/extract scripts

## Usage

```toml
[dependencies]
ship-shape = { version = "0.1.0", features = ["ui"] }
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
