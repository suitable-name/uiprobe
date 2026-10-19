//! The "Timing" section of the report files: the slowest scenarios with their biggest labels,
//! the totals per label over the run, the timeouts per label and, for a parallel run, the worker
//! count and how busy the workers were. The Markdown is the reference; the HTML and the text
//! carry the same rows. The section is left out when the report has no scenarios.

use crate::{
    report::Report,
    timing::{Table, format_us},
};
use serde_json::{Value, json};
use std::fmt::Write as _;

/// How many scenarios the section lists (the slowest by wall time).
const SLOWEST: usize = 15;
/// How many labels each listed scenario names.
const TOP_LABELS: usize = 3;

/// A scenario row: name, wall time, the biggest labels as `label time, ...`.
pub struct ScenarioRow {
    pub(crate) name: String,
    pub(crate) time: String,
    pub(crate) top: String,
}

/// A label row: label, total time, samples.
pub struct LabelRow {
    pub(crate) label: String,
    pub(crate) time: String,
    pub(crate) count: u64,
}

/// A timeout row: label, how many samples gave up, the time those took.
pub struct TimeoutRow {
    pub(crate) label: String,
    pub(crate) count: u64,
    pub(crate) time: String,
}

/// Everything the three renderers show.
pub struct Summary {
    pub(crate) scenarios: Vec<ScenarioRow>,
    pub(crate) labels: Vec<LabelRow>,
    pub(crate) timeouts: Vec<TimeoutRow>,
    pub(crate) parallel: Option<String>,
}

/// Wall time in microseconds from the report's milliseconds.
const fn ms_to_us(ms: u64) -> u64 {
    ms.saturating_mul(1000)
}

/// Busy percentage: scenario time over `workers x wall`, 0 when there is nothing to divide by.
pub fn utilisation_percent(scenario_ms: u64, workers: usize, wall_ms: u64) -> u64 {
    let capacity = u128::from(u64::try_from(workers).unwrap_or(u64::MAX)) * u128::from(wall_ms);
    if capacity == 0 {
        return 0;
    }
    u64::try_from(u128::from(scenario_ms) * 100 / capacity).unwrap_or(u64::MAX)
}

/// The rows of the section, or `None` when the report has no scenarios.
pub fn summarize(report: &Report) -> Option<Summary> {
    if report.scenarios.is_empty() {
        return None;
    }
    let mut ordered: Vec<&crate::report::ScenarioRecord> = report.scenarios.iter().collect();
    ordered.sort_by_key(|record| std::cmp::Reverse(record.duration_ms));
    let scenarios = ordered
        .into_iter()
        .take(SLOWEST)
        .map(|record| ScenarioRow {
            name: record.name.clone(),
            time: format_us(ms_to_us(record.duration_ms)),
            top: record
                .timing
                .sorted()
                .into_iter()
                .take(TOP_LABELS)
                .map(|(label, entry)| format!("{label} {}", format_us(entry.total_us)))
                .collect::<Vec<_>>()
                .join(", "),
        })
        .collect();
    let totals = totals(report);
    let labels = totals
        .sorted()
        .into_iter()
        .map(|(label, entry)| LabelRow {
            label: label.to_owned(),
            time: format_us(entry.total_us),
            count: entry.count,
        })
        .collect();
    let mut timeouts: Vec<TimeoutRow> = totals
        .sorted()
        .into_iter()
        .filter(|(_, entry)| entry.timeouts > 0)
        .map(|(label, entry)| TimeoutRow {
            label: label.to_owned(),
            count: entry.timeouts,
            time: format_us(entry.timeout_us),
        })
        .collect();
    timeouts.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.label.cmp(&b.label)));
    let parallel = report.parallel.as_ref().map(|stats| {
        let scenario_ms: u64 = report.scenarios.iter().map(|s| s.duration_ms).sum();
        format!(
            "Parallel run: {} worker(s) started, peak {} at once; wall time {}, scenario time \
             {}; utilisation {}% (scenario time / (peak workers x wall time)).",
            stats.started,
            stats.peak,
            format_us(ms_to_us(stats.wall_ms)),
            format_us(ms_to_us(scenario_ms)),
            utilisation_percent(scenario_ms, stats.peak, stats.wall_ms)
        )
    });
    Some(Summary {
        scenarios,
        labels,
        timeouts,
        parallel,
    })
}

/// All scenarios' samples added up.
fn totals(report: &Report) -> Table {
    let mut totals = Table::new();
    for record in &report.scenarios {
        totals.merge(&record.timing);
    }
    totals
}

/// The `timing` object of `report.json`: totals per label and the parallel numbers (or null).
pub fn json(report: &Report) -> Value {
    let parallel = report.parallel.as_ref().map(|stats| {
        let scenario_ms: u64 = report.scenarios.iter().map(|s| s.duration_ms).sum();
        json!({
            "workers_started": stats.started,
            "peak_workers": stats.peak,
            "wall_ms": stats.wall_ms,
            "scenario_ms": scenario_ms,
            "utilisation_percent": utilisation_percent(scenario_ms, stats.peak, stats.wall_ms),
        })
    });
    json!({
        "totals": totals(report).to_report_json(),
        "parallel": parallel,
    })
}

/// Appends the Markdown "Timing" section.
pub fn markdown(report: &Report, out: &mut String) {
    let Some(summary) = summarize(report) else {
        return;
    };
    let _ = writeln!(out, "## Timing\n");
    let _ = writeln!(out, "Slowest scenarios by wall time (at most {SLOWEST}):\n");
    for row in &summary.scenarios {
        let top = if row.top.is_empty() {
            String::new()
        } else {
            format!(" ({})", row.top)
        };
        let _ = writeln!(out, "- `{}`: {}{top}", row.name, row.time);
    }
    out.push('\n');
    if !summary.labels.is_empty() {
        let _ = writeln!(out, "Time per label over the run:\n");
        for row in &summary.labels {
            let _ = writeln!(out, "- `{}`: {} ({}x)", row.label, row.time, row.count);
        }
        out.push('\n');
    }
    if !summary.timeouts.is_empty() {
        let _ = writeln!(out, "Timeouts per label:\n");
        for row in &summary.timeouts {
            let _ = writeln!(
                out,
                "- `{}`: {} timed out, {}",
                row.label, row.count, row.time
            );
        }
        out.push('\n');
    }
    if let Some(line) = &summary.parallel {
        let _ = writeln!(out, "{line}\n");
    }
}

/// Appends the plain-text "Timing" section.
pub fn text(report: &Report, out: &mut String) {
    let Some(summary) = summarize(report) else {
        return;
    };
    out.push_str("Timing\n");
    let _ = writeln!(out, "  Slowest scenarios by wall time (at most {SLOWEST})");
    for row in &summary.scenarios {
        let top = if row.top.is_empty() {
            String::new()
        } else {
            format!(" ({})", row.top)
        };
        let _ = writeln!(out, "    {}: {}{top}", row.name, row.time);
    }
    if !summary.labels.is_empty() {
        out.push_str("  Time per label over the run\n");
        for row in &summary.labels {
            let _ = writeln!(out, "    {}: {} ({}x)", row.label, row.time, row.count);
        }
    }
    if !summary.timeouts.is_empty() {
        out.push_str("  Timeouts per label\n");
        for row in &summary.timeouts {
            let _ = writeln!(
                out,
                "    {}: {} timed out, {}",
                row.label, row.count, row.time
            );
        }
    }
    if let Some(line) = &summary.parallel {
        let _ = writeln!(out, "  {line}");
    }
    out.push('\n');
}

/// Appends the HTML "Timing" section; `escape` is the page's escaper.
pub fn html(report: &Report, out: &mut String, escape: fn(&str) -> String) {
    let Some(summary) = summarize(report) else {
        return;
    };
    out.push_str("<h2>Timing</h2>\n");
    let _ = writeln!(
        out,
        "<h3>Slowest scenarios by wall time (at most {SLOWEST})</h3>\n<table>"
    );
    out.push_str(
        "<tr><th scope=\"col\">Scenario</th><th scope=\"col\">Wall time</th>\
         <th scope=\"col\">Biggest labels</th></tr>\n",
    );
    for row in &summary.scenarios {
        let _ = writeln!(
            out,
            "<tr><td><code>{}</code></td><td>{}</td><td>{}</td></tr>",
            escape(&row.name),
            escape(&row.time),
            escape(&row.top)
        );
    }
    out.push_str("</table>\n");
    if !summary.labels.is_empty() {
        out.push_str("<h3>Time per label over the run</h3>\n<table>\n");
        out.push_str(
            "<tr><th scope=\"col\">Label</th><th scope=\"col\">Time</th>\
             <th scope=\"col\">Samples</th></tr>\n",
        );
        for row in &summary.labels {
            let _ = writeln!(
                out,
                "<tr><td><code>{}</code></td><td>{}</td><td>{}</td></tr>",
                escape(&row.label),
                escape(&row.time),
                row.count
            );
        }
        out.push_str("</table>\n");
    }
    if !summary.timeouts.is_empty() {
        out.push_str("<h3>Timeouts per label</h3>\n<table>\n");
        out.push_str(
            "<tr><th scope=\"col\">Label</th><th scope=\"col\">Timed out</th>\
             <th scope=\"col\">Time</th></tr>\n",
        );
        for row in &summary.timeouts {
            let _ = writeln!(
                out,
                "<tr><td><code>{}</code></td><td>{}</td><td>{}</td></tr>",
                escape(&row.label),
                row.count,
                escape(&row.time)
            );
        }
        out.push_str("</table>\n");
    }
    if let Some(line) = &summary.parallel {
        let _ = writeln!(out, "<p>{}</p>", escape(line));
    }
}

#[cfg(test)]
mod tests {
    use super::{markdown, utilisation_percent};
    use crate::{
        report::{ParallelStats, Report},
        timing::Table,
    };

    fn report_with_timing() -> Report {
        let mut report = Report::new("unused");
        report.begin_scenario("slow");
        report.end_scenario("slow");
        let mut table = Table::new();
        table.add("relaunch", 2_000_000, false);
        table.add("wait", 1_000_000, true);
        report.attach_timing(table);
        report.scenarios[0].duration_ms = 4000;
        report
    }

    #[test]
    fn utilisation_is_scenario_time_over_capacity() {
        assert_eq!(utilisation_percent(5000, 2, 10_000), 25);
        assert_eq!(utilisation_percent(5000, 0, 10_000), 0);
        assert_eq!(utilisation_percent(5000, 2, 0), 0);
    }

    #[test]
    fn markdown_has_every_part() {
        let mut report = report_with_timing();
        report.parallel = Some(ParallelStats {
            started: 3,
            peak: 2,
            wall_ms: 8000,
        });
        let mut out = String::new();
        markdown(&report, &mut out);
        assert!(out.starts_with("## Timing\n\n"), "{out}");
        assert!(
            out.contains("- `slow`: 4.0 s (relaunch 2.0 s, wait 1.0 s)\n"),
            "{out}"
        );
        assert!(out.contains("- `relaunch`: 2.0 s (1x)\n"), "{out}");
        assert!(out.contains("- `wait`: 1 timed out, 1.0 s\n"), "{out}");
        assert!(out.contains("utilisation 25%"), "{out}");
    }

    #[test]
    fn no_scenarios_no_section() {
        let mut out = String::new();
        markdown(&Report::new("unused"), &mut out);
        assert_eq!(out, "");
    }

    #[test]
    fn json_carries_totals_and_parallel_numbers() {
        let mut report = report_with_timing();
        assert!(super::json(&report)["parallel"].is_null());
        report.parallel = Some(ParallelStats {
            started: 3,
            peak: 2,
            wall_ms: 8000,
        });
        let value = super::json(&report);
        assert_eq!(value["totals"]["wait"]["timeouts"], 1);
        assert_eq!(value["parallel"]["peak_workers"], 2);
        assert_eq!(value["parallel"]["utilisation_percent"], 25);
    }
}
