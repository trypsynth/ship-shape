//! Telling the instance an update relaunched apart from one the user started.

use std::{env, sync::OnceLock};

/// Set by the Windows update helper on the process it relaunches.
pub const RELAUNCH_ENV: &str = "SHIP_SHAPE_RELAUNCHED";

/// Whether this process was relaunched by an update that replaced the previous one.
///
/// Only the Windows helper marks the relaunch so far, so this is false elsewhere. On Windows the
/// marker is removed the first time this is asked, so programs the app starts do not inherit it,
/// and the answer is kept for later calls.
#[must_use]
pub fn relaunched_after_update() -> bool {
	static RELAUNCHED: OnceLock<bool> = OnceLock::new();
	*RELAUNCHED.get_or_init(|| {
		let relaunched = env::var_os(RELAUNCH_ENV).is_some();
		// Removing a variable is only sound with other threads running on Windows, and nothing sets it elsewhere.
		#[cfg(target_os = "windows")]
		if relaunched {
			// SAFETY: changing the environment is always safe on Windows, whatever other threads are doing.
			unsafe { env::remove_var(RELAUNCH_ENV) };
		}
		relaunched
	})
}
