//! Self-test: `outside-window` and `zero-size` fire when they should, and the
//! `bounds_for_all_roles` switch does what it says.
//!
//! * "Edge" (a button, right edge at 350 in a 300 wide window) is an `outside-window`;
//! * "Far" (a role-text `Text` sticking out of the right edge) is an `outside-window` only while
//!   `bounds_for_all_roles` is on (the default); with it off only controls count;
//! * "Flat" (a button with no width) is exactly one `zero-size` and, having no visible part, no
//!   `outside-window`.
//!
//! Every element is first looked up by label: an element the backend did not report would make
//! the counts prove nothing, so that is a mismatch of its own.
//!
//! Needs Slint debug info at compile time (`SLINT_EMIT_DEBUG_INFO=1`, set by this workspace's
//! `.cargo/config.toml`). The fixture uses `FixedTestFont` for machine-independent geometry.

use std::{cell::RefCell, process::ExitCode, rc::Rc};

use uiprobe::{
    Harness,
    checks::{SweepOptions, sweep},
    report::{Finding, Report},
};

slint::slint! {
    import { Button } from "std-widgets.slint";

    export component Fixture inherits Window {
        width: 300px;
        height: 100px;
        default-font-family: "FixedTestFont";
        // Right edge at 350, window is 300 wide.
        Button {
            text: "Edge";
            x: 250px;
            y: 10px;
            width: 100px;
            height: 30px;
        }
        // No width: a zero-size control.
        Button {
            text: "Flat";
            x: 10px;
            y: 60px;
            width: 0px;
            height: 30px;
        }
        // Not a control, but it has an accessible role and sticks out.
        Text {
            accessible-role: text;
            text: "Far";
            x: 290px;
            y: 60px;
            width: 80px;
            height: 20px;
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

/// The findings of `check` whose element description contains `needle`.
fn with_element<'f>(findings: &'f [Finding], check: &str, needle: &str) -> Vec<&'f Finding> {
    findings
        .iter()
        .filter(|f| f.check == check && f.element.contains(needle))
        .collect()
}

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
        .scenario("bounds", move |ctx| {
            let app = ctx.launch(Fixture::new)?;
            let view = app.view();
            for label in ["Edge", "Flat", "Far"] {
                if !view.has(label) {
                    sink.borrow_mut()
                        .push(format!("{label}: not visited, nothing is proven"));
                }
            }

            let findings = sweep(&view, ctx.sweep_options());
            expect_count(&sink, "default", &findings, "outside-window", 2);
            let edge = with_element(&findings, "outside-window", "Edge");
            if edge.len() != 1 || !edge[0].detail.contains("outside window 300x100") {
                sink.borrow_mut().push(format!(
                    "default: Edge should be one outside-window naming `outside window 300x100`, got {:?}",
                    edge.iter().map(|f| f.detail.as_str()).collect::<Vec<_>>()
                ));
            }
            if with_element(&findings, "outside-window", "Far").len() != 1 {
                sink.borrow_mut()
                    .push("default: Far should be one outside-window".to_owned());
            }
            expect_count(&sink, "default", &findings, "zero-size", 1);
            if with_element(&findings, "zero-size", "Flat").len() != 1 {
                sink.borrow_mut()
                    .push("default: the zero-size finding should name Flat".to_owned());
            }

            let controls_only = SweepOptions {
                bounds_for_all_roles: false,
                ..SweepOptions::default()
            };
            let findings = sweep(&view, &controls_only);
            expect_count(&sink, "controls only", &findings, "outside-window", 1);
            if with_element(&findings, "outside-window", "Edge").len() != 1 {
                sink.borrow_mut()
                    .push("controls only: the one outside-window should be Edge".to_owned());
            }
            drop(view);
            ctx.shutdown(app);
            Ok(())
        })
        .run_to_report();
    finish(&bad, &report)
}
