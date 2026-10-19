//! Self-test: `missing-label`.
//!
//! A `TextInput` with `accessible-role: text-input` and no accessible label is exactly one
//! `missing-label`; a `LineEdit` with a placeholder text has a label (the placeholder counts)
//! and adds none. The finding is checked to belong to the `TextInput` (the rect at y=10).
//!
//! `find("Name")` then finds the `LineEdit` through its placeholder (no label matches), and
//! `find("Nope")` is still a miss.
//!
//! Needs Slint debug info at compile time (`SLINT_EMIT_DEBUG_INFO=1`, set by this workspace's
//! `.cargo/config.toml`). The fixture uses `FixedTestFont` for machine-independent geometry.

use std::{cell::RefCell, process::ExitCode, rc::Rc};

use uiprobe::{
    Harness,
    checks::sweep,
    report::{Finding, Report},
};

slint::slint! {
    import { LineEdit } from "std-widgets.slint";

    export component Fixture inherits Window {
        width: 300px;
        height: 150px;
        default-font-family: "FixedTestFont";
        TextInput {
            accessible-role: text-input;
            font-family: "FixedTestFont";
            x: 10px;
            y: 10px;
            width: 200px;
            height: 30px;
        }
        LineEdit {
            placeholder-text: "Name";
            x: 10px;
            y: 80px;
            width: 200px;
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
        .scenario("missing label", move |ctx| {
            let app = ctx.launch(Fixture::new)?;
            let findings = sweep(&app.view(), ctx.sweep_options());
            let hits: Vec<&Finding> = findings
                .iter()
                .filter(|f| f.check == "missing-label")
                .collect();
            if hits.len() != 1 {
                use std::fmt::Write as _;
                let mut text = format!("missing-label: expected 1, got {}", hits.len());
                for f in &hits {
                    let _ = write!(text, "\n    {} | {} | {}", f.window, f.element, f.detail);
                }
                sink.borrow_mut().push(text);
            } else if !hits[0].detail.contains("y=10 ") {
                sink.borrow_mut().push(format!(
                    "missing-label belongs to the wrong element (expected the TextInput at y=10): {} | {}",
                    hits[0].element, hits[0].detail
                ));
            }
            // A text field carries a placeholder, not a label: the lookup falls back to it,
            // and a label that exists nowhere is still a miss.
            let view = app.view();
            if view.find("Name").is_err() {
                sink.borrow_mut()
                    .push("placeholder: find(\"Name\") does not find the LineEdit".to_owned());
            }
            if view.find("Nope").is_ok() {
                sink.borrow_mut()
                    .push("placeholder: find(\"Nope\") found an element".to_owned());
            }
            Ok(())
        })
        .run_to_report();
    finish(&bad, &report)
}
