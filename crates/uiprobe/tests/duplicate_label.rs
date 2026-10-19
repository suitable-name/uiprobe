//! Self-test: a label carried by two elements is an `ambiguous-label` finding from
//! `WindowView::find`, never a panic; a missing label is `not-found`; `find_nth` picks one.
//!
//! The fixture is a `slint!` window, which needs element debug info at COMPILE time: the
//! repository's `.cargo/config.toml` sets `SLINT_EMIT_DEBUG_INFO=1` (set it yourself elsewhere).
//! Assertions are made here and printed on a mismatch; they are not added to the report, which
//! must stay at zero findings.

use std::{cell::RefCell, process::ExitCode, rc::Rc};
use uiprobe::prelude::*;

slint::slint! {
    import { Button } from "std-widgets.slint";

    export component TwoOk inherits Window {
        default-font-family: "FixedTestFont";
        preferred-width: 300px;
        preferred-height: 200px;

        Button {
            x: 10px;
            y: 10px;
            width: 100px;
            height: 40px;
            text: "OK";
        }

        Button {
            x: 10px;
            y: 100px;
            width: 100px;
            height: 40px;
            text: "OK";
        }
    }
}

/// Records a mismatch unless `result` is an `Err` finding of check `wanted`.
fn expect_finding(
    mismatches: &mut Vec<String>,
    what: &str,
    result: Result<ElementHandle, Finding>,
    wanted: &str,
) {
    match result {
        Ok(_) => mismatches.push(format!(
            "{what}: expected a {wanted} finding, got an element"
        )),
        Err(finding) if finding.check == wanted => {}
        Err(finding) => mismatches.push(format!(
            "{what}: expected check {wanted}, got {} ({})",
            finding.check, finding.detail
        )),
    }
}

fn main() -> ExitCode {
    let mismatches = Rc::new(RefCell::new(Vec::<String>::new()));
    let sink = Rc::clone(&mismatches);
    let report = Harness::new()
        .report_dir(uiprobe::target_dir!().join("uiprobe-selftest-duplicate-label"))
        .scenario("duplicate-label", move |ctx| {
            let app = ctx.launch(TwoOk::new)?;
            let view = ctx.view(&app);
            let mut bad = sink.borrow_mut();
            expect_finding(&mut bad, "find(\"OK\")", view.find("OK"), "ambiguous-label");
            expect_finding(&mut bad, "find(\"Nope\")", view.find("Nope"), "not-found");
            if let Err(finding) = view.find_nth("OK", 1) {
                bad.push(format!("find_nth(\"OK\", 1) failed: {}", finding.detail));
            }
            expect_finding(
                &mut bad,
                "find_nth(\"OK\", 2)",
                view.find_nth("OK", 2),
                "not-found",
            );
            drop(bad);
            ctx.shutdown(app);
            Ok(())
        })
        .run_to_report();

    let mut failed = false;
    for mismatch in mismatches.borrow().iter() {
        eprintln!("duplicate_label: MISMATCH: {mismatch}");
        failed = true;
    }
    if !report.is_empty() {
        for finding in report.findings() {
            eprintln!(
                "duplicate_label: unexpected finding {}: {}",
                finding.check, finding.detail
            );
        }
        failed = true;
    }
    if failed {
        ExitCode::FAILURE
    } else {
        println!("duplicate_label: ok");
        ExitCode::SUCCESS
    }
}
