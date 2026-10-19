//! Test-only seams a Slint app calls under its own `ui-harness` Cargo feature, so the
//! `uiprobe` UI harness can answer native file dialogs, capture clipboard writes and
//! opened URLs, and redirect a data directory. No dependencies, no Slint.
//!
//! The app depends on this crate as an OPTIONAL NORMAL dependency (`uiprobe` itself is
//! a dev-dependency, and code under `src/` can never name one) and calls it only at
//! `#[cfg(feature = "ui-harness")]` sites; with the feature off the call sites do not exist,
//! so release builds are unchanged by construction. The feature-on arm is this crate, the
//! feature-off arm the real thing, both with one signature:
//!
//! ```ignore
//! // Compiled for real in `examples/notes/src/lib.rs` of the uiprobe repository; it
//! // cannot be a doc test here because the feature belongs to the app.
//! fn pick_file(title: &str) -> Option<std::path::PathBuf> {
//!     #[cfg(feature = "ui-harness")]
//!     {
//!         uiprobe_hooks::picker::next(title)
//!     }
//!     #[cfg(not(feature = "ui-harness"))]
//!     {
//!         rfd::FileDialog::new().set_title(title).pick_file()
//!     }
//! }
//! ```
//!
//! What the seam does at run time, seen from the harness's side (this one runs as a doc test):
//!
//! ```
//! use uiprobe_hooks::picker;
//!
//! // The app's picker as compiled with the feature on: it takes the next queued answer.
//! fn pick_file(title: &str) -> Option<std::path::PathBuf> {
//!     picker::next(title)
//! }
//!
//! picker::queue_path("notes.txt");
//! assert_eq!(pick_file("Open"), Some("notes.txt".into()));
//! // Nothing queued: the dialog counts as cancelled and the failure is recorded for the
//! // harness, which reports it as an `unexpected-file-dialog` finding.
//! assert_eq!(pick_file("Open"), None);
//! assert_eq!(picker::take_failures(), ["unexpected file dialog: Open"]);
//! ```
//!
//! The harness (`uiprobe`, a dev-dependency) re-exports this crate as
//! `uiprobe::hooks`; a scenario queues answers with `ctx.hooks().queue_pick(..)` /
//! `queue_cancel()` and reads back `take_clipboard_writes()`, `take_opened()` and
//! `take_picker_failures()`. The README of `uiprobe` has the setup.
//!
//! Everything lives in process-global statics (not thread-locals): the app asks on the UI
//! thread, but the harness may inspect the queues from anywhere.

use std::sync::{Mutex, MutexGuard, PoisonError};

fn lock<T>(mutex: &'static Mutex<T>) -> MutexGuard<'static, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The native file-dialog answer queue.
///
/// With the feature on, the app's picker never opens an OS dialog: it takes the next queued
/// answer ([`picker::next`]) instead. A picker call with an empty queue answers "cancelled" and
/// records an "unexpected file dialog" failure that the harness reads back with
/// [`picker::take_failures`] and reports.
pub mod picker {
    use super::lock;
    use std::{collections::VecDeque, path::PathBuf, sync::Mutex};

    /// What the next file dialog should answer.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Answer {
        /// The user chose this file or folder.
        Path(PathBuf),
        /// The user dismissed the dialog.
        Cancel,
    }

    static ANSWERS: Mutex<VecDeque<Answer>> = Mutex::new(VecDeque::new());
    static FAILURES: Mutex<Vec<String>> = Mutex::new(Vec::new());

    /// Queues the answer for the next file dialog the app opens. Answers are consumed in order.
    pub fn queue(answer: Answer) {
        lock(&ANSWERS).push_back(answer);
    }

    /// Queues "the user picked `path`" for the next file dialog.
    pub fn queue_path(path: impl Into<PathBuf>) {
        queue(Answer::Path(path.into()));
    }

    /// Queues "the user cancelled" for the next file dialog.
    pub fn queue_cancel() {
        queue(Answer::Cancel);
    }

    /// Number of queued answers no dialog has consumed yet (a scenario can assert it is zero).
    #[must_use]
    pub fn pending() -> usize {
        lock(&ANSWERS).len()
    }

    /// Drops every queued answer.
    pub fn clear() {
        lock(&ANSWERS).clear();
    }

    /// Removes and returns the "unexpected file dialog: ..." failures recorded so far.
    #[must_use]
    pub fn take_failures() -> Vec<String> {
        std::mem::take(&mut *lock(&FAILURES))
    }

    /// Called by the app's picker (feature on only): the answer for the dialog described by
    /// `detail` (title, kind, suggested name). `None` means cancelled. An empty queue records a
    /// failure and answers cancelled.
    #[must_use]
    pub fn next(detail: &str) -> Option<PathBuf> {
        let next = lock(&ANSWERS).pop_front();
        match next {
            Some(Answer::Path(path)) => Some(path),
            Some(Answer::Cancel) => None,
            None => {
                lock(&FAILURES).push(format!("unexpected file dialog: {detail}"));
                None
            }
        }
    }
}

/// Clipboard writes the app would have sent to the system clipboard.
pub mod clipboard {
    use super::lock;
    use std::sync::Mutex;

    static CLIPBOARD: Mutex<Vec<String>> = Mutex::new(Vec::new());

    /// Called by the app's copy routine (feature on only) instead of writing the system
    /// clipboard: remembers the text. Always returns `true` (handled).
    #[must_use]
    pub fn capture(text: &str) -> bool {
        lock(&CLIPBOARD).push(text.to_owned());
        true
    }

    /// Removes and returns the texts the app "copied to the clipboard" since the last call.
    #[must_use]
    pub fn take() -> Vec<String> {
        std::mem::take(&mut *lock(&CLIPBOARD))
    }
}

/// URLs, folders and files the app would have handed to the OS opener (browser, file manager).
pub mod opener {
    use super::lock;
    use std::sync::Mutex;

    static OPENED: Mutex<Vec<String>> = Mutex::new(Vec::new());

    /// Called by the app's launcher (feature on only) instead of starting the OS opener:
    /// remembers the (already validated) URL or path. Always returns `true` (handled).
    #[must_use]
    pub fn capture(target: &str) -> bool {
        lock(&OPENED).push(target.to_owned());
        true
    }

    /// Removes and returns what the app "opened" since the last call.
    #[must_use]
    pub fn take() -> Vec<String> {
        std::mem::take(&mut *lock(&OPENED))
    }
}

/// A data-directory override for apps that resolve their config dir through the OS.
///
/// Known Folders on Windows ignore `APPDATA`, so the harness's environment redirect cannot reach
/// them. The harness sets it to a directory inside its throw-away profile; the app, under its
/// feature, asks [`paths::data_dir`] first and falls back to its normal lookup on `None`.
pub mod paths {
    use super::lock;
    use std::{path::PathBuf, sync::Mutex};

    static DATA_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

    /// Sets the directory [`data_dir`] returns from now on.
    pub fn set_data_dir(dir: PathBuf) {
        *lock(&DATA_DIR) = Some(dir);
    }

    /// The overridden data directory, or `None` when the harness set none.
    #[must_use]
    pub fn data_dir() -> Option<PathBuf> {
        lock(&DATA_DIR).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::{clipboard, opener, paths, picker};
    use std::path::PathBuf;

    /// One test fn on purpose: every seam is a process-global static and tests run on parallel
    /// threads, so separate `#[test]`s would interleave their queues.
    #[test]
    fn seams_queue_capture_and_report_back() {
        // Picker: answers come back in queue order.
        picker::clear();
        let _ = picker::take_failures();
        picker::queue_path("a.txt");
        picker::queue_cancel();
        picker::queue_path("b.txt");
        assert_eq!(picker::pending(), 3);
        assert_eq!(picker::next("first"), Some(PathBuf::from("a.txt")));
        assert_eq!(picker::next("second"), None, "a queued cancel answers None");
        assert_eq!(picker::pending(), 1);
        assert_eq!(picker::next("third"), Some(PathBuf::from("b.txt")));
        assert_eq!(picker::pending(), 0);
        assert!(
            picker::take_failures().is_empty(),
            "no dialog hit an empty queue"
        );

        // An empty queue answers cancelled and records a failure that names the detail.
        assert_eq!(picker::next("Open Project"), None);
        let failures = picker::take_failures();
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("Open Project"), "{failures:?}");
        assert!(picker::take_failures().is_empty(), "take_failures drains");

        // clear drops queued answers.
        picker::queue_path("c.txt");
        picker::queue_cancel();
        picker::clear();
        assert_eq!(picker::pending(), 0);
        let _ = picker::take_failures();

        // Clipboard: capture -> take -> empty.
        let _ = clipboard::take();
        assert!(clipboard::capture("one"));
        assert!(clipboard::capture("two"));
        assert_eq!(clipboard::take(), ["one", "two"]);
        assert_eq!(clipboard::take().len(), 0);

        // Opener: capture -> take -> empty.
        let _ = opener::take();
        assert!(opener::capture("https://example.com"));
        assert_eq!(opener::take(), ["https://example.com"]);
        assert_eq!(opener::take().len(), 0);

        // Paths: unset until set, then returned.
        paths::set_data_dir(PathBuf::from("profile/data"));
        assert_eq!(paths::data_dir(), Some(PathBuf::from("profile/data")));
    }
}
