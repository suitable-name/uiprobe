//! Parallel scenario runs: the COORDINATOR half (the worker half is in `parallel/worker.rs`).
//!
//! With `Jobs::Fixed(n)` or `Jobs::Auto` the test binary starts copies of itself as worker
//! processes (threads are impossible: Slint's platform is per process and bound to one thread,
//! the profile redirect is a process-wide `set_var`, the hooks, the panic recorder and the abort
//! request are process statics). The coordinator creates no profile and no backend; it
//! schedules scenarios, replays the console output in scenario order and merges the results into
//! ONE report that is identical to a serial run's (findings order, repeat counts, per-check
//! counts, scenario list) apart from one extra note.
//!
//! Protocol: one JSON message per line. Coordinator to worker on stdin: `{"run": <scenario
//! index>, "name": "<name>"}` or `{"quit": true}` (stdin EOF also means quit). Worker to
//! coordinator on stdout: lines holding the marker `MARKER` followed by JSON (`ready`, `done`,
//! `bye`, `abort`; `done` carries the scenario's `timing` table and the worker's coverage
//! `recorder` snapshot, which the coordinator unions); every other line is console output that the coordinator replays. Before each
//! marker line the worker writes a newline, so app output without a trailing newline cannot
//! swallow the marker; the coordinator drops that separator again and finds the marker anywhere
//! in a line.
//!
//! "Last" scenarios (`Harness::scenario_last`) are queued apart and handed out only when every
//! ordinary scenario has a result; their `run` command also carries `"coverage"`, the union of
//! the recorders received so far, which the worker merges before running. A worker with nothing
//! to do meanwhile waits instead of quitting.
//!
//! The run index (`UIPROBE_WORKER`) tells a worker which `run_to_report` call of the
//! re-executed `main` is the parallel one.

mod worker;

pub use worker::{WorkerSetup, run_worker};

use crate::{
    coverage::Coverage,
    cpu::CpuSampler,
    report::{Finding, ParallelStats, Report, ScenarioRecord},
    runner::Plan,
    timing::Table,
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    time::{Duration, Instant},
};

/// Starts every worker-to-coordinator protocol line.
pub const MARKER: &str = "\u{1e}uiprobe-worker ";
/// The environment variable that makes a process a worker (internal; its value is the index of
/// the `run_to_report` call that is the parallel run).
const WORKER_ENV: &str = "UIPROBE_WORKER";
/// How often `Jobs::Auto` looks at the CPU.
const GROW_EVERY: Duration = Duration::from_secs(5);
/// Idle cores needed to add a worker.
const MIN_IDLE_CORES: f64 = 1.5;
/// Workers that die before `ready` this many times in a row end the run.
const MAX_EARLY_DEATHS: usize = 3;
/// Output lines kept per worker for a crash finding.
const TAIL_LINES: usize = 10;

/// The run index this process was told to serve, if it is a worker.
pub fn worker_run_index() -> Option<usize> {
    std::env::var(WORKER_ENV).ok()?.trim().parse().ok()
}

/// Sends one protocol message to the coordinator (worker side).
pub fn send_message(value: &Value) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(b"\n");
    let _ = out.flush();
    let _ = writeln!(out, "{MARKER}{value}");
    let _ = out.flush();
}

/// Whether `Jobs::Auto` adds a worker now: scenarios wait, the cap is not reached and at least
/// 1.5 cores were idle over the last interval (`None`, not measurable, means grow by one per
/// interval).
pub fn should_grow(idle_cores: Option<f64>, running: usize, max: usize, queued: usize) -> bool {
    queued > 0 && running < max && idle_cores.is_none_or(|idle| idle >= MIN_IDLE_CORES)
}

/// Everything the coordinator needs from `Harness::run_to_report`.
pub struct CoordinatorSetup {
    /// The report so far (config findings); the merged results go into it.
    pub(crate) report: Report,
    /// The index workers are told to serve.
    pub(crate) run_index: usize,
    pub(crate) plan: Plan,
    /// All scenario names, in order.
    pub(crate) names: Vec<String>,
    /// Indices into `names` of the selected scenarios.
    pub(crate) chosen: Vec<usize>,
    /// How many of `names` are ordinary scenarios; the rest (from this index on) are the
    /// "last" scenarios, dispatched only when every ordinary one is done.
    pub(crate) ordinary: usize,
    pub(crate) coverage: Option<Coverage>,
    pub(crate) manifest_dir: PathBuf,
    /// Whether a scenario filter is active.
    pub(crate) filtered: bool,
}

/// What a scenario produced, as the worker reported it (or as a crash made it).
struct Outcome {
    findings: Vec<(Finding, usize)>,
    notes: Vec<String>,
    duration_ms: u64,
    made: usize,
    timing: Table,
}

/// One selected scenario: its buffered console output and, once known, its result.
struct Slot {
    index: usize,
    name: String,
    /// A `scenario_last` scenario.
    last: bool,
    began: Option<Instant>,
    lines: Vec<(bool, String)>,
    result: Option<Outcome>,
}

/// One worker process as the coordinator sees it.
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent lifecycle flags set by separate worker messages, not exclusive states"
)]
struct Worker {
    child: Child,
    stdin: Option<ChildStdin>,
    open_streams: usize,
    current: Option<usize>,
    ready: bool,
    quit_sent: bool,
    bye: bool,
    aborted: bool,
    exited: bool,
    tail: VecDeque<String>,
}

/// A line or the end of one of a worker's pipes.
enum Event {
    Line {
        worker: usize,
        err: bool,
        text: String,
    },
    Closed {
        worker: usize,
    },
}

struct Coordinator {
    run_index: usize,
    plan: Plan,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    workers: Vec<Worker>,
    slots: Vec<Slot>,
    /// Ordinary scenarios not yet handed to a worker.
    queue: VecDeque<usize>,
    /// "Last" scenarios not yet handed out; they wait for `ordinary_pending` to reach 0.
    last_queue: VecDeque<usize>,
    /// Ordinary scenarios without a result yet (queued or running).
    ordinary_pending: usize,
    started_at: Instant,
    next_print: usize,
    report: Report,
    started: usize,
    peak: usize,
    early_deaths: usize,
    abort: Option<String>,
    target: usize,
    last_check: Instant,
    sampler: Option<CpuSampler>,
}

/// Runs the selected scenarios in worker processes and returns the merged, finished report
/// (coverage gate included). Ends the process with exit code 2 when a worker aborts.
pub fn coordinate(setup: CoordinatorSetup) -> Report {
    let CoordinatorSetup {
        report,
        run_index,
        plan,
        names,
        chosen,
        ordinary,
        coverage,
        manifest_dir,
        filtered,
    } = setup;
    println!(
        "uiprobe: parallel run: {}, up to {} worker(s)",
        plan.label(),
        plan.cap
    );
    let (tx, rx) = mpsc::channel();
    let slots: Vec<Slot> = chosen
        .iter()
        .map(|&index| Slot {
            index,
            name: names[index].clone(),
            last: index >= ordinary,
            began: None,
            lines: Vec::new(),
            result: None,
        })
        .collect();
    let queue: VecDeque<usize> = (0..slots.len()).filter(|&i| !slots[i].last).collect();
    let last_queue: VecDeque<usize> = (0..slots.len()).filter(|&i| slots[i].last).collect();
    let ordinary_pending = queue.len();
    let mut coordinator = Coordinator {
        run_index,
        plan,
        tx,
        rx,
        workers: Vec::new(),
        slots,
        queue,
        last_queue,
        ordinary_pending,
        started_at: Instant::now(),
        next_print: 0,
        report,
        started: 0,
        peak: 0,
        early_deaths: 0,
        abort: None,
        target: plan.initial,
        last_check: Instant::now(),
        sampler: plan.auto.then(CpuSampler::new),
    };
    for _ in 0..plan.initial {
        coordinator.start_worker();
    }
    coordinator.run_loop();
    coordinator.finish(coverage.as_ref(), &manifest_dir, filtered)
}

impl Coordinator {
    /// Workers that are alive and have not been told to quit.
    fn live(&self) -> usize {
        self.workers
            .iter()
            .filter(|w| !w.exited && !w.quit_sent)
            .count()
    }

    fn start_worker(&mut self) {
        let id = self.workers.len();
        let spawned = std::env::current_exe().and_then(|exe| {
            Command::new(exe)
                .args(std::env::args_os().skip(1))
                .env(WORKER_ENV, self.run_index.to_string())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
        });
        let mut child = match spawned {
            Ok(child) => child,
            Err(e) => crate::profile::abort_run(&format!("cannot start worker {}: {e}", id + 1)),
        };
        let stdin = child.stdin.take();
        let mut open_streams = 0;
        if let Some(out) = child.stdout.take() {
            spawn_reader(out, id, false, self.tx.clone());
            open_streams += 1;
        }
        if let Some(err) = child.stderr.take() {
            spawn_reader(err, id, true, self.tx.clone());
            open_streams += 1;
        }
        self.workers.push(Worker {
            child,
            stdin,
            open_streams,
            current: None,
            ready: false,
            quit_sent: false,
            bye: false,
            aborted: false,
            exited: false,
            tail: VecDeque::new(),
        });
        self.started += 1;
        let live = self.live();
        self.peak = self.peak.max(live);
        println!("uiprobe: worker {} started ({live} running)", id + 1);
    }

    fn finished(&self) -> bool {
        self.workers.iter().all(|w| w.exited)
            && (self.abort.is_some() || self.next_print >= self.slots.len())
    }

    fn run_loop(&mut self) {
        while !self.finished() {
            match self.rx.recv_timeout(Duration::from_millis(250)) {
                Ok(Event::Line { worker, err, text }) => self.on_line(worker, err, text),
                Ok(Event::Closed { worker }) => {
                    self.workers[worker].open_streams =
                        self.workers[worker].open_streams.saturating_sub(1);
                    if self.workers[worker].open_streams == 0 {
                        self.on_exit(worker);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.grow_if_due();
            self.check_stall();
        }
    }

    /// With every worker gone and scenarios unfinished: start a worker when some are queued,
    /// else end the run (nothing can finish them).
    fn check_stall(&mut self) {
        let unfinished = self.next_print < self.slots.len();
        if self.abort.is_some() || !unfinished || self.workers.iter().any(|w| !w.exited) {
            return;
        }
        if self.queue.is_empty() && self.last_queue.is_empty() {
            self.begin_abort("every worker exited with scenarios unfinished".to_owned());
        } else {
            self.start_worker();
        }
    }

    /// Scenarios a worker could start right now: the ordinary queue, plus the "last" queue once
    /// every ordinary scenario has a result.
    fn dispatchable(&self) -> usize {
        let last = if self.ordinary_pending == 0 {
            self.last_queue.len()
        } else {
            0
        };
        self.queue.len() + last
    }

    /// The next scenario for a free worker: an ordinary one, else a "last" one when no ordinary
    /// scenario is still running or queued.
    fn next_slot(&mut self) -> Option<usize> {
        if let Some(slot) = self.queue.pop_front() {
            return Some(slot);
        }
        if self.ordinary_pending == 0 {
            return self.last_queue.pop_front();
        }
        None
    }

    fn grow_if_due(&mut self) {
        if !self.plan.auto || self.abort.is_some() || self.last_check.elapsed() < GROW_EVERY {
            return;
        }
        self.last_check = Instant::now();
        let idle = self.sampler.as_mut().and_then(CpuSampler::idle_cores);
        if should_grow(idle, self.live(), self.plan.cap, self.dispatchable()) {
            self.target = self.live() + 1;
            self.start_worker();
        }
    }

    /// Starts a replacement worker when scenarios are queued and fewer workers live than wanted.
    fn replace_if_needed(&mut self) {
        if self.abort.is_none() && self.dispatchable() > 0 && self.live() < self.target {
            self.start_worker();
        }
    }

    fn on_line(&mut self, w: usize, err: bool, text: String) {
        if !err && let Some(at) = text.find(MARKER) {
            let before = &text[..at];
            if !before.is_empty() {
                self.console(w, false, before.to_owned());
            }
            match serde_json::from_str::<Value>(&text[at + MARKER.len()..]) {
                Ok(message) => self.on_message(w, &message),
                Err(_) => self.console(w, false, text),
            }
            return;
        }
        self.console(w, err, text);
    }

    /// Console output of worker `w`: buffered with its running scenario, else printed at once
    /// with a worker prefix.
    fn console(&mut self, w: usize, err: bool, text: String) {
        let worker = &mut self.workers[w];
        worker.tail.push_back(text.clone());
        if worker.tail.len() > TAIL_LINES {
            worker.tail.pop_front();
        }
        if let Some(slot) = worker.current {
            self.slots[slot].lines.push((err, text));
        } else {
            let line = format!("uiprobe: worker {}: {text}", w + 1);
            if err {
                eprintln!("{line}");
            } else {
                println!("{line}");
            }
        }
    }

    fn on_message(&mut self, w: usize, message: &Value) {
        if message.get("ready").is_some() {
            self.workers[w].ready = true;
            self.early_deaths = 0;
            self.dispatch(w);
        } else if message.get("done").is_some() {
            // The worker's recorder so far goes into the union the "last" scenarios receive.
            if let Some(recorder) = message.get("recorder") {
                crate::coverage::recorder::merge_snapshot(recorder);
            }
            if let Some(slot) = self.workers[w].current.take() {
                self.set_result(slot, parse_outcome(message));
            }
            self.dispatch(w);
        } else if message.get("bye").is_some() {
            if let Some(recorder) = message.get("recorder") {
                crate::coverage::recorder::merge_snapshot(recorder);
            }
            if message.get("armed").and_then(Value::as_bool) == Some(true) {
                crate::coverage::set_armed(true);
            }
            self.workers[w].bye = true;
        } else if let Some(text) = message.get("abort").and_then(Value::as_str) {
            self.workers[w].aborted = true;
            self.begin_abort(text.to_owned());
        }
    }

    /// Stores the result of `slot`, prints what is ready, and, when that was the last ordinary
    /// scenario, wakes the idle workers for the "last" ones.
    fn set_result(&mut self, slot: usize, outcome: Outcome) {
        self.slots[slot].result = Some(outcome);
        if !self.slots[slot].last {
            self.ordinary_pending = self.ordinary_pending.saturating_sub(1);
        }
        self.flush_ready();
        if self.ordinary_pending == 0 && !self.last_queue.is_empty() {
            for w in 0..self.workers.len() {
                let worker = &self.workers[w];
                if worker.ready && worker.current.is_none() {
                    self.dispatch(w);
                }
            }
        }
    }

    /// Gives worker `w` the next scenario. With none to give it waits (a "last" scenario is
    /// queued behind ordinary ones still running) or is told to quit (nothing is left).
    fn dispatch(&mut self, w: usize) {
        if self.workers[w].exited || self.workers[w].quit_sent || self.workers[w].current.is_some()
        {
            return;
        }
        if self.abort.is_none() {
            if let Some(slot) = self.next_slot() {
                let mut message =
                    json!({ "run": self.slots[slot].index, "name": self.slots[slot].name });
                if self.slots[slot].last {
                    message["coverage"] = crate::coverage::recorder::snapshot();
                }
                self.workers[w].current = Some(slot);
                self.slots[slot].began = Some(Instant::now());
                if !self.send(w, &message) {
                    self.workers[w].current = None;
                    if self.slots[slot].last {
                        self.last_queue.push_front(slot);
                    } else {
                        self.queue.push_front(slot);
                    }
                }
                return;
            }
            if self.ordinary_pending > 0 && !self.last_queue.is_empty() {
                return;
            }
        }
        self.quit(w);
    }

    fn send(&mut self, w: usize, value: &Value) -> bool {
        let Some(stdin) = self.workers[w].stdin.as_mut() else {
            return false;
        };
        writeln!(stdin, "{value}")
            .and_then(|()| stdin.flush())
            .is_ok()
    }

    fn quit(&mut self, w: usize) {
        if self.workers[w].quit_sent {
            return;
        }
        let _ = self.send(w, &json!({ "quit": true }));
        self.workers[w].quit_sent = true;
        self.workers[w].stdin = None;
    }

    /// Ends the run after the workers finish their current scenario: nothing more is queued,
    /// every other worker is told to quit; the process exits with 2 once all are gone.
    fn begin_abort(&mut self, message: String) {
        if self.abort.is_some() {
            return;
        }
        self.abort = Some(message);
        self.queue.clear();
        self.last_queue.clear();
        for w in 0..self.workers.len() {
            if !self.workers[w].exited && !self.workers[w].aborted {
                self.quit(w);
            }
        }
    }

    /// Both pipes of worker `w` are closed: reap it and decide whether that was a crash.
    fn on_exit(&mut self, w: usize) {
        let status = match self.workers[w].child.wait() {
            Ok(status) => status.to_string(),
            Err(e) => format!("an unknown status ({e})"),
        };
        let worker = &mut self.workers[w];
        worker.exited = true;
        worker.stdin = None;
        let current = worker.current.take();
        let (done, ready) = (worker.bye || worker.aborted, worker.ready);
        let tail = if worker.tail.is_empty() {
            "(none)".to_owned()
        } else {
            worker.tail.iter().cloned().collect::<Vec<_>>().join(" | ")
        };
        if done {
            return;
        }
        eprintln!(
            "uiprobe: worker {} exited with {status} without finishing normally",
            w + 1
        );
        if let Some(slot) = current {
            self.crashed(slot, &status, &tail);
        } else if !ready {
            self.early_deaths += 1;
            if self.early_deaths >= MAX_EARLY_DEATHS {
                self.begin_abort(format!(
                    "worker processes exited with {status} before they were ready \
                     {MAX_EARLY_DEATHS} times in a row (does the test's main build the same \
                     harnesses in every process?); last output lines: {tail}"
                ));
                return;
            }
        }
        self.replace_if_needed();
    }

    /// The worker running `slot` died: a `worker-crash` finding stands in for its result.
    fn crashed(&mut self, slot: usize, status: &str, tail: &str) {
        let name = self.slots[slot].name.clone();
        let duration_ms = self.slots[slot].began.map_or(0, |began| {
            u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX)
        });
        let detail = format!(
            "{name}: worker exited with {status} during the scenario; its last output lines: \
             {tail}; its profile directory may have been left behind in the temp dir"
        );
        self.set_result(
            slot,
            Outcome {
                findings: vec![(Finding::simple("worker-crash", detail), 1)],
                notes: Vec::new(),
                duration_ms,
                made: 1,
                timing: Table::new(),
            },
        );
    }

    /// Prints and merges every finished scenario whose predecessors are all printed.
    fn flush_ready(&mut self) {
        while self.next_print < self.slots.len() {
            let slot = &mut self.slots[self.next_print];
            let Some(outcome) = slot.result.take() else {
                break;
            };
            print_lines(&mut slot.lines);
            merge(&mut self.report, &slot.name, outcome);
            self.next_print += 1;
        }
    }

    /// After the loop: the abort exit, or the merged report with the coverage gate.
    fn finish(
        mut self,
        coverage: Option<&Coverage>,
        manifest_dir: &Path,
        filtered: bool,
    ) -> Report {
        if let Some(message) = self.abort.take() {
            for slot in &mut self.slots[self.next_print..] {
                print_lines(&mut slot.lines);
            }
            crate::profile::abort_run(&message);
        }
        self.report.note(format!(
            "parallel run: {} worker(s) used, peak {} at once",
            self.started, self.peak
        ));
        self.report.parallel = Some(ParallelStats {
            started: self.started,
            peak: self.peak,
            wall_ms: u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
        });
        if let Some(coverage) = coverage {
            self.report.set_context("coverage");
            crate::coverage::finish(coverage, manifest_dir, filtered, &mut self.report);
        }
        self.report.finish();
        self.report
    }
}

/// Prints buffered scenario output (stdout lines to stdout, the rest to stderr) and empties it.
fn print_lines(lines: &mut Vec<(bool, String)>) {
    for (err, line) in lines.drain(..) {
        if err {
            eprintln!("{line}");
        } else {
            println!("{line}");
        }
    }
}

/// Adds one scenario's result to the report the way a serial run would have: findings with
/// their repeat counts, notes, and the scenario record with the worker's numbers.
fn merge(report: &mut Report, name: &str, outcome: Outcome) {
    report.set_context(name);
    for (finding, times) in outcome.findings {
        report.add_repeated(finding, times);
    }
    for note in outcome.notes {
        report.note(note);
    }
    report.scenarios.push(ScenarioRecord {
        name: name.to_owned(),
        findings: outcome.made,
        duration_ms: outcome.duration_ms,
        timing: outcome.timing,
    });
}

/// Reads a worker's `done` message.
fn parse_outcome(message: &Value) -> Outcome {
    let text = |value: &Value, key: &str| -> String {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let number = |value: &Value, key: &str| -> Option<u64> { value.get(key)?.as_u64() };
    let findings = message
        .get("findings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|f| {
            let finding = Finding {
                window: text(f, "window"),
                element: text(f, "element"),
                check: text(f, "check"),
                detail: text(f, "detail"),
                state: text(f, "state"),
            };
            let times = number(f, "repeats").and_then(|n| usize::try_from(n).ok());
            (finding, times.unwrap_or(1))
        })
        .collect();
    let notes = message
        .get("notes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    Outcome {
        findings,
        notes,
        duration_ms: number(message, "duration_ms").unwrap_or(0),
        made: number(message, "made")
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0),
        timing: Table::from_json(message.get("timing")),
    }
}

/// Reads `reader` line by line on its own thread and sends every line to the coordinator, then
/// `Closed`. On stdout the separator newline the worker writes before each marker line is
/// dropped again (one blank line directly before a marker line).
fn spawn_reader<R: Read + Send + 'static>(reader: R, worker: usize, err: bool, tx: Sender<Event>) {
    std::thread::spawn(move || {
        let emit = |text: String| tx.send(Event::Line { worker, err, text }).is_ok();
        let mut blanks = 0_usize;
        for raw in BufReader::new(reader).split(b'\n') {
            let Ok(bytes) = raw else { break };
            let mut text = String::from_utf8_lossy(&bytes).into_owned();
            if text.ends_with('\r') {
                text.pop();
            }
            if !err {
                if text.is_empty() {
                    blanks += 1;
                    continue;
                }
                if text.contains(MARKER) {
                    blanks = blanks.saturating_sub(1);
                }
                for _ in 0..std::mem::take(&mut blanks) {
                    if !emit(String::new()) {
                        return;
                    }
                }
            }
            if !emit(text) {
                return;
            }
        }
        for _ in 0..blanks {
            if !emit(String::new()) {
                return;
            }
        }
        let _ = tx.send(Event::Closed { worker });
    });
}

#[cfg(test)]
mod tests {
    use super::{parse_outcome, should_grow};
    use serde_json::json;

    #[test]
    fn grows_only_with_waiting_work_room_and_idle_cores() {
        assert!(should_grow(Some(2.0), 1, 4, 3));
        assert!(should_grow(Some(1.5), 1, 4, 1));
        assert!(!should_grow(Some(1.4), 1, 4, 3), "not enough idle cores");
        assert!(!should_grow(Some(8.0), 4, 4, 3), "at the cap");
        assert!(!should_grow(Some(8.0), 1, 4, 0), "nothing is waiting");
    }

    #[test]
    fn an_unmeasurable_cpu_grows_one_per_interval() {
        assert!(should_grow(None, 1, 4, 2));
        assert!(!should_grow(None, 4, 4, 2));
        assert!(!should_grow(None, 1, 4, 0));
    }

    #[test]
    fn done_messages_round_trip() {
        let message = json!({
            "done": 3,
            "findings": [{
                "window": "W", "element": "E", "check": "overlap",
                "detail": "d", "state": "s", "repeats": 4
            }],
            "notes": ["n"],
            "duration_ms": 12,
            "made": 1,
            "timing": {"wait": {"count": 2, "total_us": 5, "timeouts": 1, "timeout_us": 3}},
        });
        let outcome = parse_outcome(&message);
        let wait = outcome.timing.get("wait").copied().unwrap_or_default();
        assert_eq!(
            (wait.count, wait.total_us, wait.timeouts, wait.timeout_us),
            (2, 5, 1, 3)
        );
        assert_eq!(outcome.findings.len(), 1);
        let (finding, times) = &outcome.findings[0];
        assert_eq!(
            (finding.window.as_str(), finding.check.as_str(), *times),
            ("W", "overlap", 4)
        );
        assert_eq!(finding.state, "s");
        assert_eq!(outcome.notes, ["n"]);
        assert_eq!((outcome.duration_ms, outcome.made), (12, 1));
    }
}
