//! The pure math behind the targeting helpers ([`crate::targeting`]).
//!
//! Rectangles, clipping, the "is it clickable" rule, the wheel delta that scrolls an element into
//! view, nearest-candidate selection and the split of a wheel turn into ticks. Nothing here
//! touches a window, so every rule is unit-tested without Slint.

/// The part of an element that must be visible before a click on it is trusted, as a fraction of
/// its area.
pub const MIN_VISIBLE_FRACTION: f32 = 0.6;

/// The smallest visible width and height, in logical pixels, a click target must have. An element
/// that is itself smaller than this on an axis only needs to be fully visible on that axis.
pub const MIN_VISIBLE_SIDE: f32 = 12.0;

/// Rounding slack for the visible-side comparison (logical pixels).
const SLACK: f32 = 1.0e-3;

/// Rounding slack for the visible-fraction comparison (a fraction, so it scales with the area).
const FRACTION_SLACK: f32 = 1.0e-4;

/// The smallest wheel delta worth sending.
const MIN_SCROLL: f32 = 0.5;

/// The largest wheel delta of one tick, in logical pixels.
const WHEEL_TICK: f32 = 120.0;

/// The most ticks one wheel turn is split into.
const WHEEL_MAX_TICKS: u8 = 12;

/// An axis-aligned rectangle in logical window pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl Rect {
    /// The rectangle with this origin and size.
    #[must_use]
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// Right edge.
    #[must_use]
    pub const fn right(&self) -> f32 {
        self.x + self.w
    }

    /// Bottom edge.
    #[must_use]
    pub const fn bottom(&self) -> f32 {
        self.y + self.h
    }

    /// The area, zero when a side is not positive.
    #[must_use]
    pub const fn area(&self) -> f32 {
        self.w.max(0.0) * self.h.max(0.0)
    }

    /// The centre.
    #[must_use]
    pub const fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// Whether the rectangle has no area.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    /// The common part of both rectangles; `None` when they share no area (touching edges do not
    /// count).
    #[must_use]
    pub fn intersect(&self, other: &Self) -> Option<Self> {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        (right > left && bottom > top).then_some(Self {
            x: left,
            y: top,
            w: right - left,
            h: bottom - top,
        })
    }

    /// Whether `inner` lies inside `self`, give or take `tolerance` pixels on every side.
    #[must_use]
    pub const fn contains(&self, inner: &Self, tolerance: f32) -> bool {
        inner.x >= self.x - tolerance
            && inner.y >= self.y - tolerance
            && inner.right() <= self.right() + tolerance
            && inner.bottom() <= self.bottom() + tolerance
    }
}

/// `full` clipped against every rectangle of `clips` (scroll viewports, the window ...); `None`
/// when nothing of it is left, or when `full` has no area to begin with.
#[must_use]
pub fn clip_all(full: Rect, clips: impl IntoIterator<Item = Rect>) -> Option<Rect> {
    if full.is_empty() {
        return None;
    }
    clips
        .into_iter()
        .try_fold(full, |so_far, clip| so_far.intersect(&clip))
}

/// The fraction (0 to 1) of `full` that `visible` covers; zero without a visible part or an area.
#[must_use]
pub fn visible_fraction(full: &Rect, visible: Option<&Rect>) -> f32 {
    let total = full.area();
    match visible {
        Some(part) if total > 0.0 => (part.area() / total).min(1.0),
        _ => 0.0,
    }
}

/// Where to click an element whose visible part is `visible`: the centre of the visible part.
///
/// `None` when less than [`MIN_VISIBLE_FRACTION`] of the element is visible, or the visible part is
/// narrower or lower than [`MIN_VISIBLE_SIDE`] (an element smaller than that on an axis must be
/// fully visible on it).
///
/// Clicking the centre of the *full* rectangle is the bug this rule exists for: a row scrolled
/// partly out of its viewport has its centre outside the viewport, so the click lands on whatever
/// lies there.
#[must_use]
pub fn click_point(full: &Rect, visible: Option<&Rect>) -> Option<(f32, f32)> {
    let part = visible?;
    let total = full.area();
    if total <= 0.0 || part.area() < total * (MIN_VISIBLE_FRACTION - FRACTION_SLACK) {
        return None;
    }
    let needs_width = MIN_VISIBLE_SIDE.min(full.w);
    let needs_height = MIN_VISIBLE_SIDE.min(full.h);
    if part.w + SLACK < needs_width || part.h + SLACK < needs_height {
        return None;
    }
    Some(part.center())
}

/// The wheel delta (Slint convention: positive moves the content down, towards its top) that
/// brings `element` fully into `viewport` vertically.
///
/// `None` when it already is (or the needed move is under half a pixel). An element taller than the viewport is aligned by its top
/// edge, the best that fits. Horizontal position is not considered.
#[must_use]
pub fn scroll_delta(element: &Rect, viewport: &Rect) -> Option<f32> {
    let above = viewport.y - element.y;
    let below = element.bottom() - viewport.bottom();
    let delta = if element.h > viewport.h || above > 0.0 {
        above
    } else if below > 0.0 {
        -below
    } else {
        0.0
    };
    (delta.abs() >= MIN_SCROLL).then_some(delta)
}

/// The indices of `rects` that are distinct controls.
///
/// A rectangle inside another one (a button and its caption, with one pixel of slack) is the same
/// control as the outer one and is dropped; of identical rectangles the earliest stays.
#[must_use]
pub fn distinct_indices(rects: &[Rect]) -> Vec<usize> {
    (0..rects.len())
        .filter(|&at| {
            let inner = &rects[at];
            !rects.iter().enumerate().any(|(other_at, outer)| {
                other_at != at
                    && outer.contains(inner, 1.0)
                    && (!inner.contains(outer, 1.0) || other_at < at)
            })
        })
        .collect()
}

/// The index of the candidate centre closest to `y`, or `None` when no candidate lies within
/// `tolerance` (inclusive) of `y`. Non-finite candidates never match. Ties go to the earlier one.
#[must_use]
pub fn nearest_within(candidates: &[f32], y: f32, tolerance: f32) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (index, &center) in candidates.iter().enumerate() {
        let distance = (center - y).abs();
        if !distance.is_finite() || distance > tolerance {
            continue;
        }
        if best.is_none_or(|(_, so_far)| distance < so_far) {
            best = Some((index, distance));
        }
    }
    best.map(|(index, _)| index)
}

/// The wheel ticks that add up to `delta_y`: none for a zero or non-finite delta, otherwise
/// `ceil(|delta_y| / 120)` equal ticks (each at most 120), at most 12 of them (a larger delta gets
/// larger ticks).
#[must_use]
pub fn wheel_steps(delta_y: f32) -> Vec<f32> {
    if !delta_y.is_finite() || delta_y == 0.0 {
        return Vec::new();
    }
    let wanted = (delta_y.abs() / WHEEL_TICK).ceil();
    let count = (1..=WHEEL_MAX_TICKS)
        .find(|&n| f32::from(n) >= wanted)
        .unwrap_or(WHEEL_MAX_TICKS);
    vec![delta_y / f32::from(count); usize::from(count)]
}

#[cfg(test)]
mod tests {
    use super::{
        Rect, click_point, clip_all, distinct_indices, nearest_within, scroll_delta,
        visible_fraction, wheel_steps,
    };

    /// A 190 x 100 scroll viewport at the origin.
    const VIEWPORT: Rect = Rect::new(0.0, 0.0, 190.0, 100.0);

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1.0e-3
    }

    #[test]
    fn rectangles_intersect_only_with_shared_area() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert_eq!(
            a.intersect(&Rect::new(5.0, 5.0, 10.0, 10.0)),
            Some(Rect::new(5.0, 5.0, 5.0, 5.0))
        );
        assert_eq!(a.intersect(&Rect::new(10.0, 0.0, 5.0, 5.0)), None);
        assert_eq!(a.intersect(&Rect::new(20.0, 20.0, 5.0, 5.0)), None);
    }

    #[test]
    fn a_row_is_clipped_to_the_viewport() {
        let row = Rect::new(5.0, 60.0, 80.0, 50.0);
        let visible = clip_all(row, [VIEWPORT]).expect("part of the row is inside");
        assert_eq!(visible, Rect::new(5.0, 60.0, 80.0, 40.0));
    }

    #[test]
    fn clips_stack_and_the_window_clips_too() {
        let window = Rect::new(0.0, 0.0, 300.0, 90.0);
        let inner = Rect::new(0.0, 20.0, 190.0, 100.0);
        let row = Rect::new(5.0, 10.0, 80.0, 200.0);
        let visible = clip_all(row, [VIEWPORT, inner, window]).expect("a part is left");
        assert_eq!(visible, Rect::new(5.0, 20.0, 80.0, 70.0));
    }

    #[test]
    fn an_element_outside_a_clip_has_no_visible_rect() {
        let below = Rect::new(5.0, 120.0, 80.0, 30.0);
        assert_eq!(clip_all(below, [VIEWPORT]), None);
        assert_eq!(clip_all(Rect::new(0.0, 0.0, 0.0, 10.0), []), None);
    }

    #[test]
    fn an_unclipped_element_is_fully_visible() {
        let row = Rect::new(5.0, 60.0, 80.0, 50.0);
        assert_eq!(clip_all(row, []), Some(row));
        assert!(near(visible_fraction(&row, Some(&row)), 1.0));
    }

    #[test]
    fn the_visible_fraction_is_an_area_ratio() {
        let row = Rect::new(5.0, 60.0, 80.0, 50.0);
        let part = clip_all(row, [VIEWPORT]);
        assert!(near(visible_fraction(&row, part.as_ref()), 0.8));
        assert!(near(visible_fraction(&row, None), 0.0));
        assert!(near(
            visible_fraction(&Rect::new(0.0, 0.0, 0.0, 5.0), Some(&row)),
            0.0
        ));
    }

    #[test]
    fn a_fully_visible_element_is_clicked_at_its_centre() {
        let row = Rect::new(10.0, 10.0, 80.0, 40.0);
        assert_eq!(click_point(&row, Some(&row)), Some((50.0, 30.0)));
    }

    #[test]
    fn a_mostly_visible_element_is_clicked_at_the_centre_of_its_visible_part() {
        let row = Rect::new(5.0, 50.0, 65.0, 60.0);
        let part = clip_all(row, [VIEWPORT]);
        let (cx, cy) = click_point(&row, part.as_ref()).expect("83 percent is visible");
        assert!(near(cx, 37.5));
        assert!(
            near(cy, 75.0),
            "the centre of 50..100, not of 50..110: {cy}"
        );
    }

    #[test]
    fn less_than_sixty_percent_visible_is_not_clickable() {
        // The tier-table bug: the row's centre (105) is below the viewport (100).
        let row = Rect::new(0.0, 85.0, 190.0, 40.0);
        let part = clip_all(row, [VIEWPORT]);
        assert!(row.center().1 > VIEWPORT.bottom());
        assert_eq!(click_point(&row, part.as_ref()), None);
        // Half visible is still too little.
        let half = Rect::new(0.0, 80.0, 190.0, 40.0);
        assert_eq!(
            click_point(&half, clip_all(half, [VIEWPORT]).as_ref()),
            None
        );
    }

    #[test]
    fn exactly_sixty_percent_visible_is_clickable() {
        let row = Rect::new(0.0, 70.0, 100.0, 50.0);
        let part = clip_all(row, [VIEWPORT]).expect("30 of 50 pixels");
        assert!(near(visible_fraction(&row, Some(&part)), 0.6));
        assert!(click_point(&row, Some(&part)).is_some());
    }

    #[test]
    fn a_sliver_under_twelve_pixels_is_not_clickable_even_at_sixty_percent() {
        let row = Rect::new(0.0, 0.0, 15.0, 15.0);
        let part = Rect::new(0.0, 0.0, 15.0, 9.0);
        assert!(near(visible_fraction(&row, Some(&part)), 0.6));
        assert_eq!(click_point(&row, Some(&part)), None);
        let thin = Rect::new(0.0, 0.0, 9.0, 40.0);
        assert_eq!(click_point(&row, Some(&thin)), None);
    }

    #[test]
    fn a_small_element_only_has_to_be_fully_visible() {
        let icon = Rect::new(4.0, 4.0, 8.0, 8.0);
        assert_eq!(click_point(&icon, Some(&icon)), Some((8.0, 8.0)));
        let cut = Rect::new(4.0, 4.0, 8.0, 5.0);
        assert_eq!(click_point(&icon, Some(&cut)), None);
    }

    #[test]
    fn nothing_visible_and_no_area_are_not_clickable() {
        let row = Rect::new(0.0, 0.0, 50.0, 20.0);
        assert_eq!(click_point(&row, None), None);
        let flat = Rect::new(0.0, 0.0, 50.0, 0.0);
        assert_eq!(click_point(&flat, Some(&flat)), None);
    }

    #[test]
    fn an_element_below_the_viewport_scrolls_up() {
        let half = Rect::new(75.0, 70.0, 70.0, 60.0);
        let delta = scroll_delta(&half, &VIEWPORT).expect("30 pixels hang out");
        assert!(near(delta, -30.0), "the content moves up: {delta}");
    }

    #[test]
    fn an_element_above_the_viewport_scrolls_down() {
        let row = Rect::new(5.0, -25.0, 65.0, 30.0);
        let delta = scroll_delta(&row, &VIEWPORT).expect("25 pixels are above");
        assert!(near(delta, 25.0), "the content moves down: {delta}");
    }

    #[test]
    fn a_fully_visible_element_needs_no_scroll() {
        let row = Rect::new(5.0, 10.0, 65.0, 30.0);
        assert_eq!(scroll_delta(&row, &VIEWPORT), None);
        let flush = Rect::new(5.0, 70.0, 65.0, 30.0);
        assert_eq!(scroll_delta(&flush, &VIEWPORT), None);
    }

    #[test]
    fn a_sub_pixel_overhang_is_ignored() {
        let row = Rect::new(5.0, 70.3, 65.0, 30.0);
        assert_eq!(scroll_delta(&row, &VIEWPORT), None);
    }

    #[test]
    fn an_element_taller_than_the_viewport_is_aligned_by_its_top() {
        let tall = Rect::new(0.0, 40.0, 100.0, 300.0);
        let delta = scroll_delta(&tall, &VIEWPORT).expect("the top is 40 pixels down");
        assert!(near(delta, -40.0));
        let tall_above = Rect::new(0.0, -50.0, 100.0, 300.0);
        let delta = scroll_delta(&tall_above, &VIEWPORT).expect("the top is above");
        assert!(near(delta, 50.0));
        let aligned = Rect::new(0.0, 0.0, 100.0, 300.0);
        assert_eq!(scroll_delta(&aligned, &VIEWPORT), None);
    }

    #[test]
    fn a_caption_inside_a_button_is_the_same_control() {
        let rects = [
            Rect::new(10.0, 10.0, 100.0, 40.0),
            Rect::new(30.0, 20.0, 50.0, 20.0),
            Rect::new(10.0, 80.0, 100.0, 40.0),
        ];
        assert_eq!(distinct_indices(&rects), vec![0, 2]);
    }

    #[test]
    fn identical_rectangles_keep_the_earliest() {
        let same = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert_eq!(distinct_indices(&[same, same, same]), vec![0]);
        assert_eq!(distinct_indices(&[]).len(), 0);
    }

    #[test]
    fn side_by_side_controls_stay_distinct() {
        let rects = [
            Rect::new(0.0, 0.0, 80.0, 30.0),
            Rect::new(100.0, 0.0, 80.0, 30.0),
        ];
        assert_eq!(distinct_indices(&rects), vec![0, 1]);
    }

    #[test]
    fn a_wheel_turn_is_split_into_ticks_of_at_most_120() {
        assert_eq!(wheel_steps(100.0), vec![100.0]);
        assert_eq!(wheel_steps(-240.0), vec![-120.0, -120.0]);
        assert_eq!(wheel_steps(-300.0), vec![-100.0, -100.0, -100.0]);
    }

    #[test]
    fn a_big_wheel_turn_is_capped_at_twelve_ticks() {
        let steps = wheel_steps(3000.0);
        assert_eq!(steps.len(), 12);
        assert!((steps.iter().sum::<f32>() - 3000.0).abs() < 1e-3);
    }

    #[test]
    fn no_wheel_ticks_for_zero_or_non_finite_deltas() {
        assert_eq!(wheel_steps(0.0).len(), 0);
        assert_eq!(wheel_steps(f32::NAN).len(), 0);
        assert_eq!(wheel_steps(f32::INFINITY).len(), 0);
    }

    #[test]
    fn the_nearest_candidate_wins() {
        let centres = [10.0, 52.0, 94.0];
        assert_eq!(nearest_within(&centres, 50.0, 20.0), Some(1));
        assert_eq!(nearest_within(&centres, 12.0, 20.0), Some(0));
        assert_eq!(nearest_within(&centres, 100.0, 20.0), Some(2));
    }

    #[test]
    fn nothing_within_the_tolerance_is_none() {
        let centres = [10.0, 52.0, 94.0];
        assert_eq!(nearest_within(&centres, 70.0, 10.0), None);
        assert_eq!(nearest_within(&[], 70.0, 10.0), None);
    }

    #[test]
    fn the_tolerance_is_inclusive_and_a_tie_goes_to_the_earlier_candidate() {
        assert_eq!(nearest_within(&[40.0], 50.0, 10.0), Some(0));
        assert_eq!(nearest_within(&[40.0, 60.0], 50.0, 10.0), Some(0));
    }

    #[test]
    fn non_finite_candidates_never_match() {
        assert_eq!(
            nearest_within(&[f32::NAN, f32::INFINITY], 0.0, f32::MAX),
            None
        );
        assert_eq!(nearest_within(&[f32::NAN, 3.0], 0.0, 5.0), Some(1));
    }
}
