//! Self-test: content scrolled out of its `ScrollView` viewport is neither `outside-window` nor
//! an `overlap`.
//!
//! A 300x200 window with a `ScrollView` filling it. The `Edge` button sits at y=185 (30 px
//! high) in the scrolled content, so it straddles the bottom of the viewport and of the window:
//! unclipped it would stick out by 15 px; clipped against the viewport it does not. It must be
//! visited (a button entirely below the viewport is dropped by Slint itself and would prove
//! nothing).
//!
//! Needs Slint debug info at compile time; `.cargo/config.toml` of this workspace sets
//! `SLINT_EMIT_DEBUG_INFO=1` for the `slint!` macro. The fixture uses `FixedTestFont` so the
//! geometry is the same on every machine.

use std::{cell::RefCell, process::ExitCode, rc::Rc};

use uiprobe::{
    Harness,
    checks::sweep,
    report::{Finding, Report},
};

slint::slint! {
    import { ScrollView, Button } from "std-widgets.slint";

    export component Fixture inherits Window {
        width: 300px;
        height: 200px;
        default-font-family: "FixedTestFont";
        ScrollView {
            x: 0px;
            y: 0px;
            width: 300px;
            height: 200px;
            // Pinned at the top: an unpositioned child is centred in the viewport (y = -300).
            Rectangle {
                x: 0px;
                y: 0px;
                width: 280px;
                height: 800px;
                Button {
                    text: "Edge";
                    x: 10px;
                    y: 185px;
                    width: 80px;
                    height: 30px;
                }
            }
        }
    }
}

type Mismatches = Rc<RefCell<Vec<String>>>;

fn expect_count(bad: &Mismatches, findings: &[Finding], check: &str, want: usize) {
    let hits: Vec<&Finding> = findings.iter().filter(|f| f.check == check).collect();
    if hits.len() != want {
        use std::fmt::Write as _;
        let mut text = format!("{check}: expected {want}, got {}", hits.len());
        for f in hits {
            let _ = write!(text, "\n    {} | {} | {}", f.window, f.element, f.detail);
        }
        bad.borrow_mut().push(text);
    }
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
        .scenario("scrolled out", move |ctx| {
            let app = ctx.launch(Fixture::new)?;
            let view = app.view();
            if !view.has("Edge") {
                sink.borrow_mut().push(
                    "the Edge button is not visited, so the clipping is not exercised".to_owned(),
                );
            }
            let findings = sweep(&view, ctx.sweep_options());
            expect_count(&sink, &findings, "outside-window", 0);
            expect_count(&sink, &findings, "overlap", 0);
            Ok(())
        })
        .run_to_report();
    finish(&bad, &report)
}
