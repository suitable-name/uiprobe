//! The keystroke badge: `Recorder::show_keys("Ctrl + ↑")` draws a dark rounded pill with a
//! keycap for every key, centred near the bottom of the frame (above where subtitles sit), in
//! output space so a zoomed camera never scales it.
//!
//! The text is rendered by Slint's software renderer (see `keytext.rs`) into a coverage mask,
//! once per label, and painted here; the pill and the keycaps are drawn with an analytic
//! anti-aliased rounded-rectangle edge. A label with several keys joined by ` + ` gets one
//! keycap per key with a plain `+` between them; any other label is one keycap.

use super::{
    Recorder,
    keytext::{self, Mask},
    num::{index, px, round_u32},
    timeline::fraction,
};
use crate::screenshot::Shot;
use slint::Rgba8Pixel;
use std::{collections::HashMap, rc::Rc};

/// The pill behind the keys.
const PILL_RGB: [u8; 3] = [18, 19, 24];
/// The top face of a keycap.
const FACE_RGB: [u8; 3] = [70, 72, 82];
/// The edge below a keycap (its "depth").
const EDGE_RGB: [u8; 3] = [32, 33, 40];
/// The key labels.
const INK_RGB: [u8; 3] = [245, 245, 247];
/// The `+` between keycaps.
const PLUS_RGB: [u8; 3] = [190, 192, 200];
/// The pill's opacity (before the fade).
const PILL_ALPHA: f32 = 0.93;

/// How the keystroke badge looks and how long it stays.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BadgeStyle {
    /// How long a badge is on screen, in milliseconds of video, counted from the frame after
    /// `show_keys`.
    pub visible_ms: u32,
    /// The badge fades out linearly over the last this many milliseconds of `visible_ms`.
    pub fade_ms: u32,
    /// The label's font size in logical pixels (scaled with the frame's scale factor).
    pub font_size_logical: f32,
    /// How far the badge's lower edge is above the frame's bottom, as a fraction of the frame
    /// height (0.16 keeps it clear of burned-in subtitles).
    pub bottom_fraction: f32,
}

impl Default for BadgeStyle {
    fn default() -> Self {
        Self {
            visible_ms: 1400,
            fade_ms: 300,
            font_size_logical: 26.0,
            bottom_fraction: 0.16,
        }
    }
}

/// One element of a badge: a keycap with its label, or the `+` between two keycaps.
#[derive(Debug, Clone)]
enum Item {
    Cap(Rc<Mask>),
    Plus(Rc<Mask>),
}

impl Item {
    fn mask(&self) -> &Mask {
        match self {
            Self::Cap(mask) | Self::Plus(mask) => mask,
        }
    }
}

/// The badge on screen: what it shows and when it was asked for.
#[derive(Debug)]
pub(super) struct Badge {
    items: Vec<Item>,
    start_ms: u64,
}

/// The badge state of a recorder.
#[derive(Debug, Default)]
pub(super) struct BadgeState {
    current: Option<Badge>,
    /// The rendered masks by text.
    cache: HashMap<String, Rc<Mask>>,
    /// A rendering failure, returned by the next frame the recorder emits.
    error: Option<String>,
}

impl BadgeState {
    /// Takes the pending rendering failure.
    pub(super) const fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }
}

/// The keys of `label`: split at ` + `, trimmed, empty ones dropped.
pub(super) fn split_keys(label: &str) -> Vec<&str> {
    label
        .split(" + ")
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect()
}

/// The opacity of a badge `age_ms` after it was asked for: 1 until the fade starts, then falling
/// linearly to 0 at `visible_ms`.
pub(super) fn alpha_at(age_ms: u64, visible_ms: u32, fade_ms: u32) -> f32 {
    let visible = u64::from(visible_ms);
    if age_ms >= visible {
        return 0.0;
    }
    let fade = u64::from(fade_ms.min(visible_ms));
    if fade == 0 || age_ms <= visible - fade {
        return 1.0;
    }
    fraction(visible - age_ms, fade)
}

/// The mask of `text`, rendered the first time and cached.
fn mask_for(
    cache: &mut HashMap<String, Rc<Mask>>,
    text: &str,
    font_logical: f32,
    scale: f32,
) -> Result<Rc<Mask>, String> {
    if let Some(mask) = cache.get(text) {
        return Ok(Rc::clone(mask));
    }
    let mask = Rc::new(keytext::render(text, font_logical, scale)?);
    cache.insert(text.to_owned(), Rc::clone(&mask));
    Ok(mask)
}

/// The items of the badge for `label`.
fn items_for(
    cache: &mut HashMap<String, Rc<Mask>>,
    label: &str,
    font_logical: f32,
    scale: f32,
) -> Result<Vec<Item>, String> {
    let mut items = Vec::new();
    for (number, key) in split_keys(label).into_iter().enumerate() {
        if number > 0 {
            items.push(Item::Plus(mask_for(cache, "+", font_logical, scale)?));
        }
        items.push(Item::Cap(mask_for(cache, key, font_logical, scale)?));
    }
    Ok(items)
}

impl Recorder<'_> {
    /// Shows a keystroke badge for `label` (for example `"Ctrl + ↑"`: keys joined by ` + ` get
    /// a keycap each) from the next frame on, for [`BadgeStyle::visible_ms`], fading out over the
    /// last [`BadgeStyle::fade_ms`]. A new badge replaces the current one; an empty label only
    /// removes it. It emits no frames itself.
    ///
    /// The badge is drawn in the output frame (not zoomed), centred, with its lower edge
    /// [`BadgeStyle::bottom_fraction`] of the frame height above the bottom. The text is rendered
    /// once per label by Slint's software renderer; a label that cannot be rendered makes the
    /// next emitted frame fail with the reason.
    pub fn show_keys(&mut self, label: &str) {
        let start_ms = self.now_ms();
        self.badge.current = None;
        let style = self.options.badge;
        match items_for(
            &mut self.badge.cache,
            label.trim(),
            style.font_size_logical,
            self.scale,
        ) {
            Ok(items) if items.is_empty() => {}
            Ok(items) => self.badge.current = Some(Badge { items, start_ms }),
            Err(error) => self.badge.error = Some(error),
        }
    }

    /// Draws the badge on `frame` if one is on screen at video time `end_ms`; an expired one is
    /// dropped.
    pub(super) fn draw_badge(&mut self, frame: &mut Shot, end_ms: u64) {
        let style = self.options.badge;
        let Some(badge) = &self.badge.current else {
            return;
        };
        let age = end_ms.saturating_sub(badge.start_ms);
        if age >= u64::from(style.visible_ms) {
            self.badge.current = None;
            return;
        }
        draw(
            badge,
            frame,
            &style,
            alpha_at(age, style.visible_ms, style.fade_ms),
        );
    }
}

/// Lays the badge out (in whole physical pixels) and paints it with opacity `alpha`.
fn draw(badge: &Badge, frame: &mut Shot, style: &BadgeStyle, alpha: f32) {
    if alpha <= 0.0 {
        return;
    }
    let font = style.font_size_logical.max(4.0) * frame.scale();
    let pad_x = round_u32(0.6 * font);
    let pad_y = round_u32(0.32 * font);
    let gap = round_u32(0.5 * font);
    let depth = round_u32(0.1 * font).max(1);
    let margin = round_u32(0.35 * font);
    let text_height = badge
        .items
        .iter()
        .map(|item| item.mask().height)
        .max()
        .unwrap_or(0);
    let cap_height = text_height + 2 * pad_y;
    let widths: Vec<u32> = badge
        .items
        .iter()
        .map(|item| match item {
            Item::Cap(mask) => (mask.width + 2 * pad_x).max(cap_height),
            Item::Plus(mask) => mask.width,
        })
        .collect();
    let gaps = u32::try_from(widths.len().saturating_sub(1)).unwrap_or(0);
    let content_w = widths.iter().sum::<u32>() + gap * gaps;
    let pill = (content_w + 2 * margin, cap_height + depth + 2 * margin);
    let (frame_w, frame_h) = (frame.width(), frame.height());
    let left = frame_w.saturating_sub(pill.0) / 2;
    let bottom = round_u32(px(frame_h) * (1.0 - style.bottom_fraction.clamp(0.0, 0.9)));
    let top = bottom.saturating_sub(pill.1);
    fill_rounded(
        frame,
        (px(left), px(top), px(pill.0), px(pill.1)),
        (0.38 * px(pill.1)).min(px(pill.0) / 2.0),
        PILL_RGB,
        PILL_ALPHA * alpha,
    );
    let cap_top = top + margin;
    let cap_radius = 0.28 * px(cap_height);
    let mut x = left + margin;
    for (item, width) in badge.items.iter().zip(widths) {
        let mask = item.mask();
        let text_y = cap_top + cap_height.saturating_sub(mask.height) / 2;
        match item {
            Item::Cap(_) => {
                let (fx, fy, fw, fh) = (px(x), px(cap_top), px(width), px(cap_height));
                fill_rounded(
                    frame,
                    (fx, fy + px(depth), fw, fh),
                    cap_radius,
                    EDGE_RGB,
                    alpha,
                );
                fill_rounded(frame, (fx, fy, fw, fh), cap_radius, FACE_RGB, alpha);
                let text_x = x + width.saturating_sub(mask.width) / 2;
                paint_mask(frame, mask, (text_x, text_y), INK_RGB, alpha);
            }
            Item::Plus(_) => paint_mask(frame, mask, (x, text_y), PLUS_RGB, alpha),
        }
        x += width + gap;
    }
}

/// `dst` with `rgb` blended over it at opacity `alpha` (the frame is opaque, so its alpha stays).
fn blend(dst: &mut Rgba8Pixel, rgb: [u8; 3], alpha: f32) {
    let a = alpha.clamp(0.0, 1.0);
    let mix = |d: u8, s: u8| super::num::byte(f32::from(d).mul_add(1.0 - a, f32::from(s) * a));
    dst.r = mix(dst.r, rgb[0]);
    dst.g = mix(dst.g, rgb[1]);
    dst.b = mix(dst.b, rgb[2]);
}

/// Paints the glyph coverage `mask` with its top-left corner at `at` (physical pixels), in `rgb`
/// at opacity `alpha`; the part outside the frame is dropped.
fn paint_mask(frame: &mut Shot, mask: &Mask, at: (u32, u32), rgb: [u8; 3], alpha: f32) {
    let (frame_w, frame_h) = (frame.width(), frame.height());
    let stride = index(frame_w);
    let pixels = frame.pixels_mut();
    for row in 0..mask.height {
        let y = at.1 + row;
        if y >= frame_h {
            break;
        }
        for column in 0..mask.width {
            let x = at.0 + column;
            if x >= frame_w {
                break;
            }
            let cover = mask.cover[index(row) * index(mask.width) + index(column)];
            if cover > 0 {
                blend(
                    &mut pixels[index(y) * stride + index(x)],
                    rgb,
                    alpha * f32::from(cover) / 255.0,
                );
            }
        }
    }
}

/// Fills the rounded rectangle `rect` (x, y, width, height in physical pixels) with corner
/// `radius`, anti-aliased by the signed distance to its edge.
fn fill_rounded(
    frame: &mut Shot,
    rect: (f32, f32, f32, f32),
    radius: f32,
    rgb: [u8; 3],
    alpha: f32,
) {
    let (frame_w, frame_h) = (frame.width(), frame.height());
    let stride = index(frame_w);
    let x0 = round_u32((rect.0 - 1.0).floor().max(0.0)).min(frame_w);
    let x1 = round_u32((rect.0 + rect.2 + 1.0).ceil()).min(frame_w);
    let y0 = round_u32((rect.1 - 1.0).floor().max(0.0)).min(frame_h);
    let y1 = round_u32((rect.1 + rect.3 + 1.0).ceil()).min(frame_h);
    let (centre_x, centre_y) = (rect.0 + rect.2 / 2.0, rect.1 + rect.3 / 2.0);
    let (reach_x, reach_y) = (rect.2 / 2.0 - radius, rect.3 / 2.0 - radius);
    let pixels = frame.pixels_mut();
    for y in y0..y1 {
        for x in x0..x1 {
            let qx = (px(x) + 0.5 - centre_x).abs() - reach_x;
            let qy = (px(y) + 0.5 - centre_y).abs() - reach_y;
            let distance = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius;
            let cover = (0.5 - distance).clamp(0.0, 1.0);
            if cover > 0.0 {
                blend(
                    &mut pixels[index(y) * stride + index(x)],
                    rgb,
                    alpha * cover,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_mask(width: u32, height: u32) -> Rc<Mask> {
        Rc::new(Mask {
            width,
            height,
            cover: vec![255; index(width) * index(height)],
        })
    }

    fn white_frame() -> Shot {
        let mut frame = Shot::blank(400, 200, 2.0);
        frame.fill(Rgba8Pixel::new(255, 255, 255, 255));
        frame
    }

    /// The box around every pixel that is not white: `(left, top, right, bottom)`, inclusive.
    fn ink_box(frame: &Shot) -> Option<(u32, u32, u32, u32)> {
        let mut found: Option<(u32, u32, u32, u32)> = None;
        for y in 0..frame.height() {
            for x in 0..frame.width() {
                let pixel = frame.pixel(x, y)?;
                if (pixel.r, pixel.g, pixel.b) != (255, 255, 255) {
                    found = Some(match found {
                        None => (x, y, x, y),
                        Some((min_x, min_y, max_x, max_y)) => {
                            (min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y))
                        }
                    });
                }
            }
        }
        found
    }

    #[test]
    fn keys_are_split_at_spaced_plus_signs_only() {
        assert_eq!(split_keys("Ctrl + ↑"), vec!["Ctrl", "↑"]);
        assert_eq!(split_keys("Ctrl + Shift + P"), vec!["Ctrl", "Shift", "P"]);
        assert_eq!(split_keys("Ctrl+S"), vec!["Ctrl+S"]);
        assert_eq!(split_keys("Ctrl + +"), vec!["Ctrl", "+"]);
        assert_eq!(split_keys("  F5 "), vec!["F5"]);
        assert_eq!(split_keys("   ").len(), 0);
    }

    #[test]
    fn the_badge_is_opaque_then_fades_over_the_last_300_ms() {
        let style = BadgeStyle::default();
        let alpha = |age| alpha_at(age, style.visible_ms, style.fade_ms);
        assert!((alpha(1) - 1.0).abs() < f32::EPSILON);
        assert!((alpha(1100) - 1.0).abs() < f32::EPSILON);
        assert!((alpha(1250) - 0.5).abs() < 1e-5);
        assert!(alpha(1399) > 0.0 && alpha(1399) < 0.01);
        assert!(alpha(1400).abs() < f32::EPSILON);
        assert!(alpha(5000).abs() < f32::EPSILON);
        assert!((alpha_at(10, 100, 0) - 1.0).abs() < f32::EPSILON);
        // A fade longer than the badge is cut to the badge: it fades from the first millisecond.
        assert!((alpha_at(10, 100, 500) - 0.9).abs() < 1e-5);
    }

    #[test]
    fn a_badge_is_centred_above_the_bottom_and_does_nothing_at_zero_opacity() {
        let style = BadgeStyle::default();
        let badge = Badge {
            items: vec![
                Item::Cap(solid_mask(60, 30)),
                Item::Plus(solid_mask(14, 14)),
                Item::Cap(solid_mask(20, 30)),
            ],
            start_ms: 0,
        };
        let mut frame = white_frame();
        draw(&badge, &mut frame, &style, 0.0);
        assert_eq!(ink_box(&frame), None);
        draw(&badge, &mut frame, &style, 1.0);
        let (left, top, right, bottom) = ink_box(&frame).expect("the badge is drawn");
        let centre = u32::midpoint(left, right);
        assert!((199..=201).contains(&centre), "centre x {centre}");
        // The lower edge is 16 % of 200 px above the bottom: 168.
        assert!((166..=168).contains(&bottom), "bottom {bottom}");
        assert!(top > 40 && top < bottom, "top {top}");
        assert!(left > 20 && right < 380, "{left}..{right}");
        // The pill's middle is dark.
        let inside = frame
            .pixel(left + 6, u32::midpoint(top, bottom))
            .expect("a pixel");
        assert!(inside.r < 110 && inside.g < 110 && inside.b < 110);
    }

    #[test]
    fn a_rounded_rectangle_has_round_corners_and_a_solid_middle() {
        let mut frame = white_frame();
        fill_rounded(&mut frame, (20.0, 20.0, 100.0, 60.0), 20.0, [0, 0, 0], 1.0);
        assert_eq!(frame.pixel(70, 50).map(|p| p.r), Some(0));
        assert_eq!(
            frame.pixel(20, 20).map(|p| p.r),
            Some(255),
            "the corner is cut"
        );
        let edge = frame.pixel(20, 50).map_or(0, |pixel| pixel.r);
        assert!(edge < 255, "the straight edge is inked");
        assert_eq!(frame.pixel(10, 50).map(|p| p.r), Some(255));
    }

    #[test]
    fn a_mask_is_painted_at_its_position_and_clipped_by_the_frame() {
        let mut frame = white_frame();
        paint_mask(&mut frame, &solid_mask(4, 4), (10, 10), [0, 0, 0], 1.0);
        assert_eq!(ink_box(&frame), Some((10, 10, 13, 13)));
        let mut edge = white_frame();
        paint_mask(&mut edge, &solid_mask(4, 4), (398, 198), [0, 0, 0], 1.0);
        assert_eq!(ink_box(&edge), Some((398, 198, 399, 199)));
    }
}
