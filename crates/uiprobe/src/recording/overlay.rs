//! A second window composited over every frame of a recording (a pop-out window next to the
//! main one).
//!
//! Headless windows are separate surfaces, so the picture of the second window is captured on
//! its own and pasted into the frame at a logical position. It is drawn with a 1-logical-pixel
//! dark border just outside its edge and a soft drop shadow, so it reads as a window of its own.

use crate::screenshot::{self, Shot};
use slint::{ComponentHandle, Rgba8Pixel};

/// The border colour.
const BORDER_RGB: [u8; 3] = [24, 24, 28];

/// The shadow colour.
const SHADOW_RGB: [u8; 3] = [0, 0, 0];

/// The shadow's reach beyond the window edge, in logical pixels.
const SHADOW_BLUR_LOGICAL: f32 = 18.0;

/// How far the shadow is shifted down, in logical pixels.
const SHADOW_DROP_LOGICAL: f32 = 6.0;

/// The shadow's opacity right at the window edge.
const SHADOW_ALPHA: f32 = 0.45;

/// Keeps a strong handle to the overlay window's component without naming its type.
pub(super) trait Held {
    /// The window of the held component.
    fn window(&self) -> &slint::Window;
}

/// A held component.
struct Component<C: ComponentHandle>(C);

impl<C: ComponentHandle> Held for Component<C> {
    fn window(&self) -> &slint::Window {
        self.0.window()
    }
}

/// The attached window and where it sits.
pub(super) struct Overlay {
    held: Box<dyn Held>,
    /// The window's top-left corner in logical pixels of the main window.
    pub(super) at: (f32, f32),
}

impl Overlay {
    /// Holds a clone of `component`.
    pub(super) fn new<C: ComponentHandle + 'static>(component: &C, at: (f32, f32)) -> Self {
        Self {
            held: Box::new(Component(component.clone_strong())),
            at,
        }
    }

    /// The overlay window.
    pub(super) fn window(&self) -> &slint::Window {
        self.held.window()
    }

    /// The window's logical size, which equals its size in the frame's logical pixels.
    pub(super) fn logical_size(&self) -> (f32, f32) {
        logical_size(self.window())
    }

    /// Whether the logical point `p` (main window pixels) is inside the overlay window.
    pub(super) fn contains(&self, p: (f32, f32)) -> bool {
        let (w, h) = self.logical_size();
        self.window().is_visible()
            && p.0 >= self.at.0
            && p.1 >= self.at.1
            && p.0 < self.at.0 + w
            && p.1 < self.at.1 + h
    }

    /// Draws the window onto `frame` (shadow, picture, border). A window that is not shown
    /// any more (the app closed it) draws nothing.
    pub(super) fn draw(&self, frame: &mut Shot) -> Result<(), String> {
        let window = self.window();
        if !window.is_visible() {
            return Ok(());
        }
        let shot = screenshot::capture(window)
            .map_err(|e| format!("the attached window cannot be captured: {e}"))?;
        let scale = frame.scale();
        let (w, h) = self.logical_size();
        let size = (to_pixels(w * scale), to_pixels(h * scale));
        let shot = if (shot.width(), shot.height()) == size && (shot.scale() - scale).abs() < 1e-3 {
            shot
        } else {
            shot.resized(size.0, size.1, scale)
        };
        let origin = (to_pixels(self.at.0 * scale), to_pixels(self.at.1 * scale));
        draw_shadow(frame, origin, size);
        frame.paste(&shot, origin.0, origin.1);
        draw_border(frame, origin, size);
        Ok(())
    }
}

/// The logical size of `window` (physical size over its scale factor).
pub(super) fn logical_size(window: &slint::Window) -> (f32, f32) {
    let physical = window.size();
    let scale = window.scale_factor().max(f32::EPSILON);
    (
        px_f32(physical.width) / scale,
        px_f32(physical.height) / scale,
    )
}

/// A pixel count as `f32` (far below 2^24).
#[expect(
    clippy::cast_precision_loss,
    reason = "window dimensions are far below 2^24, where f32 is exact"
)]
const fn px_f32(value: u32) -> f32 {
    value as f32
}

/// A rounded, non-negative pixel count.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "rounded and clamped to 0..=u32::MAX first"
)]
const fn to_pixels(value: f32) -> u32 {
    value.round().clamp(0.0, u32::MAX as f32) as u32
}

/// `dst` with `rgb` blended over it at opacity `alpha`.
fn blend(dst: &mut Rgba8Pixel, rgb: [u8; 3], alpha: f32) {
    let a = alpha.clamp(0.0, 1.0);
    dst.r = mix(dst.r, rgb[0], a);
    dst.g = mix(dst.g, rgb[1], a);
    dst.b = mix(dst.b, rgb[2], a);
}

/// A channel `d` moved towards `s` by `a`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "rounded and clamped to 0..=255 first"
)]
fn mix(d: u8, s: u8, a: f32) -> u8 {
    f32::from(d)
        .mul_add(1.0 - a, f32::from(s) * a)
        .round()
        .clamp(0.0, 255.0) as u8
}

/// Darkens the frame around the window rectangle `origin` / `size` (physical pixels): opacity
/// falls quadratically from [`SHADOW_ALPHA`] at the (dropped) edge to 0 at the blur radius.
fn draw_shadow(frame: &mut Shot, origin: (u32, u32), size: (u32, u32)) {
    let scale = frame.scale();
    let blur = SHADOW_BLUR_LOGICAL * scale;
    let drop = SHADOW_DROP_LOGICAL * scale;
    let (left, top) = (px_f32(origin.0), px_f32(origin.1) + drop);
    let (right, bottom) = (left + px_f32(size.0), top + px_f32(size.1));
    let (width, height) = (frame.width(), frame.height());
    let x0 = to_pixels((left - blur).floor().max(0.0));
    let x1 = to_pixels((right + blur).ceil()).min(width);
    let y0 = to_pixels((top - blur).floor().max(0.0));
    let y1 = to_pixels((bottom + blur).ceil()).min(height);
    let stride = usize::try_from(width).expect("a picture width fits in usize");
    let pixels = frame.pixels_mut();
    for y in y0..y1 {
        for x in x0..x1 {
            let (px, py) = (px_f32(x) + 0.5, px_f32(y) + 0.5);
            let dx = (left - px).max(px - right).max(0.0);
            let dy = (top - py).max(py - bottom).max(0.0);
            let distance = dx.hypot(dy);
            if distance >= blur {
                continue;
            }
            let falloff = 1.0 - distance / blur;
            let index = usize::try_from(y).expect("a picture row fits in usize") * stride
                + usize::try_from(x).expect("a picture column fits in usize");
            blend(
                &mut pixels[index],
                SHADOW_RGB,
                SHADOW_ALPHA * falloff * falloff,
            );
        }
    }
}

/// Draws a border of one logical pixel (at least one physical pixel) around the window
/// rectangle, outside it, so the window's own pixels stay intact.
fn draw_border(frame: &mut Shot, origin: (u32, u32), size: (u32, u32)) {
    let thickness = to_pixels(frame.scale()).max(1);
    let (width, height) = (frame.width(), frame.height());
    let stride = usize::try_from(width).expect("a picture width fits in usize");
    let x0 = origin.0.saturating_sub(thickness);
    let y0 = origin.1.saturating_sub(thickness);
    let x1 = (origin.0 + size.0 + thickness).min(width);
    let y1 = (origin.1 + size.1 + thickness).min(height);
    let pixels = frame.pixels_mut();
    for y in y0..y1 {
        for x in x0..x1 {
            let inside =
                x >= origin.0 && x < origin.0 + size.0 && y >= origin.1 && y < origin.1 + size.1;
            if inside {
                continue;
            }
            let index = usize::try_from(y).expect("a picture row fits in usize") * stride
                + usize::try_from(x).expect("a picture column fits in usize");
            pixels[index] = Rgba8Pixel::new(BORDER_RGB[0], BORDER_RGB[1], BORDER_RGB[2], 255);
        }
    }
}
