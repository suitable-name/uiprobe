//! Self-test (feature `screenshots`): the software renderer draws real pixels, `screenshot`
//! captures them at scale 1 and 2, crops to an element and saves a PNG.
//!
//! The fixture is a white 200 x 100 window with a red square at logical (20, 10) .. (60, 30)
//! and a button "Target" at (100, 40), 60 x 30.
//!
//! 1. Scale 1: the capture is 200 x 100, red inside the square, white outside it.
//! 2. `set_window_size((200, 100), 2.0)`: the capture is 400 x 200 at scale 2, the square is at
//!    physical (40, 20) .. (120, 60).
//! 3. `crop_to(Target, 5.0)` at scale 2 is (60 + 10) * 2 x (30 + 10) * 2 = 140 x 80.
//! 4. `save_png` writes a PNG of that size and leaves no temporary file.
//!
//! Needs Slint debug info at compile time (`SLINT_EMIT_DEBUG_INFO=1`, set by this workspace's
//! `.cargo/config.toml`) for the element query of step 3.

use std::{cell::RefCell, process::ExitCode, rc::Rc};

use slint::Rgba8Pixel;
use uiprobe::{
    Harness,
    backend::{Renderer, pump},
    report::Report,
    screenshot::{self, Shot},
};

slint::slint! {
    import { Button } from "std-widgets.slint";

    export component ShotFixture inherits Window {
        width: 200px;
        height: 100px;
        background: #ffffff;
        Rectangle {
            x: 20px;
            y: 10px;
            width: 40px;
            height: 20px;
            background: #ff0000;
        }
        Button {
            text: "Target";
            x: 100px;
            y: 40px;
            width: 60px;
            height: 30px;
        }
    }
}

type Mismatches = Rc<RefCell<Vec<String>>>;

fn is_red(pixel: Option<Rgba8Pixel>) -> bool {
    pixel.is_some_and(|p| p.r > 200 && p.g < 60 && p.b < 60)
}

fn is_white(pixel: Option<Rgba8Pixel>) -> bool {
    pixel.is_some_and(|p| p.r > 230 && p.g > 230 && p.b > 230)
}

fn check(bad: &Mismatches, ok: bool, what: impl Into<String>) {
    if !ok {
        bad.borrow_mut().push(what.into());
    }
}

fn check_picture(bad: &Mismatches, shot: &Shot, scale: u32) {
    let label = format!("scale {scale}");
    check(
        bad,
        (shot.width(), shot.height()) == (200 * scale, 100 * scale),
        format!(
            "{label}: capture is {} x {}, wanted {} x {}",
            shot.width(),
            shot.height(),
            200 * scale,
            100 * scale
        ),
    );
    // Inside the square (logical 40, 20) and on the white background (logical 5, 90).
    check(
        bad,
        is_red(shot.pixel(40 * scale, 20 * scale)),
        format!(
            "{label}: the square is not red: {:?}",
            shot.pixel(40 * scale, 20 * scale)
        ),
    );
    check(
        bad,
        is_white(shot.pixel(5 * scale, 90 * scale)),
        format!(
            "{label}: the background is not white: {:?}",
            shot.pixel(5 * scale, 90 * scale)
        ),
    );
}

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
    let report = Harness::new()
        .renderer(Renderer::Software)
        .scenario("screenshots", move |ctx| {
            let app = ctx.launch(ShotFixture::new)?;
            pump(20);

            match screenshot::capture(app.window()) {
                Ok(shot) => check_picture(&sink, &shot, 1),
                Err(e) => check(&sink, false, format!("scale 1: {e}")),
            }

            screenshot::set_window_size(app.window(), (200, 100), 2.0);
            pump(20);
            let shot = match screenshot::capture(app.window()) {
                Ok(shot) => shot,
                Err(e) => {
                    check(&sink, false, format!("scale 2: {e}"));
                    ctx.shutdown(app);
                    return Ok(());
                }
            };
            check_picture(&sink, &shot, 2);
            check(
                &sink,
                (shot.scale() - 2.0).abs() < f32::EPSILON,
                format!("scale 2: the capture reports scale {}", shot.scale()),
            );

            let target = app.view().find("Target")?;
            let part = shot.crop_to(&target, 5.0);
            check(
                &sink,
                (part.width(), part.height()) == (140, 80),
                format!(
                    "crop_to: {} x {}, wanted 140 x 80",
                    part.width(),
                    part.height()
                ),
            );

            let dir = ctx.profile().scratch_dir();
            let path = dir.join("shots").join("target.png");
            match part.save_png(&path) {
                Ok(()) => {
                    let size = std::fs::File::open(&path)
                        .map_err(|e| e.to_string())
                        .and_then(|file| {
                            png::Decoder::new(std::io::BufReader::new(file))
                                .read_info()
                                .map(|reader| (reader.info().width, reader.info().height))
                                .map_err(|e| e.to_string())
                        });
                    check(
                        &sink,
                        size == Ok((140, 80)),
                        format!("save_png: the file reads back as {size:?}, wanted 140 x 80"),
                    );
                    check(
                        &sink,
                        !dir.join("shots").join("target.png.partial").exists(),
                        "save_png left its temporary file",
                    );
                }
                Err(e) => check(&sink, false, format!("save_png: {e}")),
            }
            ctx.shutdown(app);
            Ok(())
        })
        .run_to_report();
    finish(&bad, &report)
}
