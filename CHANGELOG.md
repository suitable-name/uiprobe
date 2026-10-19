# Changelog

## uiprobe 0.18.8

Recording (feature `recording`); the defaults keep the old output, and every change is additive
except that `RecorderOptions` gained four public fields (`camera`, `badge`, `forbidden_texts`,
`still_reuse_frames`), so a struct literal that names every field of an earlier release no
longer compiles. Build the options with `..RecorderOptions::default()`; fields of the option structs
(`RecorderOptions`, `CameraOptions`, `BadgeStyle`) can be added in patch releases.


- Virtual camera: `Recorder::zoom_to(region, d)`, `zoom_out(d)`, `set_follow_cursor(on)` and
  `camera()`, with `CameraOptions { margin: 24, max_zoom: 2.0, scale_cursor: true }` in
  `RecorderOptions::camera`. Each frame is captured in full, the camera rectangle is cropped at
  the capture's physical resolution and scaled to the frame size with an in-crate bicubic filter
  (so a zoomed 4K shot stays sharp and the output size never changes), then the cursor, ripple
  and badge are drawn. At zoom 1.0 nothing is resampled and the frames are meant to be
  bit-identical to 0.18.7's (argued from the float operations and tested against a plain capture,
  not against a recording made with 0.18.7).
  The timing label `recording-zoom` shows the cost.
- Keystroke badge: `Recorder::show_keys("Ctrl + ↑")` draws a keycap pill near the bottom of the
  frame (not zoomed) for `BadgeStyle::visible_ms` (1400) fading over `fade_ms` (300); the text is
  rendered by Slint's software renderer, once per label.
- Filmed input layer: `click_at`, `type_text`, `scroll_at` (and `scroll_at_in` for an attached
  window) and `press_keys` glide, show the ripple or the badge and dispatch real Slint events:
  the dispatch is the one activation, so the callback must not be invoked again.
- `recording::pacing`: `reading_time`, `glide_duration`, `plain`, `wrap`, `cues`, `word_count`,
  `wheel_steps` and `wait_real`, ported from the Indicatrix tutorial videos.
- Text guard: `RecorderOptions::forbidden_texts` and `Recorder::set_text_source` stop a recording
  that would show a forbidden text (case-insensitive substring). The texts are checked at the
  start and the end of every call that emits frames and at most every 250 ms of video in
  between, so a text that appears and goes within one interval can be missed; a hit fails the
  recording and deletes the half-written video.
- `wait_while_recording` and `wait_cut` pump the real time that passed per poll (5 to 100 ms), not
  a fixed 10 ms, so an app's tick budgets run at their real speed.
- `RecorderOptions::still_reuse_frames` (default 1, off), `Recorder::captures` and
  `Recorder::reused_frames`: still frames of a hold can reuse one full render of the window. The
  timing label `recording-reuse` shows the cost. Slint 1.18.1 has no redraw signal on the testing
  backend, so a result an app thread posts during a still hold can show up late by at most this
  many frames; see the module documentation.

- `Recorder::click_at` delivers the click before it plays the ripple, so the ripple frames
  already show the app's reaction.
- `Recorder::press_keys` no longer pumps 50 ms of mock time outside the frame clock; the chord
  takes effect in the next emitted frame. `view::press` is unchanged.
- `RecorderOptions::crf` above 51 is refused when the ffmpeg sink opens, with a clear message,
  instead of failing on the first frame.
- `Recorder::wait_cut` drops the reused still frame, like every other call that changes the
  window.
- `recording::pacing::wheel_steps` is a re-export of `view::geometry::wheel_steps` (same path,
  one implementation).

Targeting (all builds):

- `uiprobe::targeting`: element targeting for scripted journeys and videos.
  - `Spot` is an element's rectangle plus the clipping above it (every `Flickable` viewport, hence
    every `ScrollView` and `ListView`, and the window). `Spot::visible_rect()` is the part a user
    can see; `Spot::click_point()` is the centre of that part, or `None` when less than 60 % of the
    element (or less than 12 x 12 logical px) is visible. This fixes clicks at the centre of a row
    scrolled partly out of its viewport landing in whatever lies below it.
  - Queries on `WindowView` returning `Result<Spot, TargetError>`: `spot_label`,
    `spot_label_start`, `spot_role`, `spot_within` (scoped to an ancestor component by type name),
    `spot_label_near_y`, `spot_label_start_near_y` and the general `spot_matching(&TargetSpec)`.
    Two different visible controls with the same label are `TargetError::Ambiguous { label, count }`;
    the other errors are `NotFound`, `NotVisible` and `Disabled`. `TargetError::finding(window)`
    makes a `Finding` of it.
  - `scroll_into_view_plan(&Spot)` computes, without scrolling, the wheel delta that brings an
    element fully into its scroll container's viewport, so the caller can perform the scroll
    visibly (for example with `Recorder::scroll_at`).
- `uiprobe::view::geometry`: the pure math behind it (`Rect`, `clip_all`, `click_point`,
  `scroll_delta`, `distinct_indices`, `nearest_within`, `wheel_steps`), unit-tested without a
  window. A `Rectangle { clip: true }` cannot be detected through the testing backend; add it with
  `Spot::clipped_by`.
- `TargetError` and `LabelMatch` are `#[non_exhaustive]`: a later release can add a reason or a
  way to compare without a breaking change, so a `match` on them needs a wildcard arm.
- `impl From<TargetError> for Finding`: `view.spot_label("Save")?` works inside a scenario; the
  finding is a run-wide one (window `-`). `TargetError::finding(window)` still names the window.

Documentation: docs.rs builds the crate with all features, so `screenshot`, `recording` and
`Renderer::Software` appear in the published documentation.

## uiprobe 0.18.7

Published to crates.io (the Indicatrix tutorial-video harness is pinned to `=0.18.7`), but this
file recorded no entry for it when it was released and the release notes cannot be reconstructed
from the repository. Read 0.18.8 and 0.18.6 together for the changes since 0.18.6.

## uiprobe 0.18.6

- Video recording (opt-in feature `recording`, implies `screenshots`): the new `recording` module
  plays an app frame by frame and writes an MP4 through ffmpeg (`UIPROBE_FFMPEG`, else `ffmpeg`
  on `PATH`; H.264, `.part` file renamed on success), with a drawn mouse cursor, click ripples,
  hover events, SRT captions and a YouTube chapter list. `Recorder` (`start`, `hold`, `move_to`,
  `move_to_element`, `click`, `wait_while_recording`, `wait_cut`, `caption`, `chapter`,
  `video_time`, `refresh`, `finish`, `abandon`, `frames`), `RecorderOptions`, `SinkKind`
  (`Ffmpeg`, `PngSequence`, `Memory`), `CursorStyle`, `RecordingSummary`, and the helpers
  `srt_timestamp`, `chapter_timestamp` and `ease_in_out`. `Recorder` and `RecorderOptions` are in
  the prelude. Timing labels `recording-capture` and `recording-encode`.

## uiprobe 0.18.5

- Timing statistics: the new `timing` module (`record`, `record_timeout`, `time`; in the prelude)
  and `backend::wait_until_labeled`. The backend records `wait`, `settle` and `pump` by itself.
  Each scenario prints a `timing:` console line (the six labels with the most time), `report.json`
  gets `scenarios[i].timing` and a top-level `timing` (totals, parallel workers and utilisation),
  and the report files get a "Timing" section (15 slowest scenarios, time and timeouts per label,
  utilisation of a parallel run).
- `Harness::scenario_last`: a scenario that runs after every ordinary one. In a parallel run the
  coordinator dispatches it when all ordinary scenarios are done and hands it the union of the
  workers' coverage recorders (every `done` message now carries the worker's recorder), so
  `coverage::recorded_callbacks()` inside it sees the whole run.

## uiprobe 0.18.4

- Screenshots (opt-in feature `screenshots`): `Harness::renderer(Renderer::Software)` draws the
  windows with Slint's software renderer; the new `screenshot` module captures a window
  (`capture`), crops it (`Shot::crop`, `crop_logical`, `crop_to(&element, margin)`), saves it as
  PNG through a temporary file (`Shot::save_png`), composes captures (`Shot::blank`,
  `Shot::fill`, `Shot::paste`) and sets a window's logical size and scale
  factor for HiDPI pictures (`set_window_size`). Parallel workers use the same renderer.
- `backend::init_backend_with(Renderer)`; `init_backend()` is unchanged (mock renderer).
- `Renderer` is in the prelude.

## uiprobe 0.18.3

Ports the improvements the Indicatrix harness made after the crate was lifted from it.

- Sweep: `SweepOptions::floating_layers` (default `["PopupWindow"]`): controls in a floating layer
  (matched by type name or base) never overlap controls outside it; overlaps inside one layer are
  still reported. A full `SweepOptions { .. }` literal needs the new field (use
  `..SweepOptions::default()`).
- Smoke: `SmokeHost::admits` (default true) restricts a pass to some controls of a view;
  `Smoke::scope_may_close` counts a scoped view (`Window/<scope>`) that closes itself in
  `SmokeStats::closed` instead of a `window-invalid` finding. Every smoke press also credits the
  wrapper component's site id (`coverage::record_wrappers`).
- Coverage: credit by element type and base type, single-site credit for computed labels, label
  literals with escapes and interpolation, ids after an `if`/`for` prefix, one label literal per
  statement, `coverage::record_wrappers` and `coverage::record_site_id`,
  `Coverage::label_props` / `reader_props` / `callback_hint`. Parallel workers carry the new
  credit.

## uiprobe 0.18.2

- Parallel runs (opt-in): `Harness::jobs(Jobs::Auto)` / `Jobs::Fixed(n)` and
  `UIPROBE_JOBS=auto|<n>`, capped by `Harness::max_jobs` / `UIPROBE_JOBS_MAX`. The
  test binary starts copies of itself as worker processes, each with its own profile; `Auto`
  starts one worker and adds one every 5 s while at least 1.5 cores were idle. One merged
  report, console output in scenario order. The default stays the serial run.
- New check `worker-crash`: a worker process died during a scenario (parallel runs only).

## uiprobe 0.18.1

- Fix: the default report directory is found from the test binary's path on both Cargo
  layouts (`target/<profile>/deps/` and the newer `target/<profile>/build/<package>/<hash>/out/`)
  before looking for `CACHEDIR.TAG`. 0.18.0 relied on the tag, which a target folder made by an
  older Cargo does not have, and then wrote the report to
  `target/<profile>/build/<package>/uiprobe/<package>/` on nightly Cargo.

## uiprobe 0.18.0

First release. Serves Slint 1.18.1 (exact pin). Lifted from the Indicatrix UI harness.

- Report formats: `Harness::formats([Format::Markdown, Format::Html, Format::Text])` and
  `UIPROBE_FORMATS=md,html,txt`. `report.json` is always written, `report.md` by
  default; a run removes the format files it did not select.
- The smoke pass recognises modal backdrops with the run's `SweepOptions` (`modal_cover`),
  not the defaults.
- Usability: the first console line names the harness and Slint versions, the selected
  scenarios and the report directory; a failing run prints its findings (up to 25) after the
  per-check counts; a harness with no scenarios is a `config` finding; `WindowView::find*` fall
  back to the accessible placeholder text, so a `LineEdit` is found by its placeholder, and a
  `not-found` finding lists similar placeholders (`WindowView::placeholders`); the abort
  messages for missing debug info, a pre-set platform and a guarded path outside the profile
  say what to do; `uiprobe::SLINT_VERSION` and the `harness` / `slint` fields of
  `report.json`; the prelude adds `ElementRoot`, `Hooks`, `ProfileDir`, `DEFAULT_DENY`,
  `record_callback`, `arm_gate`, `SLINT_VERSION` and `std::process::ExitCode`.
- The HTML report has a `main` landmark and real headings, column headers with scope, a
  per-check summary, and a dark color scheme that follows the system setting.
- Docs: the crate-level example is compile-checked; README quick start and troubleshooting;
  `docs/cookbook.md`; `examples/notes` shows every hook, the coverage gate, deny rules,
  notices and the report formats.
- The default report directory is `<target>/uiprobe/<package>`, one folder per package,
  so two harness tests in one workspace do not delete each other's report files; `<target>`
  follows Cargo's `CACHEDIR.TAG`, so the newer build-dir layout no longer puts reports under
  `target/<profile>/build/`.

## uiprobe-hooks 0.1.0

First release.

- Docs: a doc test of the picker queue that runs; the feature-gated call pattern is compiled
  in `examples/notes`.
