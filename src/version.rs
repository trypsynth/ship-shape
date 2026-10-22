/// A parsed version, ordered so that a release sorts after its own pre-releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
	major: u64,
	minor: u64,
	patch: u64,
	/// False for a pre-release such as `1.2.3-beta.1`. Pre-releases of one version are not ordered
	/// among themselves; GitHub's latest release is never a pre-release, so nothing compares two.
	release: bool,
}

pub fn parse_semver(value: &str) -> Option<Version> {
	let trimmed = value.trim();
	if trimmed.is_empty() {
		return None;
	}
	let normalized = trimmed.trim_start_matches(['v', 'V']);
	let (numbers, pre_release) = normalized.split_once('-').map_or((normalized, None), |(n, p)| (n, Some(p)));
	let mut parts = numbers.split('.');
	let major = parts.next()?.parse().ok()?;
	let minor = parts.next().unwrap_or("0").parse().ok()?;
	let patch = parts.next().unwrap_or("0").parse().ok()?;
	Some(Version { major, minor, patch, release: pre_release.is_none() })
}

#[cfg(test)]
mod tests {
	use super::*;

	const fn release(major: u64, minor: u64, patch: u64) -> Version {
		Version { major, minor, patch, release: true }
	}

	#[test]
	fn semver_accepts_prefixes_and_prerelease_suffix() {
		assert_eq!(parse_semver("v1.2.3"), Some(release(1, 2, 3)));
		assert_eq!(parse_semver("V4.5.6"), Some(release(4, 5, 6)));
		assert_eq!(parse_semver("1.2.3-beta.1"), Some(Version { major: 1, minor: 2, patch: 3, release: false }));
	}

	#[test]
	fn semver_defaults_missing_parts() {
		assert_eq!(parse_semver("1"), Some(release(1, 0, 0)));
		assert_eq!(parse_semver("1.2"), Some(release(1, 2, 0)));
	}

	#[test]
	fn semver_rejects_empty_or_invalid() {
		assert_eq!(parse_semver(""), None);
		assert_eq!(parse_semver("not-a-version"), None);
		assert_eq!(parse_semver("v"), None);
		assert_eq!(parse_semver(".2.3"), None);
	}

	#[test]
	fn semver_trims_whitespace() {
		assert_eq!(parse_semver("  v2.3.4  "), Some(release(2, 3, 4)));
	}

	#[test]
	fn semver_ignores_extra_segments() {
		assert_eq!(parse_semver("1.2.3.99"), Some(release(1, 2, 3)));
	}

	/// Someone running a beta must be offered the release it leads up to.
	#[test]
	fn a_release_is_newer_than_its_own_pre_release() {
		assert!(parse_semver("1.1.0") > parse_semver("1.1.0-beta.1"));
		assert!(parse_semver("1.1.0-beta.1") > parse_semver("1.0.9"));
	}
}
