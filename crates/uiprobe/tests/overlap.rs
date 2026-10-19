//! Self-test: the `overlap` check.
//!
//! 1. Two 100x30 buttons offset by 50 px horizontally overlap partially: exactly one `overlap`.
//! 2. A header-size `TouchArea` (role button, 28 px high) declared first and a button laid on it
//!    on purpose (at least 75 % inside, centre inside): no `overlap`. With `header_max_h`
//!    lowered below the header's height the same pair IS reported, which proves the exception
//!    (and not something else) suppressed it.
//!
//! Needs Slint debug info at compile time (`SLINT_EMIT_DEBUG_INFO=1`, set by this workspace's
//! `.cargo/config.toml`). The fixtures use `FixedTestFont` for machine-independent geometry.

use std::{cell::RefCell, process::ExitCode, rc::Rc};

use uiprobe::{
    Harness,
    checks::{SweepOptions, sweep},
    report::{Finding, Report},
};

slint::slint! {
    import { Button } from "std-widgets.slint";

    export component PairFixture inherits Window {
        width: 300px;
        height: 100px;
        default-font-family: "FixedTestFont";
        Button {
            text: "Left";
            x: 0px;
            y: 10px;
            width: 100px;
            height: 30px;
        }
        Button {
            text: "Right";
            x: 50px;
            y: 10px;
            width: 100px;
            height: 30px;
        }
    }

    export component HeaderFixture inherits Window {
        width: 300px;
        height: 100px;
        default-font-family: "FixedTestFont";
        // The header hit area comes first ...
        TouchArea {
            accessible-role: button;
            accessible-label: "Header";
            x: 0px;
            y: 0px;
            width: 300px;
            height: 28px;
        }
        // ... the pill is drawn on top: 24 of its 30 px lie inside the header (80 %).
        Button {
            text: "Pill";
            x: 200px;
            y: 4px;
            width: 80px;
            height: 30px;
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
    let pair_sink = Rc::clone(&bad);
    let header_sink = Rc::clone(&bad);
    let report = harness
        .scenario("overlapping pair", move |ctx| {
            let app = ctx.launch(PairFixture::new)?;
            let findings = sweep(&app.view(), ctx.sweep_options());
            expect_count(&pair_sink, "pair", &findings, "overlap", 1);
            Ok(())
        })
        .scenario("header backdrop", move |ctx| {
            let app = ctx.launch(HeaderFixture::new)?;
            let findings = sweep(&app.view(), ctx.sweep_options());
            expect_count(&header_sink, "header", &findings, "overlap", 0);
            let strict = SweepOptions {
                header_max_h: 10.0,
                ..SweepOptions::default()
            };
            let findings = sweep(&app.view(), &strict);
            expect_count(
                &header_sink,
                "header, header_max_h=10",
                &findings,
                "overlap",
                1,
            );
            Ok(())
        })
        .run_to_report();
    finish(&bad, &report)
}
