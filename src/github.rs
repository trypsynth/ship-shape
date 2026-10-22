use std::time::Duration;

use patois::t;
use serde::Deserialize;

use crate::{UpdateError, UpdaterConfig, http, platform};

const API_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Deserialize)]
pub struct ReleaseAsset {
	name: String,
	browser_download_url: String,
}

#[derive(Debug, Deserialize)]
pub struct GithubRelease {
	pub tag_name: String,
	pub body: Option<String>,
	assets: Option<Vec<ReleaseAsset>>,
}

/// URLs of a release asset and its minisign signature.
pub struct AssetPair {
	pub download_url: String,
	pub signature_url: String,
}

pub fn fetch_latest_release(config: &UpdaterConfig) -> Result<GithubRelease, UpdateError> {
	fetch_release(config, &format!("https://api.github.com/repos/{}/releases/latest", config.github_repo))
}

pub fn fetch_release_by_tag(config: &UpdaterConfig, tag: &str) -> Result<GithubRelease, UpdateError> {
	fetch_release(config, &format!("https://api.github.com/repos/{}/releases/tags/{tag}", config.github_repo))
}

fn fetch_release(config: &UpdaterConfig, url: &str) -> Result<GithubRelease, UpdateError> {
	http::agent(API_TIMEOUT)
		.get(url)
		.header("User-Agent", &config.user_agent)
		.header("Accept", "application/vnd.github+json")
		.call()?
		.body_mut()
		.read_json::<GithubRelease>()
		.map_err(|e| UpdateError::InvalidResponse(format!("{}: {e}", t("Failed to parse release JSON"))))
}

pub fn require_asset_pair(config: &UpdaterConfig, release: &GithubRelease) -> Result<AssetPair, UpdateError> {
	match release.assets.as_deref() {
		Some(assets) if !assets.is_empty() => {
			let (prefix, ext) = platform::asset_name_parts(config.install_kind);
			pick_asset_pair(&config.app_name, config.effective_asset_suffix(), prefix, ext, assets).ok_or_else(|| {
				UpdateError::NoDownload(t("Update is available but no matching download asset or signature was found."))
			})
		}
		_ => Err(UpdateError::NoDownload(t("Latest release does not include downloadable assets."))),
	}
}

fn pick_asset_pair(
	app_name: &str,
	asset_suffix: &str,
	prefix: &str,
	ext: &str,
	assets: &[ReleaseAsset],
) -> Option<AssetPair> {
	let base = format!("{app_name}{prefix}{asset_suffix}.{ext}");
	let sig_name = format!("{base}.minisig");
	let find =
		|name: &str| assets.iter().find(|a| a.name.eq_ignore_ascii_case(name)).map(|a| a.browser_download_url.clone());
	Some(AssetPair { download_url: find(&base)?, signature_url: find(&sig_name)? })
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::InstallKind;

	fn make_assets(entries: &[(&str, &str)]) -> Vec<ReleaseAsset> {
		entries
			.iter()
			.map(|(name, url)| ReleaseAsset { name: name.to_string(), browser_download_url: url.to_string() })
			.collect()
	}

	fn urls(pair: &AssetPair) -> (&str, &str) {
		(&pair.download_url, &pair.signature_url)
	}

	#[test]
	fn pick_asset_pair_installer() {
		let assets = make_assets(&[
			("myapp.zip", "https://example.com/myapp.zip"),
			("myapp.zip.minisig", "https://example.com/myapp.zip.minisig"),
			("myapp_setup.exe", "https://example.com/myapp_setup.exe"),
			("myapp_setup.exe.minisig", "https://example.com/myapp_setup.exe.minisig"),
		]);
		assert_eq!(
			urls(&pick_asset_pair("myapp", "", "_setup", "exe", &assets).unwrap()),
			("https://example.com/myapp_setup.exe", "https://example.com/myapp_setup.exe.minisig")
		);
	}

	#[test]
	fn pick_asset_pair_zip() {
		let assets = make_assets(&[
			("myapp.zip", "https://example.com/myapp.zip"),
			("myapp.zip.minisig", "https://example.com/myapp.zip.minisig"),
			("myapp_setup.exe", "https://example.com/myapp_setup.exe"),
			("myapp_setup.exe.minisig", "https://example.com/myapp_setup.exe.minisig"),
		]);
		assert_eq!(
			urls(&pick_asset_pair("myapp", "", "", "zip", &assets).unwrap()),
			("https://example.com/myapp.zip", "https://example.com/myapp.zip.minisig")
		);
	}

	#[test]
	fn pick_asset_pair_case_insensitive() {
		let assets = make_assets(&[
			("MYAPP.ZIP", "https://example.com/MYAPP.ZIP"),
			("MYAPP.ZIP.MINISIG", "https://example.com/MYAPP.ZIP.MINISIG"),
		]);
		assert_eq!(
			urls(&pick_asset_pair("myapp", "", "", "zip", &assets).unwrap()),
			("https://example.com/MYAPP.ZIP", "https://example.com/MYAPP.ZIP.MINISIG")
		);
	}

	#[test]
	fn pick_asset_pair_returns_none_when_missing() {
		let assets = make_assets(&[("notes.txt", "https://example.com/notes.txt")]);
		assert!(pick_asset_pair("myapp", "", "_setup", "exe", &assets).is_none());
		assert!(pick_asset_pair("myapp", "", "", "zip", &assets).is_none());
	}

	#[test]
	fn pick_asset_pair_returns_none_when_sig_missing() {
		let assets = make_assets(&[("myapp.zip", "https://example.com/myapp.zip")]);
		assert!(pick_asset_pair("myapp", "", "", "zip", &assets).is_none());
	}

	#[test]
	fn pick_asset_pair_with_arch_suffix() {
		let assets = make_assets(&[
			("myapp.zip", "https://example.com/myapp.zip"),
			("myapp.zip.minisig", "https://example.com/myapp.zip.minisig"),
			("myapp-arm64.zip", "https://example.com/myapp-arm64.zip"),
			("myapp-arm64.zip.minisig", "https://example.com/myapp-arm64.zip.minisig"),
			("myapp_setup-arm64.exe", "https://example.com/myapp_setup-arm64.exe"),
			("myapp_setup-arm64.exe.minisig", "https://example.com/myapp_setup-arm64.exe.minisig"),
		]);
		assert_eq!(
			urls(&pick_asset_pair("myapp", "-arm64", "", "zip", &assets).unwrap()),
			("https://example.com/myapp-arm64.zip", "https://example.com/myapp-arm64.zip.minisig")
		);
		assert_eq!(
			urls(&pick_asset_pair("myapp", "-arm64", "_setup", "exe", &assets).unwrap()),
			("https://example.com/myapp_setup-arm64.exe", "https://example.com/myapp_setup-arm64.exe.minisig")
		);
	}

	#[test]
	fn pick_asset_pair_dmg() {
		let assets = make_assets(&[
			("myapp.dmg", "https://example.com/myapp.dmg"),
			("myapp.dmg.minisig", "https://example.com/myapp.dmg.minisig"),
		]);
		assert_eq!(
			urls(&pick_asset_pair("myapp", "", "", "dmg", &assets).unwrap()),
			("https://example.com/myapp.dmg", "https://example.com/myapp.dmg.minisig")
		);
	}

	#[test]
	fn pick_asset_pair_fat_installer_with_per_arch_zips() {
		// One architecture-independent installer serves every build while zips stay per-arch.
		let assets = make_assets(&[
			("myapp_setup.exe", "https://example.com/myapp_setup.exe"),
			("myapp_setup.exe.minisig", "https://example.com/myapp_setup.exe.minisig"),
			("myapp-x64.zip", "https://example.com/myapp-x64.zip"),
			("myapp-x64.zip.minisig", "https://example.com/myapp-x64.zip.minisig"),
		]);
		let config = UpdaterConfig::new("o/r", "myapp", "My App", "key", "1.0.0")
			.with_asset_suffix("-x64")
			.with_installer_asset_suffix("");
		let installer = config.clone().with_install_kind(InstallKind::Installer);
		let portable = config.with_install_kind(InstallKind::Portable);
		let pair = pick_asset_pair("myapp", installer.effective_asset_suffix(), "_setup", "exe", &assets).unwrap();
		assert_eq!(pair.download_url, "https://example.com/myapp_setup.exe");
		let pair = pick_asset_pair("myapp", portable.effective_asset_suffix(), "", "zip", &assets).unwrap();
		assert_eq!(pair.download_url, "https://example.com/myapp-x64.zip");
	}
}
