//! The drawn mouse cursor and the click ripple, composited onto a copy of a frame.
//!
//! Both are drawn procedurally (no image assets): the cursor is the classic arrow polygon with
//! a black outline and white fill, anti-aliased by 4 x 4 supersampling; the ripple is a ring
//! that grows and fades.

use super::timeline::ease_out;
use crate::screenshot::Shot;
use slint::Rgba8Pixel;

/// The classic arrow in a grid 12.2 wide and 20 high, tip at the origin, y pointing down.
const ARROW: [(f32, f32); 7] = [
    (0.0, 0.0),
    (0.0, 17.0),
    (4.2, 13.3),
    (7.2, 20.0),
    (10.0, 18.7),
    (7.1, 12.2),
    (12.2, 12.2),
];

/// The arrow's height in its own grid.
const ARROW_HEIGHT: f32 = 20.0;

/// The arrow's width in its own grid, rounded up.
const ARROW_WIDTH: f32 = 13.0;

/// The total thickness of the outline in logical pixels.
const OUTLINE_LOGICAL: f32 = 1.5;

/// Samples per pixel edge (4 x 4 per pixel).
const SAMPLES: u8 = 4;

/// The ripple's colour.
const RIPPLE_RGB: [u8; 3] = [40, 120, 255];

/// The ripple ring's thickness in logical pixels.
const RIPPLE_RING_LOGICAL: f32 = 3.0;

/// The ripple's opacity at its start.
const RIPPLE_ALPHA: f32 = 0.6;

/// A pixel dimension as `f32` (far below 2^24).
#[expect(
    clippy::cast_precision_loss,
    reason = "picture dimensions are far below 2^24, where f32 is exact"
)]
const fn px_f32(value: u32) -> f32 {
    value as f32
}

/// `value` as a pixel index, clamped into `0..=limit`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..=limit first, and limit fits u32"
)]
const fn clamp_px(value: f32, limit: u32) -> u32 {
    value.floor().clamp(0.0, px_f32(limit)) as u32
}

/// A colour channel from a float in `0..=255`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "rounded and clamped to 0..=255 first"
)]
const fn channel(value: f32) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

/// `dst` with `rgb` blended over it at opacity `alpha`.
fn blend(dst: &mut Rgba8Pixel, rgb: [u8; 3], alpha: f32) {
    let a = alpha.clamp(0.0, 1.0);
    let mix = |d: u8, s: u8| channel(f32::from(d).mul_add(1.0 - a, f32::from(s) * a));
    dst.r = mix(dst.r, rgb[0]);
    dst.g = mix(dst.g, rgb[1]);
    dst.b = mix(dst.b, rgb[2]);
}

/// The pixel range `[from, to)` of a rectangle, clamped to a picture of `limit` pixels.
const fn span(from: f32, to: f32, limit: u32) -> (u32, u32) {
    (clamp_px(from, limit), clamp_px(to.ceil(), limit))
}

/// Draws the arrow with its tip at `origin` (frame pixels) onto `shot`; `size_logical` is the
/// arrow's height in logical pixels and `scale` the frame pixels per logical pixel it is drawn
/// at (the shot's own scale, or the camera's zoom times it).
pub fn draw_cursor(shot: &mut Shot, origin: (f32, f32), size_logical: f32, scale: f32) {
    let unit = size_logical.max(1.0) * scale / ARROW_HEIGHT;
    let band = OUTLINE_LOGICAL * scale / 2.0 / unit;
    let reach = band.mul_add(unit, 1.0);
    let (width, height) = (shot.width(), shot.height());
    let (x0, x1) = span(
        origin.0 - reach,
        ARROW_WIDTH.mul_add(unit, origin.0) + reach,
        width,
    );
    let (y0, y1) = span(
        origin.1 - reach,
        ARROW_HEIGHT.mul_add(unit, origin.1) + reach,
        height,
    );
    let stride = usize::try_from(width).expect("a picture width fits in usize");
    let pixels = shot.pixels_mut();
    let total = f32::from(SAMPLES) * f32::from(SAMPLES);
    for y in y0..y1 {
        for x in x0..x1 {
            let (mut black, mut white) = (0_u16, 0_u16);
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let px = px_f32(x) + (f32::from(sx) + 0.5) / f32::from(SAMPLES);
                    let py = px_f32(y) + (f32::from(sy) + 0.5) / f32::from(SAMPLES);
                    let local = ((px - origin.0) / unit, (py - origin.1) / unit);
                    if edge_distance(local) <= band {
                        black += 1;
                    } else if inside(local) {
                        white += 1;
                    }
                }
            }
            if black + white == 0 {
                continue;
            }
            let index = usize::try_from(y).expect("a picture row fits in usize") * stride
                + usize::try_from(x).expect("a picture column fits in usize");
            let (cover_black, cover_white) = (f32::from(black) / total, f32::from(white) / total);
            let pixel = &mut pixels[index];
            let keep = 1.0 - cover_black - cover_white;
            let mix = |d: u8, b: u8, w: u8| {
                channel(f32::from(d).mul_add(
                    keep,
                    f32::from(b).mul_add(cover_black, f32::from(w) * cover_white),
                ))
            };
            pixel.r = mix(pixel.r, 0, 255);
            pixel.g = mix(pixel.g, 0, 255);
            pixel.b = mix(pixel.b, 0, 255);
        }
    }
}

/// Whether `point` (arrow grid) is inside the arrow polygon (even-odd rule).
fn inside(point: (f32, f32)) -> bool {
    let mut odd = false;
    let mut previous = ARROW[ARROW.len() - 1];
    for &current in &ARROW {
        if (current.1 > point.1) != (previous.1 > point.1) {
            let crossing = (previous.0 - current.0) * (point.1 - current.1)
                / (previous.1 - current.1)
                + current.0;
            if point.0 < crossing {
                odd = !odd;
            }
        }
        previous = current;
    }
    odd
}

/// The distance from `point` to the nearest edge of the arrow polygon (arrow grid units).
fn edge_distance(point: (f32, f32)) -> f32 {
    let mut best = f32::MAX;
    let mut previous = ARROW[ARROW.len() - 1];
    for &current in &ARROW {
        best = best.min(segment_distance(point, previous, current));
        previous = current;
    }
    best
}

/// The distance from `p` to the segment `a`-`b`.
fn segment_distance(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length_squared = dx.mul_add(dx, dy * dy);
    let t = if length_squared <= f32::EPSILON {
        0.0
    } else {
        ((p.0 - a.0).mul_add(dx, (p.1 - a.1) * dy) / length_squared).clamp(0.0, 1.0)
    };
    let (cx, cy) = (dx.mul_add(t, a.0), dy.mul_add(t, a.1));
    (p.0 - cx).hypot(p.1 - cy)
}

/// Draws the click ripple around `center` (frame pixels): a ring whose radius grows from 0 to
/// `max_radius_logical` and whose opacity falls from 0.6 to 0 as `progress` goes 0 to 1. `scale`
/// is the frame pixels per logical pixel the radius and the ring are drawn at.
pub fn draw_ripple(
    shot: &mut Shot,
    center: (f32, f32),
    max_radius_logical: f32,
    progress: f32,
    scale: f32,
) {
    let progress = progress.clamp(0.0, 1.0);
    let radius = max_radius_logical * scale * ease_out(progress);
    let alpha = RIPPLE_ALPHA * (1.0 - progress);
    let half_ring = RIPPLE_RING_LOGICAL * scale / 2.0;
    let (cx, cy) = center;
    let reach = radius + half_ring + 1.0;
    let (width, height) = (shot.width(), shot.height());
    let (x0, x1) = span(cx - reach, cx + reach, width);
    let (y0, y1) = span(cy - reach, cy + reach, height);
    let stride = usize::try_from(width).expect("a picture width fits in usize");
    let pixels = shot.pixels_mut();
    for y in y0..y1 {
        for x in x0..x1 {
            let distance = (px_f32(x) + 0.5 - cx).hypot(px_f32(y) + 0.5 - cy);
            let ring = (half_ring + 0.5 - (distance - radius).abs()).clamp(0.0, 1.0);
            let disc = (radius - distance + 0.5).clamp(0.0, 1.0) * 0.2;
            let cover = ring.max(disc);
            if cover <= 0.0 {
                continue;
            }
            let index = usize::try_from(y).expect("a picture row fits in usize") * stride
                + usize::try_from(x).expect("a picture column fits in usize");
            blend(&mut pixels[index], RIPPLE_RGB, alpha * cover);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arrow_polygon_contains_its_body_not_its_surroundings() {
        assert!(inside((1.0, 5.0)));
        assert!(inside((6.0, 12.5)));
        assert!(!inside((-2.0, 5.0)));
        assert!(!inside((10.0, 2.0)));
        assert!(!inside((11.0, 19.0)));
    }

    #[test]
    fn edge_distance_is_zero_on_an_edge() {
        assert!(edge_distance((0.0, 8.0)) < 1e-6);
        assert!((edge_distance((-3.0, 8.0)) - 3.0).abs() < 1e-5);
    }

    #[test]
    fn blending_mixes_by_opacity() {
        let mut pixel = Rgba8Pixel::new(255, 255, 255, 255);
        blend(&mut pixel, [0, 0, 0], 0.5);
        assert!((127..=129).contains(&pixel.r));
        assert_eq!(pixel.a, 255);
    }
}
