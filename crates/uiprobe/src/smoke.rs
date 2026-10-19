//! The click-everything engine.
//!
//! For one UI state it enumerates every enabled, visible, clickable control, activates each one
//! (the accessible default action, as a screen reader would), pumps, and records:
//!
//! * `panic`: a panic on any thread (the recording panic hook plus `catch_unwind`);
//! * `error-toast`: the app's error notice (see [`SmokeHost::notice`]) shows an error or
//!   warning that the state did not say to expect;
//! * `window-invalid` / `main-closed`: a window the state started with is gone afterwards.
//!
//! Destructive or slow controls are listed in [`DEFAULT_DENY`] (generic words) and in the
//! rules an app adds with [`Smoke::deny_rules`] (data, not code) and skipped. Controls covered
//! by a later modal backdrop cannot be clicked by a user and are left out too
//! ([`Smoke::skip_blocked`]; the backdrop threshold is the run's [`SweepOptions::modal_cover`],
//! see [`SmokeHost::sweep_options`]). Between activations the state is re-established through
//! [`SmokeHost::restore`], but only when the activation visibly changed something (the set of
//! controls differs, a panic, a closed window); [`Smoke::always_restore`] forces it every time.
//!
//! A click that opens a file dialog is answered "cancelled" by the picker hook and recorded as a
//! note (opening a dialog is what an Import button is for); the dialog-focused scenarios queue
//! real answers and check the fallout themselves.

use crate::{
    Attached,
    backend::pump,
    checks::SweepOptions,
    coverage::{record_activation, record_wrappers},
    hooks::picker::take_failures,
    report::{Finding, Report},
    scene::Scene,
    take_panics,
    view::{Control, WindowView, is_clickable_role},
};
use std::{
    borrow::Cow,
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
    time::Instant,
};

/// Which text of a control a [`DenyRule`] looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyField {
    /// The accessible label.
    Label,
    /// The element id.
    Id,
}

/// How a [`DenyRule`] compares (always case-insensitive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyMatch {
    /// The whole text equals the pattern.
    Exact,
    /// The text starts with the pattern.
    StartsWith,
    /// The text contains the pattern.
    Contains,
}

/// One entry of the "never click this" list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DenyRule {
    /// Text the rule looks at.
    pub field: DenyField,
    /// Comparison.
    pub mode: DenyMatch,
    /// Lower- or mixed-case pattern.
    pub pattern: Cow<'static, str>,
    /// Why the control is skipped (shown in the report notes).
    pub reason: Cow<'static, str>,
}

impl DenyRule {
    /// A rule on the accessible label.
    #[must_use]
    pub const fn label(mode: DenyMatch, pattern: &'static str, reason: &'static str) -> Self {
        Self {
            field: DenyField::Label,
            mode,
            pattern: Cow::Borrowed(pattern),
            reason: Cow::Borrowed(reason),
        }
    }

    /// A rule on the element id.
    #[must_use]
    pub const fn id(mode: DenyMatch, pattern: &'static str, reason: &'static str) -> Self {
        Self {
            field: DenyField::Id,
            mode,
            pattern: Cow::Borrowed(pattern),
            reason: Cow::Borrowed(reason),
        }
    }

    /// A label rule from owned text (a deny word read from a config file).
    #[must_use]
    pub fn label_owned(
        mode: DenyMatch,
        pattern: impl Into<Cow<'static, str>>,
        reason: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            field: DenyField::Label,
            mode,
            pattern: pattern.into(),
            reason: reason.into(),
        }
    }

    /// An id rule from owned text.
    #[must_use]
    pub fn id_owned(
        mode: DenyMatch,
        pattern: impl Into<Cow<'static, str>>,
        reason: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            field: DenyField::Id,
            mode,
            pattern: pattern.into(),
            reason: reason.into(),
        }
    }
}

/// Controls the engine never activates by default: generic destructive or app-ending words only.
///
/// Apps pass their own (exports, renders, network) with [`Smoke::deny_rules`]; Indicatrix
/// adds export/render/claim/enroll/sync/download/update-check rules this way.
pub const DEFAULT_DENY: &[DenyRule] = &[
    DenyRule::label(DenyMatch::Contains, "delete", "destroys data"),
    DenyRule::label(DenyMatch::Contains, "remove", "destroys data"),
    DenyRule::label(DenyMatch::Contains, "quit", "ends the app"),
    DenyRule::label(DenyMatch::Exact, "exit", "ends the app"),
    DenyRule::label(DenyMatch::Contains, "overwrite", "overwrites a file"),
    DenyRule::label(DenyMatch::Contains, "discard", "discards work"),
    DenyRule::label(DenyMatch::Contains, "purge", "destroys data"),
    DenyRule::label(DenyMatch::Contains, "factory reset", "mass change"),
    DenyRule::label(DenyMatch::Contains, "reset all", "mass change"),
    DenyRule::label(DenyMatch::Contains, "clear all", "mass change"),
    // Secondary windows: leaving them mid-pass would make the engine see a vanished window.
    DenyRule::label(
        DenyMatch::Contains,
        "close window",
        "closes the window under test",
    ),
];

/// Runs before every restore with the windows of the state being torn down (the app's own
/// close routine: discard a dirty document, stop a render thread ...).
pub type BeforeRestore = Box<dyn FnMut(&[Attached])>;

/// Settings of one click-everything pass (a builder).
pub struct Smoke {
    /// Name of the UI state (shown in findings).
    pub state: String,
    /// The deny list ([`DEFAULT_DENY`] unless replaced).
    pub deny: Vec<DenyRule>,
    /// Further denied controls for this state only ([`Smoke::deny`] and [`Smoke::deny_rules`]
    /// add here; checked after `deny`).
    pub extra_deny: Vec<DenyRule>,
    /// Mock milliseconds pumped after each activation.
    pub settle_ms: u64,
    /// Stop after this many activations (a cap for slow states).
    pub max_controls: usize,
    /// Re-establish the state after every activation, not only after visible changes.
    pub always_restore: bool,
    /// Error notices (message substrings) this state is expected to raise.
    pub allowed_notices: Vec<String>,
    /// Leave controls covered by a later modal backdrop out of the pass (they cannot be
    /// clicked by a user).
    pub skip_blocked: bool,
    /// The views are scopes of a window, named `Window/<scope>` (the names
    /// [`WindowView::subtree`] gives): a press that closes the scope (Got It, Cancel, Close ...)
    /// is the control doing its job. A view whose root became invalid and whose name contains
    /// `/` is counted in [`SmokeStats::closed`] instead of a `window-invalid` finding, the state
    /// is restored and the pass continues. A window without `/` going away stays a finding.
    pub scope_may_close: bool,
    pub(crate) before_restore: Option<BeforeRestore>,
}

impl Smoke {
    /// Defaults: [`DEFAULT_DENY`], 60 ms settle, no cap, lazy restore, blocked controls skipped.
    #[must_use]
    pub fn new(state: impl Into<String>) -> Self {
        Self {
            state: state.into(),
            deny: DEFAULT_DENY.to_vec(),
            extra_deny: Vec::new(),
            settle_ms: 60,
            max_controls: usize::MAX,
            always_restore: false,
            allowed_notices: Vec::new(),
            skip_blocked: true,
            scope_may_close: false,
            before_restore: None,
        }
    }

    /// Adds label rules (`Contains`, case-insensitive) for each word, reason "denied by the
    /// scenario".
    #[must_use]
    pub fn deny(mut self, words: impl IntoIterator<Item = impl Into<Cow<'static, str>>>) -> Self {
        self.extra_deny.extend(words.into_iter().map(|word| {
            DenyRule::label_owned(DenyMatch::Contains, word, "denied by the scenario")
        }));
        self
    }

    /// Adds explicit rules for this state.
    #[must_use]
    pub fn deny_rules(mut self, rules: &[DenyRule]) -> Self {
        self.extra_deny.extend_from_slice(rules);
        self
    }

    /// Sets the mock milliseconds pumped after each activation.
    #[must_use]
    pub const fn settle_ms(mut self, ms: u64) -> Self {
        self.settle_ms = ms;
        self
    }

    /// Caps the number of activations.
    #[must_use]
    pub const fn max_controls(mut self, max: usize) -> Self {
        self.max_controls = max;
        self
    }

    /// Restores after every activation when `on`.
    #[must_use]
    pub const fn always_restore(mut self, on: bool) -> Self {
        self.always_restore = on;
        self
    }

    /// Error notices containing one of these texts are expected, not findings.
    #[must_use]
    pub fn allowed_notices(mut self, texts: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.allowed_notices
            .extend(texts.into_iter().map(Into::into));
        self
    }

    /// Whether controls under a modal backdrop are skipped (default `true`).
    #[must_use]
    pub const fn skip_blocked(mut self, on: bool) -> Self {
        self.skip_blocked = on;
        self
    }

    /// Whether a view named `Window/<scope>` closing is expected (default `false`).
    #[must_use]
    pub const fn scope_may_close(mut self, on: bool) -> Self {
        self.scope_may_close = on;
        self
    }

    /// The app's close routine, run on the old windows before every restore.
    #[must_use]
    pub fn before_restore(mut self, f: impl FnMut(&[Attached]) + 'static) -> Self {
        self.before_restore = Some(Box::new(f));
        self
    }

    /// Takes the before-restore routine out (the `Ctx` adapter owns it during the pass).
    pub(crate) fn take_before_restore(&mut self) -> Option<BeforeRestore> {
        self.before_restore.take()
    }
}

/// What the engine needs from the code that owns the UI state.
pub trait SmokeHost {
    /// Re-establishes the state under test from scratch (relaunch the app and navigate there)
    /// and returns the windows to enumerate.
    ///
    /// # Errors
    ///
    /// A message when the state cannot be established.
    fn restore(&mut self) -> Result<Vec<WindowView>, String>;
    /// The windows as they are now (no restore), re-queried.
    fn views(&self) -> Vec<WindowView>;
    /// The app's error notice (toast, banner), `(kind, message)`, when one is showing. Kinds
    /// `error` and `warning` are findings unless allowed.
    fn notice(&mut self) -> Option<(String, String)>;
    /// Hides the notice so it does not leak into the next step.
    fn clear_notice(&mut self);
    /// Whether the first (main) window is still shown.
    fn main_alive(&self) -> bool;
    /// Where findings and notes go.
    fn report(&mut self) -> &mut Report;
    /// The sweep thresholds the pass recognises a modal backdrop with
    /// ([`SweepOptions::modal_cover`], used when [`Smoke::skip_blocked`] is on). The default is
    /// `SweepOptions::default()`; the `Ctx` adapter returns the run's.
    fn sweep_options(&self) -> SweepOptions {
        SweepOptions::default()
    }
    /// Whether the pass may enumerate, locate and press `control`; the default admits
    /// everything. Applied in `enumerate`, `locate` and the structure signature, also when
    /// [`Smoke::skip_blocked`] is on.
    ///
    /// For example, in Indicatrix: a popup's content tree has no element a scope lookup can root
    /// a view at, so the pass runs over the whole main window and admits only the controls the
    /// popup added.
    fn admits(&self, _control: &Control) -> bool {
        true
    }
}

/// Counters of one pass.
#[derive(Debug, Default, Clone)]
pub struct SmokeStats {
    /// Clickable, enabled controls found in the starting state (blocked ones not included
    /// when [`Smoke::skip_blocked`] is on).
    pub enumerated: usize,
    /// Controls actually activated.
    pub activated: usize,
    /// Controls skipped by the deny list.
    pub denied: usize,
    /// Controls skipped because a modal backdrop covers them.
    pub blocked: usize,
    /// Controls that vanished before they could be activated.
    pub gone: usize,
    /// Presses that closed the scope under test (expected, see [`Smoke::scope_may_close`]).
    pub closed: usize,
    /// Times the state was re-established.
    pub restores: usize,
    /// Wall time of the pass in milliseconds.
    pub duration_ms: u64,
}

/// A control remembered by key so it can be found again after a rebuild.
struct Target {
    window: usize,
    window_name: String,
    key: String,
    occurrence: usize,
    describe: String,
    label: String,
    id: String,
}

fn denied_by<'r>(rules: &'r [DenyRule], target: &Target) -> Option<&'r str> {
    rules.iter().find_map(|rule| {
        let text = match rule.field {
            DenyField::Label => &target.label,
            DenyField::Id => &target.id,
        }
        .to_lowercase();
        let pattern = rule.pattern.to_lowercase();
        let hit = match rule.mode {
            DenyMatch::Exact => text == pattern,
            DenyMatch::StartsWith => text.starts_with(&pattern),
            DenyMatch::Contains => text.contains(&pattern),
        };
        hit.then_some(rule.reason.as_ref())
    })
}

fn is_candidate(control: &Control) -> bool {
    is_clickable_role(control.role)
        && control.enabled
        && control.width > 0.0
        && control.height > 0.0
}

/// The candidate controls of one view: the ones the pass may activate and, when `skip_blocked`
/// is on, the ones a later modal backdrop covers. `enumerate`, `locate` and `signature` all go
/// through this, so their occurrence counters agree.
fn candidates(
    view: &WindowView,
    skip_blocked: bool,
    options: &SweepOptions,
    host: &dyn SmokeHost,
) -> (Vec<Control>, Vec<Control>) {
    if !skip_blocked {
        let live = view
            .controls()
            .into_iter()
            .filter(|c| is_candidate(c) && host.admits(c))
            .collect();
        return (live, Vec::new());
    }
    let scene = Scene::build(view, options);
    let mut live = Vec::new();
    let mut blocked = Vec::new();
    for placed in scene.placed {
        if !placed.interactive || !is_candidate(&placed.control) || !host.admits(&placed.control) {
            continue;
        }
        if placed.blocked {
            blocked.push(placed.control);
        } else {
            live.push(placed.control);
        }
    }
    (live, blocked)
}

/// Targets of every candidate control (with occurrence counters for duplicate keys) plus the
/// descriptions of the blocked ones.
fn enumerate(
    views: &[WindowView],
    skip_blocked: bool,
    options: &SweepOptions,
    host: &dyn SmokeHost,
) -> (Vec<Target>, Vec<String>) {
    let mut targets = Vec::new();
    let mut blocked_controls = Vec::new();
    for (window, view) in views.iter().enumerate() {
        let mut seen: HashMap<String, usize> = HashMap::new();
        let (live, blocked) = candidates(view, skip_blocked, options, host);
        blocked_controls.extend(blocked.iter().map(Control::describe));
        for control in live {
            let key = control.key();
            let counter = seen.entry(key.clone()).or_insert(0);
            let occurrence = *counter;
            *counter += 1;
            targets.push(Target {
                window,
                window_name: view.name.clone(),
                key,
                occurrence,
                describe: control.describe(),
                label: control.label.clone(),
                id: control.id.clone(),
            });
        }
    }
    (targets, blocked_controls)
}

/// The control matching `target` in `views`, if it still exists.
fn locate(
    views: &[WindowView],
    target: &Target,
    skip_blocked: bool,
    options: &SweepOptions,
    host: &dyn SmokeHost,
) -> Option<Control> {
    let view = views.get(target.window)?;
    candidates(view, skip_blocked, options, host)
        .0
        .into_iter()
        .filter(|c| c.key() == target.key)
        .nth(target.occurrence)
}

/// Sorted keys of every candidate control: a cheap fingerprint of the window's structure.
fn signature(
    views: &[WindowView],
    skip_blocked: bool,
    options: &SweepOptions,
    host: &dyn SmokeHost,
) -> Vec<String> {
    let mut keys: Vec<String> = enumerate(views, skip_blocked, options, host)
        .0
        .into_iter()
        .map(|t| format!("{}|{}|{}", t.window, t.key, t.occurrence))
        .collect();
    keys.sort();
    keys
}

fn first_line(text: &str) -> String {
    let line: String = text.lines().take(2).collect::<Vec<_>>().join(" / ");
    line.chars().take(300).collect()
}

/// Runs the pass and records its findings and notes through `host.report()`.
pub fn run(host: &mut dyn SmokeHost, smoke: &Smoke) -> SmokeStats {
    let started = Instant::now();
    let mut stats = SmokeStats::default();
    run_pass(host, smoke, &mut stats);
    stats.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    stats
}

fn run_pass(host: &mut dyn SmokeHost, smoke: &Smoke, stats: &mut SmokeStats) {
    let skip = smoke.skip_blocked;
    let options = host.sweep_options();
    let mut views = match host.restore() {
        Ok(views) => views,
        Err(e) => {
            host.report()
                .add_simple("-", "launch", format!("{}: cannot start: {e}", smoke.state));
            return;
        }
    };
    if views.is_empty() {
        host.report().add_simple(
            "-",
            "launch",
            format!("{}: restore returned no windows", smoke.state),
        );
        return;
    }
    let (targets, blocked) = enumerate(&views, skip, &options, &*host);
    stats.enumerated = targets.len();
    stats.blocked = blocked.len();
    for describe in &blocked {
        host.report().note(format!(
            "smoke skipped {describe} (covered by a modal backdrop)"
        ));
    }
    let mut baseline = signature(&views, skip, &options, &*host);
    // Anything left over from earlier steps must not be blamed on the first click.
    let _ = take_panics();
    let _ = take_failures();

    for target in targets.iter().take(smoke.max_controls) {
        if let Some(reason) =
            denied_by(&smoke.deny, target).or_else(|| denied_by(&smoke.extra_deny, target))
        {
            stats.denied += 1;
            host.report()
                .note(format!("smoke skipped {} ({reason})", target.describe));
            continue;
        }
        let control = match locate(&views, target, skip, &options, &*host) {
            Some(control) if control.handle.is_valid() => control,
            _ => {
                // Stale handle or a changed state: re-establish and look again.
                match restore_into(host, &mut views, &mut baseline, stats, skip, &options) {
                    Ok(()) => {
                        if let Some(control) = locate(&views, target, skip, &options, &*host) {
                            control
                        } else {
                            stats.gone += 1;
                            host.report()
                                .note(format!("smoke: {} not found again", target.describe));
                            continue;
                        }
                    }
                    Err(()) => return,
                }
            }
        };
        let needs_restore = activate(host, smoke, &views, target, &control, stats);
        let changed = needs_restore
            || smoke.always_restore
            || signature(&host.views(), skip, &options, &*host) != baseline;
        if changed && restore_into(host, &mut views, &mut baseline, stats, skip, &options).is_err()
        {
            return;
        }
    }
}

/// Re-establishes the state; on failure records a finding and returns `Err`.
fn restore_into(
    host: &mut dyn SmokeHost,
    views: &mut Vec<WindowView>,
    baseline: &mut Vec<String>,
    stats: &mut SmokeStats,
    skip_blocked: bool,
    options: &SweepOptions,
) -> Result<(), ()> {
    match host.restore() {
        Ok(fresh) => {
            if fresh.is_empty() {
                host.report()
                    .add_simple("-", "launch", "restore returned no windows".to_owned());
                return Err(());
            }
            stats.restores += 1;
            *baseline = signature(&fresh, skip_blocked, options, &*host);
            *views = fresh;
            Ok(())
        }
        Err(e) => {
            host.report()
                .add_simple("-", "launch", format!("cannot re-establish the state: {e}"));
            Err(())
        }
    }
}

/// Activates one control and records what happened. Returns whether the state must be rebuilt
/// regardless of structure (a panic or a closed window).
fn activate(
    host: &mut dyn SmokeHost,
    smoke: &Smoke,
    views: &[WindowView],
    target: &Target,
    control: &Control,
    stats: &mut SmokeStats,
) -> bool {
    let _ = take_panics();
    let _ = take_failures();
    host.clear_notice();
    let settle = smoke.settle_ms;
    record_activation(&control.handle);
    if let Some(view) = views.get(target.window) {
        // The wrapper component around a standard widget carries the site's id (see
        // `coverage::record_wrappers`).
        record_wrappers(&view.root, &control.handle, false);
    }
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        control.handle.invoke_accessible_default_action();
        pump(settle);
    }));
    stats.activated += 1;
    let mut broken = false;
    let panics = take_panics();
    if outcome.is_err() || !panics.is_empty() {
        broken = true;
        let first = panics.first().map_or("panic payload lost", String::as_str);
        add(
            host.report(),
            smoke,
            target,
            "panic",
            format!("{} ({} panic(s))", first_line(first), panics.len().max(1)),
        );
    }
    if let Some((kind, message)) = host.notice() {
        let expected = smoke
            .allowed_notices
            .iter()
            .any(|a| message.contains(a.as_str()));
        if (kind == "error" || kind == "warning") && !expected {
            add(
                host.report(),
                smoke,
                target,
                "error-toast",
                format!("{kind} toast: {message}"),
            );
        }
    }
    host.clear_notice();
    if !take_failures().is_empty() {
        host.report().note(format!(
            "smoke: {} opened a file dialog (cancelled)",
            target.describe
        ));
    }
    if !host.main_alive() {
        broken = true;
        add(
            host.report(),
            smoke,
            target,
            "main-closed",
            "the main window is no longer shown".to_owned(),
        );
    }
    for view in views {
        if !view.root.is_valid() {
            broken = true;
            if smoke.scope_may_close && view.name.contains('/') {
                stats.closed += 1;
                continue;
            }
            add(
                host.report(),
                smoke,
                target,
                "window-invalid",
                format!("window {} is gone", view.name),
            );
        }
    }
    broken
}

fn add(report: &mut Report, smoke: &Smoke, target: &Target, check: &str, detail: String) {
    let mut finding = Finding::new(
        target.window_name.clone(),
        target.describe.clone(),
        check,
        detail,
    );
    finding.state = format!("smoke: {}", smoke.state);
    report.add(finding);
}
