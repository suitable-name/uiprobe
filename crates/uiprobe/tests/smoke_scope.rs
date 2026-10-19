//! Self-test: `SmokeHost::admits` and `Smoke::scope_may_close`.
//!
//! Both are driven through a custom `SmokeHost` (`smoke::run`), because `Ctx::smoke` builds its
//! own host.
//!
//! 1. `admits`: the host rejects the control labelled "Skip". The pass enumerates and presses
//!    only "Keep" (`enumerated == 1`, `activated == 1`), "Skip" is never pressed, with
//!    `skip_blocked` on (the default, scene path) and off (the plain path).
//! 2. `scope_may_close`: the view is `ScopeFixture/panel`, a subtree whose "Close" button
//!    destroys the panel. With `scope_may_close(true)`: no `window-invalid`, `closed == 1`, and
//!    the pass goes on to press "Stay" after the restore. Without it: exactly one
//!    `window-invalid` on the "Close" press, `closed == 0`.
//!
//! Needs Slint debug info at compile time (`SLINT_EMIT_DEBUG_INFO=1`, set by this workspace's
//! `.cargo/config.toml`). The fixtures use `FixedTestFont` for machine-independent geometry.

use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    process::ExitCode,
    rc::Rc,
};

use uiprobe::{
    Attached, Ctx, Harness,
    report::Report,
    smoke::{Smoke, SmokeHost, SmokeStats, run},
    view::{Control, WindowView},
};

slint::slint! {
    import { Button } from "std-widgets.slint";

    export component AdmitFixture inherits Window {
        width: 300px;
        height: 100px;
        default-font-family: "FixedTestFont";
        callback keep-pressed();
        callback skip-pressed();
        Button {
            text: "Keep";
            x: 10px;
            y: 10px;
            width: 100px;
            height: 30px;
            clicked => {
                root.keep-pressed();
            }
        }
        Button {
            text: "Skip";
            x: 150px;
            y: 10px;
            width: 100px;
            height: 30px;
            clicked => {
                root.skip-pressed();
            }
        }
    }

    export component ScopeFixture inherits Window {
        width: 300px;
        height: 100px;
        default-font-family: "FixedTestFont";
        callback stay-pressed();
        in-out property <bool> closed: false;
        if !root.closed: panel := Rectangle {
            x: 0px;
            y: 0px;
            width: 300px;
            height: 100px;
            Button {
                text: "Close";
                x: 10px;
                y: 10px;
                width: 100px;
                height: 30px;
                clicked => {
                    root.closed = true;
                }
            }
            Button {
                text: "Stay";
                x: 150px;
                y: 10px;
                width: 100px;
                height: 30px;
                clicked => {
                    root.stay-pressed();
                }
            }
        }
    }
}

type Mismatches = Rc<RefCell<Vec<String>>>;

/// Counters the fixtures' callbacks bump.
#[derive(Clone, Default)]
struct Counters {
    keep: Rc<Cell<usize>>,
    skip: Rc<Cell<usize>>,
    stay: Rc<Cell<usize>>,
}

/// A [`SmokeHost`] over one fixture window; `scope` selects `ScopeFixture` (a subtree view)
/// instead of `AdmitFixture` (the whole window, "Skip" rejected).
struct Host<'a, 'b> {
    ctx: &'a mut Ctx<'b>,
    app: Option<Attached>,
    scope: bool,
    counters: Counters,
}

impl Host<'_, '_> {
    fn launch(&mut self) -> Result<Attached, String> {
        let counters = self.counters.clone();
        if self.scope {
            let fixture = ScopeFixture::new().map_err(|e| e.to_string())?;
            fixture.on_stay_pressed(move || counters.stay.set(counters.stay.get() + 1));
            self.ctx
                .launch(|| Ok::<_, slint::PlatformError>(fixture))
                .map_err(|f| f.detail)
        } else {
            let fixture = AdmitFixture::new().map_err(|e| e.to_string())?;
            let skip = counters.clone();
            fixture.on_keep_pressed(move || counters.keep.set(counters.keep.get() + 1));
            fixture.on_skip_pressed(move || skip.skip.set(skip.skip.get() + 1));
            self.ctx
                .launch(|| Ok::<_, slint::PlatformError>(fixture))
                .map_err(|f| f.detail)
        }
    }

    /// Shuts the last window down (the pass leaves it up).
    fn close(&mut self) {
        if let Some(app) = self.app.take() {
            self.ctx.shutdown(app);
        }
    }
}

impl SmokeHost for Host<'_, '_> {
    fn restore(&mut self) -> Result<Vec<WindowView>, String> {
        self.close();
        let app = self.launch()?;
        self.app = Some(app);
        Ok(self.views())
    }

    fn views(&self) -> Vec<WindowView> {
        let Some(app) = &self.app else {
            return Vec::new();
        };
        let view = app.view();
        if !self.scope {
            return vec![view];
        }
        match view.find_id("ScopeFixture::panel") {
            Ok(root) => vec![WindowView::subtree("ScopeFixture/panel", root, view.size)],
            Err(_) => Vec::new(),
        }
    }

    fn notice(&mut self) -> Option<(String, String)> {
        None
    }

    fn clear_notice(&mut self) {}

    fn main_alive(&self) -> bool {
        // A traversal lets Slint drop the elements of a closed `if`; `is_valid` needs it.
        if let Some(app) = &self.app {
            let _ = app.view().controls();
        }
        self.app.as_ref().is_some_and(Attached::is_visible)
    }

    fn report(&mut self) -> &mut Report {
        self.ctx.report()
    }

    fn admits(&self, control: &Control) -> bool {
        self.scope || control.label != "Skip"
    }
}

fn expect_eq(bad: &Mismatches, what: &str, got: usize, want: usize) {
    if got != want {
        bad.borrow_mut()
            .push(format!("{what}: expected {want}, got {got}"));
    }
}

fn count_in_state(report: &Report, check: &str, state: &str) -> usize {
    let wanted = format!("smoke: {state}");
    report
        .findings()
        .iter()
        .filter(|f| f.check == check && f.state == wanted)
        .count()
}

/// Runs one pass over a fresh host and shuts its last window down.
fn pass(ctx: &mut Ctx<'_>, scope: bool, smoke: &Smoke, counters: &Counters) -> SmokeStats {
    let mut host = Host {
        ctx: &mut *ctx,
        app: None,
        scope,
        counters: counters.clone(),
    };
    let stats = run(&mut host, smoke);
    host.close();
    stats
}

fn check_admits(bad: &Mismatches, ctx: &mut Ctx<'_>, label: &str, smoke: &Smoke) {
    let counters = Counters::default();
    let stats = pass(ctx, false, smoke, &counters);
    expect_eq(bad, &format!("{label}: enumerated"), stats.enumerated, 1);
    expect_eq(bad, &format!("{label}: activated"), stats.activated, 1);
    expect_eq(
        bad,
        &format!("{label}: Keep presses"),
        counters.keep.get(),
        1,
    );
    expect_eq(
        bad,
        &format!("{label}: Skip presses"),
        counters.skip.get(),
        0,
    );
}

fn finish(bad: &Mismatches, report: &Report) -> ExitCode {
    let mut ok = true;
    for m in bad.borrow().iter() {
        eprintln!("uiprobe self-test mismatch: {m}");
        ok = false;
    }
    let expected: BTreeMap<&str, usize> = BTreeMap::from([("window-invalid", 1)]);
    let mut got: BTreeMap<&str, usize> = BTreeMap::new();
    for f in report.findings() {
        *got.entry(f.check.as_str()).or_insert(0) += 1;
    }
    if got != expected {
        eprintln!("uiprobe self-test mismatch: per-check counts {got:?}, expected {expected:?}");
        for f in report.findings() {
            eprintln!(
                "    {} | {} | {} | {} | {}",
                f.window, f.element, f.check, f.detail, f.state
            );
        }
        ok = false;
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn main() -> ExitCode {
    let bad: Mismatches = Rc::default();
    let (admit_bad, scope_bad) = (Rc::clone(&bad), Rc::clone(&bad));
    let report = Harness::new()
        .scenario("admits", move |ctx| {
            check_admits(&admit_bad, ctx, "default", &Smoke::new("admit"));
            check_admits(
                &admit_bad,
                ctx,
                "skip_blocked(false)",
                &Smoke::new("admit plain").skip_blocked(false),
            );
            Ok(())
        })
        .scenario("scope may close", move |ctx| {
            let counters = Counters::default();
            let stats = pass(
                ctx,
                true,
                &Smoke::new("closable").scope_may_close(true),
                &counters,
            );
            expect_eq(&scope_bad, "closable: stats.closed", stats.closed, 1);
            expect_eq(&scope_bad, "closable: stats.activated", stats.activated, 2);
            expect_eq(&scope_bad, "closable: Stay presses", counters.stay.get(), 1);
            let invalid = count_in_state(ctx.report(), "window-invalid", "closable");
            expect_eq(&scope_bad, "closable: window-invalid", invalid, 0);

            let counters = Counters::default();
            let stats = pass(ctx, true, &Smoke::new("strict"), &counters);
            expect_eq(&scope_bad, "strict: stats.closed", stats.closed, 0);
            let invalid = count_in_state(ctx.report(), "window-invalid", "strict");
            expect_eq(&scope_bad, "strict: window-invalid", invalid, 1);
            Ok(())
        })
        .run_to_report();
    finish(&bad, &report)
}
