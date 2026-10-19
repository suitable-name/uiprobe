//! Self-test of the coverage gate (`harness = false`).
//!
//! The fixture has a global with two callbacks (`Model.used`, `Model.unused`) and one interactive
//! element (the "Press" button), so the inventory is those two callbacks and that element. The
//! recorder statics accumulate across runs in one process, so the bare run comes first. Runs:
//!
//! 0. a scenario that only launches: all three inventory entries are findings, callbacks first;
//! 1. the scenario invokes `Model.used`, records it and clicks "Press"; no allow entries: exactly
//!    one `coverage` finding, element `Model.unused` (the element being activated proves the
//!    recorder to reach lookup end to end);
//! 2. the allow list from the TOML file `tests/fixtures/coverage/allow.toml`: no finding;
//! 3. in-code entries for `Model.unused` and `Model.gone`: exactly one `coverage` finding, the
//!    stale entry `Model.gone`;
//! 4. `GateRule::Armed` and nobody arms the gate: no finding;
//! 5. `GateRule::Armed` and the scenario calls `arm_gate()`: the gate runs, `Model.unused`;
//! 6. a scan root with no `.slint` files: one finding saying it holds no global callbacks;
//! 7. an allow file that does not exist: `Model.unused` plus a "cannot read allow file" finding.
//!
//! Every `coverage` finding must carry the state `coverage`. The gate rule is the default
//! (`NoFilter`) except in runs 4 and 5: run this test WITHOUT a scenario filter argument,
//! otherwise the gate is skipped and every run reports zero findings.
//!
//! The fixture text below must match `tests/fixtures/coverage/ui/app.slint`, which is what the
//! gate scans (relative to `CARGO_MANIFEST_DIR`). `SLINT_EMIT_DEBUG_INFO=1` comes from the
//! workspace `.cargo/config.toml`.

use slint::ComponentHandle;
use std::process::ExitCode;
use uiprobe::{
    coverage::{arm_gate, record_callback},
    prelude::{Coverage, Ctx, Finding, GateRule, Harness, Report},
};

slint::slint! {
    import { Button } from "std-widgets.slint";

    export global Model {
        callback used();
        callback unused();
    }

    export component Fixture inherits Window {
        width: 200px;
        height: 100px;
        default-font-family: "FixedTestFont";
        Text {
            text: "Coverage";
        }
        Button {
            text: "Press";
            x: 10px;
            y: 40px;
            width: 100px;
            height: 30px;
        }
    }
}

const SCAN_ROOT: &str = "tests/fixtures/coverage/ui";
const ALLOW_FILE: &str = "tests/fixtures/coverage/allow.toml";
const EMPTY_ROOT: &str = "tests/fixtures/coverage/nothing";
const MISSING_ALLOW_FILE: &str = "tests/fixtures/coverage/missing.toml";

/// Launches the fixture. When `exercise` is set, also invokes `Model.used`, records that it was
/// exercised and clicks "Press"; when `arm` is set, arms the gate.
fn scenario(ctx: &mut Ctx<'_>, exercise: bool, arm: bool) -> Result<(), Finding> {
    let app = ctx.launch(Fixture::new)?;
    if arm {
        arm_gate();
    }
    if exercise {
        if let Some(fixture) = app.component::<Fixture>() {
            fixture.global::<Model>().invoke_used();
        }
        record_callback("Model.used");
        ctx.view(&app).click("Press")?;
    }
    Ok(())
}

fn run(coverage: Coverage, exercise: bool, arm: bool) -> Report {
    Harness::new()
        .report_dir(uiprobe::target_dir!().join("uiprobe-coverage"))
        .coverage(coverage)
        .scenario("coverage fixture", move |ctx| scenario(ctx, exercise, arm))
        .run_to_report()
}

/// The `element` of every `coverage` finding in `report`.
fn coverage_elements(report: &Report) -> Vec<String> {
    report
        .findings()
        .iter()
        .filter(|f| f.check == "coverage")
        .map(|f| f.element.clone())
        .collect()
}

/// Element and detail of every `coverage` finding, joined with a space.
fn coverage_texts(report: &Report) -> Vec<String> {
    report
        .findings()
        .iter()
        .filter(|f| f.check == "coverage")
        .map(|f| format!("{} {}", f.element, f.detail))
        .collect()
}

fn expect(label: &str, report: &Report, want: &[&str], failures: &mut Vec<String>) {
    let got = coverage_elements(report);
    let ok = got.len() == want.len() && got.iter().zip(want).all(|(g, w)| g == w);
    if !ok {
        failures.push(format!("{label}: expected {want:?}, got {got:?}"));
    }
    for finding in report.findings().iter().filter(|f| f.check == "coverage") {
        if finding.state != "coverage" {
            failures.push(format!(
                "{label}: finding {:?} has state {:?}, wanted \"coverage\"",
                finding.element, finding.state
            ));
        }
    }
}

/// Every wanted text must be contained in some `coverage` finding's element-and-detail text, and
/// there must be exactly as many findings as wanted texts.
fn expect_details_contain(label: &str, report: &Report, want: &[&str], failures: &mut Vec<String>) {
    let got = coverage_texts(report);
    let ok = got.len() == want.len() && want.iter().all(|w| got.iter().any(|g| g.contains(w)));
    if !ok {
        failures.push(format!("{label}: expected texts {want:?}, got {got:?}"));
    }
    for finding in report.findings().iter().filter(|f| f.check == "coverage") {
        if finding.state != "coverage" {
            failures.push(format!(
                "{label}: finding {:?} has state {:?}, wanted \"coverage\"",
                finding.detail, finding.state
            ));
        }
    }
}

fn main() -> ExitCode {
    let mut failures = Vec::new();

    let report = run(Coverage::scan(SCAN_ROOT), false, false);
    expect(
        "run 0 (nothing exercised)",
        &report,
        &["Model.used", "Model.unused", "app.slint|Fixture|\"Press\""],
        &mut failures,
    );

    let report = run(Coverage::scan(SCAN_ROOT), true, false);
    expect(
        "run 1 (no allow entries)",
        &report,
        &["Model.unused"],
        &mut failures,
    );

    let report = run(
        Coverage::scan(SCAN_ROOT).allow_file(ALLOW_FILE),
        true,
        false,
    );
    expect("run 2 (allow file)", &report, &[], &mut failures);

    let report = run(
        Coverage::scan(SCAN_ROOT)
            .allow(&[("Model.unused", "self-test"), ("Model.gone", "self-test")]),
        true,
        false,
    );
    expect(
        "run 3 (stale entry)",
        &report,
        &["Model.gone"],
        &mut failures,
    );

    let report = run(
        Coverage::scan(SCAN_ROOT).gate_when(GateRule::Armed),
        true,
        false,
    );
    expect("run 4 (armed, not armed)", &report, &[], &mut failures);

    let report = run(
        Coverage::scan(SCAN_ROOT).gate_when(GateRule::Armed),
        true,
        true,
    );
    expect(
        "run 5 (armed by the scenario)",
        &report,
        &["Model.unused"],
        &mut failures,
    );

    let report = run(Coverage::scan(EMPTY_ROOT), true, false);
    expect_details_contain(
        "run 6 (empty scan root)",
        &report,
        &["holds no global callbacks"],
        &mut failures,
    );

    let report = run(
        Coverage::scan(SCAN_ROOT).allow_file(MISSING_ALLOW_FILE),
        true,
        false,
    );
    expect_details_contain(
        "run 7 (missing allow file)",
        &report,
        &["Model.unused", "cannot read allow file"],
        &mut failures,
    );

    if failures.is_empty() {
        println!("coverage self-test: ok");
        ExitCode::SUCCESS
    } else {
        for failure in &failures {
            eprintln!("coverage self-test FAILED: {failure}");
        }
        ExitCode::FAILURE
    }
}
