# uiprobe

A headless UI harness for [Slint](https://slint.dev) apps. It drives your real app, with all its
Rust wiring, on Slint's testing backend inside a throw-away profile, and collects every problem
into one report instead of stopping at the first: an invariant sweep of the window geometry, a
click-everything smoke pass, an optional coverage gate over your `.slint` sources, and your own
scenario expectations. Reports are written as JSON plus, by default, Markdown; HTML and plain
text are a builder call or an environment variable away (see Report formats). Exit code 0 means
no findings, 1 findings, 2 an aborted run.

**Read this first.** `Harness::new()` must be the first statement of your test's `main`. The run
redirects the environment (`std::env::set_var`) into a temporary profile, which is sound only
while the process is single-threaded, so nothing may start a thread before it. On Linux the
harness warns when it finds more than one thread. At the end of the run the environment is
restored the same way, so stop the app's threads in `Harness::teardown`; on Unix a thread still
reading the environment at that point is a data race. Several runs in one process are fine under
that rule.

Contents: Quick start, Examples, Hooks, Isolation, Aborts, Checks, Report formats, Console
output, Parallel runs, Screenshots, Recording Videos (zoom camera and keystroke badges, filmed
input, text guard and pacing, element targeting), Coverage gate, Environment variables, Builder
reference, Troubleshooting, Limits, Versions and the Slint pin, License.

## Quick start

Five steps; each says what goes wrong when it is skipped. `examples/counter` is this recipe in
the smallest app; `examples/notes` is the full one (see Examples).

1. **Cargo.toml.** The harness is a dev-dependency. The small `uiprobe-hooks` crate
   (std only, no Slint) is an optional normal dependency behind a `ui-harness` feature,
   because code under `src/` can never name a dev-dependency. The test target needs
   `harness = false` (the harness owns `main`: panic recorder, exit code, argument filter)
   and `required-features`, so it is compiled only with the feature on.

   ```toml
   [features]
   ui-harness = ["dep:uiprobe-hooks"]

   [dependencies]
   slint = "=1.18.1"
   uiprobe-hooks = { version = "0.1", optional = true }

   [build-dependencies]
   slint-build = "=1.18.1"

   [dev-dependencies]
   uiprobe = "=0.18.8"

   [[test]]
   name = "ui"
   harness = false
   required-features = ["ui-harness"]
   ```

   Skipped `harness = false`: `cargo test --test ui` prints `running 0 tests` and your `main`
   never runs. Skipped `required-features`: with the feature off the UI has no debug info and
   the run aborts with exit code 2 (the abort names the fix).

2. **build.rs.** Switch Slint debug info on when the feature is on; the harness's element
   queries need it. Leave the default alone otherwise: it honours `SLINT_EMIT_DEBUG_INFO`, so
   a `with_debug_info(false)` would switch that path off.

   ```rust
   fn main() {
       let mut config = slint_build::CompilerConfiguration::new();
       if std::env::var_os("CARGO_FEATURE_UI_HARNESS").is_some() {
           config = config.with_debug_info(true);
       }
       slint_build::compile_with_config("ui/app.slint", config).expect("the UI compiles");
   }
   ```

   Skipped: the first launch aborts with `the window exposes no elements` and the three ways
   to fix it. With the `slint!` macro instead of build.rs, set `SLINT_EMIT_DEBUG_INFO=1`, for
   example through `[env]` in `.cargo/config.toml`.

3. **tests/ui.rs.** `Harness::new()` is the first statement of `main`; one `.scenario(..)` per
   journey; `.run()` returns the exit code.

   ```rust
   use my_app::MainWindow;
   use uiprobe::prelude::*;

   fn main() -> ExitCode {
       Harness::new()
           .sizes([(1024, 700), (640, 480)])
           .scenario("startup", |ctx| {
               let app = ctx.launch(MainWindow::new)?;
               ctx.sweep_at_sizes(&app, "startup");
               ctx.view(&app).click("Open")?;
               ctx.shutdown(app);
               Ok(())
           })
           .scenario("click everything", |ctx| {
               ctx.smoke(Smoke::new("startup"), |ctx| Ok(vec![ctx.launch(MainWindow::new)?]));
               Ok(())
           })
           .run()
   }
   ```

   Scenarios return `Result<(), Finding>`: an `Err` (from `?` on a failed query, a failed
   launch ...) is recorded as a finding and the run goes on with the next scenario; a panic in
   a scenario becomes a `scenario-panic` finding. A harness with no scenario is a `config`
   finding, so an empty skeleton cannot pass by accident.

4. **Run it.** `cargo test --features ui-harness --test ui`. Cargo skips a target whose
   `required-features` are off without a word, so a plain `cargo test` runs no UI test; in CI
   use `cargo test --all-features`, and locally an alias in `.cargo/config.toml` helps:

   ```toml
   [alias]
   ui = "test --features ui-harness --test ui"
   ```

   The first console line names the harness and Slint versions, the selected scenarios and the
   report directory (`<target>/uiprobe/<package>`); a failing run ends with its
   findings.

5. **Hooks**, as soon as the app has a file dialog, a clipboard, an opener or an OS-resolved
   data directory: see Hooks below. Without them a click that opens a native dialog opens a
   real one and blocks the run.

## Examples

- `examples/counter`: the recipe above in the smallest app (one window, two buttons): a sweep
  at two sizes, a click journey with an expectation, a click-everything pass.
  `cargo test -p counter --features ui-harness --test ui`.
- `examples/notes`: a small notes app that uses every seam: file dialogs answered from a queue,
  clipboard and opener captured, the settings file in the harness's data directory and
  guarded, an error notice read and cleared by the harness, a scenario deny word and an
  expected notice in the smoke pass, `record_callback`, the coverage gate with a TOML allow
  file, and all three report formats. `cargo test -p notes --features ui-harness --test ui`,
  then open `target/uiprobe/notes/report.html`.

## Hooks: file dialogs, clipboard, opener, data directory

Call these only under your feature. Without them everything still works, except that a click
which opens a native file dialog opens a real one and blocks the run.

```rust
// A file picker: the feature-on arm answers from the queue, the feature-off arm is the real
// dialog. Keep the two arms in one function (or give the feature-off arm a `const fn` twin
// when the function must stay `const`), so every call site is identical in both builds.
fn pick_file(title: &str) -> Option<std::path::PathBuf> {
    #[cfg(feature = "ui-harness")]
    {
        uiprobe_hooks::picker::next(title)
    }
    #[cfg(not(feature = "ui-harness"))]
    {
        rfd::FileDialog::new().set_title(title).pick_file()
    }
}

// Clipboard and opener: return early when the harness captured the call.
fn copy_text(text: &str) {
    #[cfg(feature = "ui-harness")]
    if uiprobe_hooks::clipboard::capture(text) {
        return;
    }
    // ... the real clipboard write
}

fn open_url(url: &str) {
    #[cfg(feature = "ui-harness")]
    if uiprobe_hooks::opener::capture(url) {
        return;
    }
    // ... start the OS opener
}

// Apps that resolve their config directory through the OS (the `dirs` crate on Windows uses
// Known Folders and ignores APPDATA) ask the harness first:
fn data_dir() -> std::path::PathBuf {
    #[cfg(feature = "ui-harness")]
    if let Some(dir) = uiprobe_hooks::paths::data_dir() {
        return dir;
    }
    // ... the normal lookup
    std::path::PathBuf::new()
}
```

In a scenario: `ctx.hooks().queue_pick(path)` / `queue_cancel()` answer the next dialog;
`take_clipboard_writes()`, `take_opened()` and `take_picker_failures()` read back what the app
did. A dialog opened with nothing queued answers "cancelled" and is an `unexpected-file-dialog`
finding.

The same pattern, compiled and driven, is `examples/notes/src/lib.rs`; the feature-off arms
there are stubs because the example ships no dialog crate.

## Isolation

`Profile::isolated()` creates `<temp>/uiprobe-<pid>-<nanos>`, points `APPDATA`,
`LOCALAPPDATA`, `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`,
`XDG_RUNTIME_DIR`, `HOME`, `USERPROFILE`, `TEMP`, `TMP` and `TMPDIR` into it, sets
`uiprobe_hooks::paths::set_data_dir` and moves the working directory into it. Windows Known
Folders are not reachable through the environment: an app that uses them asks
`uiprobe_hooks::paths::data_dir` (above). Environment redirection alone does not prove your
app obeys it, so hand over the paths the app really resolves:

```rust
.profile(Profile::isolated().guard_paths(|| vec![my_app::settings_path(), my_app::database_path()]))
```

Any path outside the profile aborts the run with exit code 2 before a window exists. The profile
is deleted at the end, and the environment and working directory are restored.

## Aborts

Exit code 2 ends the run early: a guarded path outside the profile, a Slint platform the app
installed itself (the app must not select a backend when driven by the harness), or a UI without
debug info. In every case the profile is deleted and the environment restored first, and the
message says what to change. A scenario can request the same through
`uiprobe::profile::request_abort`. No report file is written for an aborted run: the cause
is on stderr, the findings made so far on stdout only.

## Checks

| Check | Meaning |
|---|---|
| `outside-window` | an element sticks out of the window (scrolled-out content is accounted for) |
| `zero-size` | an interactive control has no size |
| `overlap` | two controls overlap (header and modal backdrops and floating layers are accounted for) |
| `missing-label` | an interactive control has no accessible label or placeholder |
| `invisible-but-enabled` | an enabled control is (nearly) transparent |
| `panic` | a panic on any thread, caught by the recorder |
| `error-toast` | the app's error or warning notice (`ctx.on_error_notice`) was showing |
| `main-closed` | the main window closed during the smoke pass |
| `window-invalid` | a window could not be queried |
| `launch` | the app did not start |
| `timeout` | `ctx.wait_for` did not see its condition |
| `scenario-panic` | a scenario panicked; it ended there |
| `unexpected-file-dialog` | a native dialog opened with no queued answer |
| `bad-chord` | `ctx.press` got a key chord it cannot parse |
| `not-found` | a `WindowView::find*` or `spot_*` query matched nothing (similar labels are listed for `find*`) |
| `ambiguous-label` | `WindowView::find` matched several elements, or a `spot_*` targeting query found several different visible controls |
| `not-visible` | a `spot_*` targeting query found the element but less than 60 % of it shows (from `TargetError::finding`) |
| `disabled` | a `spot_*` targeting query found the element but it is not enabled (`accessible-enabled: false`; from `TargetError::finding`) |
| `coverage` | an uncovered UI action, or a stale allow-list entry |
| `worker-crash` | a parallel-run worker process died during this scenario |
| `config` | a bad environment variable, an unmatched UIPROBE_FILTER, a harness without scenarios, or a report file that could not be written or removed |

Identical findings are merged with a repeat count. The report lists every scenario with its
finding count and wall time.

## Report formats

Every run writes `report.json` (the harness and Slint versions, findings with repeat counts,
scenarios with timings, notes, sections, per-check totals). Beside it, one file per selected
`Format`:

| Format | File | For |
|---|---|---|
| `Format::Markdown` (default) | `report.md` | reading and diffing; findings grouped by window, then by check |
| `Format::Html` | `report.html` | one self-contained page: inline style, no script, no external asset; a `main` landmark and real headings for screen readers, windows collapsible, a light or dark color scheme following the system setting |
| `Format::Text` (`raw`) | `report.txt` | terminals and CI logs; the Markdown without markup (section bodies, which the coverage gate writes as Markdown, are copied as they are) |

`Harness::formats([Format::Markdown, Format::Html])` selects them in code;
`UIPROBE_FORMATS=md,html,txt` replaces that list (`json` is accepted and ignored because
it is always written; an empty value means JSON only). A run removes the `report.md`,
`report.html` and `report.txt` it did not select, so the directory always shows what the last
run wrote. The finding texts are the same in every format.

## Console output

Every line starts with `uiprobe`. In order: the banner (`uiprobe 0.18.8 (Slint
1.18.1): 7 of 7 scenario(s) selected, report dir ...`), the profile directory and the guarded
paths, one `scenario <name>` line per scenario with its `-> N finding(s), M ms` and a `timing:`
line (see Timing), the smoke counters, the coverage counters, the report files written, then `PASS, no findings` or `FAIL,
N finding(s):` with the per-check counts and up to 25 findings as `[window] check: element --
detail (xN) [state]` (the rest: `... N more in <dir>`). Aborts go to stderr as
`uiprobe: ABORT: <cause and what to change>`. No color, no cursor movement: the output
reads the same in a CI log and in a screen reader.

### Timing

A run that takes hours with almost no CPU is waiting, and the report says on what. Under each
scenario's `-> N finding(s), M ms` line the console prints the labels that took the most real
time:

```text
uiprobe:   -> 0 finding(s), 51234 ms
uiprobe:   timing: relaunch 31.2 s (45x), wait 12.0 s (210x, 3 timed out 9.0 s), settle 4.1 s (80x)
```

Each label is `<name> <total> (<samples>x[, <n> timed out <time>])`, sorted by total, at most six.
The backend records three labels by itself: `wait` (every `wait_until`, `ctx.wait_for`; a call that
gave up is a timeout sample), `settle` (`settle` is not also counted as `wait`) and `pump` (the
real time a direct `pump` call spent draining events; the pumps inside a wait belong to the
wait). Your own code adds more:

```rust
use uiprobe::prelude::*;

timing::record("db-open", elapsed);                  // one sample
timing::record_timeout("relaunch", elapsed);         // a sample that hit its timeout
let app = timing::time("relaunch", || relaunch());   // times a closure
wait_until_labeled("save-done", timeout, || saved()); // a wait with its own label
```

`report.json` stores the table per scenario (`scenarios[i].timing`: label to `count`,
`total_ms`, `timeouts`, `timeout_ms`) and the run's totals plus, for a parallel run, the workers
(`timing.totals`, `timing.parallel` with `workers_started`, `peak_workers`, `wall_ms`,
`scenario_ms`, `utilisation_percent`). The report files get a "Timing" section: the 15 slowest
scenarios by wall time with their three biggest labels, the time per label over the run, the
timeouts per label and, for a parallel run, the workers used and the utilisation (the sum of the
scenario times over peak workers times wall time). The tables are per scenario: samples are
reset when a scenario begins and taken when it ends, so work done between scenarios (the
teardown) is not counted.

## Parallel runs

Opt-in; the default is the serial run described so far. A suite with many scenarios, long waits
(`ctx.wait_for`, threads that settle slowly) and a machine with idle cores finishes sooner when
scenarios run side by side. A suite whose scenarios are short and CPU-bound gains little, and
the extra processes cost start-up time.

```rust
Harness::new()
    .jobs(Jobs::Auto)      // or Jobs::Fixed(4); Jobs::Serial is the default
    .max_jobs(4)           // cap for Auto and Fixed; default: the number of cores
    // ... scenarios ...
```

`UIPROBE_JOBS=serial|1|auto|<n>` replaces `jobs` and `UIPROBE_JOBS_MAX=<n>` (at
least 1) replaces `max_jobs`, so `UIPROBE_JOBS=auto cargo test --test ui` needs no code
change; an invalid value is a `config` finding and the configured value is kept.

How it works. Threads are not an option (Slint's platform, the profile's environment redirect
and the hooks are per process), so the test binary becomes a coordinator that starts copies of
itself as worker processes. Each worker has its own throw-away profile, environment, hooks,
panic recorder and mock time, and runs one scenario at a time. `Jobs::Fixed(n)` starts `n`
workers at once; `Jobs::Auto` starts one and adds another every 5 seconds while at least 1.5
cores were idle over the last interval and scenarios are still waiting, up to the cap. The
number of workers never exceeds the number of selected scenarios; one scenario, or `Fixed(1)`,
runs serial.

What stays the same. The console shows each scenario's output in scenario order, not in
completion order; output outside scenarios (worker setup and teardown) is printed as it arrives,
prefixed `uiprobe: worker <k>: `. The result is one report: findings, repeat counts,
per-check totals and the scenario list are identical to a serial run of the same suite, with one
extra note, `parallel run: <n> worker(s) used, peak <p> at once`. The coverage gate sees the
union of what all workers reached.

A scenario that needs that union itself (a pass that invokes the callbacks nothing else
covered) is registered with `Harness::scenario_last`. It runs after every ordinary scenario; in a
parallel run it is dispatched only when all ordinary scenarios are done (workers without work wait
meanwhile) and receives the union of the coverage recorders of all workers, so
`coverage::recorded_callbacks()` shows what the whole run covered, not what its own worker did.
Several last scenarios run side by side.

Rules for your `main`. A parallel run re-executes `main` in every worker, so `main` must build
the same harnesses in the same order and do nothing before them that must not run twice;
`Harness::new()` as the first statement is already the rule. A test with several harnesses in
one `main` works. A worker reports a scenario list that differs from the coordinator's (a
scenario name behind an index does not match) as a `config` finding for that scenario.

Crashes and aborts. A worker that dies during a scenario (`process::abort`, `panic = "abort"`, a
stack overflow) gives a `worker-crash` finding for that scenario, with the exit status and the
worker's last output lines; the other scenarios still run on a replacement worker. The dead
worker's profile directory may be left behind in the temp dir, and the finding says so. A worker
that dies before it is ready three times in a row aborts the run (exit code 2). An abort
requested by a scenario (`request_abort`) or raised during worker setup stops the whole run: the
other workers finish their current scenario, run the teardown and delete their profiles, then
the run ends with exit code 2.

Keep a suite serial when its numbers are compared against a baseline taken from a serial run
(finding counts do not change, wall times and console interleaving do), or when the machine is
the limit: a GPU-heavy app is slowed by workers sharing one GPU, so cap them with
`UIPROBE_JOBS_MAX`.

## Screenshots

With the `screenshots` feature (`uiprobe = { version = "=0.18.8", features =
["screenshots"] }`) a harness can draw its windows with Slint's software renderer and save them as
PNG, for example to keep the screenshots of a README or a website in step with the UI:

```rust
Harness::new()
    .renderer(Renderer::Software)
    .scenario("main window", |ctx| {
        let app = ctx.launch(MainWindow::new)?;
        screenshot::set_window_size(app.window(), (1920, 1152), 2.0); // HiDPI picture
        pump(50);
        let shot = screenshot::capture(app.window()).map_err(|e| Finding::simple("shot", e))?;
        shot.save_png(std::path::Path::new("assets/main.png"))
            .map_err(|e| Finding::simple("shot", e))?;
        ctx.shutdown(app);
        Ok(())
    })
    .run()
```

- `screenshot::capture(window)` draws the window and returns a `Shot` (physical pixels, RGBA8);
  with the default `Renderer::Mock` it returns an error, because the mock renderer draws nothing.
- `Shot::crop(x, y, w, h)` (physical pixels), `crop_logical` (logical window pixels) and
  `crop_to(&element, margin)` cut a part out; `save_png(path)` writes it through a temporary file,
  so a failed run never leaves half a picture. `Shot::blank`, `Shot::fill` and `Shot::paste` compose captures
  (a dialog over its main window: headless windows are separate surfaces).
- `screenshot::set_window_size(window, (w, h), scale)` sets the logical size and the scale
  factor; the picture is `w * scale` x `h * scale` pixels.
- The software renderer uses the system fonts, so pictures differ between machines; geometry
  checks keep using the mock renderer. It draws only when `capture` asks, and there is no window
  frame or title bar in the picture. Pump (or `wait_until` the app's own threads) before a
  capture so layout, images and animations have settled.

## Recording Videos

With the `recording` feature (`uiprobe = { version = "=0.18.8", features = ["recording"] }`, which
implies `screenshots`) a harness can play the app and write an MP4 with a drawn mouse cursor, click
ripples, captions and YouTube chapters, for example to keep tutorial videos in step with the UI.
It needs `Harness::renderer(Renderer::Software)`, an even physical window size and ffmpeg at run
time (`ffmpeg` on `PATH`, or the full path in `UIPROBE_FFMPEG`).

```rust
use uiprobe::recording::{Recorder, RecorderOptions};

screenshot::set_window_size(app.window(), (1920, 1080), 2.0); // 3840 x 2160 pixels
pump(50);
let mut rec = Recorder::start(app.window(), RecorderOptions {
    output: "videos/demo.mp4".into(),
    encode_size: Some((1920, 1080)), // scaled down while encoding
    ..RecorderOptions::default()
})?;
rec.chapter("Intro");
rec.caption("Press the button");
rec.hold(Duration::from_secs(1))?;
rec.move_to_element(&button, Duration::from_millis(700))?; // glide, then a hover event
rec.click()?;                                              // ripple only, nothing is pressed
// invoke the button's callback yourself, then keep filming:
rec.hold(Duration::from_millis(400))?;
let summary = rec.finish()?; // demo.mp4, demo.srt, demo.chapters.txt
```

- Video time and Slint's mock time run together in `hold`, `move_to`, `move_to_element` and the
  zoom calls: every frame pumps one frame of mock time (33, 33, 34 ms at 30 fps), captures the
  window, draws the cursor over a copy and pipes the frame to ffmpeg. `hold(1 s)` at 30 fps is
  exactly 30 frames. Frames are never stored. The exception is the real-time waits (below).
- `move_to` / `move_to_element` glide with ease-in-out and end with one real `PointerMoved`
  event (`hover_events`), so hover styles show. `click` draws the ripple only: the action is
  yours to invoke, so a video never fires twice.
- Background work runs in real time. `wait_while_recording(timeout, speed, max_video, cond)` shows
  it as a time-lapse of at most `max_video` and then cuts; `wait_cut` shows nothing.
- `caption(text)` starts a cue at the current video time (empty text ends it), `chapter(title)`
  adds a chapter; `finish` writes `<stem>.srt` and `<stem>.chapters.txt` (the first chapter is
  forced to `0:00`).
- The video is written as `<output>.part` and renamed by `finish`; `abandon` (or dropping the
  recorder) kills ffmpeg and deletes the `.part`, so a failed run leaves the old video alone. An
  ffmpeg failure is returned with the tail of its stderr.
- `SinkKind::PngSequence(dir)` writes numbered PNGs instead (no ffmpeg); `SinkKind::Memory` keeps
  the frames for self-tests (`Recorder::frames`). The timing labels `recording-capture` and
  `recording-encode` show where a slow run spends its time.
- Build `RecorderOptions` (and `CameraOptions`, `BadgeStyle`) with `..Default::default()`: fields
  are added in patch releases (0.18.8 added four to `RecorderOptions`), so a literal that names
  every field stops compiling.
- The recorder borrows the window (`Recorder<'w>`): finish or abandon it before shutting the app
  down, and do not resize the window while recording (a changed size is an error).

### Zoom camera and keystroke badges

Small controls are hard to read in a 1080p video of a 1920 x 1080 layout, so a virtual camera can
zoom into the 4K capture (sharp: the crop is cut at the capture's resolution, then scaled to the
frame size with a bicubic filter; the output size never changes):

```rust
rec.zoom_to((820.0, 300.0, 280.0, 60.0), Duration::from_millis(700))?; // x, y, w, h, logical
rec.set_follow_cursor(true);        // a zoomed camera pans with move_to, the cursor stays in view
rec.move_to_element(&field, Duration::from_millis(700))?;
rec.press_keys(app.window(), "Ctrl+Up")?;   // real key events, and a "Ctrl + ↑" badge
rec.zoom_out(Duration::from_millis(600))?;
```

- `zoom_to` shows the region padded by `CameraOptions::margin` (24 logical px), grown to the
  frame's aspect ratio, limited to `CameraOptions::max_zoom` (2.0) and kept inside the window.
  `camera()` returns the current rectangle. At zoom 1.0 nothing is resampled. The cursor and the
  ripple are drawn in frame space and grow with the zoom (`scale_cursor: false` keeps them small).
- `show_keys("Ctrl + ↑")` draws a keycap badge near the bottom of the frame, not zoomed, for
  `BadgeStyle::visible_ms` (1400 ms, fading over the last 300 ms). Keys joined by ` + ` get a
  keycap each. The text is rendered by Slint's software renderer with the system fonts.

### Filmed input, text guard, pacing

- `click_at(window, at, origin)`, `type_text`, `scroll_at` and `press_keys` are the filmed input
  layer: each glides the cursor, shows the ripple or the badge and dispatches real Slint events to
  the window. **That dispatch is the one activation**; do not also invoke the callback (the video
  would fire it twice). `click()` stays ripple-only. `origin` is the position of an attached
  window in the frame (`(0.0, 0.0)` for the main window).
- `RecorderOptions::forbidden_texts` with `Recorder::set_text_source(Box::new(|| visible_texts))`
  stops the recording (an `Err` naming the fragment) when a visible text contains a fragment,
  case-insensitively; the half-written video is deleted. The texts are checked at the start and
  the end of every call that emits frames and at most every 250 ms of video in between, so a
  text that appears mid-hold can be in the frames (about 7 at 30 fps) before the next check, and
  one that goes again within an interval can be missed. Fragments without a source fail closed.
- `uiprobe::recording::pacing` has `reading_time`, `glide_duration`, `plain`, `wrap`, `cues`,
  `word_count` and `wheel_steps` for screencast pacing and captions.
- `wait_while_recording` and `wait_cut` pump the real time that passed (5 to 100 ms per poll), so
  an app's timers and tick budgets run at their real speed.
- `RecorderOptions::still_reuse_frames` (default 1 = off): still frames of a hold may reuse one
  full render of the window; with 4 to 8 holds are several times faster (a 4K software render is
  about 170 ms). The first frame of every call, recorder input, timers, animations and `refresh`
  always render; a result an app thread posts during a still hold can show up late by at most that
  many frames, because Slint's testing backend has no redraw signal. `captures()` and
  `reused_frames()` count the renders; the timing labels are `recording-capture` and
  `recording-reuse`.

### Where to click: targeting

`view.find("Save")` returns an element; for a *click at a position* (a recorded journey, a filmed
tutorial) ask for a `Spot`:

    let spot = view.spot_label("Save")?;          // Err(TargetError::Ambiguous { count: 2, .. }) if two visible controls say "Save"
    let (x, y) = spot.click_point().expect("checked by the query");

Inside a scenario `?` turns a `TargetError` into a `Finding` of window `-`; to name the window
write `.map_err(|e| e.finding("MainWindow"))?`. `TargetError` is `#[non_exhaustive]`, so a
`match` on it needs a wildcard arm.

A `Spot` knows which scroll viewports and which window edge cut its element off. `click_point()`
is the centre of the part that is visible, and the query is `NotVisible` when less than 60 % of
the element shows (a row scrolled half out of a table is not clicked at its centre, which lies
outside the viewport). Tell duplicates apart with
`spot_role(AccessibleRole::TextInput, "Name")`, `spot_within("Dialog", "OK")` or
`spot_label_near_y("Edit", row_y, row_height)`.

To bring an element into view, plan the wheel turn and perform it where the viewer can see it:

    if let Some((container, delta_y)) = uiprobe::targeting::scroll_into_view_plan(&spot) {
        recorder.scroll_at(window, container.center(), delta_y)?;
    }
    // then query again: the container may stop short, an outer one may need its own turn

A `Rectangle { clip: true }` cannot be detected; add it with `Spot::clipped_by`.

## Coverage gate

Opt-in: `Harness::coverage(Coverage::scan("ui").allow_file("tests/coverage_allow.toml"))`
scans every `.slint` under `ui/` (relative to the test crate's `CARGO_MANIFEST_DIR`) for
global callbacks and interactive element sites, and at the end of the run turns every one the
scenarios never reached into a `coverage` finding. A click through `WindowView::click` or the
smoke pass counts as activated; a `find*` counts as touched; a scenario that invokes a global
callback directly says so with `uiprobe::coverage::record_callback("Model.name")`.

Keys: `Model.name` for a callback; `file.slint|Component|id`, `file.slint|Component|"label"`
or `file.slint|Component|<Type@line>` for an element (the report's Coverage section prints
them). An element with neither an `id :=` nor a literal label is untrackable: listed, not a
finding. Allow entries come from code (`.allow(&[("Model.name", "why")])`) or the TOML file:

```toml
[[allow]]
key = "Model.unused"
reason = "driven by the OS tray"

[[allow]]
key = "dialog.slint|Confirm|*"   # trailing * = prefix
reason = "covered elsewhere"
```

`examples/notes/tests/coverage_allow.toml` is a real one.

An entry that matches nothing is a `coverage` finding too (stale). `GateRule` says when the
gate runs: `NoFilter` (default: skipped under a scenario filter), `Always`, or `Armed` (only
when a scenario called `uiprobe::coverage::arm_gate()`), set with `.gate_when(..)`.

### How an element is matched, and what to configure

An element is credited by, in order: the single-site rule (its label is computed, its element
type has exactly one site in the whole inventory and the run activated or fetched an element of
that type; an app component is also credited by the base names of the activated element), its
`Component::id`, then a `(type, label)` pair whose label matches the site's literal. Literals are
compared as the running element shows them: escapes (`\"`, `\\`, `\n`, `\u{2026}`) are resolved,
an interpolation (`"Layer \{i}"`) splits the literal into fixed pieces (at least three fixed
characters are needed), and the label must start and end with them unless the component adds to
the label. An element id behind `if cond:` or `for x in y:` is the text after the last colon, and
a one-line element with several literals gives each property its own.

- `Coverage::label_props(&["reader_label"])`: extra property names read as labels, for wrapper
  components that carry the announced label in a property of their own.
- `Coverage::reader_props(&["reader_label"])`: of those, the ones whose literal wins over `text`
  on a wrapper site (the running accessible label is the reader label when the site sets one).
  Naming a property here also reads it as a label. Both default to empty.
- `Coverage::callback_hint("add a row to the callback table")`: the advice at the end of a
  callback-gap finding (`callback never invoked (file:line); <hint>`).
- `uiprobe::coverage::record_wrappers(&view.root, &pressed, touched)`: a wrapper component
  carries the site id on its own root while the element a scenario presses is the widget inside
  it; this records the ids of every element at the pressed element's rectangle (0.6 px
  tolerance). The smoke pass calls it for every activation; call it yourself when a scenario
  presses a wrapper by hand.
- `uiprobe::coverage::record_site_id("Component::site-id")`: credits a site directly, for a
  press whose element reports another id.

A parallel run carries all of these recorder sets from the workers to the coordinator.

## Environment variables

| Variable | Effect |
|---|---|
| `UIPROBE_FILTER` | comma-separated scenario name substrings to run (positional arguments that do not start with `-` filter too); positional arguments that match no scenario run nothing and write no report (that is cargo handing another target's filter to this one), an unmatched `UIPROBE_FILTER` is a `config` finding |
| `UIPROBE_SIZES` | `1366x768,720x480`; replaces `Harness::sizes` |
| `UIPROBE_FORMATS` | `md,html,txt`: the report files besides `report.json`, which is always written; replaces `Harness::formats`; `json` is accepted and ignored, empty means JSON only, an unknown name is a `config` finding |
| `UIPROBE_JOBS` | `serial`, `1`, `auto` or a worker count: how many worker processes run the scenarios (Parallel runs); replaces `Harness::jobs`; default serial, an invalid value is a `config` finding |
| `UIPROBE_JOBS_MAX` | worker cap for `auto` and a fixed count, at least 1; replaces `Harness::max_jobs`; default the number of cores, an invalid value is a `config` finding |
| `UIPROBE_SEED` | `u64` for scenarios that randomise (`ctx.seed()`), default 0 |
| `UIPROBE_ALLOW_FAIL` | any value: exit code 0 even with findings |
| `UIPROBE_FFMPEG` | the ffmpeg executable of the `recording` feature (default `ffmpeg` on `PATH`) |
| `SLINT_EMIT_DEBUG_INFO` | `1` at compile time: Slint debug info |

Exit codes: 0 clean, 1 findings, 2 aborted (isolation could not be proven, no debug info).

## Builder reference

- `report_dir`: default `<target>/uiprobe/<package>` (`<target>` from
  `CARGO_TARGET_TMPDIR` under `cargo test`; one folder per package, so two harness tests in
  one workspace keep their own files).
- `sizes`: the window sizes `ctx.sweep_at_sizes` uses.
- `formats`: which report files are written besides `report.json` (default
  `[Format::Markdown]`); see Report formats.
- `profile`: `Profile::isolated().name_prefix(..).guard_paths(..)`.
- `coverage`: the opt-in coverage gate (above).
- `jobs` / `max_jobs`: serial (default), a fixed number of worker processes or `Jobs::Auto`,
  and the cap; see Parallel runs.
- `renderer`: `Renderer::Mock` (default, draws nothing) or, with the `screenshots` feature,
  `Renderer::Software`; see Screenshots.
- `sweep_options(SweepOptions { .. })`: the thresholds; the defaults are the Indicatrix ones.
  `floating_layers` (default `["PopupWindow"]`) lists the type names, or base names, of layers
  drawn above the content on purpose (a toast, a floating panel): a control inside a layer never
  overlaps a control outside it or in another layer; overlaps inside one layer are still
  reported.
- `Smoke` options: `deny_rules`, `skip_blocked`, `always_restore`, `allowed_notices`,
  `max_controls`, `before_restore` and `scope_may_close(true)`. The last is for passes over
  views named `Window/<scope>` (the names `WindowView::subtree` is given by convention): a press
  that closes the scope is counted in `SmokeStats::closed` instead of reported as
  `window-invalid`. A custom `SmokeHost` may override `admits(&Control)` to restrict the pass to
  some controls (default: all).
- `scenario`: one named journey.
- `scenario_last`: a journey that runs after all `scenario`s (whatever the order of the calls),
  in a parallel run with the union of every worker's coverage; see Parallel runs.
- `teardown`: runs after the last scenario and the gate, before the profile is deleted: stop
  threads, flush files.
- `run` / `run_to_report`: the latter returns the `Report` for a test that asserts counts.

`ctx.on_error_notice` and `ctx.clear_notice_with` are per scenario. `WindowView::find(label)`
matches the accessible label or, when no label matches, the accessible placeholder text (a
`LineEdit` carries only a placeholder); `find_id("Component::id")` does not depend on text;
`find_role`, `find_nth`, `labels()` and `placeholders()` complete the set, and a miss is a
`not-found` finding that lists similar labels and placeholders. `uiprobe::target_dir!()`
works in any target; integration tests get the compile-time path, others the run-time one.

The prelude (`use uiprobe::prelude::*`) brings `Harness`, `Ctx`, `Attached`, `Hooks`,
`WindowView`, `Control`, `ElementHandle`, `ElementRoot`, `AccessibleRole`, `Finding`, `Format`,
`Report`, `Profile`, `ProfileDir`, `Smoke`, `SmokeStats`, `DenyRule`, `DenyField`, `DenyMatch`,
`DEFAULT_DENY`, `SweepOptions`, `Coverage`, `GateRule`, `record_callback`, `arm_gate`,
`Jobs`, `Renderer`, `SLINT_VERSION`, `pump`, `settle`, `wait_until`, `wait_until_labeled`, the
`timing` module and `std::process::ExitCode`. API details are
on docs.rs; recipes (secondary windows, threads, sticky notices, deny lists from data, key
chords, `report.json` in CI, a custom `SmokeHost`) are in `docs/cookbook.md`.

## Troubleshooting

Symptom, cause, fix. Aborts (exit code 2) print the same advice on stderr.

- **`cargo test --test ui` prints `running 0 tests`.** The `[[test]]` lacks `harness = false`,
  so libtest owns `main` and yours is dead code. Add it (Quick start, step 1).
- **`cargo test` runs the unit tests but no UI test, and says nothing.** The target has
  `required-features = ["ui-harness"]` and the feature is off; cargo skips it silently. Run
  with `--features ui-harness` (or `--all-features` in CI), or use the `cargo ui` alias.
- **`ABORT: the window exposes no elements`.** The UI was compiled without Slint debug info.
  Either `with_debug_info(true)` in build.rs under the feature and run with the feature on, or
  `SLINT_EMIT_DEBUG_INFO=1` while compiling, or `[env] SLINT_EMIT_DEBUG_INFO = "1"` in
  `.cargo/config.toml`. If nothing changes, `cargo clean -p <your crate>`: the `slint!` macro
  is not always re-run when only the environment changed.
- **`ABORT: the Slint platform was already set`.** The code the harness runs selects a backend
  (`BackendSelector::select`, `slint::platform::set_platform`). Keep the backend choice in the
  binary's `main`; the window constructor the scenarios launch must not choose one.
- **`ABORT: the guarded path ... is outside the profile`.** The app resolves that path through
  something the profile cannot redirect (Windows Known Folders via the `dirs` crate, a
  hard-coded path). Under the feature ask `uiprobe_hooks::paths::data_dir()` first
  (Hooks); a read-only path that may live outside does not belong in `guard_paths`.
- **`no scenario matches the argument filter ...; nothing run, report not written`.** You ran
  `cargo test <name>` and cargo handed `<name>` to the harness binary as a scenario filter.
  Run `cargo test --test ui` (no filter), or filter on purpose with `UIPROBE_FILTER`.
- **`config: UIPROBE_FILTER=... matches no scenario`.** A typo in the filter; the
  finding lists the scenario names.
- **`config: no scenarios`.** The harness has no `.scenario(..)`.
- **`not-found: no element with label "Save"; similar labels: [...]`.** The label changed, the
  dialog is not open, or the element is in an invisible subtree (invisible elements are never
  visited). A text field is found by its placeholder when it has no label; `view.labels()` and
  `view.placeholders()` list what is there; `find_id("Component::id")` does not depend on
  text.
- **`error-toast` on every control after the first one in the smoke pass.** The app's notice
  does not auto-hide and nothing clears it: register `ctx.clear_notice_with(..)` next to
  `ctx.on_error_notice(..)` (both per scenario), and list the notices a state is expected to
  raise in `Smoke::allowed_notices`.
- **The run hangs at a click.** That control opened a native dialog and the app does not go
  through the hooks: add `uiprobe_hooks::picker::next` at the dialog site (Hooks). Until
  then, deny the control: `Smoke::new(..).deny(["import"])`.
- **The run ends at the first panic with no report.** `panic = "abort"` in the test profile
  disables catching. Use the default `unwind` for the profile that runs the UI test.
- **`could not delete profile ...`.** A file is still open or an app thread still runs at the
  end: stop threads and close files in `Harness::teardown`. On Linux the harness also warns
  when threads remain.
- **Linux: `warning: the process has N threads`.** Something started a thread before
  `Harness::run` redirected the environment (a logger, a runtime, a static initialiser).
  `Harness::new()` must be the first statement of `main` and nothing may spawn before `run()`.
- **Geometry findings differ between your machine and CI.** The testing backend measures text
  with the system fonts. Give fixtures `default-font-family: "FixedTestFont"` (fixed metrics
  everywhere); for a real app compare counts on one machine and give a CI box its own baseline
  or a pinned font package.
- **`cargo` fails with `failed to select a version for slint` after adding the harness.** The
  exact Slint pin (Versions and the Slint pin): pin `slint = "=1.18.1"` in your app, or use the
  harness release for your Slint line.
- **The report landed somewhere unexpected.** Run through `cargo test` (it sets
  `CARGO_TARGET_TMPDIR` and `CARGO_PKG_NAME`, from which `<target>/uiprobe/<package>`
  is derived) or set `Harness::report_dir` explicitly; the first console line always prints
  the directory.

## Limits

- **Fonts and CI baselines.** The testing backend measures text with the system fonts, so
  geometry findings depend on the machine. Only the font family `FixedTestFont` has fixed
  metrics; give your fixtures `default-font-family: "FixedTestFont"` for exact geometry. For an
  app under test, compare counts on the same machine, and give a CI box its own baseline (or
  install a pinned font package).
- **Scale factor is 1** unless a screenshot or recording run sets it with
  `screenshot::set_window_size` (a HiDPI picture). At 1 logical and physical pixels are the
  same, so a run that does not call it cannot see DPI problems; the checks measure logical
  pixels and are not a DPI test either way.
- **One process.** The harness relaunches windows, not the process. `thread_local!` state in
  your app survives a relaunch; guard it with a `Weak` owner check, or a service stays bound to
  the first window.
- **`panic = "abort"`** in your test profile disables catching: the run ends at the first panic.
  The recorder chains to the previous panic hook, so backtraces still print.
- **The text scanner** behind the coverage gate has known limits: component names must start
  with a capital, `import { X as Y }` renames are not followed, `@children` slots and `states`
  blocks are opaque, and labels built from expressions are untrackable (allow-list them).
  The gate is opt-in.
- **Native pickers without hooks block the run** (the dialog is modal). Add the hooks from day
  one for any app with file dialogs.
- **Visibility.** Invisible subtrees are never visited; `visible` and `clip` are not readable
  through the element API, and there is no way to read a window's `min-width`, so sizes are
  configuration.

## Versions and the Slint pin

`uiprobe 0.18.x` serves Slint 1.18.x exactly: the harness pins `slint` and
`i-slint-backend-testing` with `=1.18.1`, because the testing backend is an internal Slint
crate without semver that pins `i-slint-core` exactly. `uiprobe::SLINT_VERSION` holds the
version, the first console line prints it, and `report.json` carries it (`harness`, `slint`).

What that means for your app: its `slint` must resolve to the same 1.18.1. Cargo allows one
version of a 1.x crate per build, so a requirement that cannot be 1.18.1 does not build two
copies, it fails to resolve (`failed to select a version for slint`, or for `uiprobe`).
Two ways out: pin `slint = "=1.18.1"` in your app, or move to the harness release that serves
your Slint line (one harness minor per Slint patch line). `cargo tree -i i-slint-core -e
normal,dev` shows who pulls which version. A `git` or `path` Slint can still put two
`i-slint-core`s into one build; the testing platform then lives in the wrong one and the first
launch fails or aborts; `cargo tree -d` shows the duplicate.

`uiprobe-hooks` has its own `0.1.x` line and no Slint dependency. Minimum Rust: 1.92
(Slint's).

## License

MIT.
