//! The elements of a window with the part of each a user can see (`Rect`, `Placed`,
//! `Scene::build`), shared by the invariant sweep and the smoke engine.
//!
//! How the clipping is found: the testing backend gives no parent links, but its traversal is
//! depth first in drawing order, so the descendants of an element are the next N elements of
//! [`WindowView::elements`] (N from [`ElementHandle::visit_descendants`]). A `Flickable` opens
//! such a range; the clip of an element is the intersection of the viewports of the ranges it
//! lies in. A plain `Rectangle` with `clip: true` cannot be recognised (the property is not
//! readable); Slint itself already drops elements that are completely outside such a clip.
//!
//! How modal layers are found: a role-less `TouchArea` that covers at least
//! [`SweepOptions::modal_cover`] of the window is a backdrop. Slint draws in document order, so
//! every interactive control declared before the backdrop whose centre lies inside it is covered
//! and cannot be clicked: it is `blocked`.
//!
//! Everything here is crate-private.

use std::ops::ControlFlow;

use crate::{
    AccessibleRole, ElementHandle,
    checks::SweepOptions,
    view::{Control, WindowView},
};

/// An axis-aligned rectangle in window coordinates.
#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) w: f32,
    pub(crate) h: f32,
}

impl Rect {
    /// No clip at all (the start of the clip stack).
    pub(crate) const UNBOUNDED: Self = Self {
        x: -1.0e9,
        y: -1.0e9,
        w: 2.0e9,
        h: 2.0e9,
    };

    pub(crate) fn of(handle: &ElementHandle) -> Self {
        let p = handle.absolute_position();
        let s = handle.size();
        Self {
            x: p.x,
            y: p.y,
            w: s.width,
            h: s.height,
        }
    }

    pub(crate) fn right(&self) -> f32 {
        self.x + self.w
    }

    pub(crate) fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    pub(crate) fn area(&self) -> f32 {
        self.w.max(0.0) * self.h.max(0.0)
    }

    /// The common part; zero-sized when the rectangles do not meet.
    pub(crate) fn intersect(&self, other: &Self) -> Self {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        Self {
            x,
            y,
            w: (self.right().min(other.right()) - x).max(0.0),
            h: (self.bottom().min(other.bottom()) - y).max(0.0),
        }
    }

    pub(crate) fn contains_point(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.right() && py >= self.y && py <= self.bottom()
    }

    pub(crate) fn centre(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// Whether `inner` lies fully inside `self` (within `tol`).
    pub(crate) fn contains(&self, inner: &Self, tol: f32) -> bool {
        inner.x >= self.x - tol
            && inner.y >= self.y - tol
            && inner.right() <= self.right() + tol
            && inner.bottom() <= self.bottom() + tol
    }

    /// Whether the two rectangles differ by more than a hair.
    pub(crate) fn differs(&self, other: &Self) -> bool {
        (self.x - other.x).abs() > 0.01
            || (self.y - other.y).abs() > 0.01
            || (self.w - other.w).abs() > 0.01
            || (self.h - other.h).abs() > 0.01
    }
}

/// One element with a role, with the part of it a user can see.
pub struct Placed {
    pub(crate) control: Control,
    /// Document order index in [`WindowView::elements`] (drawing order: later is on top).
    pub(crate) index: usize,
    /// The element's rectangle clipped against the enclosing `Flickable` viewports.
    pub(crate) visible: Rect,
    /// An interactive control (`Control::from_handle` accepted it), not just a role element.
    pub(crate) interactive: bool,
    /// Covered by a later modal layer, so it cannot be clicked.
    pub(crate) blocked: bool,
    /// Index of the enclosing floating layer root ([`SweepOptions::floating_layers`]), if any.
    pub(crate) layer: Option<usize>,
}

/// The elements of a window with their clipped geometry and layers.
pub struct Scene {
    pub(crate) placed: Vec<Placed>,
}

impl Scene {
    pub(crate) fn build(view: &WindowView, options: &SweepOptions) -> Self {
        let elements = view.elements();
        let window_area = view.size.0 * view.size.1;
        // (end index, clip of the children): a Flickable opens a range of its descendants.
        let mut stack: Vec<(usize, Rect)> = Vec::new();
        let mut visibles: Vec<Rect> = Vec::with_capacity(elements.len());
        let mut blockers: Vec<(usize, Rect)> = Vec::new();
        // (end index, root index): a floating layer opens a range of its descendants too.
        let mut layers: Vec<(usize, usize)> = Vec::new();
        let mut layer_of: Vec<Option<usize>> = Vec::with_capacity(elements.len());
        for (index, handle) in elements.iter().enumerate() {
            while stack.last().is_some_and(|(end, _)| index >= *end) {
                stack.pop();
            }
            while layers.last().is_some_and(|(end, _)| index >= *end) {
                layers.pop();
            }
            if is_layer_root(handle, &options.floating_layers) {
                layers.push((index + 1 + descendant_count(handle), index));
            }
            layer_of.push(layers.last().map(|(_, root)| *root));
            let clip = stack.last().map_or(Rect::UNBOUNDED, |(_, c)| *c);
            let visible = Rect::of(handle).intersect(&clip);
            let type_name = handle
                .type_name()
                .map(|t| t.to_string())
                .unwrap_or_default();
            if is_flickable(handle, &type_name) {
                let count = descendant_count(handle);
                if count > 0 {
                    stack.push((index + 1 + count, visible));
                }
            } else if type_name == "TouchArea"
                && has_no_role(handle)
                && visible.area() >= options.modal_cover * window_area
            {
                blockers.push((index, visible));
            }
            visibles.push(visible);
        }
        let mut placed = Vec::new();
        for (index, handle) in elements.iter().enumerate() {
            let visible = visibles[index];
            let control = Control::from_handle(handle.clone());
            let interactive = control.is_some();
            let control = match control {
                Some(c) => c,
                None if options.bounds_for_all_roles => match role_element(handle.clone()) {
                    Some(extra) => extra,
                    None => continue,
                },
                None => continue,
            };
            let blocked = interactive && !visible.is_empty() && !blockers.is_empty() && {
                let end = index + 1 + descendant_count(handle);
                let (cx, cy) = visible.centre();
                blockers
                    .iter()
                    .any(|(at, area)| *at >= end && area.contains_point(cx, cy))
            };
            placed.push(Placed {
                control,
                index,
                visible,
                interactive,
                blocked,
                layer: layer_of[index],
            });
        }
        Self { placed }
    }
}

/// Whether the element roots a floating layer: its type name, or any base name, is in `layers`.
/// For example, in Indicatrix: `Toast`, `PopupWindow`, `GuideOverlayLayer`, `ParametersPanel`.
pub fn is_layer_root(handle: &ElementHandle, layers: &[String]) -> bool {
    if layers.is_empty() {
        return false;
    }
    handle
        .type_name()
        .is_some_and(|t| layers.iter().any(|l| l.as_str() == t.as_str()))
        || handle
            .bases()
            .is_some_and(|mut bases| bases.any(|b| layers.iter().any(|l| l.as_str() == b.as_str())))
}

/// Whether the element is a `Flickable` (the viewport inside every `ScrollView` / `ListView`).
pub fn is_flickable(handle: &ElementHandle, type_name: &str) -> bool {
    type_name == "Flickable"
        || handle
            .bases()
            .is_some_and(|mut bases| bases.any(|b| b.as_str() == "Flickable"))
}

/// Whether the element has no accessible role (a pure event swallower, for example).
pub fn has_no_role(handle: &ElementHandle) -> bool {
    handle
        .accessible_role()
        .is_none_or(|r| r == AccessibleRole::None)
}

/// How many elements lie below `handle` (the next N entries of the document-order list).
pub fn descendant_count(handle: &ElementHandle) -> usize {
    let mut count = 0usize;
    let _ = handle.visit_descendants(|_| {
        count += 1;
        ControlFlow::<()>::Continue(())
    });
    count
}

/// A non-interactive element with a role, wrapped as a [`Control`] only to reuse its geometry
/// and description. Reuses `Control::from_handle`'s fields without the interactive filter.
pub fn role_element(handle: ElementHandle) -> Option<Control> {
    let role = handle.accessible_role()?;
    if role == AccessibleRole::None {
        return None;
    }
    let position = handle.absolute_position();
    let size = handle.size();
    Some(Control {
        role,
        label: handle
            .accessible_label()
            .map(|l| l.to_string())
            .unwrap_or_default(),
        id: handle.id().map(|i| i.to_string()).unwrap_or_default(),
        type_name: handle
            .type_name()
            .map(|t| t.to_string())
            .unwrap_or_default(),
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
        enabled: true,
        opacity: handle.computed_opacity(),
        handle,
    })
}
