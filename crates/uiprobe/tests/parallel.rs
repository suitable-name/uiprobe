//! Self-test: parallel scenario workers (`harness = false`).
//!
//! The parent re-runs this binary as a child in six modes (`UIPROBE_SELFTEST_CHILD`); each
//! writes its own report directory under the target directory:
//!
//! - `serial`, `fixed3` (`Jobs::Fixed(3)`) and `auto` (`Jobs::Auto`, `max_jobs(2)`) run the same
//!   six scenarios: two overlapping buttons ("Alpha", "Beta"), a click on each, a direct
//!   `Model.ping` invoke, a forced expectation, a panic and a two-size sweep. The parallel
//!   reports must equal the serial one: findings (all fields and repeat counts), per-check
//!   counts, the coverage section and the scenario names with their finding counts. The
//!   parallel report has exactly one extra note. The coverage gate passes in all three only
//!   because each inventory item is covered by a different scenario, so the recorders of
//!   different workers must be merged. The console shows the scenarios in scenario order.
//! - `abort` (`Fixed(2)`, one scenario calls `request_abort`): exit code 2, the message on
//!   stderr, and no profile directory of the run is left in the temp dir.
//! - `crash` (`Fixed(2)`, one scenario calls `std::process::abort`): exit code 1, exactly one
//!   `worker-crash` finding that names the scenario, every other scenario's result present.
//! - `env` (`UIPROBE_JOBS=bogus`): a `config` finding, a serial run, exit code 1.
//! - `last-serial` and `last-fixed2` (`Jobs::Fixed(2)`): a `scenario_last` registered before the
//!   ordinary scenarios runs after them and sees the callbacks both recorded, one of them in
//!   the other worker; the timing statistics are in `report.json` and `report.md`.
//!
//! The fixture text below must match `tests/fixtures/parallel/ui/app.slint`, which the coverage
//! gate scans (relative to `CARGO_MANIFEST_DIR`). Run this test WITHOUT a scenario filter. The
//! parent starts no thread before any harness run.

use serde_json::Value;
use slint::ComponentHandle;
use std::{fs, path::PathBuf, process::Command, time::Duration};
use uiprobe::{prelude::*, profile::request_abort};

slint::slint! {
    import { Button } from "std-widgets.slint";

    export global Model {
        callback ping();
    }

    export component Fixture inherits Window {
        width: 300px;
        height: 100px;
        default-font-family: "FixedTestFont";
        Button {
            text: "Alpha";
            x: 0px;
            y: 10px;
            width: 100px;
            height: 30px;
        }
        Button {
            text: "Beta";
            x: 50px;
            y: 10px;
            width: 100px;
            height: 30px;
        }
    }
}

/// Set in the child process to the mode it runs.
const CHILD_FLAG: &str = "UIPROBE_SELFTEST_CHILD";
/// Prefix of the profile directory names (coordinator and workers), set by the parent so it can
/// look for leftovers.
const PREFIX_VAR: &str = "UIPROBE_PTEST_PREFIX";
const SCAN_ROOT: &str = "tests/fixtures/parallel/ui";

fn report_dir_for(mode: &str) -> PathBuf {
    uiprobe::target_dir!().join(format!("uiprobe-selftest-parallel-{mode}"))
}

/// What the `small` scenario list does in its second scenario.
#[derive(Clone, Copy)]
enum Kind {
    Plain,
    Abort,
    Crash,
}

/// The six scenarios of the comparison.
fn six(harness: Harness) -> Harness {
    harness
        .scenario("s1 alpha", |ctx| {
            let app = ctx.launch(Fixture::new)?;
            ctx.sweep(&app, "start");
            ctx.view(&app).click("Alpha")?;
            ctx.shutdown(app);
            Ok(())
        })
        .scenario("s2 beta", |ctx| {
            let app = ctx.launch(Fixture::new)?;
            ctx.view(&app).click("Beta")?;
            ctx.shutdown(app);
            Ok(())
        })
        .scenario("s3 ping", |ctx| {
            let app = ctx.launch(Fixture::new)?;
            if let Some(fixture) = app.component::<Fixture>() {
                fixture.global::<Model>().invoke_ping();
            }
            record_callback("Model.ping");
            ctx.shutdown(app);
            Ok(())
        })
        .scenario("s4 expect", |ctx| {
            ctx.expect(false, "forced", "forced finding");
            Ok(())
        })
        .scenario("s5 panics", |ctx| {
            let _app = ctx.launch(Fixture::new)?;
            panic!("scenario boom");
        })
        .scenario("s6 sizes", |ctx| {
            let app = ctx.launch(Fixture::new)?;
            ctx.sweep_at_sizes(&app, "sizes");
            ctx.shutdown(app);
            Ok(())
        })
}

fn full(mode: &str, jobs: Jobs, max: Option<usize>) -> Harness {
    let mut harness = Harness::new()
        .report_dir(report_dir_for(mode))
        .sizes([(300, 100), (200, 100)])
        .coverage(Coverage::scan(SCAN_ROOT))
        .jobs(jobs);
    if let Some(max) = max {
        harness = harness.max_jobs(max);
    }
    six(harness)
}

/// Four trivial scenarios; the second one aborts or crashes according to `kind`.
fn small(mode: &str, jobs: Jobs, kind: Kind) -> Harness {
    let mut harness = Harness::new().report_dir(report_dir_for(mode)).jobs(jobs);
    if let Ok(prefix) = std::env::var(PREFIX_VAR) {
        harness = harness.profile(Profile::isolated().name_prefix(prefix));
    }
    harness
        .scenario("one", |_ctx| Ok(()))
        .scenario("two", move |_ctx| {
            match kind {
                Kind::Plain => {}
                Kind::Abort => request_abort("selftest abort"),
                Kind::Crash => std::process::abort(),
            }
            Ok(())
        })
        .scenario("three", |_ctx| Ok(()))
        .scenario("four", |_ctx| Ok(()))
}

/// Two ordinary scenarios that record a callback each (`a` also sleeps, so with two workers the
/// two run in different workers) and a `scenario_last` registered FIRST that must still run
/// last and must see both callbacks, one of which was recorded in another worker.
fn lasting(mode: &str, jobs: Jobs) -> Harness {
    Harness::new()
        .report_dir(report_dir_for(mode))
        .jobs(jobs)
        .scenario_last("z last", |ctx| {
            let seen = uiprobe::coverage::recorded_callbacks();
            for key in ["Last.a", "Last.b"] {
                ctx.expect(
                    seen.contains(key),
                    "last-coverage",
                    format!("{key} is not recorded when the last scenario runs; saw {seen:?}"),
                );
            }
            Ok(())
        })
        .scenario("a", |_ctx| {
            record_callback("Last.a");
            timing::record("selftest", Duration::from_millis(5));
            std::thread::sleep(Duration::from_millis(600));
            Ok(())
        })
        .scenario("b", |_ctx| {
            record_callback("Last.b");
            Ok(())
        })
}

fn main() -> ExitCode {
    std::env::var(CHILD_FLAG)
        .ok()
        .as_deref()
        .map_or_else(parent, child)
}

/// The child: one harness run per mode; the exit code is the harness's own.
fn child(mode: &str) -> ExitCode {
    match mode {
        "serial" => full(mode, Jobs::Serial, None).run(),
        "fixed3" => full(mode, Jobs::Fixed(3), None).run(),
        "auto" => full(mode, Jobs::Auto, Some(2)).run(),
        "abort" => small(mode, Jobs::Fixed(2), Kind::Abort).run(),
        "crash" => small(mode, Jobs::Fixed(2), Kind::Crash).run(),
        "env" => small(mode, Jobs::Serial, Kind::Plain).run(),
        "last-serial" => lasting(mode, Jobs::Serial).run(),
        "last-fixed2" => lasting(mode, Jobs::Fixed(2)).run(),
        _ => ExitCode::FAILURE,
    }
}

/// Runs this binary as a child in `mode`; returns its exit code, stdout and stderr.
fn run_child(mode: &str, extra_env: &[(&str, &str)]) -> (Option<i32>, String, String) {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            return (
                None,
                String::new(),
                format!("cannot find the test binary: {e}"),
            );
        }
    };
    let mut command = Command::new(exe);
    command.env(CHILD_FLAG, mode);
    for (key, value) in extra_env {
        command.env(key, value);
    }
    match command.output() {
        Ok(output) => (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ),
        Err(e) => (
            None,
            String::new(),
            format!("cannot run the child process: {e}"),
        ),
    }
}

fn expect_code(
    mode: &str,
    (code, stdout, stderr): &(Option<i32>, String, String),
    want: i32,
    mismatches: &mut Vec<String>,
) {
    if *code != Some(want) {
        mismatches.push(format!(
            "child {mode}: exited with {code:?}, wanted Some({want}); stdout: {stdout}; \
             stderr: {stderr}"
        ));
    }
}

/// Deletes a stale `report.json` so a child that fails early cannot be compared by its old file.
fn clear_report(mode: &str) {
    let _ = fs::remove_file(report_dir_for(mode).join("report.json"));
}

fn load_report(mode: &str) -> Result<Value, String> {
    let path = report_dir_for(mode).join("report.json");
    let text =
        fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{} is not JSON: {e}", path.display()))
}

/// `(name, finding count)` of every scenario of a report.
fn scenario_summary(report: &Value) -> Vec<(String, u64)> {
    report["scenarios"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| {
            (
                s["name"].as_str().unwrap_or_default().to_owned(),
                s["findings"].as_u64().unwrap_or(u64::MAX),
            )
        })
        .collect()
}

/// The names of the notes of a report.
fn notes(report: &Value) -> Vec<String> {
    report["notes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| n.as_str().map(str::to_owned))
        .collect()
}

/// `other` must equal `serial` in everything but the scenario timings and the notes.
fn compare(mode: &str, serial: &Value, other: &Value, mismatches: &mut Vec<String>) {
    for key in ["findings", "per_check", "sections", "slint", "harness"] {
        if serial[key] != other[key] {
            mismatches.push(format!(
                "{mode}: `{key}` differs from the serial run:\n  serial: {}\n  {mode}: {}",
                serial[key], other[key]
            ));
        }
    }
    let (want, got) = (scenario_summary(serial), scenario_summary(other));
    if want != got {
        mismatches.push(format!(
            "{mode}: scenarios are {got:?}, serial has {want:?}"
        ));
    }
}

/// `serial` against `fixed3` and `auto`: same report, one extra note, ordered console.
fn comparison_checks(mismatches: &mut Vec<String>) {
    let modes = ["serial", "fixed3", "auto"];
    let mut results = Vec::new();
    for mode in modes {
        clear_report(mode);
        let result = run_child(mode, &[]);
        expect_code(mode, &result, 1, mismatches);
        results.push(result);
    }
    let reports: Vec<Result<Value, String>> = modes.iter().map(|m| load_report(m)).collect();
    let (Ok(serial), Ok(fixed3), Ok(auto)) = (&reports[0], &reports[1], &reports[2]) else {
        for report in reports.iter().filter_map(|r| r.as_ref().err()) {
            mismatches.push(report.clone());
        }
        return;
    };
    for check in ["overlap", "forced", "scenario-panic"] {
        if serial["per_check"][check].as_u64().unwrap_or(0) == 0 {
            mismatches.push(format!(
                "serial: no `{check}` finding, the comparison is vacuous"
            ));
        }
    }
    if !serial["per_check"]["coverage"].is_null() || !fixed3["per_check"]["coverage"].is_null() {
        mismatches.push("a coverage finding: the recorders were not merged".to_owned());
    }
    compare("fixed3", serial, fixed3, mismatches);
    compare("auto", serial, auto, mismatches);
    let serial_notes = notes(serial);
    for (mode, report) in [("fixed3", fixed3), ("auto", auto)] {
        let got = notes(report);
        let extra_ok = got.len() == serial_notes.len() + 1
            && got[..serial_notes.len()] == serial_notes[..]
            && got
                .last()
                .is_some_and(|n| n.starts_with("parallel run: ") && n.contains("worker(s) used"));
        if !extra_ok {
            mismatches.push(format!(
                "{mode}: notes {got:?} are not serial's {serial_notes:?} plus one"
            ));
        }
    }
    if !notes(serial).iter().all(|n| !n.starts_with("parallel run")) {
        mismatches.push("the serial run has a parallel note".to_owned());
    }
    console_checks(&results[1].1, mismatches);
}

/// The `fixed3` console: the parallel line, and the scenarios in scenario order.
fn console_checks(stdout: &str, mismatches: &mut Vec<String>) {
    if !stdout.contains("uiprobe: parallel run: fixed 3, up to 3 worker(s)") {
        mismatches.push(format!("fixed3: no parallel run line in: {stdout}"));
    }
    if !stdout.contains("uiprobe: worker 1 started") {
        mismatches.push("fixed3: no `worker 1 started` line".to_owned());
    }
    let names = [
        "s1 alpha",
        "s2 beta",
        "s3 ping",
        "s4 expect",
        "s5 panics",
        "s6 sizes",
    ];
    let mut last = 0;
    for name in names {
        let line = format!("uiprobe: scenario {name}\n");
        if let Some(at) = stdout[last..].find(&line) {
            last += at + line.len();
        } else {
            mismatches.push(format!(
                "fixed3: scenario {name} is missing or out of order"
            ));
            return;
        }
    }
}

/// Profile directories of the temp dir whose name starts with `prefix-`.
fn leftovers(prefix: &str) -> Vec<PathBuf> {
    let start = format!("{prefix}-");
    fs::read_dir(std::env::temp_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&start))
        .map(|entry| entry.path())
        .collect()
}

fn abort_checks(mismatches: &mut Vec<String>) {
    let prefix = format!("uiprobe-ptest-abort-{}", std::process::id());
    let result = run_child("abort", &[(PREFIX_VAR, &prefix)]);
    expect_code("abort", &result, 2, mismatches);
    if !result.2.contains("selftest abort") {
        mismatches.push(format!("abort: stderr lacks the message: {}", result.2));
    }
    let left = leftovers(&prefix);
    if !left.is_empty() {
        mismatches.push(format!("abort: profile directories left behind: {left:?}"));
    }
    for dir in left {
        let _ = fs::remove_dir_all(dir);
    }
}

fn crash_checks(mismatches: &mut Vec<String>) {
    let prefix = format!("uiprobe-ptest-crash-{}", std::process::id());
    clear_report("crash");
    let result = run_child("crash", &[(PREFIX_VAR, &prefix)]);
    expect_code("crash", &result, 1, mismatches);
    // The crashed worker may have left its profile behind (the finding says so).
    for dir in leftovers(&prefix) {
        let _ = fs::remove_dir_all(dir);
    }
    let report = match load_report("crash") {
        Ok(report) => report,
        Err(e) => {
            mismatches.push(e);
            return;
        }
    };
    let crashes: Vec<&Value> = report["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["check"] == "worker-crash")
        .collect();
    let named = crashes.len() == 1
        && crashes[0]["detail"]
            .as_str()
            .is_some_and(|d| d.starts_with("two:"));
    if !named {
        mismatches.push(format!(
            "crash: want one worker-crash for `two`, got {crashes:?}"
        ));
    }
    let want = vec![
        ("one".to_owned(), 0),
        ("two".to_owned(), 1),
        ("three".to_owned(), 0),
        ("four".to_owned(), 0),
    ];
    let got = scenario_summary(&report);
    if got != want {
        mismatches.push(format!("crash: scenarios are {got:?}, wanted {want:?}"));
    }
    if report["findings"].as_array().map_or(0, Vec::len) != 1 {
        mismatches.push(format!(
            "crash: more than the one finding: {}",
            report["findings"]
        ));
    }
}

fn env_checks(mismatches: &mut Vec<String>) {
    let result = run_child("env", &[("UIPROBE_JOBS", "bogus")]);
    expect_code("env", &result, 1, mismatches);
    if !result.1.contains("config: 1") || !result.1.contains("UIPROBE_JOBS") {
        mismatches.push(format!(
            "env: no config finding naming the variable: {}",
            result.1
        ));
    }
    if result.1.contains("parallel run") {
        mismatches.push("env: the run was parallel".to_owned());
    }
}

/// `scenario_last` runs after the ordinary scenarios (serial and in two workers), sees the
/// coverage the other worker recorded, and the timing statistics reach `report.json` and
/// `report.md` (the scenario's own sample, a `timing` object, the parallel numbers).
fn last_checks(mismatches: &mut Vec<String>) {
    for (mode, parallel) in [("last-serial", false), ("last-fixed2", true)] {
        clear_report(mode);
        let result = run_child(mode, &[]);
        expect_code(mode, &result, 0, mismatches);
        let report = match load_report(mode) {
            Ok(report) => report,
            Err(e) => {
                mismatches.push(e);
                continue;
            }
        };
        let want = vec![
            ("a".to_owned(), 0),
            ("b".to_owned(), 0),
            ("z last".to_owned(), 0),
        ];
        let got = scenario_summary(&report);
        if got != want {
            mismatches.push(format!("{mode}: scenarios are {got:?}, wanted {want:?}"));
        }
        if report["findings"].as_array().map_or(1, Vec::len) != 0 {
            mismatches.push(format!("{mode}: findings: {}", report["findings"]));
        }
        let sample = &report["scenarios"][0]["timing"]["selftest"];
        if sample["count"].as_u64() != Some(1) || sample["total_ms"].as_u64() != Some(5) {
            mismatches.push(format!(
                "{mode}: scenario a has no selftest sample: {sample}"
            ));
        }
        if !report["timing"]["totals"]["selftest"].is_object() {
            mismatches.push(format!("{mode}: no selftest total: {}", report["timing"]));
        }
        let stats = &report["timing"]["parallel"];
        if parallel == stats.is_null() || (parallel && stats["peak_workers"].as_u64() < Some(1)) {
            mismatches.push(format!("{mode}: parallel stats are {stats}"));
        }
        let markdown = fs::read_to_string(report_dir_for(mode).join("report.md"));
        if !markdown.is_ok_and(|text| text.contains("## Timing\n") && text.contains("`selftest`")) {
            mismatches.push(format!("{mode}: report.md has no Timing section"));
        }
    }
}

fn parent() -> ExitCode {
    let mut mismatches: Vec<String> = Vec::new();
    comparison_checks(&mut mismatches);
    last_checks(&mut mismatches);
    abort_checks(&mut mismatches);
    crash_checks(&mut mismatches);
    env_checks(&mut mismatches);
    for mismatch in &mismatches {
        eprintln!("parallel: MISMATCH: {mismatch}");
    }
    if mismatches.is_empty() {
        println!("parallel: ok");
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
