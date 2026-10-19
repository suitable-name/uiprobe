//! Video recording (feature `recording`): play an app in the testing backend and write the
//! result as an MP4 with a drawn mouse cursor, click ripples, captions and chapters.
//!
//! A recording is a sequence of frames at a fixed frame rate. Video time and Slint's mock time
//! run together: every frame pumps the event loop by one frame's worth of mock time (33, 33 and
//! 34 ms at 30 fps, so the sum never drifts), captures the window with the software renderer,
//! draws the cursor over a copy and hands the frame to the sink (ffmpeg's stdin by default).
//! Needs `Harness::renderer(Renderer::Software)` and a window whose physical size is even.
//!
//! The recorder never presses anything: [`Recorder::click`] only draws a ripple, and the app
//! is driven by the caller (invoke the callback, then [`Recorder::refresh`] or the next
//! [`Recorder::hold`]). [`Recorder::move_to`] does dispatch pointer moves, so hover styles show.
//!
//! Background work (a solve, a render) runs in real time: [`Recorder::wait_while_recording`]
//! shows it as a short time-lapse and then cuts, [`Recorder::wait_cut`] shows nothing.
//!
//! ```no_run
//! use std::time::Duration;
//! use uiprobe::prelude::*;
//! use uiprobe::recording::{Recorder, RecorderOptions};
//! use uiprobe::screenshot;
//!
//! # fn film(app: &Attached, button: &ElementHandle) -> Result<(), String> {
//! screenshot::set_window_size(app.window(), (1920, 1080), 2.0);
//! pump(50);
//! let mut rec = Recorder::start(
//!     app.window(),
//!     RecorderOptions {
//!         output: "videos/demo.mp4".into(),
//!         encode_size: Some((1920, 1080)),
//!         ..RecorderOptions::default()
//!     },
//! )?;
//! rec.chapter("Intro");
//! rec.caption("Press the button");
//! rec.hold(Duration::from_secs(1))?;
//! rec.move_to_element(button, Duration::from_millis(700))?;
//! rec.click()?;
//! // ... invoke the button's callback here, then:
//! rec.hold(Duration::from_millis(400))?;
//! let summary = rec.finish()?; // demo.mp4, demo.srt, demo.chapters.txt
//! # let _ = summary;
//! # Ok(())
//! # }
//! ```
//!
//! A second window (a pop-out next to the main one) is filmed by [`Recorder::attach_window`]:
//! every frame then also captures it and pastes it into the picture at a logical position, with
//! a thin dark border and a soft shadow, below the cursor. [`Recorder::detach_window`] ends it.
//!
//! Frames are piped to ffmpeg and never stored; the file is written as `<output>.part` and
//! renamed when [`Recorder::finish`] succeeds, so an abandoned or failed recording leaves the
//! previous video untouched. ffmpeg is looked up as `UIPROBE_FFMPEG`, else `ffmpeg` on `PATH`.
//! Real time goes to the timing labels `recording-capture`, `recording-zoom` and
//! `recording-encode`.
//!
//! # Camera
//!
//! A virtual camera shows the whole window by default. [`Recorder::zoom_to`] eases it onto a
//! region (logical pixels) and [`Recorder::zoom_out`] back; [`CameraOptions`] sets the margin and
//! the zoom limit. Every frame is captured in full, the camera rectangle is cropped out at the
//! capture's physical resolution and scaled to the frame size (bicubic, in this crate), and the
//! cursor, ripple and badges are drawn afterwards, so a zoomed shot of a 4K capture stays sharp
//! and the output size never changes. At zoom 1.0 nothing is resampled. With
//! [`Recorder::set_follow_cursor`] a zoomed camera pans with [`Recorder::move_to`] so the cursor
//! stays in the central 70 % of the picture.
//!
//! # Keystroke badges
//!
//! [`Recorder::show_keys`] draws a keycap badge (`"Ctrl + ↑"`) near the bottom of the frame for
//! [`BadgeStyle::visible_ms`], fading out at the end; it is never zoomed. Its text is rendered by
//! Slint's software renderer, so the glyphs are the system fonts' (a symbol missing from every
//! font shows as the font's replacement glyph).
//!
//! # Filmed input: one activation per action
//!
//! [`Recorder::click_at`], [`Recorder::type_text`], [`Recorder::scroll_at`] and
//! [`Recorder::press_keys`] are the filmed input layer: each glides the cursor and shows the
//! ripple or the badge, then dispatches real Slint pointer or key events to a window. That
//! dispatch IS the activation (a press and release makes the control's own `clicked` run once),
//! so the caller must never also invoke the callback: a video would fire the action twice.
//! [`Recorder::click`], by contrast, only draws the ripple and presses nothing. The [`pacing`]
//! module has the reading time, glide duration and caption helpers of a screencast.
//!
//! # Text guard
//!
//! `RecorderOptions::forbidden_texts` and [`Recorder::set_text_source`] stop a recording that
//! would show a private text (a path, a user name): the source returns every visible text, a
//! case-insensitive substring match makes the call return `Err` naming the fragment, before the
//! frame is written. It is checked at the start and end of every call that emits frames and at
//! most every 250 ms of video in between.
//!
//! # Reusing the window picture
//!
//! `RecorderOptions::still_reuse_frames` (default 1, off) lets up to that many frames in a row
//! reuse one full render of the window while the UI stands still; the cursor, ripple and badge
//! are still drawn per frame. [`Recorder::captures`] and [`Recorder::reused_frames`] count the
//! renders and the reuses, the timing labels `recording-capture` and `recording-reuse` show the
//! time. Slint's testing backend has no redraw signal, so what makes a picture stale is listed
//! (and its one blind spot, a result an app thread posts mid-hold, bounded) in `reuse.rs`.
//!
//! Waits follow real time: [`Recorder::wait_while_recording`] and [`Recorder::wait_cut`] pump
//! the real time that passed per poll (5 to 100 ms), not a fixed step, so an app's tick budgets
//! run at their real speed.

mod badge;
mod camera;
mod captions;
mod cursor;
mod guard;
mod input;
mod keytext;
mod num;
mod overlay;
pub mod pacing;
mod resample;
mod reuse;
mod sink;
mod timeline;
mod zoom;

pub use badge::BadgeStyle;
pub use camera::CameraOptions;
pub use captions::{chapter_timestamp, srt_timestamp};
pub use timeline::ease_in_out;

use crate::{ElementHandle, backend, screenshot, screenshot::Shot, timing};
use captions::Track;
use overlay::Overlay;
use sink::Sink;
use slint::{LogicalPosition, platform::WindowEvent};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};
use timeline::{fraction, frame_ms, lerp, millis};

/// The highest frame rate accepted.
const MAX_FPS: u32 = 240;

/// The real-time pause between polls of a wait.
const POLL: Duration = Duration::from_millis(5);

/// Where the frames of a recording go.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SinkKind {
    /// Raw RGBA frames piped to ffmpeg's stdin; writes the MP4.
    #[default]
    Ffmpeg,
    /// `<dir>/frame_000001.png`, `frame_000002.png`, ... (no ffmpeg needed).
    PngSequence(PathBuf),
    /// The frames are kept in memory ([`Recorder::frames`]); for self-tests.
    Memory,
}

/// How the drawn cursor and the click ripple look.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CursorStyle {
    /// The arrow's height in logical pixels.
    pub size_logical: f32,
    /// How long the click ripple lasts.
    pub ripple_ms: u32,
    /// The ripple's final radius in logical pixels.
    pub ripple_radius_logical: f32,
}

impl Default for CursorStyle {
    fn default() -> Self {
        Self {
            size_logical: 22.0,
            ripple_ms: 350,
            ripple_radius_logical: 18.0,
        }
    }
}

/// The settings of one recording.
#[derive(Debug, Clone)]
pub struct RecorderOptions {
    /// Frames per second (1 to 240).
    pub fps: u32,
    /// The final file; written as `<output>.part` and renamed in [`Recorder::finish`]. The
    /// captions go to the same name with `.srt`, the chapters with `.chapters.txt`.
    pub output: PathBuf,
    /// Where the frames go.
    pub sink: SinkKind,
    /// The ffmpeg executable: the environment variable `UIPROBE_FFMPEG`, else `ffmpeg`.
    pub ffmpeg: PathBuf,
    /// x264 constant rate factor, 0 to 51 (lower is better; 18 is visually lossless). A larger
    /// value makes the ffmpeg sink refuse to open.
    pub crf: u8,
    /// Scales the picture to this size (Lanczos) while encoding, for example a 3840 x 2160
    /// capture to `Some((1920, 1080))`. Both numbers must be even.
    pub encode_size: Option<(u32, u32)>,
    /// How the cursor and the ripple look.
    pub cursor: CursorStyle,
    /// Dispatch one pointer move at the end of every glide, so hover styles show.
    pub hover_events: bool,
    /// How the virtual camera frames a region ([`Recorder::zoom_to`]).
    pub camera: CameraOptions,
    /// How the keystroke badge looks and how long it stays ([`Recorder::show_keys`]).
    pub badge: BadgeStyle,
    /// Fragments that must never be visible in the video (case-insensitive); checked against
    /// the texts of [`Recorder::set_text_source`]. Empty (the default) checks nothing.
    pub forbidden_texts: Vec<String>,
    /// How many frames in a row may reuse one full render of the window while the UI stands
    /// still (1, the default, renders every frame). A 4K software render costs about 170 ms and
    /// most frames are holds, so 4 to 8 makes holds several times faster. The first frame of every
    /// recorder call, input the recorder dispatches, timers, animations and `refresh` always
    /// render; only a result an app thread posts during a still hold can show up late, by at most
    /// this many frames. See the `reuse` section of the module documentation.
    pub still_reuse_frames: u32,
}

impl Default for RecorderOptions {
    fn default() -> Self {
        Self {
            fps: 30,
            output: PathBuf::from("recording.mp4"),
            sink: SinkKind::Ffmpeg,
            ffmpeg: std::env::var_os("UIPROBE_FFMPEG")
                .filter(|v| !v.is_empty())
                .map_or_else(|| PathBuf::from("ffmpeg"), PathBuf::from),
            crf: 18,
            encode_size: None,
            cursor: CursorStyle::default(),
            hover_events: true,
            camera: CameraOptions::default(),
            badge: BadgeStyle::default(),
            forbidden_texts: Vec::new(),
            still_reuse_frames: 1,
        }
    }
}

/// What a finished recording wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingSummary {
    /// The video file (not written for the PNG and memory sinks).
    pub output: PathBuf,
    /// Frames emitted.
    pub frames: u64,
    /// Length of the video.
    pub video_time: Duration,
    /// The captions file.
    pub srt: PathBuf,
    /// The `YouTube` chapter list.
    pub chapters: PathBuf,
}

/// A click ripple in progress.
#[derive(Debug, Clone, Copy)]
struct Ripple {
    at: (f32, f32),
    start_ms: u64,
}

/// A recording of one window. See the [module documentation](self).
///
/// The recorder borrows the window it was started with (`'w`): finish or abandon it before
/// shutting the app down, and do not resize the window in between (a changed size is an error).
pub struct Recorder<'w> {
    window: &'w slint::Window,
    options: RecorderOptions,
    sink: Sink,
    /// Physical size of every frame.
    size: (u32, u32),
    scale: f32,
    /// Frames emitted so far.
    frames: u64,
    /// Video time asked for so far (milliseconds); frames are emitted while their end is not
    /// past it, so the remainder of a request that is not a whole number of frames carries over.
    requested_ms: u64,
    /// The cursor tip in logical pixels.
    cursor: (f32, f32),
    visible: bool,
    ripple: Option<Ripple>,
    track: Track,
    /// The second window composited into every frame, if one is attached.
    overlay: Option<Overlay>,
    /// The part of the window the frame shows (x, y, width, height; logical pixels).
    camera: camera::Rect,
    /// The window's logical size: the camera at zoom 1.0.
    window_size: (f32, f32),
    /// Pan the zoomed camera with `move_to`.
    follow_cursor: bool,
    /// The keystroke badge.
    badge: badge::BadgeState,
    /// The forbidden-text guard and the function that lists the visible texts.
    guard: guard::Guard,
    text_source: Option<guard::TextSource<'w>>,
    /// The cached window picture of still frames.
    reuse: reuse::Reuse,
}

impl<'w> Recorder<'w> {
    /// Spawns the sink and captures a first picture to learn the frame size. The cursor starts
    /// at the window centre and stays hidden until the first [`Recorder::move_to`]. No frame is
    /// emitted yet.
    ///
    /// # Errors
    ///
    /// The frame rate is out of range, the capture fails (the run needs
    /// `Renderer::Software`), ffmpeg cannot be started, or the picture size is odd (H.264).
    pub fn start(window: &'w slint::Window, options: RecorderOptions) -> Result<Self, String> {
        if options.fps == 0 || options.fps > MAX_FPS {
            return Err(format!(
                "fps {} is out of range (1 to {MAX_FPS})",
                options.fps
            ));
        }
        let base = capture_timed(window)?;
        let size = (base.width(), base.height());
        let scale = base.scale();
        let sink = Sink::open(&options, size)?;
        let window_size = (px(size.0) / scale, px(size.1) / scale);
        let centre = (window_size.0 / 2.0, window_size.1 / 2.0);
        let guard = guard::Guard::new(&options.forbidden_texts);
        Ok(Self {
            window,
            options,
            sink,
            size,
            scale,
            frames: 0,
            requested_ms: 0,
            cursor: centre,
            visible: false,
            ripple: None,
            track: Track::default(),
            overlay: None,
            camera: camera::full(window_size),
            window_size,
            follow_cursor: false,
            badge: badge::BadgeState::default(),
            guard,
            text_source: None,
            reuse: reuse::Reuse::default(),
        })
    }

    /// From now on every frame also shows `component`'s window, pasted with its top-left corner
    /// at `at_logical` (logical pixels of the main window) and drawn at the main window's
    /// scale (a window captured at another scale factor is resampled), with a 1-logical-pixel
    /// dark border just outside its edge and a soft shadow. The cursor and the ripple are drawn
    /// above it. Replaces an earlier attached window.
    ///
    /// The recorder keeps a strong handle to the component until [`Recorder::detach_window`]
    /// (the signature takes the component, not a borrowed `slint::Window`, because a secondary
    /// window is usually a clone that lives shorter than the recorder's `'w`). A window the app
    /// hides (its own Close button) is not drawn any more, whether or not it is still attached.
    /// Pointer moves that end over the window are dispatched to it, not to the main window.
    /// Give the attached window the main window's scale factor (`screenshot::set_window_size`)
    /// for a crisp picture.
    pub fn attach_window<C: slint::ComponentHandle + 'static>(
        &mut self,
        component: &C,
        at_logical: (f32, f32),
    ) {
        self.overlay = Some(Overlay::new(component, at_logical));
        self.reuse.invalidate();
    }

    /// Stops compositing the attached window and lets go of it.
    pub fn detach_window(&mut self) {
        self.overlay = None;
        self.reuse.invalidate();
    }

    /// The top-left corner of the attached window in logical pixels of the main window, if
    /// one is attached (add a point local to the window to it to get the cursor target).
    #[must_use]
    pub fn attached_origin(&self) -> Option<(f32, f32)> {
        self.overlay.as_ref().map(|o| o.at)
    }

    /// Where to put `window` in the frame: its own reported position (logical pixels) if the
    /// backend has one (the testing backend reports `(0, 0)` until something sets it, which
    /// counts as none), else centred over the main window.
    #[must_use]
    pub fn place_window(&self, window: &slint::Window) -> (f32, f32) {
        let reported = window.position();
        if reported.x != 0 || reported.y != 0 {
            let scale = window.scale_factor().max(f32::EPSILON);
            #[expect(
                clippy::cast_precision_loss,
                reason = "window positions are far below 2^24, where f32 is exact"
            )]
            return (reported.x as f32 / scale, reported.y as f32 / scale);
        }
        let (width, height) = overlay::logical_size(window);
        let main = (px(self.size.0) / self.scale, px(self.size.1) / self.scale);
        (
            ((main.0 - width) / 2.0).max(0.0),
            ((main.1 - height) / 2.0).max(0.0),
        )
    }

    /// Lets the app run for `d` of video time: per frame it pumps one frame of mock time,
    /// captures, draws the cursor and emits. Frame times accumulate exactly (33, 33, 34 ms ...),
    /// so video time equals the mock time pumped; a `d` that is not a whole number of frames
    /// carries its remainder into the next call.
    ///
    /// # Errors
    ///
    /// A failed capture or sink write, or a frame whose size differs from the first.
    pub fn hold(&mut self, d: Duration) -> Result<(), String> {
        self.requested_ms = self.requested_ms.saturating_add(millis(d));
        self.advance(|_, _| {})
    }

    /// Glides the cursor to `to` (logical pixels) over `d` with ease-in-out, pumping and
    /// capturing per frame like [`Recorder::hold`]; then, with `hover_events`, dispatches one
    /// pointer move at `to`. The cursor is shown from the first move on. With
    /// [`Recorder::set_follow_cursor`] a zoomed camera pans along with the glide.
    ///
    /// # Errors
    ///
    /// As [`Recorder::hold`].
    pub fn move_to(&mut self, to: (f32, f32), d: Duration) -> Result<(), String> {
        let from = self.cursor;
        let start = self.requested_ms;
        let length = millis(d);
        let camera_from = self.camera;
        let camera_to = if self.follow_cursor && !camera::is_full(camera_from, self.window_size) {
            camera::pan_to_include(camera_from, to, self.window_size)
        } else {
            camera_from
        };
        self.visible = true;
        self.requested_ms = start.saturating_add(length);
        self.advance(move |recorder, frame_end| {
            let t = timeline::ease_in_out(fraction(frame_end.saturating_sub(start), length));
            recorder.cursor = (lerp(from.0, to.0, t), lerp(from.1, to.1, t));
            recorder.camera = camera::lerp_rect(camera_from, camera_to, t);
        })?;
        self.cursor = to;
        self.camera = camera_to;
        if self.options.hover_events {
            let event = WindowEvent::PointerMoved {
                position: LogicalPosition::new(to.0, to.1),
            };
            // A window that cannot take the event (hidden, closing) is not a recording error.
            match self.overlay.as_ref().filter(|o| o.contains(to)) {
                Some(overlay) => {
                    let local = LogicalPosition::new(to.0 - overlay.at.0, to.1 - overlay.at.1);
                    let _ = overlay
                        .window()
                        .dispatch_event_with_result(WindowEvent::PointerMoved { position: local });
                }
                None => {
                    let _ = self.window().dispatch_event_with_result(event);
                }
            }
        }
        Ok(())
    }

    /// [`Recorder::move_to`] the centre of `element` (absolute position plus half its size).
    ///
    /// # Errors
    ///
    /// As [`Recorder::hold`].
    pub fn move_to_element(&mut self, element: &ElementHandle, d: Duration) -> Result<(), String> {
        let (position, size) = (element.absolute_position(), element.size());
        self.move_to(
            (
                position.x + size.width / 2.0,
                position.y + size.height / 2.0,
            ),
            d,
        )
    }

    /// Draws the click ripple at the cursor for `ripple_ms`, emitting frames as
    /// [`Recorder::hold`] does. Visual only: nothing is pressed.
    ///
    /// # Errors
    ///
    /// As [`Recorder::hold`].
    pub fn click(&mut self) -> Result<(), String> {
        self.ripple = Some(Ripple {
            at: self.cursor,
            start_ms: self.requested_ms,
        });
        self.hold(Duration::from_millis(u64::from(
            self.options.cursor.ripple_ms,
        )))
    }

    /// A real-time wait for background work (as `backend::wait_until`). Frames are emitted as a time-lapse at `speed` times real time
    /// (one frame per `speed / fps` seconds of real time) until `max_video` of video has been
    /// emitted; after that the wait goes on without emitting (a jump cut). The state at the
    /// moment `condition` holds is shown if the budget has not run out. Mock time follows real
    /// time (the real time that passed per poll, 5 to 100 ms). The camera stays where it is.
    /// Returns whether `condition` held before `timeout`.
    ///
    /// # Errors
    ///
    /// As [`Recorder::hold`].
    pub fn wait_while_recording(
        &mut self,
        timeout: Duration,
        speed: f32,
        max_video: Duration,
        mut condition: impl FnMut() -> bool,
    ) -> Result<bool, String> {
        self.guard_boundary()?;
        let started = Instant::now();
        let mut last_pump = started;
        // Background work posts results: every frame of a time-lapse is rendered.
        self.reuse.invalidate();
        let speed = if speed.is_finite() {
            speed.max(0.1)
        } else {
            1.0
        };
        let period = Duration::from_secs_f64(f64::from(speed) / f64::from(self.options.fps));
        let budget = millis(max_video);
        let (mut spent, mut next_due) = (0_u64, Duration::ZERO);
        loop {
            pacing::pump_real(&mut last_pump);
            let held = condition();
            let now = started.elapsed();
            if (held || now >= next_due) && spent < budget {
                let end = frame_ms(self.options.fps, self.frames + 1);
                spent += end - frame_ms(self.options.fps, self.frames);
                self.reuse.invalidate();
                self.emit_frame(end)?;
                self.requested_ms = self.requested_ms.max(end);
                next_due = now + period;
            }
            if held {
                self.guard_boundary()?;
                return Ok(true);
            }
            if now >= timeout {
                self.guard_boundary()?;
                return Ok(false);
            }
            std::thread::sleep(POLL);
        }
    }

    /// The same wait, emitting nothing (a cut). Mock time follows real time (the real time that
    /// passed per poll, 5 to 100 ms; [`pacing::wait_real`]). Returns whether `condition` held
    /// before `timeout`.
    pub fn wait_cut(&mut self, timeout: Duration, condition: impl FnMut() -> bool) -> bool {
        // Real time passes and app threads may post results: the next frame renders afresh.
        self.reuse.invalidate();
        pacing::wait_real(timeout, condition)
    }

    /// Starts a caption cue at the current video time and ends the previous one. Empty text
    /// only ends the cue.
    pub fn caption(&mut self, text: &str) {
        self.track.caption(self.now_ms(), text);
    }

    /// Adds a chapter at the current video time.
    pub fn chapter(&mut self, title: &str) {
        self.track.chapter(self.now_ms(), title);
    }

    /// The video time emitted so far.
    #[must_use]
    pub fn video_time(&self) -> Duration {
        Duration::from_millis(self.now_ms())
    }

    /// Captures the base picture again now, after a change made outside
    /// [`Recorder::hold`] and [`Recorder::move_to`] (an invoked callback and a pump), and
    /// checks that the window kept its size. Frames are always captured fresh, so this only
    /// draws the window once and catches a changed size early.
    ///
    /// # Errors
    ///
    /// The capture failed or the window changed size.
    pub fn refresh(&mut self) -> Result<(), String> {
        self.reuse.invalidate();
        let base = capture_timed(self.window())?;
        self.check_size(&base)
    }

    /// Closes the sink (for ffmpeg: waits for it and renames `<output>.part` to the final
    /// name) and writes `<stem>.srt` and `<stem>.chapters.txt` next to the output.
    ///
    /// # Errors
    ///
    /// ffmpeg's failure with the tail of its stderr, a file that cannot be written, or a
    /// forbidden text that is visible (or was).
    pub fn finish(mut self) -> Result<RecordingSummary, String> {
        // A forbidden text on the last picture fails the recording: dropping `self` deletes the
        // `.part` file, so the video is never completed.
        self.guard_boundary()?;
        let video_time = self.video_time();
        let total = millis(video_time);
        let Self {
            options,
            sink,
            frames,
            track,
            ..
        } = self;
        let started = Instant::now();
        sink.close()?;
        timing::record("recording-encode", started.elapsed());
        let srt = options.output.with_extension("srt");
        let chapters = options.output.with_extension("chapters.txt");
        write_text(&srt, &track.render_srt(total))?;
        write_text(&chapters, &track.render_chapters())?;
        Ok(RecordingSummary {
            output: options.output,
            frames,
            video_time,
            srt,
            chapters,
        })
    }

    /// Drops the recording: kills ffmpeg and deletes the `.part` file (dropping the recorder
    /// without [`Recorder::finish`] does the same).
    pub fn abandon(self) {
        drop(self);
    }

    /// The emitted frames of a [`SinkKind::Memory`] recording (empty for the other sinks).
    #[must_use]
    pub fn frames(&self) -> &[Shot] {
        self.sink.frames()
    }

    /// The window of this recording.
    const fn window(&self) -> &'w slint::Window {
        self.window
    }

    /// The current video time in milliseconds.
    fn now_ms(&self) -> u64 {
        frame_ms(self.options.fps, self.frames)
    }

    /// Emits every whole frame that fits before `requested_ms`; `update` runs before each with
    /// the frame's end time (video milliseconds), to move the cursor.
    fn advance(&mut self, update: impl Fn(&mut Self, u64)) -> Result<(), String> {
        self.guard_boundary()?;
        // The caller may have changed the app since the last call: render the first frame.
        self.reuse.invalidate();
        loop {
            let fps = self.options.fps;
            let end = frame_ms(fps, self.frames + 1);
            if end > self.requested_ms {
                return self.guard_boundary();
            }
            let step = end - frame_ms(fps, self.frames);
            self.note_pump(step);
            backend::pump(step);
            update(self, end);
            self.emit_frame(end)?;
        }
    }

    /// Captures the window, crops it to the camera, draws the cursor, the ripple and the badge
    /// on the copy and writes the frame that ends at video time `end_ms`.
    fn emit_frame(&mut self, end_ms: u64) -> Result<(), String> {
        if let Some(error) = self.badge.take_error() {
            return Err(error);
        }
        self.guard_check(end_ms, false)?;
        let mut frame = self.base_picture()?;
        let view = self.apply_camera(&mut frame);
        let style = self.options.cursor;
        if let Some(ripple) = self.ripple {
            let age = end_ms.saturating_sub(ripple.start_ms);
            if age >= u64::from(style.ripple_ms) {
                self.ripple = None;
            } else {
                let progress = fraction(age, u64::from(style.ripple_ms));
                cursor::draw_ripple(
                    &mut frame,
                    view.point(ripple.at),
                    style.ripple_radius_logical,
                    progress,
                    view.size_scale,
                );
            }
        }
        if self.visible {
            cursor::draw_cursor(
                &mut frame,
                view.point(self.cursor),
                style.size_logical,
                view.size_scale,
            );
        }
        self.draw_badge(&mut frame, end_ms);
        let started = Instant::now();
        self.sink.write(&frame)?;
        timing::record("recording-encode", started.elapsed());
        self.frames += 1;
        Ok(())
    }

    /// Every frame must have the size of the first.
    fn check_size(&self, shot: &Shot) -> Result<(), String> {
        if (shot.width(), shot.height()) == self.size {
            return Ok(());
        }
        Err(format!(
            "the window changed size during the recording: {} x {} pixels, started at {} x {} \
             (do not resize the window while recording)",
            shot.width(),
            shot.height(),
            self.size.0,
            self.size.1
        ))
    }
}

impl std::fmt::Debug for Recorder<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recorder")
            .field("frames", &self.frames)
            .field("size", &self.size)
            .field("scale", &self.scale)
            .field("output", &self.options.output)
            .finish_non_exhaustive()
    }
}

/// [`screenshot::capture`], timed under `recording-capture`.
fn capture_timed(window: &slint::Window) -> Result<Shot, String> {
    let started = Instant::now();
    let shot = screenshot::capture(window);
    timing::record("recording-capture", started.elapsed());
    shot
}

/// A pixel count as `f32` (far below 2^24).
#[expect(
    clippy::cast_precision_loss,
    reason = "picture dimensions are far below 2^24, where f32 is exact"
)]
const fn px(value: u32) -> f32 {
    value as f32
}

/// Writes a text file, creating its directory.
fn write_text(path: &std::path::Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}
