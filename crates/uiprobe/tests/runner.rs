//! Self-test: the runner (filters, sizes, exit codes, panics, timeouts, report files).
//!
//! The parent re-runs this binary as a child in seven modes (`UIPROBE_SELFTEST_CHILD`):
//!
//! - `sizes` with `UIPROBE_SIZES=100x50`: the scenario sees exactly that size list.
//! - `nomatch` with a positional argument that matches no scenario: exit 0, nothing run, and the
//!   report files already on disk are left alone (cargo handing another target's filter here).
//! - `envfilter` with `UIPROBE_FILTER=zzz`: a `config` finding, exit 1.
//! - `allowfail` with `UIPROBE_ALLOW_FAIL=1`: a finding, but exit 0.
//! - `formats` with `UIPROBE_FORMATS=html,txt`: `report.html` and `report.txt` are written,
//!   `report.json` too, and a `report.md` left by an earlier run is removed; exit 0.
//! - `badformat` with `UIPROBE_FORMATS=pdf`: a `config` finding, exit 1, and the default
//!   `report.md` is still written.
//! - `noscenario` with no scenario at all: a `config` finding, exit 1.
//!
//! It then runs a harness in process with `Harness::formats([Markdown, Text])`, a panicking
//! scenario, a scenario that must still run after it, a timeout, a bad key chord and a launch
//! of an empty window (which must not abort), asserts exact per-check counts, the shape of
//! `report.json`, that `report.md` and `report.txt` were written and a stale `report.html`
//! removed.
//!
//! The parent starts no thread before `Harness::run` redirects the environment.

use std::{
    cell::Cell,
    collections::BTreeMap,
    fs,
    path::PathBuf,
    process::{Command, ExitCode},
    rc::Rc,
    time::Duration,
};

use uiprobe::prelude::*;

slint::slint! {
    import { Button } from "std-widgets.slint";

    export component Fixture inherits Window {
        width: 300px;
        height: 100px;
        default-font-family: "FixedTestFont";
        Button {
            text: "Only";
            x: 10px;
            y: 10px;
            width: 100px;
            height: 30px;
        }
    }

    export component Empty inherits Window {
        width: 300px;
        height: 100px;
        default-font-family: "FixedTestFont";
    }
}

/// Set in the child process to the mode it runs.
const CHILD_FLAG: &str = "UIPROBE_SELFTEST_CHILD";

fn report_dir_for(mode: &str) -> PathBuf {
    uiprobe::target_dir!().join(format!("uiprobe-selftest-runner-{mode}"))
}

fn main() -> ExitCode {
    std::env::var(CHILD_FLAG)
        .ok()
        .as_deref()
        .map_or_else(parent, child)
}

/// The child: one harness run per mode; the exit code is the harness's own.
fn child(mode: &str) -> ExitCode {
    // Compile proof that the prelude exports these.
    let _ = (
        Format::Html,
        DEFAULT_DENY.len(),
        SLINT_VERSION,
        record_callback as fn(&str),
        arm_gate as fn(),
    );
    let harness = Harness::new().report_dir(report_dir_for(mode));
    match mode {
        "sizes" => harness
            .scenario("only", |ctx| {
                let right = ctx.sizes() == [(100, 50)];
                ctx.expect(
                    right,
                    "sizes",
                    format!("sizes are {:?}, wanted [(100, 50)]", ctx.sizes()),
                );
                Ok(())
            })
            .run(),
        "allowfail" => harness
            .scenario("only", |ctx| {
                ctx.expect(false, "forced", "forced finding");
                Ok(())
            })
            .run(),
        "noscenario" => harness.run(),
        _ => harness.scenario("only", |_ctx| Ok(())).run(),
    }
}

/// Runs this binary as a child in `mode`; returns its exit code, stdout and stderr.
fn run_child(
    mode: &str,
    args: &[&str],
    extra_env: &[(&str, &str)],
) -> (Option<i32>, String, String) {
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
    command.env(CHILD_FLAG, mode).args(args);
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

fn expect_exit(
    mode: &str,
    (code, stdout, stderr): &(Option<i32>, String, String),
    want_code: i32,
    stdout_has: Option<&str>,
    mismatches: &mut Vec<String>,
) {
    if *code != Some(want_code) {
        mismatches.push(format!(
            "child {mode}: exited with {code:?}, wanted Some({want_code}); stdout: {stdout}; \
             stderr: {stderr}"
        ));
    }
    if let Some(text) = stdout_has
        && !stdout.contains(text)
    {
        mismatches.push(format!(
            "child {mode}: stdout does not contain {text:?}: {stdout}"
        ));
    }
}

/// The seven child modes.
fn child_checks(mismatches: &mut Vec<String>) {
    let result = run_child("sizes", &[], &[("UIPROBE_SIZES", "100x50")]);
    expect_exit("sizes", &result, 0, None, mismatches);

    let dir = report_dir_for("nomatch");
    let marker = dir.join("report.md");
    if let Err(e) = fs::create_dir_all(&dir).and_then(|()| fs::write(&marker, "marker")) {
        mismatches.push(format!("cannot write the marker {}: {e}", marker.display()));
    }
    let result = run_child("nomatch", &["no-such-scenario"], &[]);
    expect_exit(
        "nomatch",
        &result,
        0,
        Some("nothing run, report not written"),
        mismatches,
    );
    let banner = format!(
        "uiprobe {} (Slint {}): 0 of 1 scenario(s) selected, report dir ",
        env!("CARGO_PKG_VERSION"),
        uiprobe::SLINT_VERSION
    );
    if !result.1.contains(&banner) {
        mismatches.push(format!(
            "child nomatch: stdout lacks the banner {banner:?}: {}",
            result.1
        ));
    }
    match fs::read_to_string(&marker) {
        Ok(text) if text == "marker" => {}
        Ok(text) => mismatches.push(format!(
            "child nomatch: the report was overwritten with {text:?}"
        )),
        Err(e) => mismatches.push(format!("child nomatch: the marker is gone: {e}")),
    }

    let result = run_child("envfilter", &[], &[("UIPROBE_FILTER", "zzz")]);
    expect_exit("envfilter", &result, 1, Some("config: 1"), mismatches);

    let result = run_child("allowfail", &[], &[("UIPROBE_ALLOW_FAIL", "1")]);
    expect_exit(
        "allowfail",
        &result,
        0,
        Some("allowed by UIPROBE_ALLOW_FAIL"),
        mismatches,
    );
    if !result.1.contains("[-] forced: forced finding [only]") {
        mismatches.push(format!(
            "child allowfail: stdout does not list the finding line: {}",
            result.1
        ));
    }

    let dir = report_dir_for("formats");
    let stale = dir.join("report.md");
    if let Err(e) = fs::create_dir_all(&dir).and_then(|()| fs::write(&stale, "stale")) {
        mismatches.push(format!("cannot write the stale {}: {e}", stale.display()));
    }
    let result = run_child("formats", &[], &[("UIPROBE_FORMATS", "html,txt")]);
    expect_exit("formats", &result, 0, Some("report.html"), mismatches);
    if stale.exists() {
        mismatches.push("child formats: the stale report.md was not removed".to_owned());
    }
    match fs::read_to_string(dir.join("report.html")) {
        Ok(html) if html.starts_with("<!doctype html>") && html.contains("uiprobe report") => {}
        Ok(html) => mismatches.push(format!(
            "child formats: report.html is not the page: {html}"
        )),
        Err(e) => mismatches.push(format!("child formats: no report.html: {e}")),
    }
    match fs::read_to_string(dir.join("report.txt")) {
        Ok(text) if text.starts_with("uiprobe report\n") => {}
        Ok(text) => mismatches.push(format!("child formats: report.txt is not the text: {text}")),
        Err(e) => mismatches.push(format!("child formats: no report.txt: {e}")),
    }
    if !dir.join("report.json").exists() {
        mismatches.push("child formats: report.json missing".to_owned());
    }

    let dir = report_dir_for("badformat");
    let result = run_child("badformat", &[], &[("UIPROBE_FORMATS", "pdf")]);
    expect_exit("badformat", &result, 1, Some("config: 1"), mismatches);
    if !dir.join("report.md").exists() {
        mismatches.push("child badformat: the default report.md was not written".to_owned());
    }

    let result = run_child("noscenario", &[], &[]);
    expect_exit("noscenario", &result, 1, Some("config: 1"), mismatches);
    if !result.1.contains("no scenarios") {
        mismatches.push(format!(
            "child noscenario: stdout does not name the cause: {}",
            result.1
        ));
    }
}

/// The in-process run: panic, continue, timeout, bad chord, empty window, teardown, report.json.
fn in_process_checks(mismatches: &mut Vec<String>) {
    let dir = uiprobe::target_dir!().join("uiprobe-selftest-runner");
    let stale = dir.join("report.html");
    if let Err(e) = fs::create_dir_all(&dir).and_then(|()| fs::write(&stale, "stale")) {
        mismatches.push(format!("cannot write the stale {}: {e}", stale.display()));
    }
    let continued = Rc::new(Cell::new(false));
    let torn_down = Rc::new(Cell::new(false));
    let continued_flag = Rc::clone(&continued);
    let teardown_flag = Rc::clone(&torn_down);
    let report = Harness::new()
        .report_dir(dir)
        .formats([Format::Markdown, Format::Text])
        .scenario("empty window", |ctx| {
            let _app = ctx.launch(Empty::new)?;
            Ok(())
        })
        .scenario("panics", |ctx| {
            let _app = ctx.launch(Fixture::new)?;
            panic!("scenario boom");
        })
        .scenario("continues", move |_ctx| {
            continued_flag.set(true);
            Ok(())
        })
        .scenario("timeout", |ctx| {
            ctx.wait_for("never", Duration::from_millis(30), || false);
            Ok(())
        })
        .scenario("bad chord", |ctx| {
            let app = ctx.launch(Fixture::new)?;
            ctx.press(&app, "Hyper+X");
            Ok(())
        })
        .teardown(move || teardown_flag.set(true))
        .run_to_report();

    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for finding in report.findings() {
        *counts.entry(finding.check.as_str()).or_insert(0) += 1;
    }
    let want: BTreeMap<&str, usize> = [("scenario-panic", 1), ("timeout", 1), ("bad-chord", 1)]
        .into_iter()
        .collect();
    if counts != want {
        mismatches.push(format!("per-check counts are {counts:?}, wanted {want:?}"));
    }
    if !report
        .findings()
        .iter()
        .any(|f| f.check == "scenario-panic" && f.detail.contains("scenario boom"))
    {
        mismatches.push("no scenario-panic finding names `scenario boom`".to_owned());
    }
    if !continued.get() {
        mismatches.push("the scenario after the panic did not run".to_owned());
    }
    if !torn_down.get() {
        mismatches.push("the teardown did not run".to_owned());
    }
    json_checks(&report, mismatches);
    if report.formats() != [Format::Markdown, Format::Text] {
        mismatches.push(format!(
            "report.formats() is {:?}, wanted [Markdown, Text]",
            report.formats()
        ));
    }
    for file in ["report.md", "report.txt"] {
        if !report.dir().join(file).exists() {
            mismatches.push(format!("the in-process run did not write {file}"));
        }
    }
    if stale.exists() {
        mismatches.push("the in-process run left the stale report.html".to_owned());
    }
}

/// `report.json` parses and has the documented shape.
fn json_checks(report: &Report, mismatches: &mut Vec<String>) {
    let path = report.dir().join("report.json");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => {
            mismatches.push(format!("cannot read {}: {e}", path.display()));
            return;
        }
    };
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(e) => {
            mismatches.push(format!("{} is not valid JSON: {e}", path.display()));
            return;
        }
    };
    if value["per_check"]["scenario-panic"] != 1 {
        mismatches.push(format!(
            "report.json per_check[scenario-panic] is {}, wanted 1",
            value["per_check"]["scenario-panic"]
        ));
    }
    let scenarios = value["scenarios"].as_array().map_or(0, Vec::len);
    if scenarios != 5 {
        mismatches.push(format!("report.json lists {scenarios} scenarios, wanted 5"));
    }
    let keys = ["window", "element", "check", "detail", "state", "repeats"];
    for finding in value["findings"].as_array().into_iter().flatten() {
        for key in keys {
            if finding.get(key).is_none() {
                mismatches.push(format!("a report.json finding lacks the key {key}"));
            }
        }
    }
}

fn parent() -> ExitCode {
    let mut mismatches: Vec<String> = Vec::new();
    child_checks(&mut mismatches);
    in_process_checks(&mut mismatches);
    for mismatch in &mismatches {
        eprintln!("runner: MISMATCH: {mismatch}");
    }
    if mismatches.is_empty() {
        println!("runner: ok");
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
