//! Self-test: the smoke pass restores the state after a control that changes it.
//!
//! "Hide Me" hides itself on click, which changes the set of controls; "Stay" is plain. The
//! pass must activate both (`activated == 2`), re-establish the state at least once after "Hide
//! Me" (`restores >= 1`), never lose a control (`gone == 0`) and report nothing.
//!
//! Needs Slint debug info at compile time (`SLINT_EMIT_DEBUG_INFO=1`, set by this workspace's
//! `.cargo/config.toml`). The fixture uses `FixedTestFont` for machine-independent geometry.

use std::{cell::RefCell, process::ExitCode, rc::Rc};

use uiprobe::{Harness, report::Report, smoke::Smoke};

slint::slint! {
    import { Button } from "std-widgets.slint";

    export component Fixture inherits Window {
        width: 300px;
        height: 100px;
        default-font-family: "FixedTestFont";
        in-out property <bool> hidden: false;
        Button {
            text: "Hide Me";
            visible: !root.hidden;
            x: 10px;
            y: 10px;
            width: 100px;
            height: 30px;
            clicked => {
                root.hidden = true;
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
}

type Mismatches = Rc<RefCell<Vec<String>>>;

fn finish(bad: &Mismatches, report: &Report) -> ExitCode {
    let mut ok = true;
    for m in bad.borrow().iter() {
        eprintln!("uiprobe self-test mismatch: {m}");
        ok = false;
    }
    for f in report.findings() {
        eprintln!(
            "uiprobe self-test unexpected finding: {} | {} | {} | {}",
            f.window, f.element, f.check, f.detail
        );
        ok = false;
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
    let sink = Rc::clone(&bad);
    let report = harness
        .scenario("smoke restore", move |ctx| {
            let stats = ctx.smoke(Smoke::new("hide"), |ctx| {
                Ok(vec![ctx.launch(Fixture::new)?])
            });
            let mut problems = sink.borrow_mut();
            if stats.gone != 0 {
                problems.push(format!("stats.gone: expected 0, got {}", stats.gone));
            }
            if stats.restores < 1 {
                problems.push(format!(
                    "stats.restores: expected at least 1, got {}",
                    stats.restores
                ));
            }
            if stats.activated != 2 {
                problems.push(format!(
                    "stats.activated: expected 2, got {}",
                    stats.activated
                ));
            }
            Ok(())
        })
        .run_to_report();
    finish(&bad, &report)
}
