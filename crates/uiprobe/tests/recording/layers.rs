//! Self-tests of the filmed input layer, the text guard, the real-time waits and the reuse of the
//! window picture (Memory sink), run by `tests/recording.rs`.
//!
//! * Input (`InputFixture`: a focus scope with a touch area): `click_at` activates exactly once
//!   and draws the cursor at `origin + at`; `type_text` delivers the characters; `scroll_at`
//!   delivers wheel ticks adding up to the delta; `press_keys` delivers the chord and shows the
//!   badge; a bad chord sends nothing.
//! * `click_reaction`: the app has reacted to the click in the first ripple frame.
//!   `press_keys_keep_the_clock`: chords do not advance mock time outside the video clock.
//! * Guard: a forbidden fragment (case-insensitive) fails the next call before a frame is
//!   written, sticks, fails `finish`, and the source is not asked on every frame; fragments
//!   without a source fail closed.
//! * Waits: a 100 ms timer ticks about five times in a 500 ms `wait_cut` and
//!   `wait_while_recording` (mock time follows real time).
//! * Reuse (`StillFixture`): a still 1 s hold renders once; the default renders every frame; a
//!   change made between calls, a timer firing mid-hold and a running animation all show up in the
//!   frame they belong to; the cursor still moves over a reused picture.

use std::{
    cell::{Cell, RefCell},
    path::Path,
    rc::Rc,
    time::Duration,
};

use slint::{ComponentHandle, Rgba8Pixel, SharedString, platform::Key};
use uiprobe::{
    Attached,
    recording::{BadgeStyle, Recorder, RecorderOptions, SinkKind},
    screenshot::Shot,
};

use super::{InputFixture, Mismatches, StillFixture, check, dark_near, is_dark, options};

fn same_pixels(first: &Shot, second: &Shot) -> bool {
    first.width() == second.width()
        && first.height() == second.height()
        && first.pixels() == second.pixels()
}

fn is_colour(pixel: Option<Rgba8Pixel>, want: (u8, u8, u8)) -> bool {
    pixel.is_some_and(|p| {
        p.r.abs_diff(want.0) < 40 && p.g.abs_diff(want.1) < 40 && p.b.abs_diff(want.2) < 40
    })
}

pub fn input_layer(bad: &Mismatches, app: &Attached, dir: &Path) -> Result<(), String> {
    let window = app.window();
    let fixture = app
        .component::<InputFixture>()
        .ok_or("the InputFixture is missing")?;
    let output = dir.join("input").join("rec.mp4");
    let mut rec = Recorder::start(
        window,
        RecorderOptions {
            badge: BadgeStyle {
                font_size_logical: 10.0,
                ..BadgeStyle::default()
            },
            ..options(SinkKind::Memory, &output)
        },
    )?;

    // One click: exactly one activation, the cursor at origin + at (the window is not moved).
    rec.click_at(window, (50.0, 50.0), (20.0, 0.0))?;
    check(
        bad,
        fixture.get_presses() == 1,
        format!(
            "click_at activated {} times, wanted exactly 1",
            fixture.get_presses()
        ),
    );
    check(
        bad,
        rec.frames().len() >= 25,
        format!("click_at emitted {} frames", rec.frames().len()),
    );
    let last = rec.frames().last().ok_or("no frames")?;
    check(
        bad,
        dark_near(last, 140, 100),
        "click_at: the cursor is not at origin + at",
    );

    // Typing: the characters arrive, a frame hold follows each.
    let frames = rec.frames().len();
    rec.type_text(window, "ab", Duration::from_millis(50))?;
    check(
        bad,
        fixture.get_typed().as_str() == "ab",
        format!(
            "type_text delivered {:?}, wanted \"ab\"",
            fixture.get_typed()
        ),
    );
    check(
        bad,
        rec.frames().len() >= frames + 2,
        "type_text emitted no frames",
    );

    // The wheel: ticks of at most 120 that add up to the delta.
    rec.scroll_at(window, (50.0, 50.0), -300.0)?;
    check(
        bad,
        (fixture.get_wheel() + 300.0).abs() < 0.5,
        format!("scroll_at delivered {}, wanted -300", fixture.get_wheel()),
    );

    // A chord: delivered like view::press, and the badge follows.
    let arrow = SharedString::from(Key::UpArrow);
    rec.press_keys(window, "Ctrl+Up")?;
    check(
        bad,
        fixture.get_typed().as_str().contains(arrow.as_str()),
        "press_keys did not deliver the arrow key",
    );
    rec.hold(Duration::from_millis(100))?;
    let last = rec.frames().last().ok_or("no frames")?;
    let dark = (160..240)
        .flat_map(|x| (125..166).map(move |y| (x, y)))
        .filter(|&(x, y)| is_dark(last.pixel(x, y)))
        .count();
    check(
        bad,
        dark >= 20,
        format!("press_keys showed no badge ({dark} dark pixels)"),
    );

    // A chord that does not parse sends nothing and shows nothing.
    let typed = fixture.get_typed();
    check(
        bad,
        rec.press_keys(window, "Hyper+X").is_err() && fixture.get_typed() == typed,
        "a bad chord was sent",
    );
    rec.abandon();
    Ok(())
}

/// `click_at` delivers the click before it plays the ripple: the green swatch of the fixture
/// (turned on by the click itself) is there from the first ripple frame to the last, about 10
/// frames of the 350 ms ripple, and not during the glide and the rest before it. A `click_at` that
/// pressed after the ripple would end on a frame without it (and show it in the next call).
pub fn click_reaction(bad: &Mismatches, app: &Attached, dir: &Path) -> Result<(), String> {
    let window = app.window();
    let fixture = app
        .component::<InputFixture>()
        .ok_or("the InputFixture is missing")?;
    fixture.set_reacted(false);
    let output = dir.join("click_reaction").join("rec.mp4");
    let mut rec = Recorder::start(window, options(SinkKind::Memory, &output))?;
    rec.click_at(window, (30.0, 80.0), (0.0, 0.0))?;

    let green = |shot: &Shot| is_colour(shot.pixel(320, 30), (0, 255, 0));
    let flags: Vec<bool> = rec.frames().iter().map(green).collect();
    let reacted = flags.iter().filter(|&&on| on).count();
    check(bad, fixture.get_reacted(), "click_at did not click");
    check(
        bad,
        flags.last() == Some(&true),
        "the last frame of click_at does not show the reaction to the click",
    );
    check(
        bad,
        (9..=12).contains(&reacted),
        format!(
            "{reacted} of {} frames show the reaction, wanted the ~10 ripple frames",
            flags.len()
        ),
    );
    let first = flags.iter().position(|&on| on).unwrap_or(0);
    check(
        bad,
        flags[first..].iter().all(|&on| on) && first >= 15,
        format!("the reaction starts at frame {first} and is not steady from there"),
    );
    rec.abandon();
    Ok(())
}

/// `press_keys` does not advance Slint's mock time: a 100 ms timer does not tick for four chords
/// (200 ms if each pumped 50 ms), and ticks again once the recorder emits frames.
pub fn press_keys_keep_the_clock(
    bad: &Mismatches,
    app: &Attached,
    dir: &Path,
) -> Result<(), String> {
    let window = app.window();
    let output = dir.join("press_clock").join("rec.mp4");
    let ticks = Rc::new(Cell::new(0_u32));
    let timer = slint::Timer::default();
    {
        let ticks = Rc::clone(&ticks);
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(100),
            move || ticks.set(ticks.get() + 1),
        );
    }
    let mut rec = Recorder::start(window, options(SinkKind::Memory, &output))?;
    for _ in 0..4 {
        rec.press_keys(window, "Ctrl+Up")?;
    }
    check(
        bad,
        ticks.get() == 0,
        format!(
            "{} timer ticks during four press_keys: each pumped mock time outside the video clock",
            ticks.get()
        ),
    );
    rec.hold(Duration::from_millis(150))?;
    check(
        bad,
        (1..=2).contains(&ticks.get()),
        format!(
            "{} timer ticks after a 150 ms hold, wanted 1 (the timer works)",
            ticks.get()
        ),
    );
    timer.stop();
    rec.abandon();
    Ok(())
}

pub fn text_guard(bad: &Mismatches, window: &slint::Window, dir: &Path) -> Result<(), String> {
    let output = dir.join("guard").join("rec.mp4");
    let texts = Rc::new(RefCell::new(vec!["Welcome".to_owned()]));
    let calls = Rc::new(Cell::new(0_u32));
    let mut rec = Recorder::start(
        window,
        RecorderOptions {
            forbidden_texts: vec!["C:\\Users\\Secret".to_owned(), "Hunter2".to_owned()],
            ..options(SinkKind::Memory, &output)
        },
    )?;
    {
        let (texts, calls) = (Rc::clone(&texts), Rc::clone(&calls));
        rec.set_text_source(Box::new(move || {
            calls.set(calls.get() + 1);
            texts.borrow().clone()
        }));
    }
    rec.hold(Duration::from_secs(1))?;
    check(
        bad,
        rec.frames().len() == 30,
        "the guarded hold lost frames",
    );
    check(
        bad,
        (3..=8).contains(&calls.get()),
        format!(
            "the text source was asked {} times in a 1 s hold, wanted a handful",
            calls.get()
        ),
    );

    texts
        .borrow_mut()
        .push("Opening c:\\users\\SECRET\\notes.txt".to_owned());
    let written = rec.frames().len();
    let error = rec.hold(Duration::from_millis(500)).err();
    check(
        bad,
        error
            .as_deref()
            .is_some_and(|e| e.contains("C:\\Users\\Secret") && !e.contains("notes.txt")),
        format!("the hit error {error:?} does not name only the fragment"),
    );
    check(
        bad,
        rec.frames().len() == written,
        "a frame was written after the hit",
    );
    check(
        bad,
        rec.hold(Duration::from_millis(100)).is_err(),
        "the failure is not sticky",
    );
    check(bad, rec.finish().is_err(), "finish succeeded after a hit");

    // Fragments without a text source fail closed; no fragments never ask the source.
    let mut closed = Recorder::start(
        window,
        RecorderOptions {
            forbidden_texts: vec!["x".to_owned()],
            ..options(SinkKind::Memory, &output)
        },
    )?;
    check(
        bad,
        closed
            .hold(Duration::from_millis(100))
            .err()
            .is_some_and(|e| e.contains("text source")),
        "a guard without a text source did not fail",
    );
    closed.abandon();
    let asked = Rc::new(Cell::new(0_u32));
    let mut open = Recorder::start(window, options(SinkKind::Memory, &output))?;
    let counter = Rc::clone(&asked);
    open.set_text_source(Box::new(move || {
        counter.set(counter.get() + 1);
        vec!["anything".to_owned()]
    }));
    open.hold(Duration::from_millis(300))?;
    check(
        bad,
        asked.get() == 0,
        "the source was asked without forbidden texts",
    );
    open.abandon();
    Ok(())
}

pub fn real_time_waits(bad: &Mismatches, window: &slint::Window, dir: &Path) -> Result<(), String> {
    let output = dir.join("waits_real").join("rec.mp4");
    let ticks = Rc::new(Cell::new(0_u32));
    let timer = slint::Timer::default();
    {
        let ticks = Rc::clone(&ticks);
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(100),
            move || ticks.set(ticks.get() + 1),
        );
    }
    let mut rec = Recorder::start(window, options(SinkKind::Memory, &output))?;
    rec.wait_cut(Duration::from_millis(500), || false);
    check(
        bad,
        (3..=6).contains(&ticks.get()),
        format!(
            "{} timer ticks in a 500 ms wait_cut, wanted about 5 (mock time must follow real time)",
            ticks.get()
        ),
    );
    ticks.set(0);
    let held = rec.wait_while_recording(
        Duration::from_millis(500),
        1.0,
        Duration::from_millis(100),
        || false,
    )?;
    check(
        bad,
        !held,
        "wait_while_recording reported a condition that never held",
    );
    check(
        bad,
        (3..=6).contains(&ticks.get()),
        format!(
            "{} timer ticks in a 500 ms wait_while_recording, wanted about 5",
            ticks.get()
        ),
    );
    timer.stop();
    rec.abandon();
    Ok(())
}

pub fn still_reuse(bad: &Mismatches, app: &Attached, dir: &Path) -> Result<(), String> {
    let window = app.window();
    let fixture = app
        .component::<StillFixture>()
        .ok_or("the StillFixture is missing")?;
    let output = dir.join("still").join("rec.mp4");
    let reusing = RecorderOptions {
        still_reuse_frames: 1000,
        ..options(SinkKind::Memory, &output)
    };

    // A still second renders once; the other 29 frames reuse the picture and are identical.
    let mut rec = Recorder::start(window, reusing)?;
    rec.hold(Duration::from_secs(1))?;
    check(
        bad,
        rec.frames().len() == 30 && rec.captures() == 1 && rec.reused_frames() == 29,
        format!(
            "a still 1 s hold: {} frames, {} renders, {} reused; wanted 30, 1, 29",
            rec.frames().len(),
            rec.captures(),
            rec.reused_frames()
        ),
    );
    check(
        bad,
        rec.frames()
            .iter()
            .all(|f| same_pixels(f, &rec.frames()[0])),
        "the reused frames differ",
    );

    // Off by default: every frame is a render.
    let mut plain = Recorder::start(window, options(SinkKind::Memory, &output))?;
    plain.hold(Duration::from_millis(500))?;
    check(
        bad,
        plain.captures() == 15 && plain.reused_frames() == 0,
        format!(
            "default options: {} renders, {} reused; wanted 15, 0",
            plain.captures(),
            plain.reused_frames()
        ),
    );
    plain.abandon();

    // A change made between calls shows in the first frame of the next call.
    fixture.set_tint(slint::Color::from_rgb_u8(0, 0, 255));
    let first = rec.frames().len();
    rec.hold(Duration::from_millis(100))?;
    check(
        bad,
        is_colour(rec.frames()[first].pixel(20, 16), (0, 0, 255)),
        "a change between calls is not in the next frame (stale picture)",
    );
    check(
        bad,
        is_colour(
            rec.frames().last().ok_or("no frames")?.pixel(20, 16),
            (0, 0, 255),
        ),
        "the reused frames of the next call are stale",
    );

    // A timer that fires during a hold: the frame it fires in shows its effect.
    let timer = slint::Timer::default();
    let target = fixture.as_weak();
    timer.start(
        slint::TimerMode::SingleShot,
        Duration::from_millis(500),
        move || {
            if let Some(fixture) = target.upgrade() {
                fixture.set_tint(slint::Color::from_rgb_u8(0, 255, 0));
            }
        },
    );
    let (first, renders) = (rec.frames().len(), rec.captures());
    rec.hold(Duration::from_secs(1))?;
    let pixel = |index: usize| rec.frames()[first + index].pixel(20, 16);
    check(
        bad,
        is_colour(pixel(13), (0, 0, 255)) && is_colour(pixel(14), (0, 255, 0)),
        format!(
            "the timer's change is not in its frame: frame 13 {:?}, frame 14 {:?}",
            pixel(13),
            pixel(14)
        ),
    );
    check(
        bad,
        is_colour(pixel(29), (0, 255, 0)),
        "the last frame of the timer hold is stale",
    );
    check(
        bad,
        rec.captures() - renders <= 3,
        format!(
            "the timer hold rendered {} times, wanted at most 3",
            rec.captures() - renders
        ),
    );

    still_animation(bad, fixture, &mut rec)?;
    rec.abandon();
    Ok(())
}

/// A running animation renders until it ends, and the cursor moves over a reused picture.
fn still_animation(
    bad: &Mismatches,
    fixture: &StillFixture,
    rec: &mut Recorder<'_>,
) -> Result<(), String> {
    // A running animation: frames render until it ends, then the picture is reused.
    fixture.set_fade(slint::Color::from_rgb_u8(0, 0, 255));
    let (first, renders) = (rec.frames().len(), rec.captures());
    rec.hold(Duration::from_millis(900))?;
    let eased = |index: usize| rec.frames()[first + index].pixel(20, 70);
    let middle = (2..10).any(|i| eased(i).is_some_and(|p| p.g > 40 && p.b > 40));
    check(bad, middle, "no frame shows the animation half way");
    check(
        bad,
        is_colour(eased(26), (0, 0, 255)),
        "the animation did not reach its end",
    );
    let spent = rec.captures() - renders;
    check(
        bad,
        (8..=16).contains(&spent),
        format!("the animation hold rendered {spent} times, wanted 8 to 16"),
    );

    // The cursor and the ripple move over a reused picture.
    let renders = rec.captures();
    rec.move_to((150.0, 60.0), Duration::from_millis(500))?;
    check(
        bad,
        dark_near(rec.frames().last().ok_or("no frames")?, 300, 120),
        "the cursor is missing over the reused picture",
    );
    check(
        bad,
        rec.captures() - renders <= 2,
        "the glide rendered the window again and again",
    );
    Ok(())
}
