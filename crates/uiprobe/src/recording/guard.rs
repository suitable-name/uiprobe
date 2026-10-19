//! The text guard: a recording never shows a forbidden text (a private path, a user name).
//!
//! `RecorderOptions::forbidden_texts` lists fragments; `Recorder::set_text_source` gives the
//! recorder a function that returns every text currently visible in the app (labels, values,
//! paths). The recorder asks it at the start and the end of every call that emits frames, and
//! during a call at most once per 250 ms of video time. A text that contains a fragment (compared
//! case-insensitively) stops the recording: the call returns an error naming the fragment, the
//! frame that would have shown it is not written, every later call fails the same way, and
//! `finish` fails too (the half-written video is deleted, as for any abandoned recording). The
//! error names the fragment, never the text around it. Fail closed: with fragments but no text
//! source, every checking call fails until a source is set.

use super::Recorder;

/// The most often the text source is asked during a call, in milliseconds of video.
const CHECK_EVERY_MS: u64 = 250;

/// A function returning the texts visible in the app right now.
pub(super) type TextSource<'w> = Box<dyn Fn() -> Vec<String> + 'w>;

/// The guard state of a recorder.
#[derive(Debug, Default)]
pub(super) struct Guard {
    /// The forbidden fragments: the original text (for the message) and its lower case.
    fragments: Vec<(String, String)>,
    /// The video time before which a mid-call check is skipped.
    next_ms: u64,
    /// The error of the first hit; sticky.
    failure: Option<String>,
}

impl Guard {
    /// A guard for `fragments` (empty ones are ignored: they would match everything).
    pub(super) fn new(fragments: &[String]) -> Self {
        Self {
            fragments: fragments
                .iter()
                .filter(|f| !f.is_empty())
                .map(|f| (f.clone(), f.to_lowercase()))
                .collect(),
            next_ms: 0,
            failure: None,
        }
    }

    /// The first forbidden fragment found in `texts`, case-insensitively.
    pub(super) fn find_hit<'a>(&'a self, texts: &[String]) -> Option<&'a str> {
        for text in texts {
            let lower = text.to_lowercase();
            for (original, needle) in &self.fragments {
                if lower.contains(needle.as_str()) {
                    return Some(original.as_str());
                }
            }
        }
        None
    }

    /// The error that stopped the recording, if one did.
    pub(super) fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }
}

impl<'w> Recorder<'w> {
    /// Sets the function that returns the texts currently visible in the app (every label, value
    /// and path a viewer could read), for [`crate::recording::RecorderOptions::forbidden_texts`].
    /// See the `guard` section of the module documentation.
    pub fn set_text_source(&mut self, f: Box<dyn Fn() -> Vec<String> + 'w>) {
        self.text_source = Some(f);
    }

    /// Checks the visible texts at video time `at_ms`. A `forced` check (a call boundary) always
    /// asks the source; the others only when 250 ms of video have passed since the last ask.
    ///
    /// # Errors
    ///
    /// A forbidden text is visible (or was earlier), or the fragments have no text source.
    pub(super) fn guard_check(&mut self, at_ms: u64, forced: bool) -> Result<(), String> {
        if let Some(failure) = self.guard.failure() {
            return Err(failure.to_owned());
        }
        if self.guard.fragments.is_empty() {
            return Ok(());
        }
        if !forced && at_ms < self.guard.next_ms {
            return Ok(());
        }
        self.guard.next_ms = at_ms.saturating_add(CHECK_EVERY_MS);
        let Some(source) = &self.text_source else {
            return Err(
                "forbidden_texts is set but the recorder has no text source: call \
                        Recorder::set_text_source before recording"
                    .to_owned(),
            );
        };
        let texts = source();
        if let Some(fragment) = self.guard.find_hit(&texts) {
            let message = format!(
                "the forbidden text \"{fragment}\" is visible on screen; the recording was \
                 stopped before a frame showing it was written"
            );
            self.guard.failure = Some(message.clone());
            return Err(message);
        }
        Ok(())
    }

    /// A forced check at the current video time (the start and the end of a call).
    pub(super) fn guard_boundary(&mut self) -> Result<(), String> {
        let now = self.now_ms();
        self.guard_check(now, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_fragment_matches_case_insensitively_inside_a_longer_text() {
        let guard = Guard::new(&texts(&["C:\\Users\\Secret", "hunter2"]));
        assert_eq!(
            guard.find_hit(&texts(&["Open", "file c:\\users\\SECRET\\a.txt"])),
            Some("C:\\Users\\Secret")
        );
        assert_eq!(guard.find_hit(&texts(&["pass: HUNTER2!"])), Some("hunter2"));
        assert_eq!(guard.find_hit(&texts(&["Welcome", "Settings"])), None);
        assert_eq!(guard.find_hit(&[]), None);
    }

    #[test]
    fn empty_fragments_are_ignored() {
        let guard = Guard::new(&texts(&["", "x"]));
        assert_eq!(guard.fragments.len(), 1);
        assert_eq!(Guard::new(&[]).find_hit(&texts(&["anything"])), None);
    }
}
