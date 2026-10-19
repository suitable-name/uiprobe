//! Finding elements: by accessible label (or, when no label matches, by the accessible
//! placeholder text a text field carries instead), element id and role.
//!
//! A query that finds nothing (or too much) is a [`Finding`] that lists the nearest labels, so a
//! renamed control is obvious; it never panics.
//!
//! Everything works on a [`WindowView`], a window's root element plus its logical size, so the
//! same helpers serve the main window and every secondary window.
//!
//! Queries return findings rather than panic so that one renamed or missing control does not
//! hide every later step of a journey: the rest of the scenario still runs, and the report names
//! the nearest labels instead of a backtrace.

use crate::{AccessibleRole, ElementHandle, ElementRoot, backend::pump, report::Finding};
use slint::{ComponentHandle, platform::WindowEvent};

pub mod geometry;

pub use geometry::{Rect, nearest_within, wheel_steps};

/// A pixel count as `f32`, the type Slint's sizes use.
#[expect(
    clippy::cast_precision_loss,
    reason = "window dimensions are far below 2^24, where f32 is still exact"
)]
pub(crate) const fn px_f32(value: u32) -> f32 {
    value as f32
}

/// The size of `window` in logical pixels (its physical size over the scale factor).
pub(crate) fn logical_size(window: &slint::Window) -> (f32, f32) {
    let physical = window.size();
    let scale = window.scale_factor().max(f32::EPSILON);
    (
        px_f32(physical.width) / scale,
        px_f32(physical.height) / scale,
    )
}

/// One window (or one subtree, such as a dialog) to query and sweep.
#[derive(Clone)]
pub struct WindowView {
    /// Name used in findings (`MainWindow`, `HelpWindow`, ...).
    pub name: String,
    /// The root element; queries search its descendants.
    pub root: ElementHandle,
    /// Logical width and height of the window the root lives in.
    pub size: (f32, f32),
}

impl WindowView {
    /// The view of a whole window component, with its current logical size.
    pub fn of<C: ComponentHandle>(name: &str, component: &C) -> Self {
        Self {
            name: name.to_owned(),
            root: component.root_element(),
            size: logical_size(component.window()),
        }
    }

    /// A view of a subtree (for example one dialog found by id) inside a window of the given
    /// logical size.
    #[must_use]
    pub fn subtree(name: &str, root: ElementHandle, window_size: (f32, f32)) -> Self {
        Self {
            name: name.to_owned(),
            root,
            size: window_size,
        }
    }

    /// Every visible element below the root, depth first in drawing order (invisible subtrees
    /// are skipped by Slint itself).
    #[must_use]
    pub fn elements(&self) -> Vec<ElementHandle> {
        self.root.query_descendants().find_all()
    }

    /// Every visible interactive control, with its geometry and label.
    #[must_use]
    pub fn controls(&self) -> Vec<Control> {
        self.elements()
            .into_iter()
            .filter_map(Control::from_handle)
            .collect()
    }

    /// All elements whose accessible label equals `label`; when none has it, all elements
    /// whose accessible placeholder text equals `label` (a `LineEdit` carries its placeholder,
    /// not a label, so a text field is found by its placeholder).
    #[must_use]
    pub fn by_label(&self, label: &str) -> Vec<ElementHandle> {
        let wanted = label.to_owned();
        let labelled = self
            .root
            .query_descendants()
            .match_predicate(move |e| e.accessible_label().is_some_and(|l| l == wanted))
            .find_all();
        if !labelled.is_empty() {
            return labelled;
        }
        let wanted = label.to_owned();
        self.root
            .query_descendants()
            .match_predicate(move |e| e.accessible_placeholder_text().is_some_and(|p| p == wanted))
            .find_all()
    }

    /// The single element labelled `label`.
    ///
    /// # Errors
    ///
    /// A `not-found` finding listing similar labels, or an `ambiguous-label` finding when more
    /// than one element carries the label.
    pub fn find(&self, label: &str) -> Result<ElementHandle, Finding> {
        let mut found = self.by_label(label);
        match found.len() {
            1 => Ok(touched(found.remove(0))),
            0 => Err(self.not_found(&format!("label {label:?}"), label)),
            n => Err(Finding::new(
                self.name.clone(),
                "",
                "ambiguous-label",
                format!(
                    "{}: {n} elements are labelled {label:?}; use find_role/find_nth",
                    self.name
                ),
            )),
        }
    }

    /// The first element labelled `label`, if any.
    #[must_use]
    pub fn try_find(&self, label: &str) -> Option<ElementHandle> {
        self.by_label(label).into_iter().next()
    }

    /// The `index`-th (document order) element labelled `label`.
    ///
    /// # Errors
    ///
    /// A `not-found` finding naming how many exist.
    pub fn find_nth(&self, label: &str, index: usize) -> Result<ElementHandle, Finding> {
        let mut found = self.by_label(label);
        if index < found.len() {
            Ok(touched(found.swap_remove(index)))
        } else {
            Err(Finding::new(
                self.name.clone(),
                "",
                "not-found",
                format!(
                    "{}: wanted element #{index} labelled {label:?}, only {} exist",
                    self.name,
                    found.len()
                ),
            ))
        }
    }

    /// The element with this role and label or placeholder.
    ///
    /// # Errors
    ///
    /// A `not-found` finding listing similar labels.
    pub fn find_role(&self, role: AccessibleRole, label: &str) -> Result<ElementHandle, Finding> {
        let wanted = label.to_owned();
        let labelled = self
            .root
            .query_descendants()
            .match_accessible_role(role)
            .match_predicate(move |e| e.accessible_label().is_some_and(|l| l == wanted))
            .find_first();
        let wanted = label.to_owned();
        let found = labelled.or_else(|| {
            self.root
                .query_descendants()
                .match_accessible_role(role)
                .match_predicate(move |e| {
                    e.accessible_placeholder_text().is_some_and(|p| p == wanted)
                })
                .find_first()
        });
        found
            .map(touched)
            .ok_or_else(|| self.not_found(&format!("{role:?} labelled {label:?}"), label))
    }

    /// The element with this id (`Component::local-id`; `_` and `-` are interchangeable).
    ///
    /// # Errors
    ///
    /// A `not-found` finding listing similar ids.
    pub fn find_id(&self, id: &str) -> Result<ElementHandle, Finding> {
        let found = self.root.query_descendants().match_id(id).find_first();
        found.map(touched).ok_or_else(|| {
            let ids: Vec<String> = self
                .elements()
                .iter()
                .filter_map(|e| e.id().map(|i| i.to_string()))
                .filter(|i| similar(i, id))
                .take(15)
                .collect();
            Finding::new(
                self.name.clone(),
                "",
                "not-found",
                format!(
                    "{}: no element with id {id:?}; similar ids: {ids:?}",
                    self.name
                ),
            )
        })
    }

    /// Whether some element carries this label.
    #[must_use]
    pub fn has(&self, label: &str) -> bool {
        self.try_find(label).is_some()
    }

    /// Every distinct non-empty accessible label, in document order.
    #[must_use]
    pub fn labels(&self) -> Vec<String> {
        let mut labels: Vec<String> = Vec::new();
        for element in self.elements() {
            if let Some(label) = element.accessible_label() {
                let label = label.to_string();
                if !label.trim().is_empty() && !labels.contains(&label) {
                    labels.push(label);
                }
            }
        }
        labels
    }

    /// Every distinct non-empty accessible placeholder text, in document order (text fields
    /// carry a placeholder instead of a label).
    #[must_use]
    pub fn placeholders(&self) -> Vec<String> {
        let mut placeholders: Vec<String> = Vec::new();
        for element in self.elements() {
            if let Some(text) = element.accessible_placeholder_text() {
                let text = text.to_string();
                if !text.trim().is_empty() && !placeholders.contains(&text) {
                    placeholders.push(text);
                }
            }
        }
        placeholders
    }

    /// Activates the control labelled `label` (accessible default action), records the
    /// activation for the coverage gate and returns it.
    ///
    /// # Errors
    ///
    /// The finding of [`WindowView::find`].
    pub fn click(&self, label: &str) -> Result<ElementHandle, Finding> {
        let element = self.find(label)?;
        crate::coverage::record_activation(&element);
        element.invoke_accessible_default_action();
        Ok(element)
    }

    /// The `not-found` finding for a failed query: lists the nearest labels and placeholders.
    fn not_found(&self, what: &str, label: &str) -> Finding {
        let near: Vec<String> = self
            .labels()
            .into_iter()
            .filter(|l| similar(l, label))
            .take(15)
            .collect();
        let near_placeholders: Vec<String> = self
            .placeholders()
            .into_iter()
            .filter(|p| similar(p, label))
            .take(15)
            .collect();
        let hint = match (near.is_empty(), near_placeholders.is_empty()) {
            (true, true) => {
                "no similar labels or placeholders (is the right dialog open, and the element \
                 visible?)"
                    .to_owned()
            }
            (false, true) => format!("similar labels: {near:?}"),
            (true, false) => format!("similar placeholders: {near_placeholders:?}"),
            (false, false) => {
                format!("similar labels: {near:?}, similar placeholders: {near_placeholders:?}")
            }
        };
        Finding::new(
            self.name.clone(),
            "",
            "not-found",
            format!("{}: no element with {what}; {hint}", self.name),
        )
    }
}

/// Passes `element` through after telling the coverage recorder a scenario fetched it to act on.
fn touched(element: ElementHandle) -> ElementHandle {
    crate::coverage::record_touch(&element);
    element
}

/// Case-insensitive "one contains the other, or they share a word" test for near matches.
fn similar(candidate: &str, wanted: &str) -> bool {
    let candidate = candidate.to_lowercase();
    let wanted = wanted.to_lowercase();
    if candidate.contains(&wanted) || wanted.contains(&candidate) {
        return true;
    }
    wanted
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .any(|w| candidate.contains(w))
}

/// Whether a role is something the user clicks, toggles or types into.
#[must_use]
pub const fn is_interactive_role(role: AccessibleRole) -> bool {
    matches!(
        role,
        AccessibleRole::Button
            | AccessibleRole::Checkbox
            | AccessibleRole::Switch
            | AccessibleRole::Combobox
            | AccessibleRole::Slider
            | AccessibleRole::Spinbox
            | AccessibleRole::Tab
            | AccessibleRole::TextInput
            | AccessibleRole::RadioButton
            | AccessibleRole::ListItem
    )
}

/// Whether activating the control (accessible default action) is a meaningful "click".
#[must_use]
pub const fn is_clickable_role(role: AccessibleRole) -> bool {
    matches!(
        role,
        AccessibleRole::Button
            | AccessibleRole::Checkbox
            | AccessibleRole::Switch
            | AccessibleRole::Combobox
            | AccessibleRole::Tab
            | AccessibleRole::RadioButton
            | AccessibleRole::ListItem
    )
}

/// A visible interactive element with everything the sweep and the smoke engine need.
#[derive(Clone)]
pub struct Control {
    /// The live element.
    pub handle: ElementHandle,
    /// Its accessible role.
    pub role: AccessibleRole,
    /// Its accessible label (may be empty: that is a finding).
    pub label: String,
    /// Its qualified element id, empty when it has none.
    pub id: String,
    /// Its element type name (`Button`, `TouchArea` ...), empty when unknown.
    pub type_name: String,
    /// Window-relative left edge.
    pub x: f32,
    /// Window-relative top edge.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
    /// `accessible-enabled` (true when the property is absent).
    pub enabled: bool,
    /// Combined opacity of the element and its ancestors.
    pub opacity: f32,
}

impl Control {
    /// The control behind `handle`, or `None` when it is not an interactive element (a
    /// `ListItem` counts only when it declares itself selectable).
    #[must_use]
    pub fn from_handle(handle: ElementHandle) -> Option<Self> {
        let role = handle.accessible_role()?;
        if !is_interactive_role(role) {
            return None;
        }
        if role == AccessibleRole::ListItem && handle.accessible_item_selectable() != Some(true) {
            return None;
        }
        let position = handle.absolute_position();
        let size = handle.size();
        let label = handle
            .accessible_label()
            .map(|l| l.to_string())
            .filter(|l| !l.trim().is_empty())
            .or_else(|| {
                handle
                    .accessible_placeholder_text()
                    .map(|p| p.to_string())
                    .filter(|p| !p.trim().is_empty())
            })
            .unwrap_or_default();
        Some(Self {
            role,
            label,
            id: handle.id().map(|i| i.to_string()).unwrap_or_default(),
            type_name: handle
                .type_name()
                .map(|t| t.to_string())
                .unwrap_or_default(),
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
            enabled: handle.accessible_enabled().unwrap_or(true),
            opacity: handle.computed_opacity(),
            handle,
        })
    }

    /// `Role "label" (Component::id)` for findings and messages.
    #[must_use]
    pub fn describe(&self) -> String {
        use std::fmt::Write as _;
        let mut text = format!("{:?}", self.role);
        if !self.label.is_empty() {
            let _ = write!(text, " {:?}", self.label);
        }
        if !self.id.is_empty() {
            let _ = write!(text, " ({})", self.id);
        } else if !self.type_name.is_empty() {
            let _ = write!(text, " <{}>", self.type_name);
        }
        text
    }

    /// Stable key that identifies this control across a window rebuild: role, id and label
    /// (duplicates are told apart by the caller's occurrence counter).
    #[must_use]
    pub fn key(&self) -> String {
        format!("{:?}|{}|{}", self.role, self.id, self.label)
    }

    /// Right edge.
    #[must_use]
    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    /// Bottom edge.
    #[must_use]
    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }
}

/// Sends a key chord to `window` ("Ctrl+S", "F5", "Escape", "?", "Ctrl+Shift+P"): Slint
/// `KeyPressed`/`KeyReleased` events, modifiers pressed first and released last, then pumps
/// 50 ms.
///
/// # Errors
///
/// A `bad-chord` finding when the chord does not parse; nothing is sent then.
pub fn press(window: &slint::Window, chord: &str) -> Result<(), Finding> {
    dispatch_chord(window, chord)?;
    pump(50);
    Ok(())
}

/// [`press`] without the 50 ms pump: the chord's events are dispatched and the event queue is
/// drained, nothing more. The recorder uses it so that video time stays the only clock.
///
/// # Errors
///
/// A `bad-chord` finding when the chord does not parse; nothing is sent then.
#[cfg(feature = "recording")]
pub(crate) fn press_events(window: &slint::Window, chord: &str) -> Result<(), Finding> {
    dispatch_chord(window, chord)?;
    crate::backend::drain_events();
    Ok(())
}

/// Dispatches the key events of `chord`: modifiers pressed first and released last.
fn dispatch_chord(window: &slint::Window, chord: &str) -> Result<(), Finding> {
    let parsed = crate::keys::parse_chord(chord).map_err(|e| Finding::simple("bad-chord", e))?;
    let modifiers = parsed.modifier_keys();
    for key in &modifiers {
        window.dispatch_event(WindowEvent::KeyPressed {
            text: (*key).into(),
        });
    }
    window.dispatch_event(WindowEvent::KeyPressed {
        text: parsed.text.clone(),
    });
    window.dispatch_event(WindowEvent::KeyReleased { text: parsed.text });
    for key in modifiers.iter().rev() {
        window.dispatch_event(WindowEvent::KeyReleased {
            text: (*key).into(),
        });
    }
    Ok(())
}
