//! The WORKER half of a parallel run: a process that runs scenarios on the coordinator's
//! orders.
//!
//! A worker follows the serial run's order (profile, testing backend, panic recorder, sizes,
//! seed), then reads one command per line from stdin and answers on stdout through
//! `send_message`. Each scenario gets a FRESH report, so the numbers it sends (distinct
//! findings, repeats, notes, wall time) are that scenario's own, and goes through the same
//! `run_scenario` step as the serial loop. On quit (or stdin EOF) it runs the teardown, sends
//! its coverage recorder, deletes its profile and exits.

use super::send_message;
use crate::{
    backend::{Renderer, init_backend_with},
    checks::SweepOptions,
    panics::install_panic_recorder,
    profile::Profile,
    report::{Finding, Report},
    runner::{ScenarioFn, StepEnv, resolve_seed, resolve_sizes, run_scenario},
    timing::Table,
};
use serde_json::{Value, json};
use std::{
    io::BufRead,
    path::{Path, PathBuf},
};

/// What `Harness::run_to_report` hands over when this process is the worker.
pub struct WorkerSetup {
    pub(crate) report_dir: PathBuf,
    pub(crate) sizes: Vec<(u32, u32)>,
    pub(crate) profile: Option<Profile>,
    pub(crate) sweep_options: SweepOptions,
    pub(crate) scenarios: Vec<(String, ScenarioFn)>,
    pub(crate) teardown: Option<Box<dyn FnOnce()>>,
    pub(crate) renderer: Renderer,
}

/// How one command ended.
enum Step {
    /// The `done` message to send.
    Done(Value),
    /// The scenario (or the harness) asked to abort the run.
    Abort(String),
}

/// Serves the coordinator until it says quit, then ends the process: exit code 0 after a clean
/// quit, 2 after an abort (the profile is deleted first in both cases).
pub fn run_worker(setup: WorkerSetup) -> ! {
    let WorkerSetup {
        report_dir,
        sizes,
        profile,
        sweep_options,
        mut scenarios,
        mut teardown,
        renderer,
    } = setup;
    let profile = profile.unwrap_or_else(Profile::isolated).create();
    if let Err(message) = init_backend_with(renderer) {
        send_message(&json!({ "abort": message }));
        profile.finish();
        std::process::exit(2);
    }
    install_panic_recorder();
    // The coordinator reports config findings about sizes; the worker only needs the value.
    let sizes = resolve_sizes(sizes, &mut Report::new(report_dir.clone()));
    let seed = resolve_seed();
    send_message(&json!({ "ready": true }));

    let stdin = std::io::stdin();
    for line in stdin.lock().lines().map_while(Result::ok) {
        let Ok(command) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if command.get("quit").is_some() {
            break;
        }
        let Some(index) = command
            .get("run")
            .and_then(Value::as_u64)
            .and_then(|index| usize::try_from(index).ok())
        else {
            continue;
        };
        // A "last" scenario comes with the union of every worker's coverage recorder.
        if let Some(union) = command.get("coverage") {
            crate::coverage::recorder::merge_snapshot(union);
        }
        let name = command
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let env = StepEnv {
            profile: &profile,
            sizes: &sizes,
            sweep_options: &sweep_options,
            seed,
        };
        match run_one(&mut scenarios, index, name, &report_dir, &env) {
            Step::Done(message) => send_message(&message),
            Step::Abort(message) => {
                if let Some(teardown) = teardown.take() {
                    teardown();
                }
                profile.finish();
                send_message(&json!({ "abort": message }));
                std::process::exit(2);
            }
        }
    }

    if let Some(teardown) = teardown.take() {
        teardown();
    }
    let recorder = crate::coverage::recorder::snapshot();
    let armed = crate::coverage::is_armed();
    profile.finish();
    send_message(&json!({ "bye": true, "recorder": recorder, "armed": armed }));
    std::process::exit(0);
}

/// Runs scenario `index` (named `name` by the coordinator) with a fresh report. A name that
/// differs from this process's own list is a `config` finding and the scenario is not run.
fn run_one(
    scenarios: &mut [(String, ScenarioFn)],
    index: usize,
    name: &str,
    report_dir: &Path,
    env: &StepEnv<'_>,
) -> Step {
    let mut report = Report::new(report_dir);
    let own = scenarios.get(index).map(|(own, _)| own.clone());
    if own.as_deref() == Some(name) {
        let run = &mut scenarios[index].1;
        if let Some(message) = run_scenario(name, run, &mut report, env) {
            return Step::Abort(message);
        }
    } else {
        report.set_context(name);
        report.add(Finding::simple(
            "config",
            format!(
                "scenario {index} is {own:?} in a worker but {name:?} in the coordinator: the \
                 test's main builds different scenario lists in different processes; scenario \
                 not run"
            ),
        ));
    }
    Step::Done(done_message(index, &report))
}

/// The `done` message: the scenario's distinct findings with repeat counts, its notes, wall time,
/// the number of findings it made, its timing samples and the worker's coverage recorder so far
/// (the coordinator unions the recorders to hand them to the "last" scenarios).
fn done_message(index: usize, report: &Report) -> Value {
    let findings: Vec<Value> = report
        .findings
        .iter()
        .map(|f| {
            json!({
                "window": f.window,
                "element": f.element,
                "check": f.check,
                "detail": f.detail,
                "state": f.state,
                "repeats": report.repeats_of(f),
            })
        })
        .collect();
    let (made, duration_ms) = report
        .scenarios
        .last()
        .map_or((report.findings.len(), 0), |s| (s.findings, s.duration_ms));
    let timing = report
        .scenarios
        .last()
        .map_or_else(|| Table::new().to_json(), |s| s.timing.to_json());
    json!({
        "done": index,
        "findings": findings,
        "notes": report.notes,
        "duration_ms": duration_ms,
        "made": made,
        "timing": timing,
        "recorder": crate::coverage::recorder::snapshot(),
    })
}
