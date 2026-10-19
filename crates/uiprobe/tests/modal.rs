//! Self-test: controls under a modal backdrop are no `overlap`, and the smoke pass leaves them
//! out.
//!
//! Two overlapping buttons (they WOULD be an overlap) sit under a full-window role-less
//! `TouchArea {}` declared after them, like the backdrop of a dialog. The sweep must report no
//! `overlap`; the click-everything pass must count both buttons as blocked and activate none.
//!
//! Needs Slint debug info at compile time (`SLINT_EMIT_DEBUG_INFO=1`, set by this workspace's
//! `.cargo/config.toml`).
//!
//! A second run with `modal_cover: 2.0` (no `TouchArea` can be a backdrop) must activate both
//! buttons and block none: the smoke pass reads the run's thresholds. The fixture uses
//! `FixedTestFont` for machine-independent geometry.

use std::{cell::RefCell, process::ExitCode, rc::Rc};

use uiprobe::{
    Harness,
    checks::{SweepOptions, sweep},
    report::{Finding, Report},
    smoke::Smoke,
};

slint::slint! {
    import { Button } from "std-widgets.slint";

    export component Fixture inherits Window {
        width: 300px;
        height: 200px;
        default-font-family: "FixedTestFont";
        Button {
            text: "First";
            x: 20px;
            y: 20px;
            width: 100px;
            height: 30px;
        }
        Button {
            text: "Second";
            x: 70px;
            y: 20px;
            width: 100px;
            height: 30px;
        }
        // The modal backdrop: drawn after the buttons, covers the whole window, no role.
        TouchArea {
            x: 0px;
            y: 0px;
            width: parent.width;
            height: parent.height;
        }
    }
}

type Mismatches = Rc<RefCell<Vec<String>>>;

fn expect_count(bad: &Mismatches, what: &str, findings: &[Finding], check: &str, want: usize) {
    let hits: Vec<&Finding> = findings.iter().filter(|f| f.check == check).collect();
    if hits.len() != want {
        use std::fmt::Write as _;
        let mut text = format!("{what}: {check}: expected {want}, got {}", hits.len());
        for f in hits {
            let _ = write!(text, "\n    {} | {} | {}", f.window, f.element, f.detail);
        }
        bad.borrow_mut().push(text);
    }
}

fn expect_eq(bad: &Mismatches, what: &str, got: usize, want: usize) {
    if got != want {
        bad.borrow_mut()
            .push(format!("{what}: expected {want}, got {got}"));
    }
}

fn finish(bad: &Mismatches, reports: &[&Report]) -> ExitCode {
    let mut ok = true;
    for m in bad.borrow().iter() {
        eprintln!("uiprobe self-test mismatch: {m}");
        ok = false;
    }
    for report in reports {
        for f in report.findings() {
            eprintln!(
                "uiprobe self-test unexpected finding: {} | {} | {} | {}",
                f.window, f.element, f.check, f.detail
            );
            ok = false;
        }
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn main() -> ExitCode {
    let harness = Harness::new();
    let bad: Mismatches = Rc::default();
    let sweep_sink = Rc::clone(&bad);
    let smoke_sink = Rc::clone(&bad);
    let report = harness
        .scenario("modal sweep", move |ctx| {
            let app = ctx.launch(Fixture::new)?;
            let findings = sweep(&app.view(), ctx.sweep_options());
            expect_count(&sweep_sink, "default sweep", &findings, "overlap", 0);
            // `modal_cover: 2.0` means no TouchArea can qualify as a backdrop, so the two
            // buttons overlap for real: the suppression above is the backdrop rule at work.
            let strict = uiprobe::checks::SweepOptions {
                modal_cover: 2.0,
                ..Default::default()
            };
            let findings = sweep(&app.view(), &strict);
            expect_count(&sweep_sink, "no backdrop sweep", &findings, "overlap", 1);
            ctx.shutdown(app);
            Ok(())
        })
        .scenario("modal smoke", move |ctx| {
            let stats = ctx.smoke(Smoke::new("modal"), |ctx| {
                Ok(vec![ctx.launch(Fixture::new)?])
            });
            expect_eq(&smoke_sink, "stats.blocked", stats.blocked, 2);
            expect_eq(&smoke_sink, "stats.activated", stats.activated, 0);
            Ok(())
        })
        .run_to_report();
    let strict_sink = Rc::clone(&bad);
    let strict = Harness::new()
        .sweep_options(SweepOptions {
            modal_cover: 2.0,
            ..SweepOptions::default()
        })
        .scenario("modal smoke strict", move |ctx| {
            let stats = ctx.smoke(Smoke::new("modal strict"), |ctx| {
                Ok(vec![ctx.launch(Fixture::new)?])
            });
            expect_eq(&strict_sink, "strict stats.blocked", stats.blocked, 0);
            expect_eq(&strict_sink, "strict stats.activated", stats.activated, 2);
            Ok(())
        })
        .run_to_report();
    finish(&bad, &[&report, &strict])
}
