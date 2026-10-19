//! The virtual camera: which part of the window (logical pixels) fills the frame.
//!
//! Pure geometry; the recorder's `zoom_to`, `zoom_out`, `set_follow_cursor` and `camera` live in
//! `zoom.rs`. A camera is a rectangle `(x, y, w, h)` in logical window pixels with the aspect
//! ratio of the window (and so of the frame), so scaling it to the frame never distorts.

use super::timeline::lerp;

/// A rectangle `(x, y, width, height)` in logical pixels.
pub type Rect = (f32, f32, f32, f32);

/// The share of the camera rectangle (per axis, centred) the cursor is kept inside while the
/// camera follows it.
pub const SAFE_FRACTION: f32 = 0.7;

/// How close to the whole window a camera must be to count as "not zoomed" (logical pixels).
const FULL_TOLERANCE: f32 = 1e-3;

/// How the virtual camera frames a region (`Recorder::zoom_to`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraOptions {
    /// Logical pixels kept free around the region on every side (the camera is at least that
    /// much larger than the region unless the zoom limit or the window forbids it).
    pub margin: f32,
    /// The largest zoom factor (window width over camera width); 1.0 or less never zooms.
    pub max_zoom: f32,
    /// Draw the cursor and the click ripple scaled with the zoom (they are part of the screen,
    /// so they grow with it); `false` keeps them at their size at zoom 1.0.
    pub scale_cursor: bool,
}

impl Default for CameraOptions {
    fn default() -> Self {
        Self {
            margin: 24.0,
            max_zoom: 2.0,
            scale_cursor: true,
        }
    }
}

/// How logical window pixels map into a frame: the cursor, the ripple and a pasted window are
/// drawn at `((p - origin) * k)`.
#[derive(Debug, Clone, Copy)]
pub struct View {
    /// The logical position of the camera's top-left corner.
    pub(crate) origin: (f32, f32),
    /// Frame pixels per logical pixel.
    pub(crate) k: f32,
    /// Frame pixels per logical pixel for the size of the cursor and the ripple.
    pub(crate) size_scale: f32,
}

impl View {
    /// The frame position of the logical point `p`.
    pub(crate) fn point(&self, p: (f32, f32)) -> (f32, f32) {
        (
            (p.0 - self.origin.0) * self.k,
            (p.1 - self.origin.1) * self.k,
        )
    }
}

/// The camera showing the whole window.
pub const fn full(window: (f32, f32)) -> Rect {
    (0.0, 0.0, window.0, window.1)
}

/// Whether `camera` shows the whole window (zoom 1.0): the frame is then the capture itself.
pub fn is_full(camera: Rect, window: (f32, f32)) -> bool {
    camera.0.abs() <= FULL_TOLERANCE
        && camera.1.abs() <= FULL_TOLERANCE
        && (camera.2 - window.0).abs() <= FULL_TOLERANCE
        && (camera.3 - window.1).abs() <= FULL_TOLERANCE
}

/// The camera that shows `region` completely: the region (clipped to the window) padded by
/// `margin` on every side, grown to the window's aspect ratio about its centre, grown further to
/// the zoom limit `max_zoom`, then moved to lie inside the window. A region that needs the whole
/// window (or none of it, or a non-finite one) gives the whole window.
///
/// The padding comes before the aspect fix, so every side keeps at least `margin` and the camera
/// is the smallest rectangle of the window's aspect that does.
pub fn fit_region(region: Rect, window: (f32, f32), margin: f32, max_zoom: f32) -> Rect {
    let (window_w, window_h) = window;
    let all_finite = [region.0, region.1, region.2, region.3, window_w, window_h]
        .iter()
        .all(|v| v.is_finite());
    if !all_finite || window_w <= 0.0 || window_h <= 0.0 {
        return full(window);
    }
    let margin = if margin.is_finite() {
        margin.max(0.0)
    } else {
        0.0
    };
    let max_zoom = if max_zoom.is_finite() {
        max_zoom.max(1.0)
    } else {
        1.0
    };
    let left = region.0.max(0.0);
    let top = region.1.max(0.0);
    let right = (region.0 + region.2).min(window_w);
    let bottom = (region.1 + region.3).min(window_h);
    if right <= left || bottom <= top {
        return full(window);
    }
    let (left, top) = (left - margin, top - margin);
    let (right, bottom) = (right + margin, bottom + margin);
    let (centre_x, centre_y) = (f32::midpoint(left, right), f32::midpoint(top, bottom));
    let (mut width, height) = (right - left, bottom - top);
    let aspect = window_w / window_h;
    // A region taller than the window's aspect grows in width; a wider one keeps its width and
    // the camera height follows from it below.
    if width / height < aspect {
        width = height * aspect;
    }
    width = width.max(window_w / max_zoom);
    if width >= window_w {
        return full(window);
    }
    let height = width * window_h / window_w;
    let x = (centre_x - width / 2.0).clamp(0.0, window_w - width);
    let y = (centre_y - height / 2.0).clamp(0.0, window_h - height);
    (x, y, width, height)
}

/// The central part of `camera` the cursor is kept inside (`SAFE_FRACTION` per axis).
pub fn safe_area(camera: Rect) -> Rect {
    let inset = (1.0 - SAFE_FRACTION) / 2.0;
    (
        camera.2.mul_add(inset, camera.0),
        camera.3.mul_add(inset, camera.1),
        camera.2 * SAFE_FRACTION,
        camera.3 * SAFE_FRACTION,
    )
}

/// Whether `point` is inside `rect` (edges included, with a small tolerance for float error).
#[cfg(test)]
pub fn contains(rect: Rect, point: (f32, f32)) -> bool {
    const SLACK: f32 = 1e-3;
    point.0 >= rect.0 - SLACK
        && point.1 >= rect.1 - SLACK
        && point.0 <= rect.0 + rect.2 + SLACK
        && point.1 <= rect.1 + rect.3 + SLACK
}

/// `camera` moved (not resized) by the least amount that puts `point` inside its safe area, then
/// kept inside the window. At the window's edge the point may stay in the outer band of the
/// camera, but never outside the camera.
pub fn pan_to_include(camera: Rect, point: (f32, f32), window: (f32, f32)) -> Rect {
    let safe = safe_area(camera);
    let shift = |value: f32, low: f32, high: f32| {
        if value < low {
            value - low
        } else if value > high {
            value - high
        } else {
            0.0
        }
    };
    let dx = shift(point.0, safe.0, safe.0 + safe.2);
    let dy = shift(point.1, safe.1, safe.1 + safe.3);
    let max_x = (window.0 - camera.2).max(0.0);
    let max_y = (window.1 - camera.3).max(0.0);
    (
        (camera.0 + dx).clamp(0.0, max_x),
        (camera.1 + dy).clamp(0.0, max_y),
        camera.2,
        camera.3,
    )
}

/// The camera between `a` (at `t = 0`) and `b` (at `t = 1`): the centre moves linearly and the
/// size geometrically, so a zoom looks even (the zoom factor changes by the same ratio per
/// step). A pure pan (equal sizes) moves linearly. A camera between two cameras inside the
/// window is inside the window. The ends are exact.
pub fn lerp_rect(a: Rect, b: Rect, t: f32) -> Rect {
    if t <= 0.0 || a == b {
        return a;
    }
    if t >= 1.0 {
        return b;
    }
    let width = a.2 * (b.2 / a.2).powf(t);
    let height = a.3 * (b.3 / a.3).powf(t);
    let centre_x = lerp(a.0 + a.2 / 2.0, b.0 + b.2 / 2.0, t);
    let centre_y = lerp(a.1 + a.3 / 2.0, b.1 + b.3 / 2.0, t);
    (
        centre_x - width / 2.0,
        centre_y - height / 2.0,
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: (f32, f32) = (1920.0, 1080.0);

    fn near(a: Rect, b: Rect) -> bool {
        (a.0 - b.0).abs() < 0.01
            && (a.1 - b.1).abs() < 0.01
            && (a.2 - b.2).abs() < 0.01
            && (a.3 - b.3).abs() < 0.01
    }

    #[test]
    fn a_small_region_is_padded_aspect_fixed_and_limited_to_the_zoom() {
        // 120 x 60 plus 24 on every side is 168 x 108; the 16:9 fix makes it 192 x 108; the
        // zoom limit 2 raises it to 960 x 540, centred on the region (960, 530).
        let camera = fit_region((900.0, 500.0, 120.0, 60.0), WINDOW, 24.0, 2.0);
        assert!(near(camera, (480.0, 260.0, 960.0, 540.0)), "{camera:?}");
    }

    #[test]
    fn a_wide_region_gets_its_height_from_the_aspect() {
        // 1000 x 100: wider than 16:9, so the camera is 1000 wide and 562.5 high; its centre
        // y (150) is too high for that, so it is moved down to the window's top edge.
        let camera = fit_region((100.0, 100.0, 1000.0, 100.0), WINDOW, 0.0, 4.0);
        assert!(near(camera, (100.0, 0.0, 1000.0, 562.5)), "{camera:?}");
    }

    #[test]
    fn a_tall_region_gets_its_width_from_the_aspect() {
        let camera = fit_region((500.0, 0.0, 100.0, 1000.0), WINDOW, 0.0, 4.0);
        assert!(near(camera, (0.0, 0.0, 1777.7778, 1000.0)), "{camera:?}");
    }

    #[test]
    fn the_margin_is_kept_on_every_side() {
        let region = (1000.0, 500.0, 200.0, 100.0);
        let camera = fit_region(region, WINDOW, 50.0, 8.0);
        assert!(
            near(camera, (922.2222, 450.0, 355.5556, 200.0)),
            "{camera:?}"
        );
        assert!(region.0 - camera.0 >= 50.0);
        assert!(region.1 - camera.1 >= 50.0 - 1e-3);
        assert!(camera.0 + camera.2 - (region.0 + region.2) >= 50.0);
        assert!(camera.1 + camera.3 - (region.1 + region.3) >= 50.0 - 1e-3);
    }

    #[test]
    fn the_camera_stays_inside_the_window() {
        let corner = fit_region((0.0, 0.0, 50.0, 50.0), WINDOW, 0.0, 4.0);
        assert!(near(corner, (0.0, 0.0, 480.0, 270.0)), "{corner:?}");
        let far = fit_region((1900.0, 1060.0, 20.0, 20.0), WINDOW, 0.0, 4.0);
        assert!(near(far, (1440.0, 810.0, 480.0, 270.0)), "{far:?}");
    }

    #[test]
    fn the_zoom_never_exceeds_the_limit_and_the_aspect_is_the_windows() {
        for region in [
            (10.0, 10.0, 5.0, 5.0),
            (800.0, 400.0, 300.0, 40.0),
            (0.0, 0.0, 700.0, 700.0),
            (1500.0, 900.0, 400.0, 170.0),
        ] {
            for limit in [1.5, 2.0, 3.0] {
                let camera = fit_region(region, WINDOW, 24.0, limit);
                assert!(WINDOW.0 / camera.2 <= limit + 1e-3, "{region:?} {camera:?}");
                assert!((camera.2 / camera.3 - WINDOW.0 / WINDOW.1).abs() < 1e-3);
                assert!(camera.0 >= -1e-3 && camera.1 >= -1e-3);
                assert!(camera.0 + camera.2 <= WINDOW.0 + 1e-3);
                assert!(camera.1 + camera.3 <= WINDOW.1 + 1e-3);
            }
        }
    }

    #[test]
    fn a_region_that_needs_the_whole_window_gives_the_whole_window() {
        let whole = full(WINDOW);
        assert!(near(
            fit_region((0.0, 0.0, 1920.0, 1080.0), WINDOW, 24.0, 2.0),
            whole
        ));
        assert!(near(
            fit_region((10.0, 10.0, 1900.0, 500.0), WINDOW, 24.0, 2.0),
            whole
        ));
        assert!(near(
            fit_region((10.0, 10.0, 50.0, 50.0), WINDOW, 24.0, 1.0),
            whole
        ));
        assert!(near(
            fit_region((5000.0, 5000.0, 50.0, 50.0), WINDOW, 24.0, 2.0),
            whole
        ));
        assert!(near(
            fit_region((f32::NAN, 0.0, 50.0, 50.0), WINDOW, 24.0, 2.0),
            whole
        ));
        assert!(is_full(whole, WINDOW));
    }

    #[test]
    fn the_cursor_stays_in_the_safe_area_while_the_camera_pans_with_it() {
        let window = (200.0, 100.0);
        let start = (0.0, 0.0, 100.0, 50.0);
        let (from, to) = ((50.0, 25.0), (150.0, 70.0));
        let end = pan_to_include(start, to, window);
        assert!(near(end, (65.0, 27.5, 100.0, 50.0)), "{end:?}");
        assert!(contains(safe_area(end), to));
        for step in 0..=40_u8 {
            let t = f32::from(step) / 40.0;
            let cursor = (lerp(from.0, to.0, t), lerp(from.1, to.1, t));
            let camera = lerp_rect(start, end, t);
            assert!(
                contains(safe_area(camera), cursor),
                "t {t}: {cursor:?} outside {camera:?}"
            );
        }
    }

    #[test]
    fn panning_does_nothing_for_a_cursor_that_is_already_inside_and_clamps_at_the_edge() {
        let window = (200.0, 100.0);
        let camera = (50.0, 25.0, 100.0, 50.0);
        assert_eq!(pan_to_include(camera, (100.0, 50.0), window), camera);
        let edge = pan_to_include(camera, (199.0, 99.0), window);
        assert!(near(edge, (100.0, 50.0, 100.0, 50.0)), "{edge:?}");
        assert!(contains(edge, (199.0, 99.0)));
        assert_eq!(
            pan_to_include(full(window), (10.0, 10.0), window),
            full(window)
        );
    }

    #[test]
    fn a_zoom_glide_has_exact_ends_stays_inside_the_window_and_zooms_evenly() {
        let a = full(WINDOW);
        let b = (480.0, 260.0, 960.0, 540.0);
        assert_eq!(lerp_rect(a, b, 0.0), a);
        assert_eq!(lerp_rect(a, b, 1.0), b);
        let mut last_width = a.2;
        for step in 1..20_u8 {
            let camera = lerp_rect(a, b, f32::from(step) / 20.0);
            assert!(camera.2 < last_width, "the width shrinks monotonically");
            last_width = camera.2;
            assert!(camera.0 >= -1e-2 && camera.1 >= -1e-2);
            assert!(camera.0 + camera.2 <= WINDOW.0 + 1e-2);
            assert!(camera.1 + camera.3 <= WINDOW.1 + 1e-2);
        }
        let middle = lerp_rect(a, b, 0.5);
        assert!(
            1920.0_f32.mul_add(-0.5_f32.sqrt(), middle.2).abs() < 0.1,
            "{middle:?}"
        );
    }

    #[test]
    fn the_view_maps_logical_points_into_the_frame() {
        let view = View {
            origin: (10.0, 20.0),
            k: 4.0,
            size_scale: 4.0,
        };
        assert_eq!(view.point((12.0, 25.0)), (8.0, 20.0));
    }
}
