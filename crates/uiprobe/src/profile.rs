//! The throw-away profile every harness run lives in, so it can never touch the owner's
//! settings, data or caches.
//!
//! [`Profile`] is the configuration; the runner turns it into a live [`ProfileDir`] as the very
//! first step of [`crate::Harness::run`], while the process is still single-threaded: a fresh
//! directory under the real temp dir, every environment variable an app resolves a path from
//! pointed into it (`APPDATA`, `LOCALAPPDATA`, `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`,
//! `XDG_DATA_HOME`, `XDG_STATE_HOME`, `XDG_RUNTIME_DIR`, `HOME`, `USERPROFILE`, `TEMP`, `TMP`,
//! `TMPDIR`), `uiprobe_hooks::paths::set_data_dir` set, and the working directory
//! moved into it. The app's own resolved paths ([`Profile::guard_paths`]) are then checked and
//! the run aborts (exit code 2) when one lies outside.
//!
//! The profile leaves the process as it found it: `ProfileDir::finish` restores every
//! redirected variable (or removes it again) and the working directory before deleting the
//! directory, so several harness runs can follow one another in one process (the self-tests do
//! this). On Unix the restore uses `set_var` again, which is sound only when no app thread is
//! alive any more: stop them in [`crate::Harness::teardown`]. Linux warns when threads remain at
//! that point; macOS has no cheap thread count and gets no warning.
//!
//! The one exception is `uiprobe_hooks::paths::data_dir`: the hooks crate has no way to
//! clear it, so after the run it again holds the value it had before the profile was created,
//! or, when there was none, a stale path into the deleted profile. Apps ask for it only under
//! their harness feature, during a run, so this is harmless.
//!
//! For example, in Indicatrix the settings file comes from `APPDATA` / `XDG_CONFIG_HOME` and the
//! design library database is a working-directory-relative file, so redirecting the variables
//! and the working directory is enough; an app that resolves its config directory through the
//! Windows Known Folders API (the `dirs` crate) ignores `APPDATA` and must consult
//! `uiprobe_hooks::paths::data_dir` under its feature, and say which paths it really
//! uses through [`Profile::guard_paths`], so the run aborts instead of writing into the owner's
//! real profile.

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Sub-directories of the profile, one per redirected location.
const APPDATA_DIR: &str = "appdata";
const LOCAL_APPDATA_DIR: &str = "localappdata";
const CONFIG_DIR: &str = "config";
const CACHE_DIR: &str = "cache";
const HOME_DIR: &str = "home";
const TEMP_DIR: &str = "temp";
/// The directory `uiprobe_hooks::paths::data_dir` returns, and `XDG_DATA_HOME`.
const DATA_DIR: &str = "data";
/// `XDG_STATE_HOME`.
const STATE_DIR: &str = "state";
/// `XDG_RUNTIME_DIR`.
const RUNTIME_DIR: &str = "runtime";
/// The working directory; for example where a CWD-relative database file appears.
const CWD_DIR: &str = "cwd";

/// Every environment variable an app resolves a path from, and where it is pointed.
const REDIRECTS: [(&str, &str); 12] = [
    ("APPDATA", APPDATA_DIR),
    ("LOCALAPPDATA", LOCAL_APPDATA_DIR),
    ("XDG_CONFIG_HOME", CONFIG_DIR),
    ("XDG_CACHE_HOME", CACHE_DIR),
    ("XDG_DATA_HOME", DATA_DIR),
    ("HOME", HOME_DIR),
    // Windows home lookups (`dirs`-style crates) use this, not HOME.
    ("USERPROFILE", HOME_DIR),
    ("TEMP", TEMP_DIR),
    ("TMP", TEMP_DIR),
    // Unix std::env::temp_dir() reads only TMPDIR.
    ("TMPDIR", TEMP_DIR),
    ("XDG_STATE_HOME", STATE_DIR),
    ("XDG_RUNTIME_DIR", RUNTIME_DIR),
];

/// Prints `message` and ends the process with exit code 2. Used for situations where
/// continuing could touch the owner's files.
pub fn abort_run(message: &str) -> ! {
    eprintln!("uiprobe: ABORT: {message}");
    std::process::exit(2);
}

static ABORT_REQUEST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Ends the run after the current scenario with exit code 2.
///
/// The teardown runs, the profile is deleted, the environment is restored, then `message` is
/// printed. For conditions that
/// make the rest of the run meaningless; the harness itself uses it when the UI carries no
/// Slint debug info. No report file is written for an aborted run; the cause is on stderr and
/// the findings made so far are on stdout only.
pub fn request_abort(message: impl Into<String>) {
    let mut slot = ABORT_REQUEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if slot.is_none() {
        *slot = Some(message.into());
    }
}

/// The pending abort request, if any (the runner takes it after every scenario).
pub(crate) fn take_abort_request() -> Option<String> {
    ABORT_REQUEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
}

/// The paths the app will really use, resolved by the app's own code after the redirect.
type GuardFn = Box<dyn Fn() -> Vec<PathBuf>>;

/// How the throw-away profile is set up.
pub struct Profile {
    /// Prefix of the profile directory name (`<prefix>-<pid>-<nanos>`).
    pub(crate) name_prefix: String,
    /// Resolves the app's paths; each must lie inside the profile.
    pub(crate) guard: Option<GuardFn>,
}

impl Profile {
    /// The default: an isolated profile named `uiprobe-<pid>-<nanos>`, no guarded paths.
    #[must_use]
    pub fn isolated() -> Self {
        Self {
            name_prefix: "uiprobe".to_owned(),
            guard: None,
        }
    }

    /// Sets the prefix of the profile directory name.
    #[must_use]
    pub fn name_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.name_prefix = prefix.into();
        self
    }

    /// `paths` returns the files and directories the app will read and write (settings file,
    /// database, cache dir ...), resolved by the app's own code. It runs after the redirect; any
    /// path outside the profile aborts the run before a window exists.
    #[must_use]
    pub fn guard_paths(mut self, paths: impl Fn() -> Vec<PathBuf> + 'static) -> Self {
        self.guard = Some(Box::new(paths));
        self
    }

    /// Creates the live profile: directories, environment redirect, data-dir hook, working
    /// directory, then the guard check. Call only while the process is single-threaded.
    pub(crate) fn create(self) -> ProfileDir {
        let original_cwd = std::env::current_dir()
            .unwrap_or_else(|e| abort_run(&format!("cannot read the working directory: {e}")));
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        // The real temp dir, read BEFORE `TEMP`/`TMP` are redirected below.
        let root = std::env::temp_dir().join(format!(
            "{}-{}-{nanos}",
            self.name_prefix,
            std::process::id()
        ));
        for sub in [
            APPDATA_DIR,
            LOCAL_APPDATA_DIR,
            CONFIG_DIR,
            CACHE_DIR,
            HOME_DIR,
            TEMP_DIR,
            DATA_DIR,
            STATE_DIR,
            RUNTIME_DIR,
            CWD_DIR,
        ] {
            fs::create_dir_all(root.join(sub)).unwrap_or_else(|e| {
                abort_run(&format!(
                    "cannot create profile dir {}: {e}",
                    root.join(sub).display()
                ));
            });
        }
        let saved_env: Vec<(&'static str, Option<OsString>)> = REDIRECTS
            .iter()
            .map(|(var, _)| (*var, std::env::var_os(var)))
            .collect();
        let previous_data_dir = crate::hooks::paths::data_dir();
        #[cfg(target_os = "linux")]
        warn_if_threaded(
            "Harness::new() must be the first statement of main, before anything starts a thread",
        );
        for (var, sub) in REDIRECTS {
            // SAFETY: `set_var` is only unsound while another thread reads or writes the
            // environment. `Harness::new()` is the first statement of `main` and `run()` creates
            // the profile before the Slint backend, the app and every worker thread exist, so the
            // process is single-threaded here.
            unsafe { std::env::set_var(var, root.join(sub)) };
        }
        crate::hooks::paths::set_data_dir(root.join(DATA_DIR));
        std::env::set_current_dir(root.join(CWD_DIR)).unwrap_or_else(|e| {
            abort_run(&format!("cannot enter the profile working directory: {e}"));
        });
        let profile = ProfileDir {
            root,
            original_cwd,
            saved_env,
            previous_data_dir,
        };
        println!("uiprobe: profile {}", profile.root.display());
        match self.guard {
            Some(guard) => {
                let paths = guard();
                profile.assert_inside("guarded path", &paths);
                let list: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
                println!("uiprobe: guarded paths: {}", list.join(", "));
            }
            None => {
                println!(
                    "uiprobe: warning: no guarded paths configured, so nothing proves the \
                     app's own paths are inside the profile (see Profile::guard_paths)"
                );
            }
        }
        profile
    }
}

/// Warns when the process has more than one thread: the environment calls would then be unsound
/// (`set_var` races with readers). `advice` says what to do about it at the call site.
#[cfg(target_os = "linux")]
fn warn_if_threaded(advice: &str) {
    let Ok(status) = fs::read_to_string("/proc/self/status") else {
        return;
    };
    let threads = status
        .lines()
        .find_map(|line| line.strip_prefix("Threads:"))
        .and_then(|value| value.trim().parse::<u32>().ok());
    if let Some(count) = threads
        && count > 1
    {
        eprintln!(
            "uiprobe: warning: the process has {count} threads; {advice} (the profile sets \
             environment variables, which is sound only while no other thread reads them)"
        );
    }
}

/// The live throw-away profile. Dropping it does nothing; the runner calls
/// `ProfileDir::finish` at the end of the run.
pub struct ProfileDir {
    root: PathBuf,
    original_cwd: PathBuf,
    /// The value (or absence) of every redirected variable before the profile.
    saved_env: Vec<(&'static str, Option<OsString>)>,
    /// `uiprobe_hooks::paths::data_dir()` before the profile set it.
    previous_data_dir: Option<PathBuf>,
}

impl ProfileDir {
    /// The profile's root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The working directory the run started in (before the profile moved it).
    #[must_use]
    pub fn original_cwd(&self) -> &Path {
        &self.original_cwd
    }

    /// The directory scenarios may use for fixture files. Inside the profile, so it is deleted
    /// at the end.
    #[must_use]
    pub fn scratch_dir(&self) -> PathBuf {
        let dir = self.root.join(TEMP_DIR).join("scratch");
        let _ = fs::create_dir_all(&dir);
        dir
    }

    /// The directory `uiprobe_hooks::paths::data_dir` returns during the run.
    #[must_use]
    pub fn data_dir(&self) -> PathBuf {
        self.root.join(DATA_DIR)
    }

    /// Whether `path` lies inside the profile root (comparing normalised text, then the
    /// canonical form of the deepest existing ancestor).
    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        let root_text = normalise(&self.root);
        if is_within(&normalise(path), &root_text) {
            return true;
        }
        let Ok(canonical_root) = fs::canonicalize(&self.root) else {
            return false;
        };
        let canonical_root = normalise(&canonical_root);
        let mut probe = Some(path);
        while let Some(candidate) = probe {
            if let Ok(canonical) = fs::canonicalize(candidate) {
                return is_within(&normalise(&canonical), &canonical_root);
            }
            probe = candidate.parent();
        }
        false
    }

    /// Aborts the run unless every path is inside the profile. Before aborting it leaves the
    /// profile and deletes it (best effort), so an aborted run leaves nothing in the temp dir.
    pub fn assert_inside(&self, what: &str, paths: &[PathBuf]) {
        for path in paths {
            if !self.contains(path) {
                let _ = std::env::set_current_dir(&self.original_cwd);
                let _ = fs::remove_dir_all(&self.root);
                abort_run(&format!(
                    "the {what} would be {} which is outside the profile {}. The app resolved \
                     it from something the profile does not redirect (Windows Known Folders, \
                     a hard-coded path): under your ui-harness feature ask \
                     uiprobe_hooks::paths::data_dir() first, or take a read-only path \
                     out of Profile::guard_paths",
                    path.display(),
                    self.root.display()
                ));
            }
        }
    }

    /// Deletes the directory `dir` and everything below it, but only when it lies inside the
    /// profile and is not the profile root itself. Returns whether it was deleted.
    #[expect(
        clippy::must_use_candidate,
        reason = "deleting is the point; the flag is informational and callers may ignore it"
    )]
    pub fn wipe_dir(&self, dir: &Path) -> bool {
        if !self.contains(dir) || self.is_root(dir) {
            return false;
        }
        fs::remove_dir_all(dir).is_ok()
    }

    /// Whether `dir` is the profile root itself, by text or by canonical form (a symlinked or
    /// 8.3-shortened temp dir names the root differently).
    fn is_root(&self, dir: &Path) -> bool {
        if normalise(dir) == normalise(&self.root) {
            return true;
        }
        match (fs::canonicalize(dir), fs::canonicalize(&self.root)) {
            (Ok(a), Ok(b)) => normalise(&a) == normalise(&b),
            _ => false,
        }
    }

    /// Deletes the file `path` when it lies inside the profile. Returns whether it was deleted.
    #[expect(
        clippy::must_use_candidate,
        reason = "deleting is the point; the flag is informational and callers may ignore it"
    )]
    pub fn wipe_file(&self, path: &Path) -> bool {
        if !self.contains(path) {
            return false;
        }
        fs::remove_file(path).is_ok()
    }

    /// Restores the original working directory and environment and deletes the profile. A
    /// leftover (a file still locked by a lingering thread) is reported, not fatal.
    pub(crate) fn finish(self) {
        let _ = std::env::set_current_dir(&self.original_cwd);
        #[cfg(target_os = "linux")]
        warn_if_threaded("stop the app's threads in Harness::teardown before the run ends");
        for (var, value) in &self.saved_env {
            // SAFETY: on Windows the environment calls are thread-safe. On Unix this is sound
            // only while no other thread reads the environment: every window was hidden and
            // dropped, the teardown has run, and the contract (module doc) makes the app stop
            // its threads there; Linux warns above when threads remain.
            unsafe {
                match value {
                    Some(original) => std::env::set_var(var, original),
                    None => std::env::remove_var(var),
                }
            }
        }
        if let Some(previous) = self.previous_data_dir.clone() {
            crate::hooks::paths::set_data_dir(previous);
        }
        if let Err(e) = fs::remove_dir_all(&self.root) {
            eprintln!(
                "uiprobe: could not delete profile {}: {e}",
                self.root.display()
            );
        }
    }
}

/// Whether normalised `path` is `root` or below it (a path-component boundary, so a sibling
/// directory sharing a name prefix does not count).
fn is_within(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Forward-slash text of a path without the Windows verbatim prefix; lower-cased on Windows
/// only (other file systems are case-sensitive).
fn normalise(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = text.strip_prefix("//?/").unwrap_or(&text);
    let text = text.trim_end_matches('/');
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{is_within, normalise};
    use std::path::Path;

    #[test]
    fn within_respects_component_boundaries() {
        assert!(is_within("/x/a/b", "/x/a"));
        assert!(is_within("/x/a", "/x/a"));
        assert!(!is_within("/x/ab", "/x/a"));
    }

    #[test]
    fn normalise_strips_verbatim_prefix_and_trailing_slash() {
        assert_eq!(
            normalise(Path::new(r"\\?\C:\Temp\")),
            normalise(Path::new("C:/Temp"))
        );
    }

    #[test]
    fn normalise_keeps_case_off_windows() {
        if !cfg!(windows) {
            assert_ne!(normalise(Path::new("/A")), normalise(Path::new("/a")));
        }
    }
}
