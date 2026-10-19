//! Video time: the frame clock, the glide easing and a few exact conversions.

use std::time::Duration;

/// The end time, in whole milliseconds of video, of frame number `index` (frame 0 ends at 0).
///
/// The times are rounded from `index * 1000 / fps`, so the frame lengths are 33, 33 and 34 ms at
/// 30 fps and their sum never drifts: after `fps` frames exactly one second has passed.
pub fn frame_ms(fps: u32, index: u64) -> u64 {
    let fps = u64::from(fps.max(1));
    (index.saturating_mul(1000) + fps / 2) / fps
}

/// `d` in whole milliseconds (truncated), saturating.
pub fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// `numerator / denominator` as `f32`, clamped to `0..=1`; 1 when the denominator is 0.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "video times are far below 2^52 ms and the result only steers a glide"
)]
pub fn fraction(numerator: u64, denominator: u64) -> f32 {
    if denominator == 0 {
        return 1.0;
    }
    ((numerator as f64 / denominator as f64) as f32).clamp(0.0, 1.0)
}

/// Cubic ease-in-out: 0 at 0, 1 at 1, monotone, slow at both ends. `t` is clamped to `0..=1`
/// (NaN counts as 0).
#[must_use]
pub fn ease_in_out(t: f32) -> f32 {
    if t.is_nan() {
        return 0.0;
    }
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        let u = (-2.0_f32).mul_add(t, 2.0);
        1.0 - u * u * u / 2.0
    }
}

/// Cubic ease-out: fast start, slow end (the click ripple's growth).
pub fn ease_out(t: f32) -> f32 {
    let u = 1.0 - t.clamp(0.0, 1.0);
    (u * u).mul_add(-u, 1.0)
}

/// `from + (to - from) * t`.
pub fn lerp(from: f32, to: f32, t: f32) -> f32 {
    (to - from).mul_add(t, from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_clock_does_not_drift() {
        assert_eq!(frame_ms(30, 0), 0);
        assert_eq!(frame_ms(30, 30), 1000);
        assert_eq!(frame_ms(30, 300), 10_000);
        assert_eq!(frame_ms(24, 24), 1000);
        assert_eq!(frame_ms(60, 3600), 60_000);
        let lengths: Vec<u64> = (0..3)
            .map(|k| frame_ms(30, k + 1) - frame_ms(30, k))
            .collect();
        assert_eq!(lengths.iter().sum::<u64>(), 100);
        assert!(lengths.iter().all(|&l| (33..=34).contains(&l)));
    }

    #[test]
    fn the_easing_is_monotone_and_hits_its_ends() {
        assert!(ease_in_out(0.0).abs() < f32::EPSILON);
        assert!((ease_in_out(1.0) - 1.0).abs() < f32::EPSILON);
        assert!((ease_in_out(0.5) - 0.5).abs() < 1e-6);
        let mut last = 0.0_f32;
        for step in 0..=100_u8 {
            let value = ease_in_out(f32::from(step) / 100.0);
            assert!(value >= last, "step {step}: {value} < {last}");
            last = value;
        }
        assert!(ease_in_out(f32::NAN).abs() < f32::EPSILON);
        assert!((ease_in_out(7.0) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn fractions_clamp() {
        assert!((fraction(1, 4) - 0.25).abs() < 1e-6);
        assert!((fraction(9, 4) - 1.0).abs() < f32::EPSILON);
        assert!((fraction(3, 0) - 1.0).abs() < f32::EPSILON);
    }
}
