//! Slint's testing backend: installing it, pumping its event loop, waiting on the app.
//!
//! The backend is initialised with mock time: timers and animations advance only through
//! [`pump`], while work done by the app's own threads (loads, solves, searches) runs in real
//! time and hands its results back through the event queue, which [`pump`] drains. The scale
//! factor is 1 (logical and physical pixels are the same) unless a screenshot run changes it
//! with `screenshot::set_window_size`.

use crate::{ElementHandle, timing};
use std::{
    sync::Once,
    time::{Duration, Instant},
};

static INIT: Once = Once::new();

/// What draws the windows of a run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Renderer {
    /// The testing backend's own mock renderer: fixed font metrics, nothing is ever drawn. The
    /// default, and the fastest.
    #[default]
    Mock,
    /// Slint's software renderer with the system fonts, so `screenshot::capture` (and
    /// `slint::Window::take_snapshot`) returns the real picture. Needs the `screenshots`
    /// feature.
    #[cfg(feature = "screenshots")]
    Software,
}

/// The message of an init that found another platform already installed.
const PLATFORM_ALREADY_SET: &str = "the Slint platform was already set before the harness \
     could install the testing backend. An app driven by the harness must not call \
     BackendSelector::select or slint::platform::set_platform in the code the harness runs: \
     keep the backend choice in the binary's main, not in the window constructor the scenarios \
     launch";

/// Installs the testing backend with mock time and the mock renderer, before any window exists.
///
/// The runner calls it after the profile is in place. Slint accepts one platform per process,
/// so later calls (a second `Harness::run` in the same process, as the self-tests do) do
/// nothing.
///
/// # Errors
///
/// Returns an error when another platform was already installed (the app selected a backend
/// itself); the run cannot continue then.
pub fn init_backend() -> Result<(), String> {
    init_backend_with(Renderer::Mock)
}

/// [`init_backend`] with a choice of [`Renderer`]. Only the first call of a process has an
/// effect: a later call with another renderer keeps the first one.
///
/// # Errors
///
/// As [`init_backend`].
pub fn init_backend_with(renderer: Renderer) -> Result<(), String> {
    let mut result = Ok(());
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        INIT.call_once(|| result = install(renderer));
    }));
    match caught {
        Ok(()) => result,
        Err(_) => Err(PLATFORM_ALREADY_SET.to_owned()),
    }
}

/// Installs the platform for `renderer` (called once).
fn install(renderer: Renderer) -> Result<(), String> {
    match renderer {
        Renderer::Mock => {
            i_slint_backend_testing::init_integration_test_with_mock_time();
            Ok(())
        }
        #[cfg(feature = "screenshots")]
        Renderer::Software => {
            use i_slint_backend_testing::{TestingBackend, TestingBackendOptions};
            // The same options as `init_integration_test_with_mock_time`, plus the rasterizer.
            let backend = TestingBackend::new(TestingBackendOptions {
                mock_time: true,
                threading: true,
                renderer_name: Some("software".into()),
            });
            slint::platform::set_platform(Box::new(backend))
                .map_err(|_| PLATFORM_ALREADY_SET.to_owned())
        }
    }
}

/// Advances mock time by `ms` (firing due timers and animations) and then runs every event the
/// app's threads have queued so far.
///
/// The real time it takes is recorded under the timing label `pump` (see [`crate::timing`]).
pub fn pump(ms: u64) {
    let start = Instant::now();
    pump_unrecorded(ms);
    timing::record_str("pump", start.elapsed(), false);
}

/// [`pump`] without a timing sample: the waits pump in a loop and record their own total.
fn pump_unrecorded(ms: u64) {
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(ms));
    drain_events();
}

/// Runs the events already in the queue: a quit is queued behind them, so the loop returns as
/// soon as it reaches it.
pub fn drain_events() {
    if slint::quit_event_loop().is_err() {
        return;
    }
    let _ = slint::run_event_loop();
}

/// Pumps (10 ms of mock time, then a short real-time sleep so worker threads get to run) until
/// `condition` holds or `timeout` of real time has passed.
///
/// Returns whether it held. The call is recorded under the timing label `wait`, as a timeout
/// sample when it gave up.
pub fn wait_until(timeout: Duration, condition: impl FnMut() -> bool) -> bool {
    wait_until_labeled("wait", timeout, condition)
}

/// [`wait_until`] that records its time under `label` instead of `wait` (so a slow wait can be
/// told from the others in the timing statistics).
pub fn wait_until_labeled(label: &str, timeout: Duration, condition: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    let held = wait_loop(timeout, condition);
    timing::record_str(label, start.elapsed(), !held);
    held
}

/// The polling loop of the waits: 10 ms of mock time per round, a 5 ms real sleep between rounds.
fn wait_loop(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    loop {
        pump_unrecorded(10);
        if condition() {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Pumps for `real_ms` of real time in 10 ms mock steps; lets background work settle. Recorded
/// under the timing label `settle` (not also `wait`).
pub fn settle(real_ms: u64) {
    let start = Instant::now();
    let _ = wait_loop(Duration::from_millis(real_ms), || false);
    timing::record_str("settle", start.elapsed(), false);
}

/// Returns the explanation, with the three ways to switch debug info on, when the Slint build
/// carries no element debug info.
///
/// Without it the accessibility tree cannot be walked and every query would silently return
/// nothing. The testing backend reports no type name for any element without debug info, while
/// with it the root always has one, even in an empty window.
///
/// # Errors
///
/// The explanation text, for the caller to turn into an abort request.
pub fn require_debug_info(root: &ElementHandle) -> Result<(), String> {
    if root.type_name().is_none() {
        return Err(
            "the window exposes no elements, so the UI was compiled without Slint debug info, \
             which the element API needs. Switch it on in one of three ways and rebuild: (1) in \
             build.rs call `.with_debug_info(true)` on the `slint_build::CompilerConfiguration` \
             when your `ui-harness` feature is on, and run `cargo test --features ui-harness \
             --test ui`; (2) set SLINT_EMIT_DEBUG_INFO=1 in the environment while compiling \
             (the `slint!` macro and slint-build both read it); (3) put `[env] \
             SLINT_EMIT_DEBUG_INFO = \"1\"` into .cargo/config.toml. If the window still shows \
             no elements, `cargo clean -p <your crate>` so the UI is compiled again."
                .to_owned(),
        );
    }
    Ok(())
}
