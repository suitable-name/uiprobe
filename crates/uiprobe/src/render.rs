//! The HTML and plain-text renderings of a [`Report`]: the same content and grouping as the
//! Markdown (`Report::render_markdown`), which stays the reference format. The HTML is one
//! self-contained page (inline style, no script, no external asset) with every data string
//! escaped; a `main` landmark, the window headings inside their `<summary>`, and a dark color
//! scheme that follows the system setting; the text is the Markdown without markup for
//! terminals and CI logs. Section bodies (the coverage gate's Markdown) are copied as they are
//! in both: escaped inside `<pre>` in HTML, verbatim in text. A multi-line finding detail (a
//! panic message) is kept verbatim in both too: `white-space: pre-wrap` in HTML, continuation
//! lines indented in text.

use std::fmt::Write as _;

use crate::report::Report;

/// HTML-escapes `& < > " '`.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// The page head: one inline style, no script, no external asset. Colors are custom properties
/// with a dark set under `prefers-color-scheme: dark`; `color-scheme` lets the browser draw its
/// own controls and scrollbars to match. Text contrast is at or above 7:1 in both schemes.
const HEAD: &str = "<!doctype html>\n\
<html lang=\"en\">\n\
<head>\n\
<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<meta name=\"color-scheme\" content=\"light dark\">\n\
<title>uiprobe report</title>\n\
<style>\n\
:root{color-scheme:light dark;--bg:#ffffff;--fg:#1a1a1a;--muted:#595959;--line:#8a8a8a;--code:#ededed;--pre:#f4f4f4}\n\
@media (prefers-color-scheme:dark){:root{--bg:#121212;--fg:#e8e8e8;--muted:#b3b3b3;--line:#6e6e6e;--code:#2a2a2a;--pre:#1c1c1c}}\n\
body{background:var(--bg);color:var(--fg);font-family:system-ui,sans-serif;max-width:60rem;margin:2rem auto;padding:0 1rem;line-height:1.5}\n\
table{border-collapse:collapse}td,th{border:1px solid var(--line);padding:.2rem .6rem;text-align:left}\n\
li{white-space:pre-wrap}code{background:var(--code);padding:0 .2rem}.state{color:var(--muted)}\n\
summary{cursor:pointer;margin-top:1rem}summary h2{display:inline;font-size:1.25rem}\n\
pre{background:var(--pre);padding:.5rem;overflow:auto}\n\
</style>\n\
</head>\n\
<body>\n\
<main>\n\
<h1>uiprobe report</h1>\n";

/// The self-contained HTML page: a `main` landmark, the pass/fail line, the per-check totals,
/// the scenario table (column headers with `scope`), section bodies in an escaped `<pre>`,
/// windows as `<details open>` whose `<summary>` holds the window's `<h2>` (so a screen
/// reader's heading list shows them), checks as `<h3>`, findings as `<li>`.
pub fn html(report: &Report) -> String {
    let mut out = String::new();
    out.push_str(HEAD);
    let count = report.findings.len();
    if count == 0 {
        let _ = writeln!(
            out,
            "<p><strong>PASS</strong>: no findings in {} scenario(s).</p>",
            report.scenarios.len()
        );
    } else {
        let _ = writeln!(
            out,
            "<p><strong>FAIL</strong>: {count} distinct finding(s) in {} scenario(s).</p>",
            report.scenarios.len()
        );
        out.push_str("<h2>Findings by Check</h2>\n<ul>\n");
        for (check, total) in report.per_check() {
            let _ = writeln!(out, "<li>{}: {total}</li>", escape(check));
        }
        out.push_str("</ul>\n");
    }
    if !report.scenarios.is_empty() {
        out.push_str("<h2>Scenarios</h2>\n<table>\n");
        out.push_str(
            "<tr><th scope=\"col\">Scenario</th><th scope=\"col\">Findings</th>\
             <th scope=\"col\">Time (ms)</th></tr>\n",
        );
        for scenario in &report.scenarios {
            let _ = writeln!(
                out,
                "<tr><td><code>{}</code></td><td>{}</td><td>{}</td></tr>",
                escape(&scenario.name),
                scenario.findings,
                scenario.duration_ms
            );
        }
        out.push_str("</table>\n");
    }
    crate::timing_report::html(report, &mut out, escape);
    for (title, body) in &report.sections {
        let _ = writeln!(out, "<h2>{}</h2>", escape(title));
        let _ = writeln!(out, "<pre>{}</pre>", escape(body));
    }
    for (window, by_check) in &report.grouped() {
        out.push_str("<details open>\n");
        let _ = writeln!(out, "<summary><h2>{}</h2></summary>", escape(window));
        for (check, findings) in by_check {
            let _ = writeln!(out, "<h3>{} ({})</h3>", escape(check), findings.len());
            out.push_str("<ul>\n");
            for finding in findings {
                let element = if finding.element.is_empty() {
                    String::new()
                } else {
                    format!("<code>{}</code> -- ", escape(&finding.element))
                };
                let repeats = report.repeats_of(finding);
                let times = if repeats > 1 {
                    format!(" <em>(x{repeats})</em>")
                } else {
                    String::new()
                };
                let _ = writeln!(
                    out,
                    "<li>{element}{}{times} <span class=\"state\">[{}]</span></li>",
                    escape(&finding.detail),
                    escape(&finding.state)
                );
            }
            out.push_str("</ul>\n");
        }
        out.push_str("</details>\n");
    }
    if !report.notes.is_empty() {
        out.push_str("<h2>Notes</h2>\n<ul>\n");
        for note in &report.notes {
            let _ = writeln!(out, "<li>{}</li>", escape(note));
        }
        out.push_str("</ul>\n");
    }
    out.push_str("</main>\n</body>\n</html>\n");
    out
}

/// The plain-text report: the Markdown content without markup, two-space indents, continuation
/// lines of a multi-line detail aligned under the detail column.
pub fn text(report: &Report) -> String {
    let mut out = String::new();
    out.push_str("uiprobe report\n");
    let _ = writeln!(
        out,
        "{} distinct finding(s) in {} scenario(s).\n",
        report.findings.len(),
        report.scenarios.len()
    );
    if !report.scenarios.is_empty() {
        out.push_str("Scenarios\n");
        for scenario in &report.scenarios {
            let _ = writeln!(
                out,
                "  {}: {} finding(s), {} ms",
                scenario.name, scenario.findings, scenario.duration_ms
            );
        }
        out.push('\n');
    }
    crate::timing_report::text(report, &mut out);
    for (title, body) in &report.sections {
        let _ = writeln!(out, "{title}");
        out.push_str(body);
        out.push('\n');
    }
    for (window, by_check) in &report.grouped() {
        let _ = writeln!(out, "[{window}]");
        for (check, findings) in by_check {
            let _ = writeln!(out, "  {check} ({})", findings.len());
            for finding in findings {
                let line = report.finding_line(finding).replace('\n', "\n      ");
                let _ = writeln!(out, "    {line}");
            }
        }
        out.push('\n');
    }
    if !report.notes.is_empty() {
        out.push_str("Notes\n");
        for note in &report.notes {
            let _ = writeln!(out, "  - {note}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::Finding;

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

    #[test]
    fn text_matches_the_pinned_grammar() {
        let expected = "uiprobe report\n2 distinct finding(s) in 0 scenario(s).\n\nCoverage\n- line\n\n[-]\n  timeout (1)\n    never [s1]\n\n[W]\n  overlap (1)\n    Button \"A\" -- a <b> & c (x2) [s1]\n\nNotes\n  - note one\n";
        assert_eq!(text(&sample()), expected);
    }

    #[test]
    fn text_indents_continuation_lines() {
        let mut report = Report::new("unused");
        report.add(Finding::simple("scenario-panic", "first\nsecond"));
        assert!(
            text(&report).contains("    first\n      second []\n"),
            "{}",
            text(&report)
        );
    }

    #[test]
    fn html_is_self_contained_escaped_and_grouped() {
        let html = html(&sample());
        assert!(html.starts_with("<!doctype html>\n"));
        assert!(html.ends_with("</html>\n"));
        assert!(html.contains("<title>uiprobe report</title>"));
        assert!(!html.contains("<script"), "no script");
        assert!(!html.contains("http"), "no URL at all");
        assert!(!html.contains("src="), "no external asset");
        assert!(!html.contains("href="), "no link");
        assert!(html.contains("a &lt;b&gt; &amp; c"), "escaped detail");
        assert!(!html.contains("a <b> & c"), "no raw detail");
        assert!(html.contains("<code>Button &quot;A&quot;</code> -- a &lt;b&gt; &amp; c <em>(x2)</em> <span class=\"state\">[s1]</span>"));
        assert!(html.contains("<summary><h2>W</h2></summary>"));
        assert!(html.contains("<summary><h2>-</h2></summary>"));
        assert!(html.contains("<main>\n") && html.contains("</main>\n</body>"));
        assert!(html.contains("<meta name=\"color-scheme\" content=\"light dark\">"));
        assert!(html.contains("@media (prefers-color-scheme:dark)"));
        assert!(
            html.contains("<p><strong>FAIL</strong>: 2 distinct finding(s) in 0 scenario(s).</p>")
        );
        assert!(html.contains(
            "<h2>Findings by Check</h2>\n<ul>\n<li>overlap: 1</li>\n<li>timeout: 1</li>\n</ul>\n"
        ));
        assert!(html.contains("<h3>overlap (1)</h3>") && html.contains("<h3>timeout (1)</h3>"));
        assert!(html.contains("<h2>Coverage</h2>\n<pre>- line\n</pre>"));
        assert!(html.contains("<h2>Notes</h2>") && html.contains("<li>note one</li>"));
        assert!(
            !html.contains("<h2>Scenarios</h2>"),
            "no scenarios in the sample"
        );
    }

    #[test]
    fn html_lists_scenarios_when_there_are_any() {
        let mut report = Report::new("unused");
        report.begin_scenario("one");
        report.end_scenario("one");
        let html = html(&report);
        assert!(html.contains("<h2>Scenarios</h2>"));
        assert!(html.contains("<th scope=\"col\">Time (ms)</th>"));
        assert!(html.contains("<p><strong>PASS</strong>: no findings in 1 scenario(s).</p>"));
        assert!(html.contains("<tr><td><code>one</code></td><td>0</td><td>"));
    }
}
