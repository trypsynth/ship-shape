//! Telling the instance an update relaunched apart from one the user started.

/// Set by the Windows update helper on the process it relaunches.
#[cfg(any(target_os = "windows", test))]
pub const RELAUNCH_ENV: &str = "SHIP_SHAPE_RELAUNCHED";

/// Whether this process was relaunched by an update that replaced the previous one.
///
/// Only Windows marks the relaunch so far, so this is always false elsewhere. The marker is
/// removed the first time this is asked, so programs the app starts do not inherit it, and the
/// answer is kept for later calls.
#[must_use]
pub fn relaunched_after_update() -> bool {
	#[cfg(target_os = "windows")]
	{
		static RELAUNCHED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
		*RELAUNCHED.get_or_init(|| {
			let relaunched = std::env::var_os(RELAUNCH_ENV).is_some();
			if relaunched {
				// SAFETY: changing the environment is always safe on Windows, whatever other threads are doing.
				unsafe { std::env::remove_var(RELAUNCH_ENV) };
			}
			relaunched
		})
	}
	#[cfg(not(target_os = "windows"))]
	false
}
