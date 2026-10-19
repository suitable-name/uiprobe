//! Self-test: `SweepOptions::floating_layers` and the cross-layer exemption of `overlap`.
//!
//! The fixture has a button "Under" on the content and a `Panel` (a custom layer type) drawn
//! over it with three buttons: "Over" overlaps "Under" across the layer boundary, "A" and "B"
//! overlap each other inside the layer.
//!
//! 1. Default options (`["PopupWindow"]`): `Panel` is not a layer, so both overlaps are
//!    reported: exactly 2 `overlap`.
//! 2. `floating_layers = ["Panel"]`: the cross-layer pair is exempt, the pair inside one layer
//!    is still reported: exactly 1 `overlap`, and it names "A" or "B", never "Over".
//! 3. `floating_layers = ["PopupWindow"]` again (the type removed from the list): the
//!    cross-layer finding is back, 2 `overlap`.
//! 4. An empty list behaves like 3; the default value is `["PopupWindow"]`.
//!
//! Needs Slint debug info at compile time (`SLINT_EMIT_DEBUG_INFO=1`, set by this workspace's
//! `.cargo/config.toml`). The fixture uses `FixedTestFont` for machine-independent geometry.

use std::{cell::RefCell, process::ExitCode, rc::Rc};

use uiprobe::{
    Harness,
    checks::{SweepOptions, sweep},
    report::{Finding, Report},
};

slint::slint! {
    import { Button } from "std-widgets.slint";

    component Panel inherits Rectangle {
        @children
    }

    export component LayerFixture inherits Window {
        width: 300px;
        height: 120px;
        default-font-family: "FixedTestFont";
        Button {
            text: "Under";
            x: 10px;
            y: 10px;
            width: 100px;
            height: 30px;
        }
        Panel {
            x: 50px;
            y: 10px;
            width: 200px;
            height: 100px;
            // Absolute x 60..160, y 10..40: 50 px over "Under".
            Button {
                text: "Over";
                x: 10px;
                y: 0px;
                width: 100px;
                height: 30px;
            }
            // Absolute y 50..80; "B" is 50 px to the right of "A".
            Button {
                text: "A";
                x: 0px;
                y: 40px;
                width: 100px;
                height: 30px;
            }
            Button {
                text: "B";
                x: 50px;
                y: 40px;
                width: 100px;
                height: 30px;
            }
        }
    }
}

type Mismatches = Rc<RefCell<Vec<String>>>;

fn overlaps(findings: &[Finding]) -> Vec<&Finding> {
    findings.iter().filter(|f| f.check == "overlap").collect()
}

fn expect_overlaps(bad: &Mismatches, what: &str, findings: &[Finding], want: usize) -> bool {
    use std::fmt::Write as _;
    let hits = overlaps(findings);
    if hits.len() == want {
        return true;
    }
    let mut text = format!("{what}: overlap: expected {want}, got {}", hits.len());
    for f in hits {
        let _ = write!(text, "\n    {} | {} | {}", f.window, f.element, f.detail);
    }
    bad.borrow_mut().push(text);
    false
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

fn with_layers(layers: &[&str]) -> SweepOptions {
    SweepOptions {
        floating_layers: layers.iter().map(ToString::to_string).collect(),
        ..SweepOptions::default()
    }
}

fn main() -> ExitCode {
    let bad: Mismatches = Rc::default();
    let sink = Rc::clone(&bad);
    let report = Harness::new()
        .scenario("floating layers", move |ctx| {
            if SweepOptions::default().floating_layers != ["PopupWindow".to_owned()] {
                sink.borrow_mut()
                    .push("default floating_layers is not [\"PopupWindow\"]".to_owned());
            }
            let app = ctx.launch(LayerFixture::new)?;
            let view = app.view();
            let default = sweep(&view, &SweepOptions::default());
            expect_overlaps(&sink, "default list", &default, 2);

            let layered = sweep(&view, &with_layers(&["Panel"]));
            if expect_overlaps(&sink, "Panel is a layer", &layered, 1) {
                let detail = &overlaps(&layered)[0].element;
                if detail.contains("\"Over\"") || detail.contains("\"Under\"") {
                    sink.borrow_mut().push(format!(
                        "Panel is a layer: the remaining overlap is the cross-layer pair: {detail}"
                    ));
                }
            }

            let removed = sweep(&view, &with_layers(&["PopupWindow"]));
            expect_overlaps(&sink, "Panel removed again", &removed, 2);
            let none = sweep(&view, &with_layers(&[]));
            expect_overlaps(&sink, "empty list", &none, 2);
            ctx.shutdown(app);
            Ok(())
        })
        .run_to_report();
    finish(&bad, &report)
}
