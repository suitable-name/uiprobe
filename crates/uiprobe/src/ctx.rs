//! What a scenario receives ([`Ctx`]): the report, the profile and the common operations
//! (launch, sweep, click-everything, health check), plus the windows it launched
//! ([`Attached`]).
//!
//! The harness relaunches WINDOWS, not the process: `thread_local!` state in the app outlives a
//! window. For example, in Indicatrix a solver service kept a `thread_local!` bound to the first
//! main window, and every relaunch after it silently talked to a dead window until the service
//! checked its owner through a `Weak`. The harness cannot detect that class of bug; guard such
//! state with a `Weak` owner check.

use crate::{
    ElementHandle, ElementRoot,
    backend::{pump, require_debug_info, wait_until},
    checks::SweepOptions,
    profile::ProfileDir,
    report::{Finding, Report},
    smoke::{BeforeRestore, Smoke, SmokeHost, SmokeStats},
    view::{WindowView, logical_size, px_f32},
};
use slint::ComponentHandle;
use std::{
    any::Any,
    fmt::Display,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

/// The app's error notice: `(kind, message)` when one is showing.
pub type NoticeFn = Box<dyn FnMut() -> Option<(String, String)>>;

/// Set by the first [`Ctx::launch`] of the process, which also checks for Slint debug info.
static DEBUG_INFO_CHECKED: AtomicBool = AtomicBool::new(false);

/// Finds the window of the stored component (see `window_of`).
type WindowOf = Box<dyn Fn(&dyn Any) -> &slint::Window>;

/// A window the harness launched or was handed. Owns the component (dropping it drops the
/// window) plus anything else the app needs alive with it.
pub struct Attached {
    name: String,
    root: ElementHandle,
    component: Box<dyn Any>,
    window: WindowOf,
    keep: Vec<Box<dyn Any>>,
}

/// The window of the `C` stored in `component` (the type `Attached::new` was called with).
fn window_of<C: ComponentHandle + 'static>(component: &dyn Any) -> &slint::Window {
    component
        .downcast_ref::<C>()
        .expect("Attached stores the component type it was created with")
        .window()
}

impl Attached {
    /// Wraps `component` under `name`.
    pub fn new<C: ComponentHandle + 'static>(name: impl Into<String>, component: C) -> Self {
        let root = component.root_element();
        Self {
            name: name.into(),
            root,
            component: Box::new(component),
            window: Box::new(window_of::<C>),
            keep: Vec::new(),
        }
    }

    /// Keeps `extra` alive as long as the window (an app handle that owns threads, models ...).
    #[must_use]
    pub fn keep(mut self, extra: impl Any) -> Self {
        self.keep.push(Box::new(extra));
        self
    }

    /// The name used in findings.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The root element.
    #[must_use]
    pub const fn root(&self) -> &ElementHandle {
        &self.root
    }

    /// The Slint window.
    #[must_use]
    pub fn window(&self) -> &slint::Window {
        (self.window)(self.component.as_ref())
    }

    /// The component, when it is a `C`.
    #[must_use]
    pub fn component<C: 'static>(&self) -> Option<&C> {
        self.component.downcast_ref()
    }

    /// The window as a queryable view with its current size.
    #[must_use]
    pub fn view(&self) -> WindowView {
        WindowView {
            name: self.name.clone(),
            root: self.root.clone(),
            size: logical_size(self.window()),
        }
    }

    /// Whether the window is shown.
    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.window().is_visible()
    }
}

/// Hides `app`, pumps and drops it.
fn close(app: Attached) {
    let _ = app.window().hide();
    pump(20);
    drop(app);
}

/// The last path segment of `C`'s type name: the window name used in findings.
fn short_type_name<C>() -> String {
    let full = std::any::type_name::<C>();
    full.rsplit("::").next().unwrap_or(full).to_owned()
}

/// The app seams of `uiprobe_hooks`, as seen from a scenario.
#[derive(Debug, Clone, Copy, Default)]
pub struct Hooks;

impl Hooks {
    /// Queues "the user picked `path`" for the next file dialog.
    pub fn queue_pick(self, path: impl Into<PathBuf>) {
        crate::hooks::picker::queue_path(path);
    }

    /// Queues "the user cancelled" for the next file dialog.
    pub fn queue_cancel(self) {
        crate::hooks::picker::queue_cancel();
    }

    /// Texts the app copied to the clipboard since the last call.
    #[must_use]
    pub fn take_clipboard_writes(self) -> Vec<String> {
        crate::hooks::clipboard::take()
    }

    /// URLs and paths the app opened since the last call.
    #[must_use]
    pub fn take_opened(self) -> Vec<String> {
        crate::hooks::opener::take()
    }

    /// "unexpected file dialog" failures since the last call.
    #[must_use]
    pub fn take_picker_failures(self) -> Vec<String> {
        crate::hooks::picker::take_failures()
    }
}

/// Everything a scenario may touch.
pub struct Ctx<'a> {
    pub(crate) report: &'a mut Report,
    pub(crate) profile: &'a ProfileDir,
    pub(crate) sizes: &'a [(u32, u32)],
    pub(crate) sweep_options: &'a SweepOptions,
    pub(crate) seed: u64,
    pub(crate) notice: Option<(String, NoticeFn)>,
    pub(crate) clear_notice: Option<Box<dyn FnMut()>>,
}

impl Ctx<'_> {
    /// Builds a window with `build`, shows it at the first configured size and pumps. The
    /// first launch of the run also checks the build carries Slint debug info; when not, the
    /// launch fails and the run aborts after this scenario with the explanation, the profile
    /// deleted.
    ///
    /// # Errors
    ///
    /// A `launch` finding with the error text of `build` or `show`.
    pub fn launch<C, E>(
        &mut self,
        build: impl FnOnce() -> Result<C, E>,
    ) -> Result<Attached, Finding>
    where
        C: ComponentHandle + 'static,
        E: Display,
    {
        let component =
            build().map_err(|e| Finding::simple("launch", format!("cannot start: {e}")))?;
        component
            .show()
            .map_err(|e| Finding::simple("launch", format!("cannot start: {e}")))?;
        let app = Attached::new(short_type_name::<C>(), component);
        if let Some(&(width, height)) = self.sizes.first() {
            app.window()
                .set_size(slint::LogicalSize::new(px_f32(width), px_f32(height)));
        }
        pump(50);
        if !DEBUG_INFO_CHECKED.swap(true, Ordering::Relaxed)
            && let Err(message) = require_debug_info(app.root())
        {
            crate::profile::request_abort(message.clone());
            return Err(Finding::simple("launch", message));
        }
        Ok(app)
    }

    /// Wraps a window the app created itself (a secondary window, a dialog window).
    pub fn attach<C: ComponentHandle + 'static>(&mut self, name: &str, component: C) -> Attached {
        Attached::new(name, component)
    }

    /// Hides `app`, pumps and drops it.
    pub fn shutdown(&mut self, app: Attached) {
        close(app);
    }

    /// The view of `app`.
    #[must_use]
    pub fn view(&self, app: &Attached) -> WindowView {
        app.view()
    }

    /// Resizes `app` (logical pixels) and lets the layout settle.
    pub fn resize(&mut self, app: &Attached, width: u32, height: u32) {
        app.window()
            .set_size(slint::LogicalSize::new(px_f32(width), px_f32(height)));
        pump(30);
    }

    /// Sweeps `app` in its current state and size; findings are tagged with `state`.
    pub fn sweep(&mut self, app: &Attached, state: &str) {
        self.sweep_view(&app.view(), state);
    }

    /// Sweeps one view (a window or a subtree) and records the findings, tagged with `state`.
    pub fn sweep_view(&mut self, view: &WindowView, state: &str) {
        let findings = crate::checks::sweep(view, self.sweep_options);
        let size = format!(
            "{} {:.0}x{:.0} {state}",
            view.name, view.size.0, view.size.1
        );
        self.report.add_all(findings, &size);
    }

    /// Sweeps `app` at every configured size, then restores the first size. Without configured
    /// sizes it sweeps the current size only.
    pub fn sweep_at_sizes(&mut self, app: &Attached, state: &str) {
        let sizes = self.sizes;
        let Some(&(first_width, first_height)) = sizes.first() else {
            self.sweep(app, state);
            return;
        };
        for &(width, height) in sizes {
            self.resize(app, width, height);
            pump(50);
            self.sweep(app, state);
        }
        self.resize(app, first_width, first_height);
    }

    /// Runs the click-everything pass over the state `restore` re-establishes. `restore` runs
    /// before the first activation and after every activation that changed the state; it
    /// returns the windows to enumerate (the first is the main window). The old windows are
    /// handed to the smoke's `before_restore` routine, then shut down, after every restore and
    /// at the end of the pass.
    ///
    /// The app's notice is hidden through [`Ctx::clear_notice_with`] before and after every
    /// activation; without it a notice is cleared only by the next restore.
    ///
    /// Blocked controls are recognised with the run's sweep thresholds
    /// ([`crate::Harness::sweep_options`]).
    pub fn smoke(
        &mut self,
        mut smoke: Smoke,
        restore: impl FnMut(&mut Ctx<'_>) -> Result<Vec<Attached>, Finding>,
    ) -> SmokeStats {
        let before_restore = smoke.take_before_restore();
        let mut host = CtxHost {
            ctx: self,
            restore,
            windows: Vec::new(),
            before_restore,
        };
        let stats = crate::smoke::run(&mut host, &smoke);
        let old = std::mem::take(&mut host.windows);
        if let Some(before) = host.before_restore.as_mut() {
            before(&old);
        }
        for window in old {
            close(window);
        }
        println!(
            "uiprobe:   smoke {}: {} control(s), {} activated, {} denied, {} blocked, \
             {} gone, {} restore(s), {} ms",
            smoke.state,
            stats.enumerated,
            stats.activated,
            stats.denied,
            stats.blocked,
            stats.gone,
            stats.restores,
            stats.duration_ms
        );
        stats
    }

    /// Sends a key chord to `app`'s window; a bad chord is a `bad-chord` finding.
    pub fn press(&mut self, app: &Attached, chord: &str) {
        if let Err(finding) = crate::view::press(app.window(), chord) {
            self.report.add(finding);
        }
    }

    /// Records a finding `check`/`detail` when `condition` is false; returns `condition`.
    pub fn expect(&mut self, condition: bool, check: &str, detail: impl Into<String>) -> bool {
        if !condition {
            self.report.add_simple("-", check, detail);
        }
        condition
    }

    /// Pumps until `condition` holds or `timeout` (real time) passes; a timeout is a `timeout`
    /// finding naming `label`.
    pub fn wait_for(
        &mut self,
        label: &str,
        timeout: Duration,
        condition: impl FnMut() -> bool,
    ) -> bool {
        let held = wait_until(timeout, condition);
        self.expect(
            held,
            "timeout",
            format!("{label}: not reached within {} ms", timeout.as_millis()),
        )
    }

    /// Records panics and unexpected file dialogs since the last check, and an error or warning
    /// notice showing (see [`Ctx::on_error_notice`]). `step` names the step.
    pub fn check_health(&mut self, step: &str) {
        for text in crate::take_panics() {
            self.report
                .add_simple("-", "panic", format!("{step}: {text}"));
        }
        for text in crate::hooks::picker::take_failures() {
            self.report
                .add_simple("-", "unexpected-file-dialog", format!("{step}: {text}"));
        }
        if let Some((window, notice)) = self.notice.as_mut()
            && let Some((kind, message)) = notice()
            && (kind == "error" || kind == "warning")
        {
            self.report
                .add_simple(window, "error-toast", format!("{step}: {kind}: {message}"));
        }
    }

    /// Tells the harness how to read the app's error notice (toast, banner) in `window`:
    /// `notice` returns `(kind, message)` while one is showing. Kinds `error` and `warning`
    /// become `error-toast` findings in [`Ctx::check_health`] and in the smoke pass. A `Ctx` is
    /// built per scenario, so register the reader in every scenario that needs it (a small
    /// helper the scenarios share is the usual shape).
    pub fn on_error_notice(
        &mut self,
        window: &str,
        notice: impl FnMut() -> Option<(String, String)> + 'static,
    ) {
        self.notice = Some((window.to_owned(), Box::new(notice)));
    }

    /// Tells the harness how to hide the app's notice, so a sticky toast raised by one control
    /// is not reported again for every control activated after it. Without it the smoke pass
    /// clears nothing and a notice is gone only when the next restore starts a fresh window.
    /// Per scenario, like [`Ctx::on_error_notice`].
    pub fn clear_notice_with(&mut self, clear: impl FnMut() + 'static) {
        self.clear_notice = Some(Box::new(clear));
    }

    /// A path inside the profile's scratch directory for a file the scenario writes.
    #[must_use]
    pub fn scratch_file(&self, name: &str) -> PathBuf {
        self.profile.scratch_dir().join(name)
    }

    /// The live throw-away profile.
    #[must_use]
    pub const fn profile(&self) -> &ProfileDir {
        self.profile
    }

    /// The report and the profile at once: for an app that keeps its own scenario context (its
    /// own window handle, settings seed) and needs both borrows together.
    pub const fn parts(&mut self) -> (&mut Report, &ProfileDir) {
        (self.report, self.profile)
    }

    /// The report (add findings, notes, sections).
    pub const fn report(&mut self) -> &mut Report {
        self.report
    }

    /// The app seams (picker queue, clipboard and opener captures).
    #[must_use]
    pub const fn hooks(&self) -> Hooks {
        Hooks
    }

    /// The run's seed (`UIPROBE_SEED`, default 0) for scenarios that randomise.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// The configured window sizes.
    #[must_use]
    pub const fn sizes(&self) -> &[(u32, u32)] {
        self.sizes
    }

    /// The sweep thresholds of the run.
    #[must_use]
    pub const fn sweep_options(&self) -> &SweepOptions {
        self.sweep_options
    }
}

/// [`SmokeHost`] over a [`Ctx`]: restore = shut the current windows down and call the
/// scenario's `restore` closure.
struct CtxHost<'c, 'a, R> {
    ctx: &'c mut Ctx<'a>,
    restore: R,
    windows: Vec<Attached>,
    before_restore: Option<BeforeRestore>,
}

impl<R> SmokeHost for CtxHost<'_, '_, R>
where
    R: FnMut(&mut Ctx<'_>) -> Result<Vec<Attached>, Finding>,
{
    fn restore(&mut self) -> Result<Vec<WindowView>, String> {
        let old = std::mem::take(&mut self.windows);
        if let Some(before) = self.before_restore.as_mut() {
            before(&old);
        }
        for window in old {
            close(window);
        }
        match (self.restore)(&mut *self.ctx) {
            Ok(windows) => {
                self.windows = windows;
                Ok(self.views())
            }
            Err(finding) => Err(finding.detail),
        }
    }

    fn views(&self) -> Vec<WindowView> {
        self.windows.iter().map(Attached::view).collect()
    }

    fn notice(&mut self) -> Option<(String, String)> {
        self.ctx.notice.as_mut().and_then(|(_, notice)| notice())
    }

    fn clear_notice(&mut self) {
        if let Some(clear) = self.ctx.clear_notice.as_mut() {
            clear();
        }
    }

    fn main_alive(&self) -> bool {
        self.windows.first().is_some_and(Attached::is_visible)
    }

    fn report(&mut self) -> &mut Report {
        &mut *self.ctx.report
    }

    fn sweep_options(&self) -> SweepOptions {
        self.ctx.sweep_options.clone()
    }
}
