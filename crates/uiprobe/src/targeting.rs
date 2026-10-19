//! Generic element targeting for scripted journeys and videos: find a control by label, say
//! *where on screen to click it*, and plan the scroll that brings it into view.
//!
//! A [`Spot`] is an element's rectangle in logical window pixels plus the clipping it sits under:
//! the viewports of the `Flickable`s (every `ScrollView` and `ListView` has one) that contain it
//! and the window itself. [`Spot::visible_rect`] is the part a user can see and
//! [`Spot::click_point`] the point to click, or `None` when too little of the element shows. Clicking the
//! centre of the *full* rectangle is wrong for a row that is scrolled partly out of its viewport:
//! the centre lies outside the viewport and the click lands on whatever is there.
//!
//! The queries ([`WindowView::spot_label`] and friends) return `Result<Spot, TargetError>`:
//! two different visible controls with the same label are [`TargetError::Ambiguous`] rather than
//! "the first one". A button and its caption (one rectangle inside the other) count as one control.
//!
//! How the clipping is found: the testing backend gives no parent links, but its traversal is
//! depth first in drawing order, so the descendants of an element are the next N elements of
//! [`WindowView::elements`]. A `Flickable` opens such a range and the clip of an element is the
//! intersection of the viewports of the ranges it lies in. A plain `Rectangle` with `clip: true`
//! cannot be recognised (the property is not readable); add such a clip by hand with
//! [`Spot::clipped_by`].
//!
//! The geometry (clipping, the 60 % rule, scroll deltas, nearest candidate, wheel ticks) is pure
//! and lives in [`crate::view::geometry`].

use std::{fmt, ops::Range};

use crate::{
    AccessibleRole, ElementHandle,
    report::Finding,
    scene::{descendant_count, is_flickable},
    view::{WindowView, geometry},
};

pub use crate::view::geometry::Rect;

/// Elements narrower or lower than this (logical pixels) are layout scaffolding, not targets.
const MIN_TARGET_SIDE: f32 = 2.0;

/// One clipping rectangle above a [`Spot`].
#[derive(Clone)]
pub struct Clip {
    /// The clip rectangle in window coordinates (a scroll viewport).
    pub rect: Rect,
    /// The scroll container (`Flickable`) that clips, or `None` for a clip added by hand.
    pub container: Option<ElementHandle>,
}

/// A rectangle on screen in logical pixels, with the element it came from (a group has none) and
/// the clipping it sits under.
///
/// [`Spot::of`] knows nothing about clipping (it has no window to look at); the queries of
/// [`WindowView`] and [`Spot::in_view`] fill it in.
#[derive(Clone)]
pub struct Spot {
    /// Left edge of the full (unclipped) rectangle.
    pub x: f32,
    /// Top edge of the full rectangle.
    pub y: f32,
    /// Width of the full rectangle.
    pub w: f32,
    /// Height of the full rectangle.
    pub h: f32,
    /// The element, when the spot is one element.
    pub handle: Option<ElementHandle>,
    /// Logical size of the window the element lives in, when known.
    window: Option<(f32, f32)>,
    /// The scroll viewports above the element, outermost first.
    clips: Vec<Clip>,
}

impl Spot {
    /// The rectangle `element` covers, without any clipping information.
    #[must_use]
    pub fn of(element: &ElementHandle) -> Self {
        let rect = rect_of(element);
        Self {
            handle: Some(element.clone()),
            ..Self::from_rect(rect)
        }
    }

    /// A spot for a bare rectangle (no element, no clipping).
    #[must_use]
    pub const fn from_rect(rect: Rect) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
            handle: None,
            window: None,
            clips: Vec::new(),
        }
    }

    /// The spot of `element` with its clipping found in `view`: the `Flickable` viewports that
    /// contain it and the window. The element is located in the view by type, label, role and
    /// geometry; one that is not found there (it is gone, or hidden) gets only the window clip.
    #[must_use]
    pub fn in_view(view: &WindowView, element: &ElementHandle) -> Self {
        let scan = Scan::build(view);
        let wanted = fingerprint(element);
        scan.elements
            .iter()
            .position(|candidate| fingerprint(candidate) == wanted)
            .map_or_else(
                || Self::of(element).in_window(view.size),
                |index| scan.spot_at(index, view.size),
            )
    }

    /// The same spot, clipped against a window of this logical size.
    #[must_use]
    pub const fn in_window(mut self, size: (f32, f32)) -> Self {
        self.window = Some(size);
        self
    }

    /// The same spot with one more clip (a `Rectangle { clip: true }`, say) that cannot be
    /// found automatically. It clips the visible rectangle but is not a scroll container.
    #[must_use]
    pub fn clipped_by(mut self, rect: Rect) -> Self {
        self.clips.push(Clip {
            rect,
            container: None,
        });
        self
    }

    /// The full (unclipped) rectangle.
    #[must_use]
    pub const fn rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.w, self.h)
    }

    /// The centre of the full rectangle. For a click use [`Spot::click_point`].
    #[must_use]
    pub const fn center(&self) -> (f32, f32) {
        self.rect().center()
    }

    /// The window size this spot is clipped against, when known.
    #[must_use]
    pub const fn window(&self) -> Option<(f32, f32)> {
        self.window
    }

    /// The scroll viewports (and by-hand clips) above the element, outermost first.
    #[must_use]
    pub fn clips(&self) -> &[Clip] {
        &self.clips
    }

    /// The smallest rectangle holding both (without an element or clipping).
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        let left = self.x.min(other.x);
        let top = self.y.min(other.y);
        Self::from_rect(Rect::new(
            left,
            top,
            (self.x + self.w).max(other.x + other.w) - left,
            (self.y + self.h).max(other.y + other.h) - top,
        ))
    }

    /// Whether the rectangle is big enough to be a control and its centre is inside a window of
    /// `window` logical pixels.
    #[must_use]
    pub const fn usable(&self, window: (f32, f32)) -> bool {
        let (cx, cy) = self.center();
        self.w >= MIN_TARGET_SIDE
            && self.h >= MIN_TARGET_SIDE
            && cx >= 0.0
            && cy >= 0.0
            && cx <= window.0
            && cy <= window.1
    }

    /// The part of the element a user can see: the rectangle clipped against every scroll viewport
    /// above it and against the window. `None` when nothing is left.
    #[must_use]
    pub fn visible_rect(&self) -> Option<Rect> {
        let window = self
            .window
            .map(|(width, height)| Rect::new(0.0, 0.0, width, height));
        geometry::clip_all(
            self.rect(),
            window
                .into_iter()
                .chain(self.clips.iter().map(|clip| clip.rect)),
        )
    }

    /// The fraction (0 to 1) of the element that is visible.
    #[must_use]
    pub fn visible_fraction(&self) -> f32 {
        geometry::visible_fraction(&self.rect(), self.visible_rect().as_ref())
    }

    /// Where to click: the centre of the visible rectangle, or `None` when less than 60 % of the
    /// element (or less than 12 x 12 logical pixels, an element smaller than that on an axis
    /// needs to be fully visible on it) can be seen. See [`geometry::click_point`].
    #[must_use]
    pub fn click_point(&self) -> Option<(f32, f32)> {
        geometry::click_point(&self.rect(), self.visible_rect().as_ref())
    }

    /// Whether the element accepts input (`accessible-enabled` is not `false`; a bare rectangle
    /// is enabled).
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.handle
            .as_ref()
            .is_none_or(|handle| handle.accessible_enabled() != Some(false))
    }
}

/// The scroll that brings `spot` fully into the viewport of the innermost scroll container that
/// cuts it off.
///
/// The plan is that container's spot and the wheel delta to send over it (Slint's convention:
/// positive moves the content down, towards its top). `None` when the element is not in a scroll
/// container or already fits every viewport.
///
/// The plan is pure: nothing is scrolled. Turn the wheel yourself, visibly, through the recorder's
/// `scroll_at` (or `PointerScrolled` events split by [`geometry::wheel_steps`]) at the container's
/// centre, then query again: the container may stop short of the delta (the end of the content),
/// and an outer container may need its own turn next.
#[must_use]
pub fn scroll_into_view_plan(spot: &Spot) -> Option<(Spot, f32)> {
    let element = spot.rect();
    spot.clips.iter().enumerate().rev().find_map(|(at, clip)| {
        let container = clip.container.as_ref()?;
        let delta = geometry::scroll_delta(&element, &clip.rect)?;
        let mut holder = Spot::of(container);
        holder.window = spot.window;
        holder.clips = spot.clips[..at].to_vec();
        Some((holder, delta))
    })
}

/// Why a query did not produce a [`Spot`] to click.
///
/// Non-exhaustive: later releases may add reasons, so a `match` needs a wildcard arm. With `?`
/// inside a scenario the error becomes a [`Finding`] of window `-` (see the `From` impl); use
/// [`TargetError::finding`] to name the window.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TargetError {
    /// Nothing carries the label (or it is not on screen at all). `label` names what was asked.
    NotFound {
        /// The label (or prefix) asked for.
        label: String,
    },
    /// The element exists but [`Spot::click_point`] is `None`: scrolled out or clipped away.
    NotVisible {
        /// The label (or prefix) asked for.
        label: String,
    },
    /// The element is visible but `accessible-enabled` is `false`.
    Disabled {
        /// The label (or prefix) asked for.
        label: String,
    },
    /// Two or more different visible controls carry the label.
    Ambiguous {
        /// The label (or prefix) asked for.
        label: String,
        /// How many different visible controls matched.
        count: usize,
    },
}

impl TargetError {
    /// The label (or prefix) the query asked for.
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::NotFound { label }
            | Self::NotVisible { label }
            | Self::Disabled { label }
            | Self::Ambiguous { label, .. } => label,
        }
    }

    /// The check name of the matching [`Finding`].
    #[must_use]
    pub const fn check(&self) -> &'static str {
        match self {
            Self::NotFound { .. } => "not-found",
            Self::NotVisible { .. } => "not-visible",
            Self::Disabled { .. } => "disabled",
            Self::Ambiguous { .. } => "ambiguous-label",
        }
    }

    /// The error as a finding of `window`.
    #[must_use]
    pub fn finding(&self, window: &str) -> Finding {
        Finding::new(window, "", self.check(), self.to_string())
    }
}

impl fmt::Display for TargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { label } => write!(f, "no element labelled {label:?} on screen"),
            Self::NotVisible { label } => write!(
                f,
                "{label:?} exists but too little of it is visible to click (scrolled out or clipped)"
            ),
            Self::Disabled { label } => write!(f, "{label:?} is disabled"),
            Self::Ambiguous { label, count } => {
                write!(
                    f,
                    "{count} different visible controls are labelled {label:?}"
                )
            }
        }
    }
}

impl std::error::Error for TargetError {}

/// Lets `view.spot_label("Save")?` work inside a scenario (whose error type is [`Finding`]): the
/// finding is a run-wide one (window `-`). Name the window with
/// `.map_err(|error| error.finding("MainWindow"))?` instead.
impl From<TargetError> for Finding {
    fn from(error: TargetError) -> Self {
        error.finding("-")
    }
}

/// How a query compares an element's label (or placeholder) with the text asked for.
///
/// Non-exhaustive: later releases may add ways to compare.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LabelMatch {
    /// The label equals this text.
    Exact(String),
    /// The label starts with this text.
    Prefix(String),
}

impl LabelMatch {
    /// The text asked for.
    #[must_use]
    pub fn text(&self) -> &str {
        match self {
            Self::Exact(text) | Self::Prefix(text) => text,
        }
    }

    /// Whether `candidate` satisfies the match.
    #[must_use]
    pub fn matches(&self, candidate: &str) -> bool {
        match self {
            Self::Exact(text) => candidate == text.as_str(),
            Self::Prefix(text) => candidate.starts_with(text.as_str()),
        }
    }
}

/// A query: a label match, optionally narrowed by role, by an ancestor component and by the row
/// it is on. The `spot_*` methods of [`WindowView`] build the common ones.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetSpec {
    /// The label (or, when no label matches, the placeholder text) to look for.
    pub label: LabelMatch,
    /// Only elements with this accessible role.
    pub role: Option<AccessibleRole>,
    /// Only elements inside a component that is, or inherits, this type name.
    pub within: Option<String>,
    /// Choose the match whose centre is nearest this y (and within the tolerance) instead of
    /// requiring a single one: `(center_y, tolerance)`.
    pub near_y: Option<(f32, f32)>,
}

impl TargetSpec {
    /// Elements labelled exactly `label`.
    #[must_use]
    pub fn exact(label: &str) -> Self {
        Self::new(LabelMatch::Exact(label.to_owned()))
    }

    /// Elements whose label starts with `prefix`.
    #[must_use]
    pub fn prefix(prefix: &str) -> Self {
        Self::new(LabelMatch::Prefix(prefix.to_owned()))
    }

    const fn new(label: LabelMatch) -> Self {
        Self {
            label,
            role: None,
            within: None,
            near_y: None,
        }
    }

    /// Narrows to elements with this accessible role.
    #[must_use]
    pub const fn with_role(mut self, role: AccessibleRole) -> Self {
        self.role = Some(role);
        self
    }

    /// Narrows to elements inside a component that is, or inherits, `ancestor` (a type name such
    /// as `EditorInspector`).
    #[must_use]
    pub fn within_component(mut self, ancestor: &str) -> Self {
        self.within = Some(ancestor.to_owned());
        self
    }

    /// Picks the match whose centre is nearest `center_y`, provided it is within `tolerance`
    /// logical pixels of it (vertically, inclusive): the one on a given row where every row has
    /// such a control. With this a duplicate label is not ambiguous; pass the row height as
    /// the tolerance so another row's control is never picked.
    #[must_use]
    pub const fn near_y(mut self, center_y: f32, tolerance: f32) -> Self {
        self.near_y = Some((center_y, tolerance));
        self
    }
}

impl WindowView {
    /// The spot of `element` with its clipping found in this view. See [`Spot::in_view`].
    #[must_use]
    pub fn spot(&self, element: &ElementHandle) -> Spot {
        Spot::in_view(self, element)
    }

    /// The one visible, enabled control labelled exactly `label` (or, when none has that label,
    /// whose placeholder text it is).
    ///
    /// # Errors
    ///
    /// A [`TargetError`]: `NotFound`, `NotVisible`, `Disabled`, or `Ambiguous` when two different
    /// visible controls carry the label (use [`WindowView::spot_role`] or
    /// [`WindowView::spot_within`] to tell them apart).
    pub fn spot_label(&self, label: &str) -> Result<Spot, TargetError> {
        self.spot_matching(&TargetSpec::exact(label))
    }

    /// The one visible, enabled control whose label starts with `prefix`.
    ///
    /// # Errors
    ///
    /// A [`TargetError`], as [`WindowView::spot_label`].
    pub fn spot_label_start(&self, prefix: &str) -> Result<Spot, TargetError> {
        self.spot_matching(&TargetSpec::prefix(prefix))
    }

    /// The one visible, enabled control with this role and label (a `TextInput` rather than the
    /// caption next to it, say).
    ///
    /// # Errors
    ///
    /// A [`TargetError`], as [`WindowView::spot_label`].
    pub fn spot_role(&self, role: AccessibleRole, label: &str) -> Result<Spot, TargetError> {
        self.spot_matching(&TargetSpec::exact(label).with_role(role))
    }

    /// The one visible, enabled control labelled `label` inside the component `ancestor` (matched
    /// by type name or base, as `match_inherits`): a label the window also carries elsewhere is
    /// never mistaken for it.
    ///
    /// # Errors
    ///
    /// A [`TargetError`], as [`WindowView::spot_label`]; `NotFound` also when no `ancestor` is on
    /// screen.
    pub fn spot_within(&self, ancestor: &str, label: &str) -> Result<Spot, TargetError> {
        self.spot_matching(&TargetSpec::exact(label).within_component(ancestor))
    }

    /// The control labelled `label` whose centre is nearest `center_y` and within `tolerance`
    /// pixels of it: the one on a row of a list that repeats the label on every row.
    ///
    /// # Errors
    ///
    /// A [`TargetError`]: `NotFound` when no match is that close, `NotVisible`, or `Disabled`.
    pub fn spot_label_near_y(
        &self,
        label: &str,
        center_y: f32,
        tolerance: f32,
    ) -> Result<Spot, TargetError> {
        self.spot_matching(&TargetSpec::exact(label).near_y(center_y, tolerance))
    }

    /// [`WindowView::spot_label_near_y`] for a label that starts with `prefix`.
    ///
    /// # Errors
    ///
    /// A [`TargetError`], as [`WindowView::spot_label_near_y`].
    pub fn spot_label_start_near_y(
        &self,
        prefix: &str,
        center_y: f32,
        tolerance: f32,
    ) -> Result<Spot, TargetError> {
        self.spot_matching(&TargetSpec::prefix(prefix).near_y(center_y, tolerance))
    }

    /// Runs `spec`. The elements it matches (one rectangle inside another counts once) are
    /// judged by what a user sees: those with less than 60 % showing do not count as visible.
    ///
    /// # Errors
    ///
    /// A [`TargetError`]: `NotFound` when nothing matches, `NotVisible` when matches exist but none
    /// shows enough, `Ambiguous` when two or more different matches do (unless `spec` picks by
    /// row with [`TargetSpec::near_y`]), `Disabled` when the one that shows is disabled.
    pub fn spot_matching(&self, spec: &TargetSpec) -> Result<Spot, TargetError> {
        let scan = Scan::build(self);
        let found = scan.candidates(spec, self.size)?;
        let rects: Vec<Rect> = found.iter().map(Spot::rect).collect();
        let keep = geometry::distinct_indices(&rects);
        let spots: Vec<Spot> = found
            .into_iter()
            .enumerate()
            .filter_map(|(at, spot)| keep.contains(&at).then_some(spot))
            .collect();
        let picked = choose(spec, spots)?;
        if let Some(handle) = &picked.handle {
            crate::coverage::record_touch(handle);
        }
        Ok(picked)
    }
}

/// The one spot `spec` asks for among `spots` (already one per control), or why there is none.
fn choose(spec: &TargetSpec, spots: Vec<Spot>) -> Result<Spot, TargetError> {
    let label = spec.label.text().to_owned();
    if spots.is_empty() {
        return Err(TargetError::NotFound { label });
    }
    let picked = match spec.near_y {
        Some((center_y, tolerance)) => nearest(spots, center_y, tolerance, label.as_str())?,
        None => only_visible(spots, label.as_str())?,
    };
    if picked.is_enabled() {
        Ok(picked)
    } else {
        Err(TargetError::Disabled { label })
    }
}

/// The spot whose centre is nearest `center_y` within `tolerance`; it must be clickable.
fn nearest(
    spots: Vec<Spot>,
    center_y: f32,
    tolerance: f32,
    label: &str,
) -> Result<Spot, TargetError> {
    let centres: Vec<f32> = spots.iter().map(|spot| spot.center().1).collect();
    let spot = geometry::nearest_within(&centres, center_y, tolerance)
        .and_then(|at| spots.into_iter().nth(at))
        .ok_or_else(|| TargetError::NotFound {
            label: label.to_owned(),
        })?;
    if spot.click_point().is_some() {
        Ok(spot)
    } else {
        Err(TargetError::NotVisible {
            label: label.to_owned(),
        })
    }
}

/// The one clickable spot among `spots`; none is `NotVisible`, several are `Ambiguous`.
fn only_visible(spots: Vec<Spot>, label: &str) -> Result<Spot, TargetError> {
    let mut visible: Vec<Spot> = spots
        .into_iter()
        .filter(|spot| spot.click_point().is_some())
        .collect();
    match visible.len() {
        0 => Err(TargetError::NotVisible {
            label: label.to_owned(),
        }),
        1 => Ok(visible.remove(0)),
        count => Err(TargetError::Ambiguous {
            label: label.to_owned(),
            count,
        }),
    }
}

/// Which accessible text a query compares.
#[derive(Clone, Copy)]
enum Field {
    Label,
    Placeholder,
}

/// A scroll container and the span of elements (document order) it clips.
struct Opened {
    /// Index of the element that opens the range; its descendants follow it.
    start: usize,
    /// One past the last descendant.
    end: usize,
    clip: Clip,
}

/// Every element of a window in drawing order, with the scroll containers that clip them.
struct Scan {
    elements: Vec<ElementHandle>,
    flickables: Vec<Opened>,
}

impl Scan {
    fn build(view: &WindowView) -> Self {
        let elements = view.elements();
        let mut flickables = Vec::new();
        for (index, handle) in elements.iter().enumerate() {
            let type_name = handle
                .type_name()
                .map(|name| name.to_string())
                .unwrap_or_default();
            if !is_flickable(handle, &type_name) {
                continue;
            }
            let count = descendant_count(handle);
            if count > 0 {
                flickables.push(Opened {
                    start: index,
                    end: index + 1 + count,
                    clip: Clip {
                        rect: rect_of(handle),
                        container: Some(handle.clone()),
                    },
                });
            }
        }
        Self {
            elements,
            flickables,
        }
    }

    /// The element at `index` as a spot with its clips and the window.
    fn spot_at(&self, index: usize, window: (f32, f32)) -> Spot {
        let mut spot = Spot::of(&self.elements[index]).in_window(window);
        spot.clips = self
            .flickables
            .iter()
            .filter(|opened| opened.start < index && index < opened.end)
            .map(|opened| opened.clip.clone())
            .collect();
        spot
    }

    /// The element ranges of every component that is, or inherits, `name`.
    fn scopes(&self, name: &str) -> Vec<Range<usize>> {
        self.elements
            .iter()
            .enumerate()
            .filter(|(_, handle)| inherits(handle, name))
            .map(|(index, handle)| index + 1..index + 1 + descendant_count(handle))
            .collect()
    }

    /// Whether the element at `index` satisfies the role and label of `spec` for `field`.
    fn matches(&self, index: usize, spec: &TargetSpec, field: Field) -> bool {
        let handle = &self.elements[index];
        if spec
            .role
            .is_some_and(|role| handle.accessible_role() != Some(role))
        {
            return false;
        }
        let text = match field {
            Field::Label => handle.accessible_label(),
            Field::Placeholder => handle.accessible_placeholder_text(),
        };
        text.is_some_and(|text| spec.label.matches(text.as_str()))
    }

    /// Every target-sized element `spec` matches: by label, and when none has it by placeholder
    /// text (a text field carries its placeholder instead of a label).
    fn candidates(&self, spec: &TargetSpec, window: (f32, f32)) -> Result<Vec<Spot>, TargetError> {
        let scopes = match &spec.within {
            Some(name) => {
                let found = self.scopes(name);
                if found.is_empty() {
                    return Err(TargetError::NotFound {
                        label: format!(
                            "{} (inside {name}, which is not on screen)",
                            spec.label.text()
                        ),
                    });
                }
                Some(found)
            }
            None => None,
        };
        for field in [Field::Label, Field::Placeholder] {
            let spots: Vec<Spot> = (0..self.elements.len())
                .filter(|index| {
                    scopes
                        .as_ref()
                        .is_none_or(|ranges| ranges.iter().any(|range| range.contains(index)))
                })
                .filter(|&index| self.matches(index, spec, field))
                .map(|index| self.spot_at(index, window))
                .filter(|spot| spot.w >= MIN_TARGET_SIDE && spot.h >= MIN_TARGET_SIDE)
                .collect();
            if !spots.is_empty() {
                return Ok(spots);
            }
        }
        Ok(Vec::new())
    }
}

/// The rectangle `handle` covers in window coordinates.
fn rect_of(handle: &ElementHandle) -> Rect {
    let position = handle.absolute_position();
    let size = handle.size();
    Rect::new(position.x, position.y, size.width, size.height)
}

/// Whether the element is, or inherits, the component `name`.
fn inherits(handle: &ElementHandle, name: &str) -> bool {
    handle
        .type_name()
        .is_some_and(|type_name| type_name.as_str() == name)
        || handle
            .bases()
            .is_some_and(|mut bases| bases.any(|base| base.as_str() == name))
}

/// A string that tells an element from the others of a window: type, label, role and geometry.
fn fingerprint(handle: &ElementHandle) -> String {
    let rect = rect_of(handle);
    format!(
        "{}|{}|{:?}|{:.2}|{:.2}|{:.2}|{:.2}",
        handle
            .type_name()
            .map(|name| name.to_string())
            .unwrap_or_default(),
        handle
            .accessible_label()
            .map(|label| label.to_string())
            .unwrap_or_default(),
        handle.accessible_role(),
        rect.x,
        rect.y,
        rect.w,
        rect.h,
    )
}

#[cfg(test)]
mod tests;
