//! Self-test (feature `recording`): the recorder emits the exact number of frames, glides the
//! cursor to its target, draws the click ripple, writes captions and chapters, and (when ffmpeg
//! is installed) encodes an MP4 through the `.part` file.
//!
//! The fixture is a white 200 x 100 window drawn at scale 2 (a 400 x 200 picture) with a red
//! square in the top-left corner. Video time at 30 fps: a 1 s hold is exactly 30 frames.
//!
//! 1. Memory sink: `hold(1 s)` emits 30 frames and the cursor is still hidden; `move_to` ends
//!    with the arrow tip on the target; frames of a `click` carry ripple pixels left of the tip
//!    (the arrow itself lies right of and below it), and the frame before and the frames after
//!    the ripple carry none; the SRT and the chapter list have the expected text.
//! 2. `wait_while_recording` emits a bounded time-lapse; `wait_cut` emits nothing.
//! 3. PNG sequence sink: numbered files.
//! 4. ffmpeg sink (skipped with a note when ffmpeg is not found): the video appears only after
//!    `finish`, `abandon` leaves nothing behind.
//!
//! 5. The virtual camera and the keystroke badge (`recording/camera_badge.rs`).
//! 6. The filmed input layer, the text guard, the real-time waits and the reuse of the window
//!    picture (`recording/layers.rs`, with the `InputFixture` and `StillFixture` scenarios); the
//!    first ripple frame of `click_at` already shows the app's reaction, and `press_keys` does not
//!    advance mock time.
//!
//! Needs Slint debug info at compile time only through the shared workspace configuration.

use std::{
    cell::{Cell, RefCell},
    path::Path,
    process::ExitCode,
    rc::Rc,
    time::Duration,
};

use slint::Rgba8Pixel;
use uiprobe::{
    Harness,
    backend::{Renderer, pump},
    recording::{Recorder, RecorderOptions, SinkKind},
    report::Report,
    screenshot::{self, Shot},
};

#[path = "recording/camera_badge.rs"]
mod camera_badge;
#[path = "recording/layers.rs"]
mod layers;

slint::slint! {
    export component RecordFixture inherits Window {
        width: 200px;
        height: 100px;
        background: #ffffff;
        Rectangle {
            x: 4px;
            y: 4px;
            width: 20px;
            height: 10px;
            background: #ff0000;
        }
    }

    // Invisible input targets: a focus scope that collects the typed text, with a touch area that
    // counts presses and adds up wheel deltas.
    export component InputFixture inherits Window {
        width: 200px;
        height: 100px;
        background: #ffffff;
        in-out property <int> presses;
        in-out property <string> typed;
        in-out property <length> wheel;
        // Set by the click itself: the green swatch shows whether the app has reacted yet.
        in-out property <bool> reacted;
        // The key events of `type_text` and `press_keys` go to the focused element; a click on
        // the touch area does not move the focus, so the scope takes it when the window opens.
        forward-focus: keys;
        keys := FocusScope {
            width: parent.width;
            height: parent.height;
            key-pressed(event) => {
                root.typed += event.text;
                accept
            }
            TouchArea {
                width: parent.width;
                height: parent.height;
                clicked => {
                    root.presses += 1;
                    root.reacted = true;
                }
                scroll-event(event) => {
                    root.wheel += event.delta-y;
                    accept
                }
            }
            // Far from every click the tests make (physical pixels 280 to 360 by 10 to 50).
            Rectangle {
                x: 140px;
                y: 5px;
                width: 40px;
                height: 20px;
                background: root.reacted ? #00ff00 : #ffffff;
            }
        }
    }

    // A plain colour swatch and an eased one, to see what a reused picture does with changes.
    export component StillFixture inherits Window {
        width: 200px;
        height: 100px;
        background: #ffffff;
        in-out property <color> tint: #ff0000;
        in-out property <color> fade: #00ff00;
        Rectangle {
            x: 4px;
            y: 4px;
            width: 20px;
            height: 10px;
            background: root.tint;
        }
        Rectangle {
            x: 4px;
            y: 30px;
            width: 20px;
            height: 10px;
            background: root.fade;
            animate background {
                duration: 300ms;
            }
        }
    }
}

type Mismatches = Rc<RefCell<Vec<String>>>;

fn check(bad: &Mismatches, ok: bool, what: impl Into<String>) {
    if !ok {
        bad.borrow_mut().push(what.into());
    }
}

fn is_white(pixel: Option<Rgba8Pixel>) -> bool {
    pixel.is_some_and(|p| p.r > 235 && p.g > 235 && p.b > 235)
}

fn is_dark(pixel: Option<Rgba8Pixel>) -> bool {
    pixel.is_some_and(|p| p.r < 110 && p.g < 110 && p.b < 110)
}

/// Whether any pixel of row `y` between `x_from` and `x_to` (exclusive) is not white.
fn row_has_ink(shot: &Shot, y: u32, x_from: u32, x_to: u32) -> bool {
    (x_from..x_to).any(|x| !is_white(shot.pixel(x, y)))
}

/// Whether the 5 x 5 pixels around `(x, y)` hold a dark pixel (the arrow's outline at its tip).
fn dark_near(shot: &Shot, x: u32, y: u32) -> bool {
    (x - 2..=x + 2).any(|px| (y - 2..=y + 2).any(|py| is_dark(shot.pixel(px, py))))
}

pub(crate) fn options(sink: SinkKind, output: &Path) -> RecorderOptions {
    RecorderOptions {
        sink,
        output: output.to_path_buf(),
        ..RecorderOptions::default()
    }
}

fn ffmpeg_available(ffmpeg: &Path) -> bool {
    std::process::Command::new(ffmpeg)
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn memory_recording(bad: &Mismatches, window: &slint::Window, dir: &Path) -> Result<(), String> {
    let output = dir.join("memory").join("rec.mp4");
    let mut rec = Recorder::start(window, options(SinkKind::Memory, &output))?;
    check(bad, rec.frames().is_empty(), "start emitted a frame");

    rec.chapter("Intro");
    rec.caption("Hello\nWorld");
    rec.hold(Duration::from_secs(1))?;
    check(
        bad,
        rec.frames().len() == 30,
        format!("1 s hold gave {} frames, wanted 30", rec.frames().len()),
    );
    let first = &rec.frames()[0];
    check(
        bad,
        (first.width(), first.height()) == (400, 200),
        format!("frame size {} x {}", first.width(), first.height()),
    );
    check(
        bad,
        !dark_near(first, 200, 100),
        "the cursor is drawn before the first move",
    );
    check(
        bad,
        is_white(first.pixel(300, 150)),
        "the background is not white",
    );

    rec.caption("Second");
    rec.chapter("Move");
    rec.move_to((150.0, 60.0), Duration::from_millis(500))?;
    check(
        bad,
        rec.frames().len() == 45,
        format!("the glide left {} frames, wanted 45", rec.frames().len()),
    );
    let last = rec.frames().last().ok_or("no frames")?;
    check(
        bad,
        dark_near(last, 300, 120),
        "the arrow tip is not at the glide target",
    );

    rec.click()?;
    let after_click = rec.frames().len();
    check(
        bad,
        after_click == 55,
        format!("the click left {after_click} frames, wanted 55"),
    );
    check(
        bad,
        !row_has_ink(&rec.frames()[44], 120, 230, 296),
        "ripple pixels before the click",
    );
    check(
        bad,
        row_has_ink(&rec.frames()[49], 120, 230, 296),
        "no ripple pixels in the middle of the click",
    );

    rec.hold(Duration::from_millis(500))?;
    check(
        bad,
        !row_has_ink(rec.frames().last().ok_or("no frames")?, 120, 230, 296),
        "ripple pixels after the ripple ended",
    );
    rec.caption("");

    let summary = rec.finish()?;
    check(
        bad,
        summary.frames == 70 && summary.video_time == Duration::from_millis(2333),
        format!(
            "summary: {} frames, {:?}; wanted 70 frames, 2333 ms",
            summary.frames, summary.video_time
        ),
    );
    let srt = std::fs::read_to_string(&summary.srt).map_err(|e| e.to_string())?;
    check(
        bad,
        srt == "1\n00:00:00,000 --> 00:00:01,000\nHello\nWorld\n\n\
                2\n00:00:01,000 --> 00:00:02,333\nSecond\n\n",
        format!("srt text: {srt:?}"),
    );
    let chapters = std::fs::read_to_string(&summary.chapters).map_err(|e| e.to_string())?;
    check(
        bad,
        chapters == "0:00 Intro\n0:01 Move\n",
        format!("chapters text: {chapters:?}"),
    );
    Ok(())
}

fn waits(bad: &Mismatches, window: &slint::Window, dir: &Path) -> Result<(), String> {
    let output = dir.join("waits").join("rec.mp4");
    let mut rec = Recorder::start(window, options(SinkKind::Memory, &output))?;
    let polls = Cell::new(0_u32);
    let held = rec.wait_while_recording(
        Duration::from_secs(10),
        8.0,
        Duration::from_millis(100),
        || {
            polls.set(polls.get() + 1);
            polls.get() >= 30
        },
    )?;
    let emitted = rec.frames().len();
    check(bad, held, "wait_while_recording gave up");
    check(
        bad,
        (1..=4).contains(&emitted),
        format!("the time-lapse budget of 100 ms gave {emitted} frames"),
    );
    let gave_up = rec.wait_cut(Duration::from_millis(30), || false);
    check(
        bad,
        !gave_up,
        "wait_cut reported a condition that never held",
    );
    check(
        bad,
        rec.frames().len() == emitted,
        "wait_cut emitted frames",
    );
    rec.abandon();
    Ok(())
}

fn png_sequence(bad: &Mismatches, window: &slint::Window, dir: &Path) -> Result<(), String> {
    let frames = dir.join("png");
    let output = dir.join("png_out").join("rec.mp4");
    let mut rec = Recorder::start(
        window,
        options(SinkKind::PngSequence(frames.clone()), &output),
    )?;
    rec.hold(Duration::from_millis(100))?;
    let summary = rec.finish()?;
    check(
        bad,
        summary.frames == 3,
        format!("{} png frames", summary.frames),
    );
    for number in 1..=3 {
        let file = frames.join(format!("frame_{number:06}.png"));
        check(bad, file.exists(), format!("missing {}", file.display()));
    }
    Ok(())
}

fn ffmpeg_sink(bad: &Mismatches, window: &slint::Window, dir: &Path) -> Result<(), String> {
    let defaults = RecorderOptions::default();
    if !ffmpeg_available(&defaults.ffmpeg) {
        println!(
            "uiprobe recording self-test: ffmpeg ({}) not found, the encoder part is skipped",
            defaults.ffmpeg.display()
        );
        return Ok(());
    }
    let output = dir.join("video").join("out.mp4");
    let part = dir.join("video").join("out.mp4.part");
    let mut rec = Recorder::start(window, options(SinkKind::Ffmpeg, &output))?;
    rec.hold(Duration::from_millis(500))?;
    check(bad, !output.exists(), "the video exists before finish");
    let summary = rec.finish()?;
    let size = std::fs::metadata(&summary.output).map_or(0, |metadata| metadata.len());
    check(bad, size > 0, "the video is missing or empty");
    check(bad, !part.exists(), "the .part file is left behind");
    check(
        bad,
        summary.srt.exists() && summary.chapters.exists(),
        "captions are missing",
    );

    let dropped = dir.join("video").join("dropped.mp4");
    let mut rec = Recorder::start(window, options(SinkKind::Ffmpeg, &dropped))?;
    rec.hold(Duration::from_millis(100))?;
    rec.abandon();
    check(
        bad,
        !dropped.exists() && !dir.join("video").join("dropped.mp4.part").exists(),
        "an abandoned recording left files behind",
    );
    Ok(())
}

/// One step of the "recording" scenario.
type Step = fn(&Mismatches, &slint::Window, &Path) -> Result<(), String>;

fn finish(bad: &Mismatches, report: &Report) -> ExitCode {
    let mut ok = true;
    for m in bad.borrow().iter() {
        eprintln!("uiprobe self-test mismatch: {m}");
        ok = false;
    }
    for f in report.findings() {
        eprintln!(
            "uiprobe self-test unexpected finding: {} | {} | {} | {}",
            f.window, f.element, f.check, f.detail
        );
        ok = false;
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn main() -> ExitCode {
    let bad: Mismatches = Rc::default();
    let sink = Rc::clone(&bad);
    let input_sink = Rc::clone(&bad);
    let still_sink = Rc::clone(&bad);
    let report = Harness::new()
        .renderer(Renderer::Software)
        .scenario("recording", move |ctx| {
            let app = ctx.launch(RecordFixture::new)?;
            screenshot::set_window_size(app.window(), (200, 100), 2.0);
            pump(20);
            let dir = ctx.profile().scratch_dir();
            let steps: [(&str, Step); 9] = [
                ("memory", memory_recording),
                ("waits", waits),
                ("png", png_sequence),
                ("ffmpeg", ffmpeg_sink),
                ("zoom", camera_badge::zoom),
                ("follow", camera_badge::follow),
                ("badge", camera_badge::badge),
                ("guard", layers::text_guard),
                ("real-time waits", layers::real_time_waits),
            ];
            for (name, step) in steps {
                if let Err(e) = step(&sink, app.window(), &dir) {
                    check(&sink, false, format!("{name}: {e}"));
                }
            }
            ctx.shutdown(app);
            Ok(())
        })
        .scenario("recording input", move |ctx| {
            let app = ctx.launch(InputFixture::new)?;
            screenshot::set_window_size(app.window(), (200, 100), 2.0);
            pump(20);
            let dir = ctx.profile().scratch_dir();
            if let Err(e) = layers::input_layer(&input_sink, &app, &dir) {
                check(&input_sink, false, format!("input: {e}"));
            }
            if let Err(e) = layers::click_reaction(&input_sink, &app, &dir) {
                check(&input_sink, false, format!("click reaction: {e}"));
            }
            if let Err(e) = layers::press_keys_keep_the_clock(&input_sink, &app, &dir) {
                check(&input_sink, false, format!("press_keys clock: {e}"));
            }
            ctx.shutdown(app);
            Ok(())
        })
        .scenario("recording still", move |ctx| {
            let app = ctx.launch(StillFixture::new)?;
            screenshot::set_window_size(app.window(), (200, 100), 2.0);
            pump(20);
            let dir = ctx.profile().scratch_dir();
            if let Err(e) = layers::still_reuse(&still_sink, &app, &dir) {
                check(&still_sink, false, format!("still: {e}"));
            }
            ctx.shutdown(app);
            Ok(())
        })
        .run_to_report();
    finish(&bad, &report)
}
