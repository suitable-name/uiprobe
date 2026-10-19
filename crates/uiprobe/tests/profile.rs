//! Self-test: the profile isolates the run and cleans up after it.
//!
//! The parent process re-runs this binary as a child in three modes, selected by the environment
//! variable `UIPROBE_SELFTEST_CHILD`, and checks each one ends with exit code 2, names its
//! cause on stderr, and leaves no profile directory behind (the directory is named on stdout):
//!
//! - `outside`: a guard path OUTSIDE the profile.
//! - `platform`: the app installed a Slint platform before the harness could.
//! - `abort`: a scenario calls `uiprobe::profile::request_abort`.
//!
//! It then runs a harness whose guard path is inside, and checks that the data dir, working
//! directory, temp dir and the XDG state/runtime variables were inside the profile during the
//! scenario, that `wipe_dir` refuses the profile root (also by its canonical form), that no
//! finding was made, and that afterwards the profile root is gone and the working directory and
//! temp dir are back as they were.
//!
//! The parent starts no thread before `Harness::run` redirects the environment (spawning and
//! waiting for the child uses none), which is what the profile requires.

use std::{
    cell::{Cell, RefCell},
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    rc::Rc,
};
use uiprobe::prelude::*;

slint::slint! {
    export component Probe inherits Window {
        width: 200px;
        height: 100px;
        default-font-family: "FixedTestFont";
        Text {
            text: "probe";
        }
    }
}

/// Set in the child process to the mode it runs: `outside`, `platform` or `abort`.
const CHILD_FLAG: &str = "UIPROBE_SELFTEST_CHILD";
/// A directory outside the profile, handed to the `outside` child.
const OUTSIDE_VAR: &str = "UIPROBE_SELFTEST_OUTSIDE";

fn main() -> ExitCode {
    match std::env::var(CHILD_FLAG).ok().as_deref() {
        Some("outside") => child_outside(),
        Some("platform") => child_platform(),
        Some("abort") => child_abort(),
        Some(other) => {
            eprintln!("profile: unknown child mode {other:?}");
            ExitCode::FAILURE
        }
        None => parent(),
    }
}

/// The guard names a path outside the profile, so `run` must abort with exit code 2.
fn child_outside() -> ExitCode {
    Harness::new()
        .profile(Profile::isolated().guard_paths(|| {
            let outside = std::env::var_os(OUTSIDE_VAR)
                .map(PathBuf::from)
                .unwrap_or_default();
            vec![outside.join("outside.txt")]
        }))
        .scenario("never-runs", |_ctx| Ok(()))
        .run()
}

/// The app installed its own platform first, so the harness cannot install the testing backend.
fn child_platform() -> ExitCode {
    i_slint_backend_testing::init_no_event_loop();
    Harness::new().scenario("never-runs", |_ctx| Ok(())).run()
}

/// A scenario requests an abort: the run ends after it with exit code 2.
fn child_abort() -> ExitCode {
    Harness::new()
        .scenario("abort", |ctx| {
            let _app = ctx.launch(Probe::new)?;
            uiprobe::profile::request_abort("self-test abort");
            Ok(())
        })
        .run()
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

/// The profile directory a child printed (`uiprobe: profile <path>`).
fn profile_path(stdout: &str) -> Option<PathBuf> {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("uiprobe: profile "))
        .map(|rest| PathBuf::from(rest.trim()))
}

/// One abort mode: exit code 2, the cause on stderr, the printed profile directory deleted.
fn check_abort_mode(
    mode: &str,
    extra_env: &[(&str, &str)],
    stderr_has: &str,
    mismatches: &mut Vec<String>,
) {
    let (code, stdout, stderr) = run_child(mode, extra_env);
    if code != Some(2) {
        mismatches.push(format!(
            "child {mode}: exited with {code:?}, wanted Some(2); stderr: {stderr}"
        ));
    }
    if !stderr.contains(stderr_has) {
        mismatches.push(format!(
            "child {mode}: stderr does not contain {stderr_has:?}: {stderr}"
        ));
    }
    match profile_path(&stdout) {
        Some(path) if path.exists() => mismatches.push(format!(
            "child {mode}: the profile {} was left behind",
            path.display()
        )),
        Some(_) => {}
        None => mismatches.push(format!(
            "child {mode}: stdout names no profile directory: {stdout}"
        )),
    }
}

/// The parent: abort checks through child processes, then a clean in-process run.
fn parent() -> ExitCode {
    let mut mismatches: Vec<String> = Vec::new();

    // 1. Every abort path ends with exit code 2 and cleans the profile up.
    let outside = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let outside_text = outside.to_string_lossy().into_owned();
    check_abort_mode(
        "outside",
        &[(OUTSIDE_VAR, outside_text.as_str())],
        "outside the profile",
        &mut mismatches,
    );
    check_abort_mode("platform", &[], "platform", &mut mismatches);
    check_abort_mode("abort", &[], "self-test abort", &mut mismatches);

    // 2. A guard path inside the profile runs clean and the profile cleans up.
    let cwd_before = std::env::current_dir().ok();
    let temp_before = std::env::temp_dir();
    let root: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
    let inside = Rc::new(Cell::new(false));
    let scenario_root = Rc::clone(&root);
    let scenario_inside = Rc::clone(&inside);
    let report = Harness::new()
        .report_dir(uiprobe::target_dir!().join("uiprobe-selftest-profile"))
        .profile(
            Profile::isolated()
                .name_prefix("uiprobe-selftest")
                .guard_paths(|| {
                    vec![
                        uiprobe::hooks::paths::data_dir()
                            .unwrap_or_default()
                            .join("settings.toml"),
                    ]
                }),
        )
        .scenario("inside", move |ctx| {
            let profile_root = ctx.profile().root().to_path_buf();
            *scenario_root.borrow_mut() = Some(profile_root.clone());
            let data_dir = uiprobe::hooks::paths::data_dir().unwrap_or_default();
            let cwd = std::env::current_dir().unwrap_or_default();
            let temp = std::env::temp_dir();
            let redirected = ["TMPDIR", "XDG_STATE_HOME", "XDG_RUNTIME_DIR"]
                .iter()
                .all(|var| {
                    std::env::var_os(var)
                        .is_some_and(|path| ctx.profile().contains(Path::new(&path)))
                });
            let root_kept = !ctx.profile().wipe_dir(&profile_root)
                && fs::canonicalize(&profile_root)
                    .is_ok_and(|canonical| !ctx.profile().wipe_dir(&canonical))
                && profile_root.exists();
            let all_inside = ctx.profile().contains(&data_dir)
                && ctx.profile().contains(&cwd)
                && ctx.profile().contains(&temp)
                && ctx.profile().contains(&ctx.scratch_file("probe.txt"))
                && redirected
                && root_kept
                && ctx.profile().wipe_dir(&ctx.profile().scratch_dir());
            scenario_inside.set(all_inside);
            Ok(())
        })
        .run_to_report();

    if !report.is_empty() {
        for finding in report.findings() {
            mismatches.push(format!(
                "unexpected finding {}: {}",
                finding.check, finding.detail
            ));
        }
    }
    if !inside.get() {
        mismatches.push(
            "the data dir, working directory, temp dir, TMPDIR/XDG_STATE_HOME/XDG_RUNTIME_DIR or \
             scratch file were not inside the profile, or wipe_dir touched the profile root"
                .to_owned(),
        );
    }
    match root.borrow().as_ref() {
        Some(root) if root.exists() => {
            mismatches.push(format!("the profile {} still exists", root.display()));
        }
        Some(_) => {}
        None => mismatches.push("the scenario did not run".to_owned()),
    }
    if std::env::current_dir().ok() != cwd_before {
        mismatches.push("the working directory was not restored".to_owned());
    }
    if std::env::temp_dir() != temp_before {
        mismatches.push("the temp directory variables were not restored".to_owned());
    }

    for mismatch in &mismatches {
        eprintln!("profile: MISMATCH: {mismatch}");
    }
    if mismatches.is_empty() {
        println!("profile: ok");
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
