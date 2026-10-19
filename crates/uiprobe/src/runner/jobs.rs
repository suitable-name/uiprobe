//! How many worker processes run the scenarios: the [`Jobs`] setting, the environment variables
//! that replace it (`UIPROBE_JOBS`, `UIPROBE_JOBS_MAX`) and the decision between a
//! serial run and a parallel one.
//!
//! The decision is pure (`plan`) so it is unit-tested without starting a process.

use crate::report::{Finding, Report};

/// How many worker processes run the scenarios.
///
/// The default, [`Jobs::Serial`], is the classic run: one process, scenarios in order. A parallel
/// run turns the test binary into a COORDINATOR that starts copies of itself as WORKER processes;
/// each worker runs one scenario at a time with its own throw-away profile, environment
/// redirect, hook state, panic recorder and mock time. Threads cannot do this: Slint's platform
/// is per process and bound to one thread, and the profile redirect is a process-wide
/// `set_var`.
///
/// Because every worker re-executes the test's `main`, `main` must build the same harnesses in
/// the same order in every process and do nothing before them that must not run twice
/// (`Harness::new()` first is already the rule). With several harnesses in one `main` (the
/// coverage self-test builds eight), each worker runs only the harness whose run matches the
/// coordinator's and skips the others.
///
/// The environment variable `UIPROBE_JOBS` (`serial`, `1`, `auto` or a number) replaces
/// the configured value; `UIPROBE_JOBS_MAX` caps the workers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Jobs {
    /// One process, scenarios in order: today's behavior (the default).
    #[default]
    Serial,
    /// This many workers from the start (0 and 1 mean `Serial`).
    Fixed(usize),
    /// Start with one worker; every 5 s add one while at least 1.5 cores were idle over the last
    /// interval and scenarios are still waiting, up to the cap.
    Auto,
}

/// A parallel run decided by [`plan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// Whether the worker count grows while the run goes on.
    pub(crate) auto: bool,
    /// Workers started at the beginning.
    pub(crate) initial: usize,
    /// The most workers at once.
    pub(crate) cap: usize,
}

impl Plan {
    /// The mode as the console line shows it (`fixed 3`, `auto`).
    pub(crate) fn label(&self) -> String {
        if self.auto {
            "auto".to_owned()
        } else {
            format!("fixed {}", self.initial)
        }
    }
}

/// The default worker cap: the machine's parallelism, at least 1.
pub fn default_max() -> usize {
    std::thread::available_parallelism().map_or(1, usize::from)
}

/// Parses `serial`, `auto` or a number (case-insensitive); `None` on anything else.
pub fn parse_jobs(text: &str) -> Option<Jobs> {
    let text = text.trim().to_ascii_lowercase();
    match text.as_str() {
        "serial" => Some(Jobs::Serial),
        "auto" => Some(Jobs::Auto),
        _ => text.parse::<usize>().ok().map(Jobs::Fixed),
    }
}

/// Parses the worker cap: a number of at least 1.
pub fn parse_max(text: &str) -> Option<usize> {
    text.trim().parse::<usize>().ok().filter(|n| *n >= 1)
}

/// The jobs setting and the cap of the run: `UIPROBE_JOBS` / `UIPROBE_JOBS_MAX`
/// when set and valid, else the configured values (`max` `None` means the default cap).
/// Invalid text is a `config` finding naming the variable and the accepted forms, and the
/// configured value is kept.
pub fn resolve(configured: Jobs, max: Option<usize>, report: &mut Report) -> (Jobs, usize) {
    let jobs = resolve_one("UIPROBE_JOBS", "serial, 1, auto or a number", report)
        .and_then(|text| parse_jobs(&text))
        .unwrap_or(configured);
    let cap = resolve_one("UIPROBE_JOBS_MAX", "a number of at least 1", report)
        .and_then(|text| parse_max(&text))
        .or(max)
        .unwrap_or_else(default_max);
    (jobs, cap)
}

/// The text of the variable `name` when it is set and valid for the matching parser; an invalid
/// value is reported and `None` is returned. `forms` names the accepted forms.
fn resolve_one(name: &str, forms: &str, report: &mut Report) -> Option<String> {
    let text = std::env::var(name).ok()?;
    let valid = if name == "UIPROBE_JOBS" {
        parse_jobs(&text).is_some()
    } else {
        parse_max(&text).is_some()
    };
    if valid {
        return Some(text);
    }
    report.add(Finding::simple(
        "config",
        format!("{name}={text:?} is not one of: {forms}; ignored"),
    ));
    None
}

/// Whether the run uses workers and how: `None` means serial (one selected scenario, `Serial`,
/// or an effective count of 1 under `Fixed`). Workers never exceed the selected scenarios or
/// `max`; `Auto` with at least two selected scenarios always uses workers (starting with one).
pub fn plan(jobs: Jobs, max: usize, selected: usize) -> Option<Plan> {
    if selected < 2 {
        return None;
    }
    let cap = max.max(1).min(selected);
    match jobs {
        Jobs::Serial => None,
        Jobs::Fixed(n) => {
            let n = n.min(cap);
            (n >= 2).then_some(Plan {
                auto: false,
                initial: n,
                cap: n,
            })
        }
        Jobs::Auto => Some(Plan {
            auto: true,
            initial: 1,
            cap,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{Jobs, Plan, parse_jobs, parse_max, plan};

    #[test]
    fn parse_jobs_accepts_the_documented_forms() {
        assert_eq!(parse_jobs("serial"), Some(Jobs::Serial));
        assert_eq!(parse_jobs(" AUTO "), Some(Jobs::Auto));
        assert_eq!(parse_jobs("1"), Some(Jobs::Fixed(1)));
        assert_eq!(parse_jobs("4"), Some(Jobs::Fixed(4)));
        assert_eq!(parse_jobs("bogus"), None);
        assert_eq!(parse_jobs("-2"), None);
        assert_eq!(parse_jobs(""), None);
    }

    #[test]
    fn parse_max_needs_at_least_one() {
        assert_eq!(parse_max("3"), Some(3));
        assert_eq!(parse_max(" 1 "), Some(1));
        assert_eq!(parse_max("0"), None);
        assert_eq!(parse_max("auto"), None);
    }

    #[test]
    fn plan_decides_serial_or_workers() {
        assert_eq!(plan(Jobs::Serial, 8, 10), None);
        assert_eq!(plan(Jobs::Fixed(4), 8, 1), None);
        assert_eq!(plan(Jobs::Fixed(1), 8, 10), None);
        assert_eq!(plan(Jobs::Fixed(0), 8, 10), None);
        assert_eq!(plan(Jobs::Fixed(4), 1, 10), None);
        let fixed = plan(Jobs::Fixed(4), 8, 10).expect("four workers");
        assert_eq!((fixed.initial, fixed.cap, fixed.auto), (4, 4, false));
        // Never more workers than scenarios, nor than the cap.
        assert_eq!(plan(Jobs::Fixed(6), 8, 3).map(|p| p.initial), Some(3));
        assert_eq!(plan(Jobs::Fixed(6), 2, 10).map(|p| p.initial), Some(2));
        assert_eq!(
            plan(Jobs::Auto, 8, 5),
            Some(Plan {
                auto: true,
                initial: 1,
                cap: 5
            })
        );
        assert_eq!(plan(Jobs::Auto, 8, 1), None);
        assert_eq!(plan(Jobs::Auto, 2, 5).map(|p| p.cap), Some(2));
    }

    #[test]
    fn labels_name_the_mode() {
        let fixed = plan(Jobs::Fixed(3), 8, 10).expect("workers");
        assert_eq!(fixed.label(), "fixed 3");
        assert_eq!(plan(Jobs::Auto, 8, 10).expect("workers").label(), "auto");
    }
}
