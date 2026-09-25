/// How the running copy of the app was installed. Selects which release asset to download and how
/// to apply it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InstallKind {
	/// Installed by `{app_name}_setup.exe`. The update runs the new installer.
	#[default]
	Installer,
	/// Unpacked from `{app_name}.zip`. The update extracts the new archive over the app folder.
	Portable,
}

/// Configuration for the updater. Construct once and pass to all ship-shape functions.
#[derive(Debug, Clone)]
pub struct UpdaterConfig {
	/// GitHub repository in `"owner/repo"` format.
	pub github_repo: String,
	/// App name used to derive asset file names.
	/// On Windows a zip asset is expected to be `{app_name}.zip` and an installer
	/// `{app_name}_setup.exe`. On Linux the equivalents are `{app_name}.tar.gz` and
	/// `{app_name}.AppImage`. On macOS the expected asset is a disk image,
	/// `{app_name}.dmg`; `install_kind` is ignored there since there is only one asset kind.
	pub app_name: String,
	/// Human-readable app name used in dialog titles and messages (e.g. `"Paperback"`).
	pub app_display_name: String,
	/// Base64-encoded minisign public key used to verify downloaded files.
	pub minisign_public_key: String,
	/// Semver version of the running app, compared against release tags on
	/// [`UpdateChannel::Stable`](crate::UpdateChannel::Stable).
	pub current_version: String,
	/// Short or full git commit hash of the running app, compared against the rolling dev release
	/// on [`UpdateChannel::Dev`](crate::UpdateChannel::Dev). Empty by default.
	pub current_commit: String,
	/// How the running app was installed. Defaults to [`InstallKind::Installer`].
	pub install_kind: InstallKind,
	/// Value sent as the `User-Agent` header for all HTTP requests. Defaults to
	/// `"{app_name}/{current_version}"`.
	pub user_agent: String,
	/// Inserted immediately before the extension in the expected asset file names, e.g.
	/// `"-arm64"` to look for `{app_name}-arm64.zip` / `{app_name}_setup-arm64.exe` instead of
	/// the unsuffixed names. Empty by default. Use this when a single release publishes
	/// multiple architecture-specific builds under distinct asset names.
	pub asset_suffix: String,
	/// Overrides [`UpdaterConfig::asset_suffix`] for the installer asset name only. `None`
	/// (the default) applies `asset_suffix` to installer and zip names alike. Set it to `""`
	/// when a single architecture-independent (fat) installer serves every build while zip
	/// assets stay per-architecture.
	pub installer_asset_suffix: Option<String>,
	/// Command-line arguments passed to a downloaded installer on Windows. Defaults to
	/// `["/silent"]` (Inno Setup); NSIS installers need `["/S"]`.
	pub installer_args: Vec<String>,
}

impl UpdaterConfig {
	/// Creates a config with default values for every optional field.
	pub fn new(
		github_repo: impl Into<String>,
		app_name: impl Into<String>,
		app_display_name: impl Into<String>,
		minisign_public_key: impl Into<String>,
		current_version: impl Into<String>,
	) -> Self {
		let app_name = app_name.into();
		let current_version = current_version.into();
		Self {
			github_repo: github_repo.into(),
			user_agent: format!("{app_name}/{current_version}"),
			app_name,
			app_display_name: app_display_name.into(),
			minisign_public_key: minisign_public_key.into(),
			current_version,
			current_commit: String::new(),
			install_kind: InstallKind::default(),
			asset_suffix: String::new(),
			installer_asset_suffix: None,
			installer_args: vec!["/silent".into()],
		}
	}

	/// Sets the commit hash of the running app. See [`UpdaterConfig::current_commit`].
	#[must_use]
	pub fn with_commit(mut self, commit: impl Into<String>) -> Self {
		self.current_commit = commit.into();
		self
	}

	/// Sets how the running app was installed. See [`UpdaterConfig::install_kind`].
	#[must_use]
	pub const fn with_install_kind(mut self, install_kind: InstallKind) -> Self {
		self.install_kind = install_kind;
		self
	}

	/// Sets the `User-Agent` header. See [`UpdaterConfig::user_agent`].
	#[must_use]
	pub fn with_user_agent(mut self, user_agent: impl Into<String>) -> Self {
		self.user_agent = user_agent.into();
		self
	}

	/// Sets the suffix inserted before the extension in the expected asset file names. See
	/// [`UpdaterConfig::asset_suffix`].
	#[must_use]
	pub fn with_asset_suffix(mut self, suffix: impl Into<String>) -> Self {
		self.asset_suffix = suffix.into();
		self
	}

	/// Sets the suffix used for the installer asset name only. See
	/// [`UpdaterConfig::installer_asset_suffix`].
	#[must_use]
	pub fn with_installer_asset_suffix(mut self, suffix: impl Into<String>) -> Self {
		self.installer_asset_suffix = Some(suffix.into());
		self
	}

	/// Sets the command-line arguments passed to a downloaded installer. See
	/// [`UpdaterConfig::installer_args`].
	#[must_use]
	pub fn with_installer_args<I, S>(mut self, args: I) -> Self
	where
		I: IntoIterator<Item = S>,
		S: Into<String>,
	{
		self.installer_args = args.into_iter().map(Into::into).collect();
		self
	}

	pub(crate) fn effective_asset_suffix(&self) -> &str {
		match self.install_kind {
			InstallKind::Installer => self.installer_asset_suffix.as_deref().unwrap_or(&self.asset_suffix),
			InstallKind::Portable => &self.asset_suffix,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn config() -> UpdaterConfig {
		UpdaterConfig::new("o/r", "myapp", "My App", "key", "1.0.0")
	}

	#[test]
	fn user_agent_defaults_to_name_and_version() {
		assert_eq!(config().user_agent, "myapp/1.0.0");
	}

	#[test]
	fn installer_args_default_to_inno_silent() {
		assert_eq!(config().installer_args, vec!["/silent".to_string()]);
	}

	#[test]
	fn with_installer_args_replaces_default() {
		assert_eq!(config().with_installer_args(["/S"]).installer_args, vec!["/S".to_string()]);
	}

	#[test]
	fn effective_asset_suffix_defaults_to_shared_suffix() {
		let config = config().with_asset_suffix("-x64");
		assert_eq!(config.clone().with_install_kind(InstallKind::Installer).effective_asset_suffix(), "-x64");
		assert_eq!(config.with_install_kind(InstallKind::Portable).effective_asset_suffix(), "-x64");
	}

	#[test]
	fn installer_asset_suffix_overrides_installer_only() {
		let config = config().with_asset_suffix("-x64").with_installer_asset_suffix("");
		assert_eq!(config.clone().with_install_kind(InstallKind::Installer).effective_asset_suffix(), "");
		assert_eq!(config.with_install_kind(InstallKind::Portable).effective_asset_suffix(), "-x64");
	}
}
