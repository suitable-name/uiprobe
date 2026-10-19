//! What the run did, by identity strings only (no live handles).
//!
//! The harness feeds the recorder from three places: [`record_activation`] (a click, an
//! accessible default action or a smoke press reached the element), [`record_touch`] (a scenario
//! fetched the element to act on it: set a value, type, drag) and [`record_callback`] (a scenario
//! invoked a global callback directly and asserted its effect). The end-of-run gate
//! (`coverage::finish`) compares these sets with the static inventory.
//!
//! The state lives in a process-global [`Mutex`], so it accumulates across `run_to_report()`
//! calls in one process; that is what the coverage self-test relies on.

use crate::ElementHandle;
use std::{
    collections::BTreeSet,
    sync::{Mutex, MutexGuard, PoisonError},
};

/// What the run did, by identity strings only (no live handles).
pub(crate) struct Recorder {
    /// `Component::id` of elements a click / smoke activation reached.
    pub(crate) activated_ids: BTreeSet<String>,
    /// Accessible labels of those elements.
    pub(crate) activated_labels: BTreeSet<String>,
    /// `(element type name, accessible label)` of activated elements: how an instantiation site
    /// without an id is recognised.
    pub(crate) activated_pairs: BTreeSet<(String, String)>,
    /// Same for elements a scenario fetched to act on (`find*`).
    pub(crate) touched_ids: BTreeSet<String>,
    /// `(type, label)` of touched elements.
    pub(crate) touched_pairs: BTreeSet<(String, String)>,
    /// Labels of touched elements.
    pub(crate) touched_labels: BTreeSet<String>,
    /// `Model.name` of callbacks that were invoked.
    pub(crate) callbacks: BTreeSet<String>,
    /// Element type names (label or not) of activated elements: how the one site of a component
    /// type whose label is computed (`for` rows, state-dependent text) is recognised.
    pub(crate) activated_types: BTreeSet<String>,
    /// Same for elements a scenario fetched to act on.
    pub(crate) touched_types: BTreeSet<String>,
    /// The BASE type names (inherited-from components) of activated elements, kept apart from
    /// `activated_types` because every press reports builtin bases (`Rectangle`, `TouchArea` ...)
    /// that are shared by hundreds of sites; only an app component name may be credited from here.
    pub(crate) activated_bases: BTreeSet<String>,
    /// Same for elements a scenario fetched to act on.
    pub(crate) touched_bases: BTreeSet<String>,
}

impl Recorder {
    pub(crate) const fn new() -> Self {
        Self {
            activated_types: BTreeSet::new(),
            touched_types: BTreeSet::new(),
            activated_bases: BTreeSet::new(),
            touched_bases: BTreeSet::new(),
            activated_ids: BTreeSet::new(),
            activated_labels: BTreeSet::new(),
            activated_pairs: BTreeSet::new(),
            touched_pairs: BTreeSet::new(),
            touched_ids: BTreeSet::new(),
            touched_labels: BTreeSet::new(),
            callbacks: BTreeSet::new(),
        }
    }
}

static RECORDER: Mutex<Recorder> = Mutex::new(Recorder::new());

/// Locks the process-global recorder (a poisoned lock is still usable: the sets stay valid).
pub(crate) fn recorder() -> MutexGuard<'static, Recorder> {
    RECORDER.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Normalised id: Slint treats `_` and `-` alike.
pub(crate) fn normalise_id(id: &str) -> String {
    id.replace('_', "-")
}

/// Records that the harness activated `handle` (click, accessible default action, smoke).
pub fn record_activation(handle: &ElementHandle) {
    record(handle, false);
}

/// Records that a scenario fetched `handle` to act on it (set a value, type, drag ...).
pub fn record_touch(handle: &ElementHandle) {
    record(handle, true);
}

fn record(handle: &ElementHandle, touched: bool) {
    let id = handle
        .id()
        .map(|i| normalise_id(&i))
        .filter(|i| !i.is_empty());
    let label = handle
        .accessible_label()
        .map(|l| l.to_string())
        .filter(|l| !l.trim().is_empty());
    let type_name = handle
        .type_name()
        .map(|t| t.to_string())
        .filter(|t| !t.is_empty());
    // The component names the element inherits from: an instance of a component whose root is
    // the interactive element reports the component among its bases (for example, in
    // Indicatrix, `VerdictBadge`), which is how the one site of such a component is recognised.
    let bases: Vec<String> = handle
        .bases()
        .map(|bases| bases.map(|base| base.to_string()).collect())
        .unwrap_or_default();
    let mut rec = recorder();
    if let Some(name) = &type_name {
        if touched {
            rec.touched_types.insert(name.clone());
        } else {
            rec.activated_types.insert(name.clone());
        }
    }
    for name in &bases {
        if touched {
            rec.touched_bases.insert(name.clone());
        } else {
            rec.activated_bases.insert(name.clone());
        }
    }
    if let Some(label) = &label {
        // The type name AND every base name pair with the label: which of the names the site
        // spells (for example, in Indicatrix, `MeetsCombo` inherits `ComboBox`, `PillButton`
        // inherits `Rectangle`) depends on how the component was inlined. The label must equal
        // the site's literal, so a base shared by many sites credits only a site that spells the
        // same label.
        for name in type_name.iter().chain(bases.iter()) {
            let pair = (name.clone(), label.clone());
            if touched {
                rec.touched_pairs.insert(pair);
            } else {
                rec.activated_pairs.insert(pair);
            }
        }
    }
    if let Some(id) = id {
        if touched {
            rec.touched_ids.insert(id);
        } else {
            rec.activated_ids.insert(id);
        }
    }
    if let Some(label) = label {
        if touched {
            rec.touched_labels.insert(label);
        } else {
            rec.activated_labels.insert(label);
        }
    }
}

/// Records the ids of the elements that sit exactly where `pressed` sits, below `root`.
///
/// A wrapper component puts its id at the site on its own root, while the element the harness
/// presses is the standard widget inside it: the two share one rectangle (0.6 px tolerance on
/// position and size), so the ids found there are the wrapper's (for example, in Indicatrix,
/// `TipButton`, `TipField`, `RoughPill`). `touched`: the element was fetched to act on (a field
/// set, a slider stepped) rather than pressed. Nothing is recorded for an element without a size
/// or a root that is no longer valid.
///
/// The smoke pass calls it for every activation; a scenario that presses or fetches a wrapper by
/// hand calls it with the view's root.
pub fn record_wrappers(root: &ElementHandle, pressed: &ElementHandle, touched: bool) {
    let position = pressed.absolute_position();
    let size = pressed.size();
    if size.width <= 0.0 || size.height <= 0.0 || !root.is_valid() {
        return;
    }
    let mut found: Vec<String> = Vec::new();
    let _: Option<()> = root.visit_descendants(|element| {
        let other = element.size();
        if (other.width - size.width).abs() < 0.6 && (other.height - size.height).abs() < 0.6 {
            let at = element.absolute_position();
            if (at.x - position.x).abs() < 0.6
                && (at.y - position.y).abs() < 0.6
                && let Some(id) = element.id()
            {
                found.push(normalise_id(&id));
            }
        }
        std::ops::ControlFlow::Continue(())
    });
    let mut rec = recorder();
    for id in found.into_iter().filter(|id| !id.is_empty()) {
        if touched {
            rec.touched_ids.insert(id);
        } else {
            rec.activated_ids.insert(id);
        }
    }
}

/// Records that the element `Component::id` was pressed by a scenario that found the button by
/// its label and saw it act.
///
/// `Component::id` is the id of an instantiation site (in Indicatrix, for example,
/// `PreformTab::tip-button-preform-pending`). Needed for a wrapper site whose pressed element
/// reports the inner widget's id, not the site's: the id rule of the gate cannot see the press by
/// itself.
pub fn record_site_id(id: &str) {
    recorder().activated_ids.insert(normalise_id(id));
}

/// The recorder as JSON (every set as an array), for a parallel worker to send to the
/// coordinator.
pub(crate) fn snapshot() -> serde_json::Value {
    let rec = recorder();
    serde_json::json!({
        "activated_ids": rec.activated_ids,
        "activated_labels": rec.activated_labels,
        "activated_pairs": rec.activated_pairs,
        "touched_ids": rec.touched_ids,
        "touched_pairs": rec.touched_pairs,
        "touched_labels": rec.touched_labels,
        "callbacks": rec.callbacks,
        "activated_types": rec.activated_types,
        "touched_types": rec.touched_types,
        "activated_bases": rec.activated_bases,
        "touched_bases": rec.touched_bases,
    })
}

/// Unions a [`snapshot`] into the process-global recorder. Missing or malformed sets are
/// skipped.
pub(crate) fn merge_snapshot(value: &serde_json::Value) {
    fn set<T: serde::de::DeserializeOwned + Ord>(value: &serde_json::Value, key: &str) -> Vec<T> {
        value
            .get(key)
            .and_then(|v| serde_json::from_value::<Vec<T>>(v.clone()).ok())
            .unwrap_or_default()
    }
    let mut rec = recorder();
    rec.activated_ids
        .extend(set::<String>(value, "activated_ids"));
    rec.activated_labels
        .extend(set::<String>(value, "activated_labels"));
    rec.activated_pairs
        .extend(set::<(String, String)>(value, "activated_pairs"));
    rec.touched_ids.extend(set::<String>(value, "touched_ids"));
    rec.touched_pairs
        .extend(set::<(String, String)>(value, "touched_pairs"));
    rec.touched_labels
        .extend(set::<String>(value, "touched_labels"));
    rec.callbacks.extend(set::<String>(value, "callbacks"));
    rec.activated_types
        .extend(set::<String>(value, "activated_types"));
    rec.touched_types
        .extend(set::<String>(value, "touched_types"));
    rec.activated_bases
        .extend(set::<String>(value, "activated_bases"));
    rec.touched_bases
        .extend(set::<String>(value, "touched_bases"));
}

/// Records that the callback `Model.name` (snake case) was invoked and its handler ran.
pub fn record_callback(key: &str) {
    recorder().callbacks.insert(key.replace('-', "_"));
}

/// The callbacks recorded so far.
#[must_use]
pub fn recorded_callbacks() -> BTreeSet<String> {
    recorder().callbacks.clone()
}
