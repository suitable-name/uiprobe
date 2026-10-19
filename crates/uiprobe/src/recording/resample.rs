//! Crop and scale for the virtual camera: a part of a picture, given in (fractional) physical
//! pixels, scaled to the frame size with a bicubic (Catmull-Rom) filter.
//!
//! The scaling is separable (a horizontal pass over the source rows a band of output rows needs,
//! then a vertical pass), the weights of every output column and row are computed once, and the
//! output rows are split over the CPU cores with scoped threads (no dependency). The camera
//! never shows more than the window, so the filter only ever scales up (or 1:1): there is no
//! prefilter. Taps beyond the picture edge repeat the edge pixel; taps beyond the cropped part
//! but inside the picture are real pixels, so a pan is smooth.
//!
//! The alpha channel is filtered like the colours (window captures are opaque).

use super::num::{byte, floor_i64, index, px};
use crate::screenshot::Shot;
use slint::{Rgba8Pixel, SharedPixelBuffer};

/// The most threads one resample uses.
const MAX_THREADS: usize = 16;

/// The taps of one axis: for every output pixel, the four source indices and their weights.
struct Axis {
    taps: Vec<[usize; 4]>,
    weights: Vec<[f32; 4]>,
}

/// The Catmull-Rom weight of a tap at distance `x` from the sample point.
fn cubic(x: f32) -> f32 {
    let x = x.abs();
    if x <= 1.0 {
        (1.5_f32.mul_add(x, -2.5) * x).mul_add(x, 1.0)
    } else if x < 2.0 {
        (-0.5_f32).mul_add(x, 2.5).mul_add(x, -4.0).mul_add(x, 2.0)
    } else {
        0.0
    }
}

/// The taps for `out` output pixels covering the source span `start .. start + length` of an axis
/// of `source` pixels. Output pixel `o` samples the continuous source position
/// `start + (o + 0.5) * length / out`, in pixel-centre coordinates.
fn axis(start: f32, length: f32, out: u32, source: u32) -> Axis {
    let step = length / px(out.max(1));
    let last = i64::from(source.max(1)) - 1;
    let mut taps = Vec::with_capacity(index(out));
    let mut weights = Vec::with_capacity(index(out));
    for o in 0..out {
        let position = (px(o) + 0.5).mul_add(step, start) - 0.5;
        let base = floor_i64(position);
        let fraction = position - position.floor();
        let mut tap = [0_usize; 4];
        let mut weight = [0.0_f32; 4];
        for (k, (offset, offset_f32)) in [(-1_i64, -1.0_f32), (0, 0.0), (1, 1.0), (2, 2.0)]
            .into_iter()
            .enumerate()
        {
            let clamped = (base + offset).clamp(0, last);
            tap[k] = usize::try_from(clamped).unwrap_or(0);
            weight[k] = cubic(fraction - offset_f32);
        }
        taps.push(tap);
        weights.push(weight);
    }
    Axis { taps, weights }
}

/// The part `region` (x, y, width, height in physical pixels of `source`, fractions allowed) of
/// `source`, scaled to `out` pixels, drawn at the scale of `source`.
pub fn crop_scale(source: &Shot, region: (f32, f32, f32, f32), out: (u32, u32)) -> Shot {
    let out = (out.0.max(1), out.1.max(1));
    let (source_w, source_h) = (source.width().max(1), source.height().max(1));
    let columns = axis(region.0, region.2, out.0, source_w);
    let rows = axis(region.1, region.3, out.1, source_h);
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(out.0, out.1);
    let target = buffer.make_mut_slice();
    let (out_w, out_h) = (index(out.0), index(out.1));
    let threads = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .clamp(1, MAX_THREADS)
        .min(out_h);
    let rows_per_chunk = out_h.div_ceil(threads);
    let pixels = source.pixels();
    let source_stride = index(source_w);
    std::thread::scope(|scope| {
        for (number, chunk) in target.chunks_mut(rows_per_chunk * out_w).enumerate() {
            let (columns, rows) = (&columns, &rows);
            scope.spawn(move || {
                fill_band(
                    pixels,
                    source_stride,
                    columns,
                    rows,
                    number * rows_per_chunk,
                    chunk,
                );
            });
        }
    });
    Shot::from_buffer(buffer, source.scale())
}

/// Computes the output rows `first_row ..` that fill `band` (a whole number of output rows).
fn fill_band(
    source: &[Rgba8Pixel],
    source_stride: usize,
    columns: &Axis,
    rows: &Axis,
    first_row: usize,
    band: &mut [Rgba8Pixel],
) {
    let out_w = columns.taps.len();
    let row_count = band.len() / out_w;
    if row_count == 0 {
        return;
    }
    let y_low = rows.taps[first_row][0];
    let y_high = rows.taps[first_row + row_count - 1][3];
    let line = out_w * 4;
    // Horizontal pass over every source row the band touches.
    let mut across = vec![0.0_f32; (y_high - y_low + 1) * line];
    for (number, destination) in across.chunks_mut(line).enumerate() {
        let start = (y_low + number) * source_stride;
        let source_row = &source[start..start + source_stride];
        for ((taps, weights), cell) in columns
            .taps
            .iter()
            .zip(&columns.weights)
            .zip(destination.chunks_mut(4))
        {
            let mut sum = [0.0_f32; 4];
            for (tap, weight) in taps.iter().zip(weights) {
                let pixel = source_row[*tap];
                sum[0] = weight.mul_add(f32::from(pixel.r), sum[0]);
                sum[1] = weight.mul_add(f32::from(pixel.g), sum[1]);
                sum[2] = weight.mul_add(f32::from(pixel.b), sum[2]);
                sum[3] = weight.mul_add(f32::from(pixel.a), sum[3]);
            }
            cell.copy_from_slice(&sum);
        }
    }
    // Vertical pass.
    for (number, destination) in band.chunks_mut(out_w).enumerate() {
        let taps = &rows.taps[first_row + number];
        let weights = &rows.weights[first_row + number];
        for (x, pixel) in destination.iter_mut().enumerate() {
            let mut sum = [0.0_f32; 4];
            for (tap, weight) in taps.iter().zip(weights) {
                let at = (tap - y_low) * line + x * 4;
                for (channel, value) in sum.iter_mut().zip(&across[at..at + 4]) {
                    *channel = weight.mul_add(*value, *channel);
                }
            }
            *pixel = Rgba8Pixel::new(byte(sum[0]), byte(sum[1]), byte(sum[2]), byte(sum[3]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `width` x `height` picture whose pixel `(x, y)` is `(10 x, 10 y, 7, 255)`.
    fn gradient(width: u32, height: u32) -> Shot {
        let mut shot = Shot::blank(width, height, 2.0);
        for (i, pixel) in shot.pixels_mut().iter_mut().enumerate() {
            let x = u8::try_from(i % index(width)).unwrap_or(0);
            let y = u8::try_from(i / index(width)).unwrap_or(0);
            *pixel = Rgba8Pixel::new(10 * x, 10 * y, 7, 255);
        }
        shot
    }

    #[test]
    fn the_weights_of_a_sample_sum_to_one() {
        for step in 0..=20_u8 {
            let fraction = f32::from(step) / 20.0;
            let sum: f32 = [-1.0_f32, 0.0, 1.0, 2.0]
                .iter()
                .map(|offset| cubic(fraction - offset))
                .sum();
            assert!((sum - 1.0).abs() < 1e-5, "fraction {fraction}: {sum}");
        }
    }

    #[test]
    fn the_whole_picture_at_its_own_size_is_copied_exactly() {
        let source = gradient(9, 7);
        let copy = crop_scale(&source, (0.0, 0.0, 9.0, 7.0), (9, 7));
        assert_eq!(copy.pixels(), source.pixels());
        assert!((copy.scale() - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_flat_colour_stays_flat_for_a_fractional_part() {
        let mut source = Shot::blank(8, 6, 1.0);
        source.fill(Rgba8Pixel::new(10, 20, 30, 255));
        let big = crop_scale(&source, (1.5, 1.0, 4.0, 3.0), (16, 12));
        assert_eq!((big.width(), big.height()), (16, 12));
        assert!(
            big.pixels()
                .iter()
                .all(|p| *p == Rgba8Pixel::new(10, 20, 30, 255))
        );
    }

    #[test]
    fn a_gradient_stays_linear_when_scaled_up() {
        // Catmull-Rom reproduces a linear ramp: the middle of the ramp is where it should be.
        let source = gradient(10, 4);
        let big = crop_scale(&source, (0.0, 0.0, 10.0, 4.0), (20, 8));
        // Output column 8 samples source x = (8.5 * 0.5) - 0.5 = 3.75: red 37.5.
        let red = big.pixel(8, 4).map_or(0, |pixel| pixel.r);
        assert!((37..=38).contains(&red), "red {red}");
        // Row 4 samples source y = 1.75: green 17.5.
        let green = big.pixel(8, 4).map_or(0, |pixel| pixel.g);
        assert!((17..=18).contains(&green), "green {green}");
    }

    #[test]
    fn an_edge_stays_where_it_is_and_gets_a_short_transition() {
        let mut source = Shot::blank(8, 2, 1.0);
        for (i, pixel) in source.pixels_mut().iter_mut().enumerate() {
            let white = i % 8 >= 4;
            *pixel = if white {
                Rgba8Pixel::new(255, 255, 255, 255)
            } else {
                Rgba8Pixel::new(0, 0, 0, 255)
            };
        }
        let big = crop_scale(&source, (0.0, 0.0, 8.0, 2.0), (16, 4));
        let at = |x: u32| big.pixel(x, 1).map_or(0, |pixel| pixel.r);
        assert_eq!(at(0), 0);
        assert_eq!(at(15), 255);
        assert!((40..=65).contains(&at(7)), "left of the edge: {}", at(7));
        assert!((190..=215).contains(&at(8)), "right of the edge: {}", at(8));
    }

    #[test]
    fn a_cropped_part_shows_the_pixels_of_that_part() {
        // The part x 2..6 of a 10-wide ramp, scaled 1:1 by 4 columns: red 20, 30, 40, 50.
        let source = gradient(10, 4);
        let part = crop_scale(&source, (2.0, 0.0, 4.0, 4.0), (4, 4));
        let reds: Vec<u8> = (0..4)
            .map(|x| part.pixel(x, 0).map_or(0, |pixel| pixel.r))
            .collect();
        assert_eq!(reds, vec![20, 30, 40, 50]);
    }
}
