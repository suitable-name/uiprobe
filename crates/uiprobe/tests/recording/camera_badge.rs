//! Self-tests of the virtual camera and the keystroke badge (Memory sink), run by
//! `tests/recording.rs` on the 200 x 100 logical, scale 2 fixture (frames of 400 x 200 pixels).
//!
//! Camera: zoom 1.0 frames equal the plain capture bit for bit (also after a zoom and a zoom
//! out); after `zoom_to` the red square fills the expected output region; the cursor and the
//! ripple are mapped through the camera; with follow-cursor the camera pans so the cursor ends
//! inside the central 70 %, without it the camera stays.
//!
//! Badge: no frames from `show_keys`; the following frames carry the pill and the rendered text;
//! it fades and is gone `visible_ms` later (the frame is then bit-identical to the plain
//! capture); an empty label removes it; a zoomed camera does not scale it.

use std::{path::Path, time::Duration};

use slint::Rgba8Pixel;
use uiprobe::{
    recording::{BadgeStyle, CameraOptions, Recorder, RecorderOptions, SinkKind},
    screenshot::{self, Shot},
};

use super::{Mismatches, check, dark_near, is_dark, is_white, options};

fn same_pixels(first: &Shot, second: &Shot) -> bool {
    first.width() == second.width()
        && first.height() == second.height()
        && first.pixels() == second.pixels()
}

fn is_red(pixel: Option<Rgba8Pixel>) -> bool {
    pixel.is_some_and(|p| p.r > 200 && p.g < 70 && p.b < 70)
}

fn near(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    (a.0 - b.0).abs() < 0.05
        && (a.1 - b.1).abs() < 0.05
        && (a.2 - b.2).abs() < 0.05
        && (a.3 - b.3).abs() < 0.05
}

/// The box around every pixel where `a` and `b` differ: `(left, top, right, bottom)`, inclusive.
fn diff_box(first: &Shot, second: &Shot) -> Option<(u32, u32, u32, u32)> {
    let mut found: Option<(u32, u32, u32, u32)> = None;
    for y in 0..first.height() {
        for x in 0..first.width() {
            if first.pixel(x, y) != second.pixel(x, y) {
                found = Some(match found {
                    None => (x, y, x, y),
                    Some((min_x, min_y, max_x, max_y)) => {
                        (min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y))
                    }
                });
            }
        }
    }
    found
}

fn camera_options(output: &Path) -> RecorderOptions {
    RecorderOptions {
        camera: CameraOptions {
            margin: 0.0,
            max_zoom: 2.0,
            scale_cursor: true,
        },
        ..options(SinkKind::Memory, output)
    }
}

fn badge_options(output: &Path) -> RecorderOptions {
    RecorderOptions {
        badge: BadgeStyle {
            font_size_logical: 10.0,
            ..BadgeStyle::default()
        },
        ..options(SinkKind::Memory, output)
    }
}

pub fn zoom(bad: &Mismatches, window: &slint::Window, dir: &Path) -> Result<(), String> {
    let raw = screenshot::capture(window)?;
    let output = dir.join("zoom").join("rec.mp4");
    let mut rec = Recorder::start(window, camera_options(&output))?;

    // Zoom 1.0: the frame is the capture itself.
    rec.hold(Duration::from_millis(100))?;
    check(
        bad,
        rec.frames().len() == 3 && rec.frames().iter().all(|f| same_pixels(f, &raw)),
        "zoom 1.0 frames differ from the plain capture",
    );
    check(
        bad,
        near(rec.camera(), (0.0, 0.0, 200.0, 100.0)),
        format!("default camera {:?}", rec.camera()),
    );

    // The red square (4, 4, 20 x 10) with margin 0 and max zoom 2: the camera is the window's
    // top-left quarter (0, 0, 100, 50), so 4 output pixels per logical pixel.
    rec.zoom_to((4.0, 4.0, 20.0, 10.0), Duration::from_millis(300))?;
    check(
        bad,
        near(rec.camera(), (0.0, 0.0, 100.0, 50.0)),
        format!("camera after zoom_to {:?}", rec.camera()),
    );
    rec.hold(Duration::from_millis(100))?;
    let zoomed = rec.frames().last().ok_or("no frames")?;
    check(
        bad,
        !same_pixels(zoomed, &raw),
        "the zoomed frame is the capture",
    );
    check(
        bad,
        (zoomed.width(), zoomed.height()) == (400, 200),
        "the output size changed",
    );
    check(
        bad,
        is_red(zoomed.pixel(50, 36)),
        "the square does not fill its region",
    );
    check(
        bad,
        is_red(zoomed.pixel(20, 20)),
        "the square misses its top-left corner",
    );
    check(
        bad,
        is_red(zoomed.pixel(92, 52)),
        "the square misses its bottom-right corner",
    );
    check(
        bad,
        is_white(zoomed.pixel(10, 10)),
        "ink left of / above the square",
    );
    check(
        bad,
        is_white(zoomed.pixel(110, 36)),
        "ink right of the square",
    );
    check(bad, is_white(zoomed.pixel(50, 70)), "ink below the square");

    zoom_cursor(bad, &mut rec)?;
    rec.abandon();
    zoom_roundtrip(bad, window, &raw, &output)
}

/// The cursor and the ripple through a zoomed camera, then back to the whole window.
fn zoom_cursor(bad: &Mismatches, rec: &mut Recorder<'_>) -> Result<(), String> {
    // The cursor maps through the camera: logical (60, 30) is output (240, 120).
    rec.move_to((60.0, 30.0), Duration::from_millis(100))?;
    let moved = rec.frames().last().ok_or("no frames")?;
    check(
        bad,
        dark_near(moved, 240, 120),
        "the cursor tip is not at the mapped position",
    );
    check(
        bad,
        !dark_near(moved, 120, 60),
        "the cursor is drawn unzoomed",
    );
    // The ripple is mapped too: a ring (radius up to 72 pixels at zoom 2) around the tip, so it
    // crosses the tip's row left of the tip.
    rec.click()?;
    let ripple_frame = &rec.frames()[rec.frames().len() - 5];
    check(
        bad,
        (150..236).any(|x| {
            ripple_frame
                .pixel(x, 120)
                .is_some_and(|p| u16::from(p.b) > u16::from(p.r) + 40)
        }),
        "no ripple pixels around the mapped cursor",
    );

    // Back to the whole window: the cursor is at the identity mapping again.
    rec.zoom_out(Duration::from_millis(300))?;
    rec.hold(Duration::from_millis(500))?;
    check(
        bad,
        near(rec.camera(), (0.0, 0.0, 200.0, 100.0)),
        format!("camera after zoom_out {:?}", rec.camera()),
    );
    let back = rec.frames().last().ok_or("no frames")?;
    check(
        bad,
        dark_near(back, 120, 60),
        "the cursor tip after zoom_out",
    );
    Ok(())
}

/// A zoom and a zoom out leave frames that are bit-identical to the capture again.
fn zoom_roundtrip(
    bad: &Mismatches,
    window: &slint::Window,
    raw: &Shot,
    output: &Path,
) -> Result<(), String> {
    let mut again = Recorder::start(window, camera_options(output))?;
    again.zoom_to((4.0, 4.0, 20.0, 10.0), Duration::from_millis(200))?;
    again.zoom_out(Duration::from_millis(200))?;
    again.hold(Duration::from_millis(100))?;
    let last = again.frames().last().ok_or("no frames")?;
    check(
        bad,
        same_pixels(last, raw),
        "frames after zoom_out are not plain",
    );
    // A midway frame is neither plain nor the final zoom.
    check(
        bad,
        again.frames().iter().any(|f| !same_pixels(f, raw)),
        "the zoom never showed",
    );
    check(
        bad,
        again
            .zoom_to((f32::NAN, 0.0, 10.0, 10.0), Duration::ZERO)
            .is_err()
            && again
                .zoom_to((500.0, 500.0, 10.0, 10.0), Duration::ZERO)
                .is_err()
            && again
                .zoom_to((0.0, 0.0, 0.0, 10.0), Duration::ZERO)
                .is_err(),
        "zoom_to accepted an invalid region",
    );
    again.abandon();
    Ok(())
}

pub fn follow(bad: &Mismatches, window: &slint::Window, dir: &Path) -> Result<(), String> {
    let output = dir.join("follow").join("rec.mp4");
    let mut rec = Recorder::start(window, camera_options(&output))?;
    rec.zoom_to((4.0, 4.0, 20.0, 10.0), Duration::ZERO)?;
    check(
        bad,
        near(rec.camera(), (0.0, 0.0, 100.0, 50.0)),
        format!("camera after an instant zoom_to {:?}", rec.camera()),
    );
    // Without following, a far target leaves the camera where it is (and the cursor off it).
    rec.move_to((50.0, 25.0), Duration::from_millis(100))?;
    rec.move_to((150.0, 70.0), Duration::from_millis(100))?;
    check(
        bad,
        near(rec.camera(), (0.0, 0.0, 100.0, 50.0)),
        format!("the camera moved without follow: {:?}", rec.camera()),
    );
    rec.move_to((50.0, 25.0), Duration::from_millis(100))?;

    // With following, the camera pans by the least amount that puts the target in the central
    // 70 % (x 80..150, y 35..70 of the window for the camera (65, 27.5, 100, 50)).
    rec.set_follow_cursor(true);
    rec.move_to((150.0, 70.0), Duration::from_millis(300))?;
    let camera = rec.camera();
    check(
        bad,
        near(camera, (65.0, 27.5, 100.0, 50.0)),
        format!("camera after the followed glide {camera:?}"),
    );
    let (safe_x, safe_y) = (
        (
            0.15_f32.mul_add(camera.2, camera.0),
            0.85_f32.mul_add(camera.2, camera.0),
        ),
        (
            0.15_f32.mul_add(camera.3, camera.1),
            0.85_f32.mul_add(camera.3, camera.1),
        ),
    );
    check(
        bad,
        (safe_x.0 - 0.01..=safe_x.1 + 0.01).contains(&150.0)
            && (safe_y.0 - 0.01..=safe_y.1 + 0.01).contains(&70.0),
        format!("the cursor (150, 70) is outside the safe area {safe_x:?} {safe_y:?}"),
    );
    let last = rec.frames().last().ok_or("no frames")?;
    check(
        bad,
        dark_near(last, 340, 170),
        "the cursor tip is not at the mapped position after the pan",
    );
    // A second move within the safe area does not pan.
    rec.move_to((120.0, 50.0), Duration::from_millis(100))?;
    check(
        bad,
        near(rec.camera(), camera),
        format!("the camera moved for a target inside: {:?}", rec.camera()),
    );
    // Following does nothing at zoom 1.0.
    rec.zoom_out(Duration::from_millis(100))?;
    rec.move_to((10.0, 10.0), Duration::from_millis(100))?;
    check(
        bad,
        near(rec.camera(), (0.0, 0.0, 200.0, 100.0)),
        format!("following moved the full camera: {:?}", rec.camera()),
    );
    rec.abandon();
    Ok(())
}

pub fn badge(bad: &Mismatches, window: &slint::Window, dir: &Path) -> Result<(), String> {
    let raw = screenshot::capture(window)?;
    let output = dir.join("badge").join("rec.mp4");
    let mut rec = Recorder::start(window, badge_options(&output))?;
    rec.hold(Duration::from_millis(100))?;
    check(
        bad,
        same_pixels(rec.frames().last().ok_or("no frames")?, &raw),
        "plain frames differ from the capture",
    );
    rec.show_keys("Ctrl + ↑");
    check(bad, rec.frames().len() == 3, "show_keys emitted a frame");
    rec.hold(Duration::from_millis(100))?;
    let with = rec.frames().last().ok_or("no frames")?;
    let Some((left, top, right, bottom)) = diff_box(&raw, with) else {
        check(bad, false, "the badge left no pixels");
        return Ok(());
    };
    let centre = u32::midpoint(left, right);
    check(
        bad,
        (197..=203).contains(&centre),
        format!("badge centre x {centre}, wanted 200"),
    );
    check(
        bad,
        (160..=172).contains(&bottom) && top > 60,
        format!("badge rows {top}..{bottom}, wanted the lower part ending near 168"),
    );
    let dark = (left..=right)
        .flat_map(|x| (top..=bottom).map(move |y| (x, y)))
        .filter(|&(x, y)| is_dark(with.pixel(x, y)))
        .count();
    check(bad, dark > 200, format!("only {dark} dark badge pixels"));
    // Text: light pixels well inside the dark pill can only be glyphs.
    let ink = (left + 9..right.saturating_sub(9))
        .flat_map(|x| (top + 9..bottom.saturating_sub(9)).map(move |y| (x, y)))
        .filter(|&(x, y)| {
            with.pixel(x, y)
                .is_some_and(|p| p.r > 200 && p.g > 200 && p.b > 200)
        })
        .count();
    check(
        bad,
        ink > 10,
        format!("only {ink} text pixels inside the badge"),
    );

    // Visible for 1400 ms from the frame after show_keys (start 100 ms): the frame that ends at
    // 1467 ms (age 1367) is fading, the one that ends at 1500 ms (age 1400) is plain.
    rec.hold(Duration::from_millis(1500))?;
    check(
        bad,
        rec.frames().len() == 51,
        format!("{} frames, wanted 51", rec.frames().len()),
    );
    check(
        bad,
        rec.frames()[43].pixel(left + 6, u32::midpoint(top, bottom))
            != raw.pixel(left + 6, u32::midpoint(top, bottom)),
        "the fading badge is already gone",
    );
    check(
        bad,
        same_pixels(&rec.frames()[44], &raw) && same_pixels(&rec.frames()[50], &raw),
        "the badge is still there after visible_ms",
    );
    // The fade: the badge is fainter at 1467 ms than at 1100 ms.
    let faint = rec.frames()[43]
        .pixel(left + 6, u32::midpoint(top, bottom))
        .map_or(0, |p| p.r);
    let solid = rec.frames()[36]
        .pixel(left + 6, u32::midpoint(top, bottom))
        .map_or(255, |p| p.r);
    check(
        bad,
        faint > solid + 60,
        format!("the fade: red {faint} at 1467 ms, {solid} at 1233 ms"),
    );

    badge_replaced(bad, &mut rec, &raw)?;
    rec.abandon();
    badge_under_zoom(bad, window, &output, (left, top, right, bottom))
}

/// A new badge replaces the old one; an empty label removes it.
fn badge_replaced(bad: &Mismatches, rec: &mut Recorder<'_>, raw: &Shot) -> Result<(), String> {
    rec.show_keys("F5");
    rec.hold(Duration::from_millis(100))?;
    check(
        bad,
        diff_box(raw, rec.frames().last().ok_or("no frames")?).is_some(),
        "the second badge is missing",
    );
    rec.show_keys("");
    rec.hold(Duration::from_millis(100))?;
    check(
        bad,
        same_pixels(rec.frames().last().ok_or("no frames")?, raw),
        "an empty label did not remove the badge",
    );
    Ok(())
}

/// Under a zoomed camera the badge is neither moved nor scaled; `badge_box` is where the badge
/// sits in the unzoomed frame.
fn badge_under_zoom(
    bad: &Mismatches,
    window: &slint::Window,
    output: &Path,
    badge_box: (u32, u32, u32, u32),
) -> Result<(), String> {
    let mut zoomed = Recorder::start(window, camera_options(output))?;
    zoomed.zoom_to((4.0, 4.0, 20.0, 10.0), Duration::ZERO)?;
    zoomed.hold(Duration::from_millis(100))?;
    let plain = zoomed.frames().last().ok_or("no frames")?.clone();
    let mut zoomed_options = badge_options(output);
    zoomed_options.camera = camera_options(output).camera;
    zoomed.abandon();
    let mut zoomed = Recorder::start(window, zoomed_options)?;
    zoomed.zoom_to((4.0, 4.0, 20.0, 10.0), Duration::ZERO)?;
    zoomed.hold(Duration::from_millis(100))?;
    zoomed.show_keys("Ctrl + ↑");
    zoomed.hold(Duration::from_millis(100))?;
    let zoomed_with = zoomed.frames().last().ok_or("no frames")?;
    check(
        bad,
        diff_box(&plain, zoomed_with) == Some(badge_box),
        format!(
            "the badge moved under zoom: {:?}, unzoomed {badge_box:?}",
            diff_box(&plain, zoomed_with),
        ),
    );
    zoomed.abandon();
    Ok(())
}
