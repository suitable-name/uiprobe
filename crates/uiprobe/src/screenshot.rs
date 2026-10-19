//! Window screenshots (feature `screenshots`): capture a window drawn by Slint's software
//! renderer, crop it to an element, save it as PNG.
//!
//! A screenshot run installs the software renderer with
//! `Harness::new().renderer(Renderer::Software)`; with the default mock renderer [`capture`]
//! returns an error. The renderer draws only when [`capture`] asks, so pumping costs the same
//! as in a normal run. Pump (or [`crate::backend::wait_until`] the app's own threads) before a
//! capture so the layout, the images and the animations have settled.
//!
//! ```no_run
//! use uiprobe::prelude::*;
//! use uiprobe::screenshot;
//!
//! # fn shot(app: &Attached, view: &WindowView, out: &std::path::Path) -> Result<(), String> {
//! screenshot::set_window_size(app.window(), (1920, 1152), 2.0);
//! pump(50);
//! let shot = screenshot::capture(app.window())?;
//! let table = view.find("Cutting Instructions").map_err(|f| f.detail)?;
//! shot.crop_to(&table, 8.0).save_png(&out.join("cutting_instructions.png"))?;
//! # Ok(())
//! # }
//! ```

use crate::ElementHandle;
use slint::{LogicalSize, PhysicalSize, Rgba8Pixel, SharedPixelBuffer, platform::WindowEvent};
use std::{
    fs,
    io::BufWriter,
    path::{Path, PathBuf},
};

/// A captured window picture (physical pixels, RGBA8, straight alpha) and the scale factor it
/// was drawn at, so element rectangles (logical pixels) can be mapped onto it.
#[derive(Clone)]
pub struct Shot {
    buffer: SharedPixelBuffer<Rgba8Pixel>,
    scale: f32,
    /// The physical-pixel origin of this picture inside the full window picture (non-zero after
    /// a crop), so [`Shot::crop_to`] still finds an element after an earlier crop.
    origin: (u32, u32),
}

/// Resizes `window` to `logical` pixels drawn at `scale` physical pixels per logical pixel (2.0
/// for a `HiDPI` picture); the capture is then `logical * scale` pixels.
///
/// The testing backend has no real scale factor of its own: this dispatches the scale change,
/// sets the physical size and then the logical size, in that order. Pump afterwards so the layout
/// follows.
pub fn set_window_size(window: &slint::Window, logical: (u32, u32), scale: f32) {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    window.dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: scale,
    });
    let physical = PhysicalSize::new(scaled(logical.0, scale), scaled(logical.1, scale));
    // The testing window stores the physical size and announces it as the logical size (it
    // assumes scale 1); the Resized event below puts the logical size right.
    window.set_size(physical);
    window.dispatch_event(WindowEvent::Resized {
        size: LogicalSize::new(window_f32(logical.0), window_f32(logical.1)),
    });
}

/// A window dimension in logical pixels as `f32`, the type Slint's sizes use.
#[expect(
    clippy::cast_precision_loss,
    reason = "window dimensions are far below 2^24, where f32 is still exact"
)]
const fn window_f32(value: u32) -> f32 {
    value as f32
}

/// `value * scale`, rounded, at least 1.
fn scaled(value: u32, scale: f32) -> u32 {
    rounded_pixels(f64::from(value) * f64::from(scale)).max(1)
}

/// `value` rounded to whole pixels, clamped to `0..=u32::MAX` (NaN gives 0).
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..=u32::MAX first; `as` maps NaN to 0, which is the wanted result"
)]
fn rounded_pixels(value: f64) -> u32 {
    value.round().clamp(0.0, f64::from(u32::MAX)) as u32
}

/// Draws `window` with the software renderer and returns the picture.
///
/// # Errors
///
/// The renderer's error: most often that the run uses the mock renderer
/// (`Harness::renderer(Renderer::Software)` is missing), or that the window is not shown.
pub fn capture(window: &slint::Window) -> Result<Shot, String> {
    let buffer = window.take_snapshot().map_err(|e| {
        format!(
            "cannot capture the window: {e} (a screenshot run needs \
             Harness::renderer(Renderer::Software) and a shown window)"
        )
    })?;
    Ok(Shot {
        buffer,
        scale: window.scale_factor(),
        origin: (0, 0),
    })
}

impl Shot {
    /// Width in physical pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.buffer.width()
    }

    /// Height in physical pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.buffer.height()
    }

    /// Physical pixels per logical pixel of the capture.
    #[must_use]
    pub const fn scale(&self) -> f32 {
        self.scale
    }

    /// The pixels, row by row.
    #[must_use]
    pub fn pixels(&self) -> &[Rgba8Pixel] {
        self.buffer.as_slice()
    }

    /// The pixels as bytes (RGBA, row by row), for the video encoder's pipe.
    #[cfg(feature = "recording")]
    pub(crate) fn bytes(&self) -> &[u8] {
        self.buffer.as_bytes()
    }

    /// The pixels for drawing into (copies them first when the buffer is shared).
    #[cfg(feature = "recording")]
    pub(crate) fn pixels_mut(&mut self) -> &mut [Rgba8Pixel] {
        self.buffer.make_mut_slice()
    }

    /// A picture made of `buffer`, drawn at `scale` (for the recorder's resampled frames).
    #[cfg(feature = "recording")]
    pub(crate) const fn from_buffer(buffer: SharedPixelBuffer<Rgba8Pixel>, scale: f32) -> Self {
        Self {
            buffer,
            scale,
            origin: (0, 0),
        }
    }

    /// The pixel at `(x, y)` (physical pixels of this picture), `None` outside it.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<Rgba8Pixel> {
        if x >= self.width() || y >= self.height() {
            return None;
        }
        let index = usize::try_from(u64::from(y) * u64::from(self.width()) + u64::from(x)).ok()?;
        self.pixels().get(index).copied()
    }

    /// The part `x, y, width, height` (physical pixels of this picture), clamped to the picture;
    /// an empty or fully outside rectangle gives a 1 x 1 picture of the nearest corner.
    #[must_use]
    pub fn crop(&self, x: u32, y: u32, width: u32, height: u32) -> Self {
        if self.width() == 0 || self.height() == 0 {
            return self.clone();
        }
        let x = x.min(self.width().saturating_sub(1));
        let y = y.min(self.height().saturating_sub(1));
        let width = width.clamp(1, self.width() - x);
        let height = height.clamp(1, self.height() - y);
        let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
        let source = self.pixels();
        let target = buffer.make_mut_slice();
        let (row_from, row_len) = (to_index(x), to_index(width));
        let stride = to_index(self.width());
        for row in 0..to_index(height) {
            let from = (to_index(y) + row) * stride + row_from;
            target[row * row_len..(row + 1) * row_len]
                .copy_from_slice(&source[from..from + row_len]);
        }
        Self {
            buffer,
            scale: self.scale,
            origin: (self.origin.0 + x, self.origin.1 + y),
        }
    }

    /// The part showing `element` plus `margin` logical pixels on every side, clamped to the
    /// picture.
    #[must_use]
    pub fn crop_to(&self, element: &ElementHandle, margin: f32) -> Self {
        let position = element.absolute_position();
        let size = element.size();
        self.crop_logical(
            position.x - margin,
            position.y - margin,
            2.0_f32.mul_add(margin, size.width),
            2.0_f32.mul_add(margin, size.height),
        )
    }

    /// The part `x, y, width, height` given in logical window pixels (as element geometry is),
    /// clamped to the picture.
    #[must_use]
    pub fn crop_logical(&self, x: f32, y: f32, width: f32, height: f32) -> Self {
        let left = physical(x, self.scale);
        let top = physical(y, self.scale);
        let right = physical(x + width, self.scale);
        let bottom = physical(y + height, self.scale);
        let left_here = left.saturating_sub(self.origin.0);
        let top_here = top.saturating_sub(self.origin.1);
        self.crop(
            left_here,
            top_here,
            right.saturating_sub(left.max(self.origin.0)),
            bottom.saturating_sub(top.max(self.origin.1)),
        )
    }

    /// A blank (fully transparent) picture of `width` x `height` physical pixels at `scale`,
    /// for composing several captures (side by side, a dialog over its main window) with
    /// [`Shot::paste`].
    #[must_use]
    pub fn blank(width: u32, height: u32, scale: f32) -> Self {
        Self {
            buffer: SharedPixelBuffer::new(width.max(1), height.max(1)),
            scale,
            origin: (0, 0),
        }
    }

    /// Sets every pixel to `pixel` (a background for [`Shot::paste`] compositions; a
    /// [`Shot::blank`] picture is transparent).
    pub fn fill(&mut self, pixel: Rgba8Pixel) {
        self.buffer.make_mut_slice().fill(pixel);
    }

    /// Copies `other` into this picture with its top-left corner at `(x, y)` (physical pixels),
    /// replacing what is there (no blending); the part outside this picture is dropped. Headless
    /// windows are separate surfaces, so a dialog over its main window is two captures pasted
    /// into one.
    pub fn paste(&mut self, other: &Self, x: u32, y: u32) {
        if x >= self.width() || y >= self.height() {
            return;
        }
        let columns = to_index(other.width().min(self.width() - x));
        let rows = to_index(other.height().min(self.height() - y));
        let (stride, other_stride) = (to_index(self.width()), to_index(other.width()));
        let (x, y) = (to_index(x), to_index(y));
        let source = other.buffer.as_slice();
        let target = self.buffer.make_mut_slice();
        for row in 0..rows {
            let to = (y + row) * stride + x;
            let from = row * other_stride;
            target[to..to + columns].copy_from_slice(&source[from..from + columns]);
        }
    }

    /// This picture resampled (bilinear) to `width` x `height` physical pixels, drawn at `scale`.
    /// Used to bring a second window captured at another scale factor to the scale of the
    /// recording it is composited into.
    #[cfg(feature = "recording")]
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "picture dimensions are far below 2^24; sample indices are clamped into the \
                  picture before they are truncated"
    )]
    pub(crate) fn resized(&self, width: u32, height: u32, scale: f32) -> Self {
        let (width, height) = (width.max(1), height.max(1));
        let (sw, sh) = (self.width().max(1), self.height().max(1));
        let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
        let target = buffer.make_mut_slice();
        let source = self.pixels();
        let fx = sw as f32 / width as f32;
        let fy = sh as f32 / height as f32;
        let stride = to_index(sw);
        for y in 0..height {
            let gy = (y as f32 + 0.5)
                .mul_add(fy, -0.5)
                .clamp(0.0, (sh - 1) as f32);
            let (y0, ty) = (gy.floor() as u32, gy.fract());
            let y1 = (y0 + 1).min(sh - 1);
            for x in 0..width {
                let gx = (x as f32 + 0.5)
                    .mul_add(fx, -0.5)
                    .clamp(0.0, (sw - 1) as f32);
                let (x0, tx) = (gx.floor() as u32, gx.fract());
                let x1 = (x0 + 1).min(sw - 1);
                let at = |px: u32, py: u32| source[to_index(py) * stride + to_index(px)];
                let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
                let mix = |p: u8, q: u8, r: u8, s: u8| {
                    let top = f32::from(p).mul_add(1.0 - tx, f32::from(q) * tx);
                    let bottom = f32::from(r).mul_add(1.0 - tx, f32::from(s) * tx);
                    top.mul_add(1.0 - ty, bottom * ty).round().clamp(0.0, 255.0) as u8
                };
                target[to_index(y) * to_index(width) + to_index(x)] = Rgba8Pixel::new(
                    mix(a.r, b.r, c.r, d.r),
                    mix(a.g, b.g, c.g, d.g),
                    mix(a.b, b.b, c.b, d.b),
                    mix(a.a, b.a, c.a, d.a),
                );
            }
        }
        Self {
            buffer,
            scale,
            origin: (0, 0),
        }
    }

    /// Writes the picture as an 8-bit RGBA PNG. The file is written next to `path` under a
    /// temporary name and then renamed, so a failed run never leaves half a picture behind;
    /// missing parent directories are created.
    ///
    /// # Errors
    ///
    /// The I/O or encoder error, with the path.
    pub fn save_png(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        let temporary = temporary_path(path);
        let written = self.write_png(&temporary);
        if let Err(e) = written {
            let _ = fs::remove_file(&temporary);
            return Err(e);
        }
        fs::rename(&temporary, path).map_err(|e| {
            let _ = fs::remove_file(&temporary);
            format!("cannot move the picture to {}: {e}", path.display())
        })
    }

    /// Encodes the picture into `path`.
    fn write_png(&self, path: &Path) -> Result<(), String> {
        let file =
            fs::File::create(path).map_err(|e| format!("cannot create {}: {e}", path.display()))?;
        let mut encoder = png::Encoder::new(BufWriter::new(file), self.width(), self.height());
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        writer
            .write_image_data(self.buffer.as_bytes())
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        writer
            .finish()
            .map_err(|e| format!("cannot write {}: {e}", path.display()))
    }
}

/// `<name>.partial` next to `path`.
fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");
    path.with_file_name(name)
}

/// A logical coordinate in physical pixels, rounded, clamped at 0.
fn physical(logical: f32, scale: f32) -> u32 {
    rounded_pixels(f64::from(logical) * f64::from(scale))
}

/// A pixel count as an index; a picture's dimensions always fit in memory, so in `usize`.
fn to_index(value: u32) -> usize {
    usize::try_from(value).expect("a picture dimension fits in usize")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `width` x `height` picture whose pixel `(x, y)` has red `x` and green `y`.
    fn gradient(width: u32, height: u32, scale: f32) -> Shot {
        let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
        for (index, pixel) in buffer.make_mut_slice().iter_mut().enumerate() {
            let x = u8::try_from(index % to_index(width)).unwrap_or(u8::MAX);
            let y = u8::try_from(index / to_index(width)).unwrap_or(u8::MAX);
            *pixel = Rgba8Pixel::new(x, y, 0, 255);
        }
        Shot {
            buffer,
            scale,
            origin: (0, 0),
        }
    }

    #[test]
    fn crop_copies_the_rectangle() {
        let shot = gradient(10, 8, 1.0).crop(2, 3, 4, 2);
        assert_eq!((shot.width(), shot.height()), (4, 2));
        assert_eq!(shot.pixel(0, 0), Some(Rgba8Pixel::new(2, 3, 0, 255)));
        assert_eq!(shot.pixel(3, 1), Some(Rgba8Pixel::new(5, 4, 0, 255)));
        assert_eq!(shot.pixel(4, 0), None);
    }

    #[test]
    fn crop_is_clamped_to_the_picture() {
        let shot = gradient(10, 8, 1.0).crop(8, 6, 50, 50);
        assert_eq!((shot.width(), shot.height()), (2, 2));
        let outside = gradient(10, 8, 1.0).crop(40, 40, 5, 5);
        assert_eq!((outside.width(), outside.height()), (1, 1));
        assert_eq!(outside.pixel(0, 0), Some(Rgba8Pixel::new(9, 7, 0, 255)));
    }

    #[test]
    fn logical_crop_follows_the_scale_and_an_earlier_crop() {
        // Scale 2: logical (1, 1) .. (3, 2) is physical (2, 2) .. (6, 4).
        let full = gradient(20, 16, 2.0);
        let part = full.crop_logical(1.0, 1.0, 2.0, 1.0);
        assert_eq!((part.width(), part.height()), (4, 2));
        assert_eq!(part.pixel(0, 0), Some(Rgba8Pixel::new(2, 2, 0, 255)));
        // The same logical rectangle out of a picture already cropped at physical (1, 1).
        let again = full.crop(1, 1, 19, 15).crop_logical(1.0, 1.0, 2.0, 1.0);
        assert_eq!((again.width(), again.height()), (4, 2));
        assert_eq!(again.pixel(0, 0), Some(Rgba8Pixel::new(2, 2, 0, 255)));
    }

    #[test]
    fn paste_copies_and_clips() {
        let mut canvas = Shot::blank(6, 5, 1.0);
        canvas.paste(&gradient(4, 4, 1.0), 3, 2);
        assert_eq!(canvas.pixel(3, 2), Some(Rgba8Pixel::new(0, 0, 0, 255)));
        assert_eq!(canvas.pixel(5, 4), Some(Rgba8Pixel::new(2, 2, 0, 255)));
        assert_eq!(canvas.pixel(2, 2), Some(Rgba8Pixel::new(0, 0, 0, 0)));
        canvas.paste(&gradient(4, 4, 1.0), 9, 9);
        assert_eq!(canvas.pixel(5, 4), Some(Rgba8Pixel::new(2, 2, 0, 255)));
    }

    #[cfg(feature = "recording")]
    #[test]
    fn resized_keeps_a_flat_colour_and_the_new_size() {
        let mut flat = Shot::blank(4, 4, 1.0);
        flat.fill(Rgba8Pixel::new(10, 20, 30, 255));
        let big = flat.resized(8, 6, 2.0);
        assert_eq!((big.width(), big.height()), (8, 6));
        assert!((big.scale() - 2.0).abs() < f32::EPSILON);
        assert_eq!(big.pixel(7, 5), Some(Rgba8Pixel::new(10, 20, 30, 255)));
        assert_eq!(big.pixel(3, 2), Some(Rgba8Pixel::new(10, 20, 30, 255)));
    }

    #[test]
    fn scaled_sizes_round_and_never_reach_zero() {
        assert_eq!(scaled(1366, 2.0), 2732);
        assert_eq!(scaled(3, 0.1), 1);
        assert_eq!(physical(-4.0, 2.0), 0);
        assert_eq!(physical(1.25, 2.0), 3);
    }

    #[test]
    fn save_png_round_trips_the_size() {
        let dir = std::env::temp_dir().join(format!("uiprobe-shot-{}", std::process::id()));
        let path = dir.join("nested").join("shot.png");
        gradient(6, 4, 1.0)
            .save_png(&path)
            .expect("the picture is written");
        let decoder = png::Decoder::new(std::io::BufReader::new(
            fs::File::open(&path).expect("the picture exists"),
        ));
        let reader = decoder.read_info().expect("a valid PNG");
        assert_eq!((reader.info().width, reader.info().height), (6, 4));
        assert!(!temporary_path(&path).exists(), "no temporary file is left");
        let _ = fs::remove_dir_all(&dir);
    }
}
