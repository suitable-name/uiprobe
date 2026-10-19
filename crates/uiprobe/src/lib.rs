//! A headless UI harness for Slint apps on Slint's testing backend.
//!
//! It drives the real app (all its Rust wiring) inside a throw-away profile and collects every
//! problem into a report instead of stopping at the first: an invariant sweep of the window
//! geometry (`outside-window`, `overlap`, `zero-size`, `missing-label`,
//! `invisible-but-enabled`), a click-everything smoke pass (panics, error notices, closed
//! windows), an optional coverage gate over the `.slint` sources, and the scenario's own
//! expectations. Exit code 0 means no findings, 1 findings, 2 an aborted run.
//!
//! `Harness::new()` MUST be the first statement of the test's `main`: the run redirects the
//! environment (`set_var`), which is sound only while the process is single-threaded.
//!
//! A complete test (`tests/ui.rs` of an app, a `[[test]]` with `harness = false`):
//!
//! ```no_run
//! use uiprobe::prelude::*;
//!
//! slint::slint! {
//!     import { Button } from "std-widgets.slint";
//!     export component MainWindow inherits Window {
//!         in-out property <int> count;
//!         default-font-family: "FixedTestFont";
//!         Button { text: "Add"; clicked => { root.count += 1; } }
//!     }
//! }
//!
//! fn main() -> ExitCode {
//!     Harness::new()
//!         .scenario("add once", |ctx| {
//!             let app = ctx.launch(MainWindow::new)?;
//!             ctx.sweep(&app, "startup");
//!             ctx.view(&app).click("Add")?;
//!             pump(20);
//!             let count = app.component::<MainWindow>().map_or(-1, MainWindow::get_count);
//!             ctx.expect(count == 1, "count", format!("count is {count}, wanted 1"));
//!             ctx.shutdown(app);
//!             Ok(())
//!         })
//!         .scenario("click everything", |ctx| {
//!             ctx.smoke(Smoke::new("startup"), |ctx| Ok(vec![ctx.launch(MainWindow::new)?]));
//!             Ok(())
//!         })
//!         .run()
//! }
//! ```
//!
//! The doc tests compile this example and do not run it: a run needs the UI compiled with
//! Slint debug info (`SLINT_EMIT_DEBUG_INFO=1`, or `with_debug_info(true)` in `build.rs`),
//! otherwise the element API sees nothing and the run aborts with that explanation.
//!
//! Where things are:
//!
//! * [`Harness`] builds and runs; [`Ctx`] is what a scenario receives; [`Attached`] is a
//!   launched window; the [`prelude`] has all of them.
//! * [`view`]: finding and clicking elements by label, placeholder, id or role.
//! * [`targeting`]: where to click and what to scroll: `Spot`s with their visible rectangle,
//!   queries that fail on ambiguity, and scroll plans ([`view::geometry`] holds the pure math).
//! * [`checks`]: the geometry sweep and its thresholds ([`checks::SweepOptions`]).
//! * [`smoke`]: the click-everything pass, its deny rules and counters.
//! * [`coverage`]: the opt-in gate over the `.slint` sources and its allow list.
//! * [`profile`]: the throw-away profile, guarded paths and aborts.
//! * [`report`]: findings, the report formats and files.
//! * [`hooks`]: the app-side seams (the `uiprobe-hooks` crate): file dialogs, clipboard,
//!   opener, data directory.
//! * [`backend`]: pumping mock time and waiting on the app.
//! * [`timing`]: where a run spends its real time (per-scenario samples, console line, report
//!   section).
//! * `recording` (feature `recording`): MP4 videos of an app run with a drawn cursor, captions
//!   and chapters.
//!
//! The README (the crates.io page and the repository root) is the setup guide: the quick-start
//! checklist, every hook, the coverage gate, troubleshooting, and the `examples/counter` and
//! `examples/notes` apps. `docs/cookbook.md` in the repository holds recipes.

pub mod backend;
pub mod checks;
pub mod coverage;
mod cpu;
mod ctx;
pub mod keys;
mod panics;
mod parallel;
pub mod profile;
#[cfg(feature = "recording")]
pub mod recording;
mod render;
pub mod report;
mod runner;
mod scene;
#[cfg(feature = "screenshots")]
pub mod screenshot;
pub mod smoke;
pub mod targeting;
pub mod timing;
mod timing_report;
pub mod view;

pub use ctx::{Attached, Ctx, Hooks};
pub use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementRoot};
pub use panics::take_panics;
pub use runner::{Harness, Jobs, target_dir_from};
pub use uiprobe_hooks as hooks;

/// The exact Slint version this release of the harness is built against.
///
/// It is the `=` pin on `slint` and `i-slint-backend-testing` in the harness's own manifest. The
/// crate's minor version tracks Slint's (`uiprobe 0.18.x` serves Slint 1.18.x), which the unit test
/// in this file enforces. The first console line of every run and the `slint` field of
/// `report.json` show it.
pub const SLINT_VERSION: &str = "1.18.1";

/// Everything a harness `main` and its scenarios usually need, `ExitCode` for the `main`
/// signature included.
pub mod prelude {
    #[cfg(feature = "recording")]
    pub use crate::recording::{Recorder, RecorderOptions};
    pub use crate::{
        AccessibleRole, Attached, Ctx, ElementHandle, ElementRoot, Harness, Hooks, Jobs,
        SLINT_VERSION,
        backend::{Renderer, pump, settle, wait_until, wait_until_labeled},
        checks::SweepOptions,
        coverage::{
            Coverage, GateRule, arm_gate, record_callback, record_site_id, record_wrappers,
        },
        profile::{Profile, ProfileDir},
        report::{Finding, Format, Report},
        smoke::{DEFAULT_DENY, DenyField, DenyMatch, DenyRule, Smoke, SmokeStats},
        timing,
        view::{Control, WindowView},
    };
    pub use std::process::ExitCode;
}

/// The cargo target directory of the calling crate.
///
/// Integration tests and benches get it from the compile-time `CARGO_TARGET_TMPDIR`; any other target (a bin, a unit test) falls back to
/// the run-time variable, then to the test binary's location. A macro because `option_env!`
/// expands in the crate being compiled: inside this library it would name the harness crate.
#[macro_export]
macro_rules! target_dir {
    () => {
        $crate::target_dir_from(::core::option_env!("CARGO_TARGET_TMPDIR"))
    };
}

#[cfg(test)]
mod tests {
    use super::SLINT_VERSION;

    /// `uiprobe 0.N.x` serves Slint 1.N.x: the second version component must agree.
    #[test]
    fn crate_minor_tracks_slint_minor() {
        let crate_minor = env!("CARGO_PKG_VERSION").split('.').nth(1);
        let slint_minor = SLINT_VERSION.split('.').nth(1);
        assert_eq!(
            crate_minor,
            slint_minor,
            "uiprobe {} serves Slint {SLINT_VERSION}: the minor versions must agree",
            env!("CARGO_PKG_VERSION")
        );
    }
}
