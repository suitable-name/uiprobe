//! Self-test: what the click-everything pass and `Ctx::check_health` record.
//!
//! Seven scenarios, each with its own smoke state name so findings do not merge:
//!
//! 1. `toast cascade`: a sticky error notice raised by "Toast" is blamed on every control
//!    activated after it when the app registers no clear routine: 2 `error-toast`.
//! 2. `toast cleared`: with `Ctx::clear_notice_with` the same fixture gives exactly 1.
//! 3. `health`: `check_health` reports a showing notice and an unexpected file dialog, and a
//!    queued answer is no finding.
//! 4. `actions`: a panicking callback is a `panic`, a callback that hides the window is a
//!    `main-closed`, a denied word is never clicked, and the `before_restore` routine runs
//!    after every restore and once more when the pass ends.
//! 5. `capped`: `max_controls` caps the activations.
//! 6. `always`: `always_restore` restores after every activation.
//! 7. `no windows`: a restore that returns no windows is a `launch` finding, not a silent pass.
//!
//! Needs Slint debug info at compile time (`SLINT_EMIT_DEBUG_INFO=1`, set by this workspace's
//! `.cargo/config.toml`). The fixtures use `FixedTestFont` for machine-independent geometry.

use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    process::ExitCode,
    rc::Rc,
};

use slint::{ComponentHandle, Weak};
use uiprobe::{
    Attached, Ctx, Harness,
    backend::pump,
    report::{Finding, Report},
    smoke::Smoke,
};

slint::slint! {
    import { Button } from "std-widgets.slint";

    export component ToastFixture inherits Window {
        width: 300px;
        height: 100px;
        default-font-family: "FixedTestFont";
        in-out property <string> notice-kind;
        in-out property <string> notice-text;
        // Document order matters: "Toast" is activated first.
        Button {
            text: "Toast";
            x: 10px;
            y: 10px;
            width: 100px;
            height: 30px;
            clicked => {
                root.notice-kind = "error";
                root.notice-text = "sticky";
            }
        }
        Button {
            text: "Stay";
            x: 150px;
            y: 10px;
            width: 100px;
            height: 30px;
        }
    }

    export component ActionFixture inherits Window {
        width: 300px;
        height: 100px;
        default-font-family: "FixedTestFont";
        callback boom();
        callback close-me();
        callback open-file();
        callback wipe();
        Button {
            text: "Boom";
            x: 10px;
            y: 10px;
            width: 80px;
            height: 30px;
            clicked => {
                root.boom();
            }
        }
        Button {
            text: "Close";
            x: 110px;
            y: 10px;
            width: 80px;
            height: 30px;
            clicked => {
                root.close-me();
            }
        }
        Button {
            text: "Open";
            x: 210px;
            y: 10px;
            width: 80px;
            height: 30px;
            clicked => {
                root.open-file();
            }
        }
        Button {
            text: "Delete all";
            x: 10px;
            y: 55px;
            width: 80px;
            height: 30px;
            clicked => {
                root.wipe();
            }
        }
        Button {
            text: "Stay";
            x: 110px;
            y: 55px;
            width: 80px;
            height: 30px;
        }
    }
}

type Mismatches = Rc<RefCell<Vec<String>>>;
type ToastSlot = Rc<RefCell<Option<Weak<ToastFixture>>>>;

fn mismatch(bad: &Mismatches, text: String) {
    bad.borrow_mut().push(text);
}

fn expect_eq(bad: &Mismatches, what: &str, got: usize, want: usize) {
    if got != want {
        mismatch(bad, format!("{what}: expected {want}, got {got}"));
    }
}

/// Elements of the findings of `check` made in the smoke state `state`.
fn elements_of(report: &Report, check: &str, state: &str) -> Vec<String> {
    let wanted = format!("smoke: {state}");
    report
        .findings()
        .iter()
        .filter(|f| f.check == check && f.state == wanted)
        .map(|f| f.element.clone())
        .collect()
}

/// Whether exactly one element of `elements` names the quoted `label`.
fn names_once(elements: &[String], label: &str) -> bool {
    let quoted = format!("\"{label}\"");
    elements.iter().filter(|e| e.contains(&quoted)).count() == 1
}

/// Registers the notice reader of `ToastFixture` (kind and text properties).
fn register_notice(ctx: &mut Ctx<'_>, slot: &ToastSlot) {
    let slot = Rc::clone(slot);
    ctx.on_error_notice("ToastFixture", move || {
        let guard = slot.borrow();
        let fixture = guard.as_ref()?.upgrade()?;
        let kind = fixture.get_notice_kind().to_string();
        if kind.is_empty() {
            return None;
        }
        Some((kind, fixture.get_notice_text().to_string()))
    });
}

/// Registers the routine that hides the notice again.
fn register_clear(ctx: &mut Ctx<'_>, slot: &ToastSlot) {
    let slot = Rc::clone(slot);
    ctx.clear_notice_with(move || {
        if let Some(fixture) = slot.borrow().as_ref().and_then(Weak::upgrade) {
            fixture.set_notice_kind("".into());
            fixture.set_notice_text("".into());
        }
    });
}

/// Launches a fresh `ToastFixture` and remembers a weak handle for the notice routines.
fn launch_toast(ctx: &mut Ctx<'_>, slot: &ToastSlot) -> Result<Attached, Finding> {
    let fixture =
        ToastFixture::new().map_err(|e| Finding::simple("launch", format!("cannot start: {e}")))?;
    *slot.borrow_mut() = Some(fixture.as_weak());
    ctx.launch(|| Ok::<_, slint::PlatformError>(fixture))
}

/// Launches a fresh `ActionFixture` with its callbacks wired. `hit` is set when "Delete all"
/// runs, which the default deny list must prevent.
fn launch_action(ctx: &mut Ctx<'_>, hit: &Rc<Cell<bool>>) -> Result<Attached, Finding> {
    let fixture = ActionFixture::new()
        .map_err(|e| Finding::simple("launch", format!("cannot start: {e}")))?;
    fixture.on_boom(|| panic!("boom pressed"));
    let weak = fixture.as_weak();
    fixture.on_close_me(move || {
        if let Some(fixture) = weak.upgrade() {
            let _ = fixture.hide();
        }
    });
    fixture.on_open_file(|| {
        let _ = uiprobe::hooks::picker::next("Open file");
    });
    let hit = Rc::clone(hit);
    fixture.on_wipe(move || hit.set(true));
    ctx.launch(|| Ok::<_, slint::PlatformError>(fixture))
}

fn count_of(report: &Report, check: &str) -> usize {
    report
        .findings()
        .iter()
        .filter(|f| f.check == check)
        .count()
}

fn finish(bad: &Mismatches, report: &Report) -> ExitCode {
    let mut ok = true;
    for m in bad.borrow().iter() {
        eprintln!("uiprobe self-test mismatch: {m}");
        ok = false;
    }
    let expected: BTreeMap<&str, usize> = [
        ("error-toast", 4),
        ("panic", 1),
        ("main-closed", 1),
        ("unexpected-file-dialog", 1),
        ("launch", 1),
    ]
    .into_iter()
    .collect();
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
    let report = scenarios(&bad).run_to_report();
    finish(&bad, &report)
}

fn scenarios(bad: &Mismatches) -> Harness {
    let harness = Harness::new();
    let harness = toast_scenarios(harness, bad);
    let harness = health_scenario(harness, bad);
    let harness = action_scenario(harness, bad);
    option_scenarios(harness, bad)
}

/// The error-toast cascade and its clearing.
fn toast_scenarios(harness: Harness, bad: &Mismatches) -> Harness {
    let (cascade_bad, cleared_bad) = (Rc::clone(bad), Rc::clone(bad));
    harness
        .scenario("toast cascade", move |ctx| {
            let slot = ToastSlot::default();
            register_notice(ctx, &slot);
            let _ = ctx.smoke(Smoke::new("cascade"), |ctx| {
                launch_toast(ctx, &slot).map(|app| vec![app])
            });
            let toasts = elements_of(ctx.report(), "error-toast", "cascade");
            if toasts.len() != 2 || !names_once(&toasts, "Toast") || !names_once(&toasts, "Stay") {
                mismatch(
                    &cascade_bad,
                    format!("cascade: expected Toast and Stay, got {toasts:?}"),
                );
            }
            Ok(())
        })
        .scenario("toast cleared", move |ctx| {
            let slot = ToastSlot::default();
            register_notice(ctx, &slot);
            register_clear(ctx, &slot);
            let _ = ctx.smoke(Smoke::new("cleared"), |ctx| {
                launch_toast(ctx, &slot).map(|app| vec![app])
            });
            let toasts = elements_of(ctx.report(), "error-toast", "cleared");
            if toasts.len() != 1 || !names_once(&toasts, "Toast") {
                mismatch(
                    &cleared_bad,
                    format!("cleared: expected only Toast, got {toasts:?}"),
                );
            }
            Ok(())
        })
}

/// The end-of-scenario health check: a sticky toast and an unexpected file dialog.
fn health_scenario(harness: Harness, bad: &Mismatches) -> Harness {
    let health_bad = Rc::clone(bad);
    harness.scenario("health", move |ctx| {
        let slot = ToastSlot::default();
        register_notice(ctx, &slot);
        let app = launch_toast(ctx, &slot)?;
        let view = ctx.view(&app);
        let _ = view.click("Toast")?;
        pump(20);
        ctx.check_health("after toast");
        let shown = ctx.report().findings().iter().any(|f| {
            f.check == "error-toast" && f.detail.starts_with("after toast: error: sticky")
        });
        if !shown {
            mismatch(
                &health_bad,
                "health: no error-toast `after toast: error: sticky`".to_owned(),
            );
        }
        drop(view);
        ctx.shutdown(app);

        let hit = Rc::new(Cell::new(false));
        let app = launch_action(ctx, &hit)?;
        let view = ctx.view(&app);
        let _ = view.click("Open")?;
        ctx.check_health("dialog");
        let dialogs = count_of(ctx.report(), "unexpected-file-dialog");
        let named = ctx
            .report()
            .findings()
            .iter()
            .any(|f| f.check == "unexpected-file-dialog" && f.detail.contains("Open file"));
        expect_eq(&health_bad, "health: dialog findings", dialogs, 1);
        if !named {
            mismatch(
                &health_bad,
                "health: the dialog finding does not name `Open file`".to_owned(),
            );
        }
        ctx.hooks().queue_cancel();
        let _ = view.click("Open")?;
        ctx.check_health("queued");
        let after = count_of(ctx.report(), "unexpected-file-dialog");
        expect_eq(
            &health_bad,
            "health: dialog findings after a queued answer",
            after,
            1,
        );
        drop(view);
        ctx.shutdown(app);
        Ok(())
    })
}

/// The deny list, a panic and a closed main window.
fn action_scenario(harness: Harness, bad: &Mismatches) -> Harness {
    let actions_bad = Rc::clone(bad);
    harness.scenario("actions", move |ctx| {
        let hit = Rc::new(Cell::new(false));
        let before = Rc::new(Cell::new(0_usize));
        let counter = Rc::clone(&before);
        let smoke = Smoke::new("actions").before_restore(move |_| counter.set(counter.get() + 1));
        let stats = ctx.smoke(smoke, |ctx| launch_action(ctx, &hit).map(|app| vec![app]));
        let panics = elements_of(ctx.report(), "panic", "actions");
        if panics.len() != 1 || !names_once(&panics, "Boom") {
            mismatch(
                &actions_bad,
                format!("actions: expected one panic on Boom, got {panics:?}"),
            );
        }
        let detail_ok = ctx
            .report()
            .findings()
            .iter()
            .any(|f| f.check == "panic" && f.detail.contains("boom pressed"));
        if !detail_ok {
            mismatch(
                &actions_bad,
                "actions: no panic detail contains `boom pressed`".to_owned(),
            );
        }
        let closed = elements_of(ctx.report(), "main-closed", "actions");
        if closed.len() != 1 || !names_once(&closed, "Close") {
            mismatch(
                &actions_bad,
                format!("actions: expected one main-closed on Close, got {closed:?}"),
            );
        }
        expect_eq(&actions_bad, "actions: stats.denied", stats.denied, 1);
        expect_eq(&actions_bad, "actions: stats.activated", stats.activated, 4);
        expect_eq(&actions_bad, "actions: stats.gone", stats.gone, 0);
        if hit.get() {
            mismatch(
                &actions_bad,
                "actions: `Delete all` was activated".to_owned(),
            );
        }
        // The initial restore and the end of the pass run it as well as every restore.
        expect_eq(
            &actions_bad,
            "actions: before_restore calls",
            before.get(),
            stats.restores + 2,
        );
        Ok(())
    })
}

/// The smoke options: the control cap, always restoring and no windows.
fn option_scenarios(harness: Harness, bad: &Mismatches) -> Harness {
    let (capped_bad, always_bad, none_bad) = (Rc::clone(bad), Rc::clone(bad), Rc::clone(bad));
    harness
        .scenario("capped", move |ctx| {
            let slot = ToastSlot::default();
            let stats = ctx.smoke(Smoke::new("capped").max_controls(1), |ctx| {
                launch_toast(ctx, &slot).map(|app| vec![app])
            });
            expect_eq(&capped_bad, "capped: stats.activated", stats.activated, 1);
            Ok(())
        })
        .scenario("always", move |ctx| {
            let slot = ToastSlot::default();
            let stats = ctx.smoke(Smoke::new("always").always_restore(true), |ctx| {
                launch_toast(ctx, &slot).map(|app| vec![app])
            });
            expect_eq(&always_bad, "always: stats.activated", stats.activated, 2);
            expect_eq(
                &always_bad,
                "always: stats.restores",
                stats.restores,
                stats.activated,
            );
            Ok(())
        })
        .scenario("no windows", move |ctx| {
            let _ = ctx.smoke(Smoke::new("none"), |_| Ok(Vec::new()));
            let found = ctx
                .report()
                .findings()
                .iter()
                .any(|f| f.check == "launch" && f.detail.contains("no windows"));
            if !found {
                mismatch(
                    &none_bad,
                    "no windows: no `launch` finding naming `no windows`".to_owned(),
                );
            }
            Ok(())
        })
}
