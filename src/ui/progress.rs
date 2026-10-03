//! wx progress updates yield to the event loop. Keep dialog ownership and completion separate
//! so a nested callback cannot destroy a dialog, or borrow its storage, during Update/Pulse.
use std::{
	cell::{Cell, RefCell},
	rc::Rc,
};

pub(super) struct ProgressLifecycle<D> {
	dialog: RefCell<Option<Rc<D>>>,
	updating: Cell<bool>,
	completion: RefCell<Option<Box<dyn FnOnce()>>>,
}

impl<D> ProgressLifecycle<D> {
	pub(super) const fn new() -> Self {
		Self { dialog: RefCell::new(None), updating: Cell::new(false), completion: RefCell::new(None) }
	}

	pub(super) fn set(&self, dialog: D) {
		*self.dialog.borrow_mut() = Some(Rc::new(dialog));
	}

	pub(super) fn clear(&self) {
		// Native dialog destruction can process events too. Release the borrow before dropping.
		let dialog = self.dialog.borrow_mut().take();
		drop(dialog);
	}

	pub(super) fn begin_update(&self) -> Option<ProgressUpdate<'_, D>> {
		if self.updating.get() {
			return None;
		}
		let dialog = self.dialog.borrow().as_ref().map(Rc::clone)?;
		self.updating.set(true);
		Some(ProgressUpdate { state: self, dialog: Some(dialog) })
	}

	pub(super) fn finish(&self, completion: Box<dyn FnOnce()>) {
		if self.updating.get() {
			*self.completion.borrow_mut() = Some(completion);
		} else {
			completion();
		}
	}
}

pub(super) struct ProgressUpdate<'a, D> {
	state: &'a ProgressLifecycle<D>,
	dialog: Option<Rc<D>>,
}

impl<D> ProgressUpdate<'_, D> {
	pub(super) fn dialog(&self) -> &D {
		self.dialog.as_ref().expect("progress update owns its dialog")
	}
}

impl<D> Drop for ProgressUpdate<'_, D> {
	fn drop(&mut self) {
		// Native Update/Pulse has returned. Release its reference before handing off shutdown.
		drop(self.dialog.take());
		self.state.updating.set(false);
		let completion = self.state.completion.borrow_mut().take();
		if let Some(completion) = completion {
			completion();
		}
	}
}

/// Keep the gauge below its native completion value until preparation and verification finish.
/// `Update(100)` without `AutoHide` opens a nested modal loop and waits for the user to close it.
pub(super) fn download_percent(downloaded: u64, total: u64) -> Option<i32> {
	downloaded.saturating_mul(100).checked_div(total).map(|value| i32::try_from(value.min(99)).unwrap_or(99))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn completed_download_never_enters_native_completion_loop() {
		assert_eq!(download_percent(50, 100), Some(50));
		assert_eq!(download_percent(100, 100), Some(99));
		assert_eq!(download_percent(200, 100), Some(99));
		assert_eq!(download_percent(u64::MAX, 1), Some(99));
		assert_eq!(download_percent(0, 0), None);
	}

	#[test]
	fn nested_completion_waits_for_native_update_to_return_and_destroy_dialog() {
		struct Dialog(Rc<RefCell<Vec<&'static str>>>);
		impl Drop for Dialog {
			fn drop(&mut self) {
				self.0.borrow_mut().push("destroyed");
			}
		}
		let events = Rc::new(RefCell::new(Vec::new()));
		let state = Rc::new(ProgressLifecycle::new());
		state.set(Dialog(Rc::clone(&events)));
		let guard = state.begin_update().unwrap();
		assert!(state.begin_update().is_none());
		let complete_state = Rc::clone(&state);
		let complete_events = Rc::clone(&events);
		state.finish(Box::new(move || {
			complete_state.clear();
			complete_events.borrow_mut().push("shutdown");
		}));
		assert!(events.borrow().is_empty());
		// A reentrant callback can borrow the storage while Update/Pulse is active.
		assert!(state.dialog.try_borrow_mut().is_ok());
		let _ = guard.dialog();
		drop(guard);
		assert_eq!(*events.borrow(), ["destroyed", "shutdown"]);
		assert!(state.begin_update().is_none());
	}

	#[test]
	fn cancellation_keeps_native_dialog_alive_until_update_returns() {
		let state = ProgressLifecycle::new();
		state.set(42);
		let guard = state.begin_update().unwrap();
		state.clear();
		assert_eq!(*guard.dialog(), 42);
		drop(guard);
		assert!(state.begin_update().is_none());
	}
}
