//! Self-test: `invisible-but-enabled`.
//!
//! An enabled `Button` with `opacity: 0` is exactly one `invisible-but-enabled`, and, being off
//! screen for a user, no `outside-window` and no `overlap` (even though a second button sits
//! partly under it and it also sticks out of the window).
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
    import { Button } from "std-widgets.slint";

    export component Fixture inherits Window {
        width: 300px;
        height: 100px;
        default-font-family: "FixedTestFont";
        Button {
            text: "Visible";
            x: 10px;
            y: 10px;
            width: 100px;
            height: 30px;
        }
        // Overlaps "Visible" and sticks out of the right edge, but is not on screen.
        Button {
            text: "Ghost";
            opacity: 0;
            x: 60px;
            y: 10px;
            width: 100px;
            height: 30px;
        }
        Button {
            text: "Ghost outside";
            opacity: 0;
            x: 280px;
            y: 60px;
            width: 100px;
            height: 30px;
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
        .scenario("invisible but enabled", move |ctx| {
            let app = ctx.launch(Fixture::new)?;
            let findings = sweep(&app.view(), ctx.sweep_options());
            // Two ghosts: the one over "Visible" and the one outside the window.
            expect_count(&sink, &findings, "invisible-but-enabled", 2);
            expect_count(&sink, &findings, "outside-window", 0);
            expect_count(&sink, &findings, "overlap", 0);
            Ok(())
        })
        .run_to_report();
    finish(&bad, &report)
}
