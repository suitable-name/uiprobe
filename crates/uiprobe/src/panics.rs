//! The panic recorder: every panic on any thread (a worker included) is recorded, so the run
//! can report it as a finding and go on with the next scenario.
//!
//! The recorder chains to the hook that was installed before it, so the usual message and
//! backtrace still reach stderr. A user profile with `panic = "abort"` disables catching
//! altogether; the run then ends at the first panic.

use std::sync::{Mutex, Once, PoisonError};

static PANICS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static INSTALL: Once = Once::new();

/// Installs the recording hook (once per process; later calls do nothing).
pub fn install_panic_recorder() {
    INSTALL.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let thread = std::thread::current();
            let text = format!("[thread {}] {info}", thread.name().unwrap_or("unnamed"));
            eprintln!("uiprobe: panic recorded: {text}");
            PANICS
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(text);
            previous(info);
        }));
    });
}

/// Removes and returns the panics recorded since the last call.
#[must_use]
pub fn take_panics() -> Vec<String> {
    std::mem::take(&mut *PANICS.lock().unwrap_or_else(PoisonError::into_inner))
}
