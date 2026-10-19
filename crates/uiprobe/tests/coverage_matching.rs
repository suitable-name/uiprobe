//! Self-test of the coverage gate's matching rules, serial and parallel (`harness = false`).
//!
//! The parent re-runs this binary as a child in two modes (`UIPROBE_SELFTEST_CHILD`):
//! `serial` and `fixed2` (`Jobs::Fixed(2)`). Both run the same three scenarios against one
//! fixture and assert the same gate outcome in-process, so the recorder statics of one mode never
//! leak into the other:
//!
//! 1. `literals`: clicks a literal with escaped quotes (`Say \"hi\"`) and a literal with a unicode
//!    escape (`More\u{2026}`; an interpolation cannot be written inside `slint!`, the unit tests
//!    of `coverage/matching` cover it), fetches a field whose one-line statements hold two
//!    literals (`placeholder-text: "Vol/W"; accessible-label: "Printed Volume"`);
//! 2. `ids`: fetches an element with an id behind an `if` prefix (the label is computed), a
//!    wrapper site whose literal `reader_label` is the running label, credits a wrapper site by
//!    `record_wrappers` (the id sits on the wrapper's root, the fetched element is the inner
//!    button) and a site by `record_site_id`;
//! 3. `single site`: fetches the one site of an app component whose label is computed (credited
//!    by the type / base recorder sets).
//!
//! The one element nobody touches ("Never") must be the only `coverage` finding, and the
//! section must count no untrackable element: the single-site credit is what keeps the app
//! component out of that count. In the parallel mode the coordinator runs no scenario itself, so
//! the gate passes only because the worker snapshots carry the types and bases sets.
//!
//! The fixture text below must match `tests/fixtures/coverage_matching/ui/app.slint`, which the
//! gate scans (relative to `CARGO_MANIFEST_DIR`). Run this test WITHOUT a scenario filter.

use std::process::{Command, ExitCode};
use uiprobe::{
    coverage::{record_site_id, record_wrappers},
    prelude::{Coverage, Ctx, Finding, Harness, Jobs, Report},
};

slint::slint! {
    import { Button, LineEdit } from "std-widgets.slint";

    component Chip inherits Rectangle {
        in property <string> caption;
        accessible-role: button;
        accessible-label: root.caption;
        TouchArea { }
    }

    component Wrap inherits Rectangle {
        in property <string> text;
        in property <string> reader_label;
        Button {
            width: parent.width;
            height: parent.height;
            text: root.text;
            accessible-label: root.reader_label == "" ? root.text : root.reader_label;
        }
    }

    export component Fixture inherits Window {
        width: 200px;
        height: 330px;
        default-font-family: "FixedTestFont";
        in property <string> shown: "Shown";
        in property <string> other: "Other";
        in property <string> third: "Third";
        in property <string> cap: "Chip Label";
        Button { x: 10px; y: 0px; width: 150px; height: 30px; text: "Say \"hi\""; }
        Button { x: 10px; y: 35px; width: 150px; height: 30px; text: "More\u{2026}"; }
        if root.shown != "": cond-button := Button { x: 10px; y: 70px; width: 150px; height: 30px; text: root.shown; }
        LineEdit { x: 10px; y: 105px; width: 150px; height: 30px; placeholder-text: "Vol/W"; accessible-label: "Printed Volume"; }
        Wrap { x: 10px; y: 140px; width: 150px; height: 30px; text: "Visible"; reader_label: "Spoken"; }
        deliver := Wrap { x: 10px; y: 175px; width: 150px; height: 30px; text: root.other; }
        manual := Button { x: 10px; y: 210px; width: 150px; height: 30px; text: root.third; }
        Chip { x: 10px; y: 245px; width: 150px; height: 30px; caption: root.cap; }
        Button { x: 10px; y: 280px; width: 150px; height: 30px; text: "Never"; }
    }
}

/// Set in the child process to the mode it runs.
const CHILD_FLAG: &str = "UIPROBE_SELFTEST_CHILD";
const SCAN_ROOT: &str = "tests/fixtures/coverage_matching/ui";

fn literals(ctx: &mut Ctx<'_>) -> Result<(), Finding> {
    let app = ctx.launch(Fixture::new)?;
    let view = ctx.view(&app);
    view.click("Say \"hi\"")?;
    view.click("More\u{2026}")?;
    view.find("Printed Volume")?;
    ctx.shutdown(app);
    Ok(())
}

fn ids(ctx: &mut Ctx<'_>) -> Result<(), Finding> {
    let app = ctx.launch(Fixture::new)?;
    let view = ctx.view(&app);
    view.find("Shown")?;
    view.find("Spoken")?;
    // The fetched element is the inner button; the id `deliver` sits on the wrapper's root.
    let inner = view.find("Other")?;
    record_wrappers(&view.root, &inner, true);
    record_site_id("Fixture::manual");
    ctx.shutdown(app);
    Ok(())
}

fn single_site(ctx: &mut Ctx<'_>) -> Result<(), Finding> {
    let app = ctx.launch(Fixture::new)?;
    let view = ctx.view(&app);
    view.click("Chip Label")?;
    ctx.shutdown(app);
    Ok(())
}

fn harness(mode: &str, jobs: Jobs) -> Harness {
    Harness::new()
        .report_dir(uiprobe::target_dir!().join(format!("uiprobe-coverage-matching-{mode}")))
        .coverage(
            Coverage::scan(SCAN_ROOT)
                .label_props(&["reader_label"])
                .reader_props(&["reader_label"]),
        )
        .jobs(jobs)
        .scenario("literals", literals)
        .scenario("ids", ids)
        .scenario("single site", single_site)
}

/// What the gate must report for the fixture; every mismatch is pushed to `failures`.
fn check(mode: &str, report: &Report, failures: &mut Vec<String>) {
    let coverage: Vec<String> = report
        .findings()
        .iter()
        .filter(|f| f.check == "coverage")
        .map(|f| f.element.clone())
        .collect();
    let want = ["app.slint|Fixture|\"Never\""];
    if coverage != want {
        failures.push(format!(
            "{mode}: coverage findings {coverage:?}, wanted {want:?}"
        ));
    }
    let others: Vec<String> = report
        .findings()
        .iter()
        .filter(|f| f.check != "coverage")
        .map(|f| format!("{}: {}", f.check, f.detail))
        .collect();
    if !others.is_empty() {
        failures.push(format!("{mode}: unexpected findings {others:?}"));
    }
    let markdown = report.render_markdown();
    if !markdown.contains("(0 allow-listed, 1 uncovered, 0 untrackable:") {
        failures.push(format!(
            "{mode}: the Coverage section does not say `0 allow-listed, 1 uncovered, 0 untrackable`:\n{markdown}"
        ));
    }
    if !markdown.contains("/ 9 inventoried") {
        failures.push(format!(
            "{mode}: the inventory is not 9 elements:\n{markdown}"
        ));
    }
}

/// The child: one harness run, assertions in-process, exit code 0 when all hold.
fn child(mode: &str) -> ExitCode {
    let jobs = if mode == "fixed2" {
        Jobs::Fixed(2)
    } else {
        Jobs::Serial
    };
    let report = harness(mode, jobs).run_to_report();
    let mut failures = Vec::new();
    check(mode, &report, &mut failures);
    if failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        for failure in &failures {
            eprintln!("coverage_matching FAILED: {failure}");
        }
        ExitCode::FAILURE
    }
}

/// Runs this binary as a child in `mode`; `Err` carries the failure text.
fn run_child(mode: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find the test binary: {e}"))?;
    let output = Command::new(exe)
        .env(CHILD_FLAG, mode)
        .output()
        .map_err(|e| format!("cannot run the child process: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "child {mode} exited with {:?}\nstdout: {}\nstderr: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

fn main() -> ExitCode {
    if let Ok(mode) = std::env::var(CHILD_FLAG) {
        return child(&mode);
    }
    let mut failed = false;
    for mode in ["serial", "fixed2"] {
        if let Err(message) = run_child(mode) {
            eprintln!("coverage_matching FAILED: {message}");
            failed = true;
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        println!("coverage_matching: ok");
        ExitCode::SUCCESS
    }
}
