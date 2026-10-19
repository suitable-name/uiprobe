//! The recorder's virtual camera: `zoom_to`, `zoom_out`, `set_follow_cursor`, `camera`, and the
//! step that crops a captured frame to the camera.
//!
//! Every frame is captured in full (at the window's physical resolution), the camera rectangle
//! is cropped out of it at that resolution and scaled to the frame size with a bicubic filter
//! (`resample.rs`), and only then are the cursor, the ripple and the badge drawn, in frame
//! space. A 4K capture zoomed to 2.0 therefore stays as sharp as a 1080p picture. At zoom 1.0
//! nothing is resampled and the frame is the capture itself.

use super::{
    Recorder, camera,
    camera::{Rect, View},
    fraction, millis, px, resample,
    timeline::ease_in_out,
};
use crate::{screenshot::Shot, timing};
use std::time::{Duration, Instant};

impl Recorder<'_> {
    /// Eases the camera over `d` (frames are emitted as in [`Recorder::hold`], with
    /// ease-in-out) until `region` (x, y, width, height in logical window pixels) is fully
    /// visible.
    ///
    /// The camera is the region padded by [`crate::recording::CameraOptions::margin`] on every
    /// side, grown to the frame's aspect ratio, limited to
    /// [`crate::recording::CameraOptions::max_zoom`] (a region that would need more zoom is
    /// shown with the zoom at the limit, centred on the region) and moved to lie inside the
    /// window. A region that needs the whole window zooms out.
    ///
    /// # Errors
    ///
    /// A region that is not finite, empty or entirely outside the window; or as
    /// [`Recorder::hold`].
    pub fn zoom_to(&mut self, region: (f32, f32, f32, f32), d: Duration) -> Result<(), String> {
        let (window_w, window_h) = self.window_size;
        let finite = <[f32; 4]>::from(region).into_iter().all(f32::is_finite);
        if !finite || region.2 <= 0.0 || region.3 <= 0.0 {
            return Err(format!(
                "zoom_to: the region {region:?} must be finite with a positive width and height"
            ));
        }
        if region.0 + region.2 <= 0.0
            || region.1 + region.3 <= 0.0
            || region.0 >= window_w
            || region.1 >= window_h
        {
            return Err(format!(
                "zoom_to: the region {region:?} lies outside the {window_w} x {window_h} window"
            ));
        }
        let options = self.options.camera;
        let target = camera::fit_region(region, self.window_size, options.margin, options.max_zoom);
        self.glide_camera(target, d)
    }

    /// Eases the camera back to the whole window over `d` (frames are emitted as in
    /// [`Recorder::hold`]).
    ///
    /// # Errors
    ///
    /// As [`Recorder::hold`].
    pub fn zoom_out(&mut self, d: Duration) -> Result<(), String> {
        self.glide_camera(camera::full(self.window_size), d)
    }

    /// While on and the camera is zoomed, [`Recorder::move_to`] pans the camera along with its
    /// glide (same duration and easing) by the least amount that puts the target inside the
    /// central 70 % of the camera, so the cursor never leaves the picture on its way to a click.
    /// The cursor is inside that area during the whole glide if it started inside it. Off by
    /// default.
    pub const fn set_follow_cursor(&mut self, on: bool) {
        self.follow_cursor = on;
    }

    /// The camera rectangle (x, y, width, height) in logical window pixels: the whole window
    /// unless zoomed.
    #[must_use]
    pub const fn camera(&self) -> (f32, f32, f32, f32) {
        self.camera
    }

    /// Moves the camera to `target` over `d`.
    fn glide_camera(&mut self, target: Rect, d: Duration) -> Result<(), String> {
        let from = self.camera;
        let start = self.requested_ms;
        let length = millis(d);
        self.requested_ms = start.saturating_add(length);
        self.advance(move |recorder, frame_end| {
            let t = ease_in_out(fraction(frame_end.saturating_sub(start), length));
            recorder.camera = camera::lerp_rect(from, target, t);
        })?;
        self.camera = target;
        Ok(())
    }

    /// Crops `frame` (a full capture) to the camera and scales it back to the frame size, and
    /// returns how logical window pixels map into the result. At zoom 1.0 the frame is left as
    /// it is.
    pub(super) fn apply_camera(&self, frame: &mut Shot) -> View {
        let scale = frame.scale();
        let cam = self.camera;
        if camera::is_full(cam, self.window_size) {
            return View {
                origin: (0.0, 0.0),
                k: scale,
                size_scale: scale,
            };
        }
        let started = Instant::now();
        let region = (cam.0 * scale, cam.1 * scale, cam.2 * scale, cam.3 * scale);
        *frame = resample::crop_scale(frame, region, self.size);
        timing::record("recording-zoom", started.elapsed());
        let k = px(self.size.0) / cam.2;
        View {
            origin: (cam.0, cam.1),
            k,
            size_scale: if self.options.camera.scale_cursor {
                k
            } else {
                scale
            },
        }
    }
}
