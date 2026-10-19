//! Collects every problem the harness finds (never first-failure), writes `report.json` and, by
//! default, `report.md` (also `report.html` and `report.txt` on request, see [`Format`]) and
//! decides the exit code.
//!
//! Identical findings (same window, element, check, detail and state) are merged into one with a
//! repeat count, so a layout bug present in forty smoke restores is one line with `(x40)`. Every
//! scenario records how many distinct findings it made and its wall time, so a slow scenario is
//! visible in the report.

use crate::timing::Table;
use std::{
    collections::{BTreeMap, HashMap},
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

/// How many finding lines `Report::finish` prints to the console after the per-check counts,
/// so a CI log shows what failed without opening the report files.
const CONSOLE_FINDINGS: usize = 25;

/// One problem found in the UI.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Finding {
    /// Window the problem is in (`MainWindow`, `Help`, ...), or `-` for run-wide problems.
    pub window: String,
    /// The element: `Role "label" (Component::id)`; empty when the problem is not element-specific.
    pub element: String,
    /// Which check found it (`outside-window`, `zero-size`, `panic`, ...).
    pub check: String,
    /// Human-readable detail with the measured numbers.
    pub detail: String,
    /// Scenario and UI state the finding was made in; filled in by [`Report::add`] when empty.
    pub state: String,
}

impl Finding {
    /// A finding without a state (the report fills it from its current context).
    #[must_use]
    pub fn new(
        window: impl Into<String>,
        element: impl Into<String>,
        check: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            window: window.into(),
            element: element.into(),
            check: check.into(),
            detail: detail.into(),
            state: String::new(),
        }
    }

    /// A run-wide finding (window `-`, no element).
    #[must_use]
    pub fn simple(check: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new("-", "", check, detail)
    }

    /// Identity used to merge repeats of the same problem.
    fn key(&self) -> String {
        format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
            self.window, self.element, self.check, self.detail, self.state
        )
    }
}

/// A report file format besides `report.json`, which every run writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// `report.md`: findings grouped by window and check (the default).
    Markdown,
    /// `report.html`: the same as one self-contained page (inline style, no script, no
    /// external asset), windows collapsible.
    Html,
    /// `report.txt`: the same as plain text for terminals and CI logs.
    Text,
}

impl Format {
    /// Every format, in the order the files are written.
    pub const ALL: [Self; 3] = [Self::Markdown, Self::Html, Self::Text];

    /// The file name in the report directory.
    #[must_use]
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::Markdown => "report.md",
            Self::Html => "report.html",
            Self::Text => "report.txt",
        }
    }

    /// `md`/`markdown`, `html`, `txt`/`text`/`raw` (case-insensitive); `None` otherwise.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "md" | "markdown" => Some(Self::Markdown),
            "html" => Some(Self::Html),
            "txt" | "text" | "raw" => Some(Self::Text),
            _ => None,
        }
    }
}

/// What one scenario cost: distinct findings made, wall time and the timing samples.
pub(crate) struct ScenarioRecord {
    pub(crate) name: String,
    pub(crate) findings: usize,
    pub(crate) duration_ms: u64,
    pub(crate) timing: Table,
}

/// What the coordinator of a parallel run knows about the workers (for the Timing section).
pub(crate) struct ParallelStats {
    /// Worker processes started over the run (replacements included).
    pub(crate) started: usize,
    /// The most workers alive at once.
    pub(crate) peak: usize,
    /// Wall time of the scenario phase.
    pub(crate) wall_ms: u64,
}

/// All findings, informational notes, sections and per-scenario timings of one harness run.
pub struct Report {
    pub(crate) dir: PathBuf,
    pub(crate) formats: Vec<Format>,
    pub(crate) findings: Vec<Finding>,
    pub(crate) repeats: HashMap<String, usize>,
    pub(crate) notes: Vec<String>,
    pub(crate) scenarios: Vec<ScenarioRecord>,
    pub(crate) context: String,
    pub(crate) current_scenario_start: usize,
    pub(crate) current_scenario_began: Option<Instant>,
    pub(crate) sections: Vec<(String, String)>,
    /// Set by the coordinator of a parallel run.
    pub(crate) parallel: Option<ParallelStats>,
}

impl Report {
    /// An empty report that [`Report::finish`] writes into `dir` (`report.json` and, by
    /// default, `report.md`; see [`Report::set_formats`]).
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            formats: vec![Format::Markdown],
            findings: Vec::new(),
            repeats: HashMap::new(),
            notes: Vec::new(),
            scenarios: Vec::new(),
            context: String::new(),
            current_scenario_start: 0,
            current_scenario_began: None,
            sections: Vec::new(),
            parallel: None,
        }
    }

    /// The directory the report files go to.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Sets which format files [`Report::finish`] writes besides `report.json`.
    pub fn set_formats(&mut self, formats: Vec<Format>) {
        self.formats = formats;
    }

    /// The format files [`Report::finish`] writes besides `report.json`.
    #[must_use]
    pub fn formats(&self) -> &[Format] {
        &self.formats
    }

    /// Sets the context (scenario name + UI state) stamped on findings added afterwards.
    pub fn set_context(&mut self, context: impl Into<String>) {
        self.context = context.into();
    }

    /// Marks the start of a scenario (name, start time); [`Report::end_scenario`] records how
    /// many findings it made and how long it took.
    pub fn begin_scenario(&mut self, name: &str) {
        name.clone_into(&mut self.context);
        self.current_scenario_start = self.findings.len();
        self.current_scenario_began = Some(Instant::now());
        println!("uiprobe: scenario {name}");
    }

    /// Closes the scenario opened by [`Report::begin_scenario`]: records its finding count and
    /// its wall time.
    pub fn end_scenario(&mut self, name: &str) {
        let made = self
            .findings
            .len()
            .saturating_sub(self.current_scenario_start);
        let duration_ms = self.current_scenario_began.take().map_or(0, |began| {
            u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX)
        });
        println!("uiprobe:   -> {made} finding(s), {duration_ms} ms");
        self.scenarios.push(ScenarioRecord {
            name: name.to_owned(),
            findings: made,
            duration_ms,
            timing: Table::new(),
        });
    }

    /// Stores the timing samples of the scenario just ended with its record and prints the
    /// `timing:` console line under the `-> N finding(s)` line (nothing is printed for an
    /// empty table).
    pub(crate) fn attach_timing(&mut self, table: Table) {
        if let Some(text) = table.console_text() {
            println!("uiprobe:   timing: {text}");
        }
        if let Some(record) = self.scenarios.last_mut() {
            record.timing = table;
        }
    }

    /// Adds one finding (merging exact repeats into a count).
    pub fn add(&mut self, mut finding: Finding) {
        if finding.state.is_empty() {
            finding.state.clone_from(&self.context);
        }
        let key = finding.key();
        let count = self.repeats.entry(key).or_insert(0);
        *count += 1;
        if *count == 1 {
            self.findings.push(finding);
        }
    }

    /// Adds `finding` as if [`Report::add`] had been called `times` times (a parallel run merges
    /// a worker's already-counted findings this way). `times == 0` adds nothing.
    pub(crate) fn add_repeated(&mut self, mut finding: Finding, times: usize) {
        if times == 0 {
            return;
        }
        if finding.state.is_empty() {
            finding.state.clone_from(&self.context);
        }
        let key = finding.key();
        let count = self.repeats.entry(key).or_insert(0);
        let first = *count == 0;
        *count += times;
        if first {
            self.findings.push(finding);
        }
    }

    /// Adds many findings, appending `state_suffix` (for example the window size) to their state.
    pub fn add_all(&mut self, findings: Vec<Finding>, state_suffix: &str) {
        for mut finding in findings {
            if finding.state.is_empty() {
                finding.state = format!("{} @ {state_suffix}", self.context);
            }
            self.add(finding);
        }
    }

    /// Convenience for a run-wide problem not tied to an element.
    pub fn add_simple(&mut self, window: &str, check: &str, detail: impl Into<String>) {
        self.add(Finding::new(window, "", check, detail));
    }

    /// Records an informational line (not a failure), for example "a click opened a file dialog".
    pub fn note(&mut self, line: impl Into<String>) {
        let line = line.into();
        if !self.notes.contains(&line) {
            self.notes.push(line);
        }
    }

    /// Adds a free-form Markdown section (the coverage gate's inventory counts and lists),
    /// rendered between the scenario list and the findings.
    pub fn add_section(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.sections.push((title.into(), body.into()));
    }

    /// The distinct findings so far, in the order they were first added.
    #[must_use]
    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    /// Number of distinct findings.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.findings.len()
    }

    /// Whether nothing was found.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.findings.is_empty()
    }

    /// Renders the Markdown report: scenarios with timings, sections, findings grouped by window
    /// then by check, notes.
    #[must_use]
    pub fn render_markdown(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# uiprobe report\n");
        let _ = writeln!(
            out,
            "{} distinct finding(s) in {} scenario(s).\n",
            self.findings.len(),
            self.scenarios.len()
        );
        if !self.scenarios.is_empty() {
            let _ = writeln!(out, "## Scenarios\n");
            for scenario in &self.scenarios {
                let _ = writeln!(
                    out,
                    "- `{}`: {} finding(s), {} ms",
                    scenario.name, scenario.findings, scenario.duration_ms
                );
            }
            out.push('\n');
        }
        crate::timing_report::markdown(self, &mut out);
        for (title, body) in &self.sections {
            let _ = writeln!(out, "## {title}\n");
            out.push_str(body);
            out.push('\n');
        }
        let grouped = self.grouped();
        for (window, by_check) in &grouped {
            let _ = writeln!(out, "## {window}\n");
            for (check, findings) in by_check {
                let _ = writeln!(out, "### {check} ({})\n", findings.len());
                for finding in findings {
                    self.render_finding(&mut out, finding);
                }
                out.push('\n');
            }
        }
        if !self.notes.is_empty() {
            let _ = writeln!(out, "## Notes\n");
            for note in &self.notes {
                let _ = writeln!(out, "- {note}");
            }
        }
        out
    }

    fn render_finding(&self, out: &mut String, finding: &Finding) {
        let _ = writeln!(out, "- {}", self.finding_line(finding));
    }

    /// Findings by window, then by check, in `BTreeMap` order; findings in first-added order.
    pub(crate) fn grouped(&self) -> BTreeMap<&str, BTreeMap<&str, Vec<&Finding>>> {
        let mut grouped: BTreeMap<&str, BTreeMap<&str, Vec<&Finding>>> = BTreeMap::new();
        for finding in &self.findings {
            grouped
                .entry(finding.window.as_str())
                .or_default()
                .entry(finding.check.as_str())
                .or_default()
                .push(finding);
        }
        grouped
    }

    /// How often `finding` was added (1 when never repeated).
    #[must_use]
    pub fn repeats_of(&self, finding: &Finding) -> usize {
        self.repeats.get(&finding.key()).copied().unwrap_or(1)
    }

    /// One finding as a line without list markup: `element -- detail (xN) [state]` (the
    /// element part only when there is one, the repeat count only above 1).
    pub(crate) fn finding_line(&self, finding: &Finding) -> String {
        let repeats = self.repeats_of(finding);
        let times = if repeats > 1 {
            format!(" (x{repeats})")
        } else {
            String::new()
        };
        let element = if finding.element.is_empty() {
            String::new()
        } else {
            format!("{} -- ", finding.element)
        };
        format!("{element}{}{times} [{}]", finding.detail, finding.state)
    }

    /// Findings per check name, sorted by name.
    pub(crate) fn per_check(&self) -> BTreeMap<&str, usize> {
        let mut per_check: BTreeMap<&str, usize> = BTreeMap::new();
        for finding in &self.findings {
            *per_check.entry(finding.check.as_str()).or_insert(0) += 1;
        }
        per_check
    }

    /// Renders the JSON report: the `harness` and `slint` versions, scenarios with counts and
    /// `duration_ms`, findings with repeat counts, notes, per-check totals and sections.
    #[must_use]
    pub fn render_json(&self) -> String {
        let findings: Vec<serde_json::Value> = self
            .findings
            .iter()
            .map(|f| {
                serde_json::json!({
                    "window": f.window,
                    "element": f.element,
                    "check": f.check,
                    "detail": f.detail,
                    "state": f.state,
                    "repeats": self.repeats_of(f),
                })
            })
            .collect();
        let scenarios: Vec<serde_json::Value> = self
            .scenarios
            .iter()
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "findings": s.findings,
                    "duration_ms": s.duration_ms,
                    "timing": s.timing.to_report_json(),
                })
            })
            .collect();
        let sections: Vec<serde_json::Value> = self
            .sections
            .iter()
            .map(|(title, body)| serde_json::json!({ "title": title, "body": body }))
            .collect();
        let value = serde_json::json!({
            "harness": env!("CARGO_PKG_VERSION"),
            "slint": crate::SLINT_VERSION,
            "findings": findings,
            "scenarios": scenarios,
            "timing": crate::timing_report::json(self),
            "per_check": self.per_check(),
            "notes": self.notes,
            "sections": sections,
        });
        serde_json::to_string_pretty(&value).unwrap_or_default()
    }

    /// Renders the report in `format` (see also [`Report::render_json`]).
    #[must_use]
    pub fn render(&self, format: Format) -> String {
        match format {
            Format::Markdown => self.render_markdown(),
            Format::Html => self.render_html(),
            Format::Text => self.render_text(),
        }
    }

    /// Renders the self-contained HTML page (one file, inline style, no script, no external
    /// asset; every data string escaped; windows collapsible).
    #[must_use]
    pub fn render_html(&self) -> String {
        crate::render::html(self)
    }

    /// Renders the plain-text report for terminals and CI logs.
    #[must_use]
    pub fn render_text(&self) -> String {
        crate::render::text(self)
    }

    /// Writes `report.json` and one file per selected format ([`Report::formats`]: `report.md`
    /// by default), removes the format files this run did not select, and prints a summary. A
    /// file that cannot be written or removed is a `config` finding (printed, not in the
    /// files), so the run fails.
    pub fn finish(&mut self) {
        let _ = fs::create_dir_all(&self.dir);
        let mut failed: Vec<String> = Vec::new();
        let mut files: Vec<(&'static str, String)> = Vec::new();
        for format in Format::ALL {
            let path = self.dir.join(format.file_name());
            if self.formats.contains(&format) {
                files.push((format.file_name(), self.render(format)));
            } else if path.exists()
                && let Err(e) = fs::remove_file(&path)
            {
                failed.push(format!("cannot remove the stale {}: {e}", path.display()));
            }
        }
        files.push(("report.json", self.render_json()));
        for (file, content) in files {
            let path = self.dir.join(file);
            match fs::write(&path, content) {
                Ok(()) => println!("uiprobe: report written to {}", path.display()),
                Err(e) => failed.push(format!("cannot write {}: {e}", path.display())),
            }
        }
        for text in failed {
            eprintln!("uiprobe: {text}");
            self.add(Finding::simple("config", text));
        }
        if self.findings.is_empty() {
            println!("uiprobe: PASS, no findings");
            return;
        }
        println!("uiprobe: FAIL, {} finding(s):", self.findings.len());
        for (check, count) in self.per_check() {
            println!("uiprobe:   {check}: {count}");
        }
        self.print_findings();
    }

    /// Prints the findings as `[window] check: <report line>`, grouped as the report is, up to
    /// `CONSOLE_FINDINGS`, then how many more the files hold. A multi-line detail is joined
    /// with ` / ` so one finding stays one line.
    fn print_findings(&self) {
        let mut printed = 0;
        for (window, by_check) in &self.grouped() {
            for (check, findings) in by_check {
                for finding in findings {
                    if printed == CONSOLE_FINDINGS {
                        println!(
                            "uiprobe:   ... {} more in {}",
                            self.findings.len() - printed,
                            self.dir.display()
                        );
                        return;
                    }
                    let line = self.finding_line(finding).replace('\n', " / ");
                    println!("uiprobe:   [{window}] {check}: {line}");
                    printed += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The report every renderer test uses: two windows, a repeat, a section, a note, and a
    /// detail with characters HTML must escape.
    fn sample() -> Report {
        let mut report = Report::new("unused");
        report.set_context("s1");
        report.add(Finding::new("W", "Button \"A\"", "overlap", "a <b> & c"));
        report.add(Finding::new("W", "Button \"A\"", "overlap", "a <b> & c"));
        report.add(Finding::simple("timeout", "never"));
        report.add_section("Coverage", "- line\n");
        report.note("note one");
        report
    }

    const SAMPLE_MARKDOWN: &str = "# uiprobe report\n\n2 distinct finding(s) in 0 scenario(s).\n\n## Coverage\n\n- line\n\n## -\n\n### timeout (1)\n\n- never [s1]\n\n## W\n\n### overlap (1)\n\n- Button \"A\" -- a <b> & c (x2) [s1]\n\n## Notes\n\n- note one\n";

    #[test]
    fn markdown_is_byte_identical_to_the_pinned_text() {
        assert_eq!(sample().render_markdown(), SAMPLE_MARKDOWN);
    }

    #[test]
    fn finish_writes_the_selected_formats_and_removes_the_others() {
        let dir = std::env::temp_dir().join(format!("uiprobe-formats-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("the report dir is created");
        fs::write(dir.join("report.md"), "stale").expect("the stale file is written");
        let mut report = sample();
        report.dir = dir.clone();
        report.set_formats(vec![Format::Html, Format::Text]);
        report.finish();
        let stale_gone = !dir.join("report.md").exists();
        let html = fs::read_to_string(dir.join("report.html"));
        let text = fs::read_to_string(dir.join("report.txt"));
        let json = fs::read_to_string(dir.join("report.json"));
        let _ = fs::remove_dir_all(&dir);
        assert!(stale_gone, "the stale report.md is removed");
        assert_eq!(html.unwrap(), report.render_html());
        assert_eq!(text.unwrap(), report.render_text());
        assert_eq!(json.unwrap(), report.render_json());
        assert!(
            report.findings().iter().all(|f| f.check != "config"),
            "{:?}",
            report.findings()
        );
    }

    #[test]
    fn add_merges_repeats() {
        let mut report = Report::new("unused");
        report.add(Finding::simple("x", "d"));
        report.add(Finding::simple("x", "d"));
        assert_eq!(report.len(), 1);
        assert!(report.render_markdown().contains("(x2)"));
        let value: serde_json::Value =
            serde_json::from_str(&report.render_json()).expect("the report is JSON");
        assert_eq!(value["findings"][0]["repeats"], 2);
        assert_eq!(value["per_check"]["x"], 1);
        assert_eq!(value["harness"], env!("CARGO_PKG_VERSION"));
        assert_eq!(value["slint"], crate::SLINT_VERSION);
    }

    #[test]
    fn add_all_appends_the_suffix() {
        let mut report = Report::new("unused");
        report.set_context("s");
        report.add_all(vec![Finding::simple("x", "d")], "W 300x100 state");
        assert_eq!(report.findings()[0].state, "s @ W 300x100 state");
    }

    #[test]
    fn unwritable_dir_is_a_config_finding() {
        let blocker =
            std::env::temp_dir().join(format!("uiprobe-report-test-{}", std::process::id()));
        fs::write(&blocker, "file").expect("the blocking file is written");
        let mut report = Report::new(blocker.join("sub"));
        report.finish();
        let _ = fs::remove_file(&blocker);
        assert!(
            report
                .findings()
                .iter()
                .any(|f| f.check == "config" && f.detail.contains("cannot write")),
            "findings: {:?}",
            report.findings()
        );
    }
}
