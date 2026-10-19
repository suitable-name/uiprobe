//! Reusing the last full capture while the UI stands still (`RecorderOptions::still_reuse_frames`).
//!
//! A full software render of a 4K window costs about 170 ms and most frames of a tutorial are
//! holds of a still UI, so the recorder can keep the last window picture and put the cursor, the
//! ripple and the badge (which change without the UI changing) on a copy of it.
//!
//! # Why this is a setting and not automatic
//!
//! Slint 1.18.1 has no reliable "the window needs a redraw" signal on the testing backend: its
//! window adapter ignores `request_redraw`, the software renderer's dirty region is internal and
//! `take_snapshot` always renders the whole window, and the event queue that worker threads post
//! results to cannot be inspected from outside. So the recorder knows about every change it
//! causes or can see, and bounds the rest:
//!
//! * the first frame of every recorder call (`hold`, `move_to`, `zoom_to`, ...) is rendered, so
//!   anything the caller did between calls (invoking a callback, a pump) is shown at once;
//! * input the recorder dispatches (`click_at`, `type_text`, `scroll_at`, `press_keys`, the hover
//!   event of a glide), `refresh`, `attach_window` and `detach_window` mark the picture stale;
//! * a timer due within the next frame (`slint::platform::duration_until_next_timer_update`)
//!   marks it stale before the pump that would fire it;
//! * an animation still running after a render (`Window::has_active_animations`, read right after
//!   the render, the only moment it is valid) makes the next frame render;
//! * the time-lapse of `wait_while_recording` always renders (background work posts results);
//! * a picture is reused for at most `still_reuse_frames - 1` frames in a row.
//!
//! The one change that cannot be seen is a result an app thread posts with
//! `slint::invoke_from_event_loop` during a still hold: it shows up at the next render, at most
//! `still_reuse_frames` frames (`still_reuse_frames / fps` seconds) late. With the default of 1
//! every frame is rendered and nothing is ever stale.

use super::Recorder;
use crate::{screenshot::Shot, timing};
use std::time::{Duration, Instant};

/// The reuse state of a recorder.
#[derive(Default)]
pub(super) struct Reuse {
    /// The last rendered picture of the window (with an attached window pasted in).
    cache: Option<Shot>,
    /// The cache must not be used for the next frame.
    dirty: bool,
    /// Frames the cache has been reused in a row.
    run: u32,
    /// Full renders so far.
    captures: u64,
    /// Frames made from the cache so far.
    reused: u64,
}

impl Reuse {
    /// Marks the picture stale.
    pub(super) const fn invalidate(&mut self) {
        self.dirty = true;
    }
}

impl Recorder<'_> {
    /// Full renders of the window so far (one per emitted frame unless
    /// `RecorderOptions::still_reuse_frames` lets still frames reuse the last picture).
    #[must_use]
    pub const fn captures(&self) -> u64 {
        self.reuse.captures
    }

    /// Frames that reused the last full render instead of rendering the window again.
    #[must_use]
    pub const fn reused_frames(&self) -> u64 {
        self.reuse.reused
    }

    /// Called before a pump of `ms` of mock time: a timer that is due within it marks the picture
    /// stale.
    pub(super) fn note_pump(&mut self, ms: u64) {
        if self.options.still_reuse_frames > 1
            && slint::platform::duration_until_next_timer_update()
                .is_some_and(|due| due <= Duration::from_millis(ms))
        {
            self.reuse.dirty = true;
        }
    }

    /// The window picture for the next frame: the cached one while it may be reused, else a
    /// fresh capture (with the attached window pasted in) that becomes the cache.
    pub(super) fn base_picture(&mut self) -> Result<Shot, String> {
        let limit = self.options.still_reuse_frames.max(1);
        if limit > 1
            && !self.reuse.dirty
            && self.reuse.run + 1 < limit
            && let Some(cached) = &self.reuse.cache
        {
            let started = Instant::now();
            let picture = cached.clone();
            timing::record("recording-reuse", started.elapsed());
            self.reuse.run += 1;
            self.reuse.reused += 1;
            return Ok(picture);
        }
        let mut base = super::capture_timed(self.window())?;
        self.check_size(&base)?;
        if let Some(overlay) = &self.overlay {
            overlay.draw(&mut base)?;
        }
        self.reuse.captures += 1;
        self.reuse.run = 0;
        if limit > 1 {
            // Valid only right after the render: the next pump resets it.
            self.reuse.dirty = self.window().has_active_animations();
            self.reuse.cache = Some(base.clone());
        }
        Ok(base)
    }
}
