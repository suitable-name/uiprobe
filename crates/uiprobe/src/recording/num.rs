//! The float and integer conversions of the recording code, each in one place with its reason.

/// A pixel count as `f32`.
#[expect(
    clippy::cast_precision_loss,
    reason = "picture dimensions are far below 2^24, where f32 is exact"
)]
pub(super) const fn px(value: u32) -> f32 {
    value as f32
}

/// `value` rounded to a pixel count, clamped to `0..=4e9` (NaN gives 0).
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..=4e9 first, which fits u32; `as` maps NaN to 0"
)]
pub(super) const fn round_u32(value: f32) -> u32 {
    value.round().clamp(0.0, 4.0e9) as u32
}

/// `value` rounded up to a pixel count, clamped to `0..=4e9` (NaN gives 0).
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..=4e9 first, which fits u32; `as` maps NaN to 0"
)]
pub(super) const fn ceil_u32(value: f32) -> u32 {
    value.ceil().clamp(0.0, 4.0e9) as u32
}

/// `value` rounded down to an integer, clamped to `-1e12..=1e12` (NaN gives 0).
#[expect(
    clippy::cast_possible_truncation,
    reason = "clamped to a range i64 holds first; `as` maps NaN to 0"
)]
pub(super) const fn floor_i64(value: f32) -> i64 {
    value.floor().clamp(-1.0e12, 1.0e12) as i64
}

/// A colour channel from a float, rounded and clamped to `0..=255`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "rounded and clamped to 0..=255 first"
)]
pub(super) const fn byte(value: f32) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

/// A pixel count as an index; a picture's dimensions always fit in memory, so in `usize`.
pub(super) fn index(value: u32) -> usize {
    usize::try_from(value).expect("a picture dimension fits in usize")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions_round_and_clamp() {
        assert_eq!(round_u32(2.5), 3);
        assert_eq!(round_u32(-4.0), 0);
        assert_eq!(round_u32(f32::NAN), 0);
        assert_eq!(ceil_u32(2.1), 3);
        assert_eq!(floor_i64(-0.5), -1);
        assert_eq!(byte(300.0), 255);
        assert_eq!(byte(-3.0), 0);
        assert_eq!(byte(127.5), 128);
        assert!((px(7) - 7.0).abs() < f32::EPSILON);
        assert_eq!(index(9), 9);
    }
}
