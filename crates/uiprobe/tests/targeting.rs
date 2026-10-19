//! Self-test: element targeting (`uiprobe::targeting`) against a real window.
//!
//! A 400x260 window. On the left a 190x100 `ScrollView` whose content holds three tiles: `Row A`
//! (fully visible), `Mostly` (50..110, so 83 % shows) and `Half` (70..130, so 50 % shows). Below
//! it a std `Button`, a `Panel` with an `Apply` tile and a second `Apply` outside it. On the right
//! two `Dup` tiles, a disabled `Locked` tile, three `Edit` tiles at different heights, and a
//! `Name` caption beside a `Name` text field. Every case must behave as specified:
//!
//! * an element scrolled partly out of its viewport is clicked at the centre of its *visible*
//!   part, and one that is mostly out (`Half`) is `NotVisible`, not clicked at its centre;
//! * the same label on two different visible controls is `Ambiguous`, a missing one `NotFound`, a
//!   disabled one `Disabled`; a role, an ancestor component or a row (nearest y) tells them apart;
//! * a button and its caption count as one control;
//! * the scroll plan for `Half` is negative (content up), and turning the wheel by it makes `Half`
//!   clickable.
//!
//! Needs Slint debug info at compile time; `.cargo/config.toml` of this workspace sets
//! `SLINT_EMIT_DEBUG_INFO=1` for the `slint!` macro. The fixture uses `FixedTestFont` so the
//! geometry is the same on every machine. Assertions are made here and printed on a mismatch;
//! they are not added to the report, which must stay at zero findings.

use std::{cell::RefCell, rc::Rc};

use slint::{LogicalPosition, platform::WindowEvent};
use uiprobe::{
    prelude::*,
    targeting::{Spot, TargetError, scroll_into_view_plan},
    view::geometry::wheel_steps,
};

slint::slint! {
    import { ScrollView, Button } from "std-widgets.slint";

    component Tile inherits Rectangle {
        in property <string> caption;
        in property <bool> active: true;
        background: #3a6ea5;
        accessible-role: button;
        accessible-label: root.caption;
        accessible-enabled: root.active;
    }

    component Field inherits Rectangle {
        background: #e8e8e8;
        accessible-role: text-input;
        accessible-label: "Name";
    }

    component Panel inherits Rectangle {
        background: #cccccc;
        @children
    }

    export component Fixture inherits Window {
        width: 400px;
        height: 260px;
        default-font-family: "FixedTestFont";

        ScrollView {
            x: 0px;
            y: 0px;
            width: 190px;
            height: 100px;
            // A Flickable does not take its content size from a child's `height`; without this
            // the content is as high as the viewport and the wheel has nothing to scroll.
            content-height: 400px;
            // Pinned at the top (an unpositioned child is centred in the viewport) and narrower
            // than the viewport, so no horizontal scrollbar eats into its height.
            Rectangle {
                x: 0px;
                y: 0px;
                width: 150px;
                height: 400px;
                Tile { caption: "Row A"; x: 5px; y: 0px; width: 65px; height: 30px; }
                Tile { caption: "Mostly"; x: 5px; y: 50px; width: 65px; height: 60px; }
                Tile { caption: "Half"; x: 75px; y: 70px; width: 70px; height: 60px; }
            }
        }

        Button { text: "Std OK"; x: 5px; y: 110px; width: 100px; height: 30px; }
        Panel {
            x: 5px;
            y: 150px;
            width: 180px;
            height: 50px;
            Tile { caption: "Apply"; x: 5px; y: 10px; width: 80px; height: 30px; }
        }
        Tile { caption: "Apply"; x: 5px; y: 210px; width: 80px; height: 30px; }

        Tile { caption: "Dup"; x: 200px; y: 5px; width: 80px; height: 30px; }
        Tile { caption: "Dup"; x: 200px; y: 45px; width: 80px; height: 30px; }
        Tile { caption: "Locked"; active: false; x: 200px; y: 85px; width: 80px; height: 30px; }
        Tile { caption: "Edit"; x: 200px; y: 125px; width: 80px; height: 25px; }
        Tile { caption: "Edit"; x: 200px; y: 155px; width: 80px; height: 25px; }
        Tile { caption: "Edit"; x: 200px; y: 185px; width: 80px; height: 25px; }
        Text {
            x: 200px;
            y: 225px;
            width: 40px;
            height: 24px;
            text: "Name";
            accessible-role: text;
            accessible-label: "Name";
        }
        Field { x: 245px; y: 225px; width: 140px; height: 24px; }
    }
}

type Mismatches = Rc<RefCell<Vec<String>>>;

/// Records a mismatch unless `ok`.
fn check(bad: &mut Vec<String>, what: &str, ok: bool, detail: impl FnOnce() -> String) {
    if !ok {
        bad.push(format!("{what}: {}", detail()));
    }
}

/// A one-line description of a query result.
fn describe(result: &Result<Spot, TargetError>) -> String {
    match result {
        Ok(spot) => format!(
            "Ok at ({:.1}, {:.1}) size {:.1}x{:.1}",
            spot.x, spot.y, spot.w, spot.h
        ),
        Err(error) => format!("Err({error:?})"),
    }
}

/// Whether `result` is `Ok` with the centre of its click point at `(x, y)` give or take `slack`
/// (`None` skips an axis).
fn clicks_at(
    result: &Result<Spot, TargetError>,
    x: Option<f32>,
    y: Option<f32>,
    slack: f32,
) -> bool {
    result.as_ref().is_ok_and(|spot| {
        spot.click_point().is_some_and(|(cx, cy)| {
            x.is_none_or(|wanted| (cx - wanted).abs() <= slack)
                && y.is_none_or(|wanted| (cy - wanted).abs() <= slack)
        })
    })
}

/// Every query case except the scroll, which changes the window.
fn query_cases(bad: &mut Vec<String>, view: &WindowView) {
    let row_a = view.spot_label("Row A");
    check(
        bad,
        "spot_label(\"Row A\") is Ok and clickable",
        clicks_at(&row_a, Some(37.5), Some(15.0), 1.0),
        || describe(&row_a),
    );

    // Mostly is 50..110 in a viewport ending at 100: its click point is the centre of what shows
    // (about 75), not the centre of the full rectangle (80).
    let mostly = view.spot_label("Mostly");
    let mostly_ok = mostly.as_ref().is_ok_and(|spot| {
        let full_centre = spot.center().1;
        let shows_less = spot
            .visible_rect()
            .is_some_and(|visible| visible.bottom() < spot.rect().bottom() - 1.0);
        shows_less
            && spot
                .click_point()
                .is_some_and(|(_, cy)| cy < full_centre - 2.0)
    });
    check(
        bad,
        "spot_label(\"Mostly\") clicks the visible part",
        mostly_ok,
        || describe(&mostly),
    );

    let half = view.spot_label("Half");
    check(
        bad,
        "spot_label(\"Half\") is NotVisible",
        matches!(half, Err(TargetError::NotVisible { .. })),
        || describe(&half),
    );

    let dup = view.spot_label("Dup");
    check(
        bad,
        "spot_label(\"Dup\") is Ambiguous with count 2",
        matches!(&dup, Err(TargetError::Ambiguous { count: 2, .. })),
        || describe(&dup),
    );
    let nope = view.spot_label("Nope");
    check(
        bad,
        "spot_label(\"Nope\") is NotFound",
        matches!(nope, Err(TargetError::NotFound { .. })),
        || describe(&nope),
    );
    let locked = view.spot_label("Locked");
    check(
        bad,
        "spot_label(\"Locked\") is Disabled",
        matches!(locked, Err(TargetError::Disabled { .. })),
        || describe(&locked),
    );

    let std_ok = view.spot_label("Std OK");
    check(
        bad,
        "spot_label(\"Std OK\") finds the button once, caption included",
        std_ok.is_ok(),
        || describe(&std_ok),
    );

    question_mark_cases(bad, view);
    prefix_cases(bad, view);
    scope_cases(bad, view);
    row_cases(bad, view);
}

/// The README's `let spot = view.spot_label("Save")?;` inside a function returning
/// `Result<_, Finding>`, exactly as a scenario writes it.
fn question_mark_in_a_scenario(view: &WindowView, label: &str) -> Result<(f32, f32), Finding> {
    let spot = view.spot_label(label)?;
    Ok(spot.click_point().unwrap_or((0.0, 0.0)))
}

/// `?` on a query turns a `TargetError` into a finding of window `-`, with the check and detail
/// of `TargetError::finding`.
fn question_mark_cases(bad: &mut Vec<String>, view: &WindowView) {
    let found = question_mark_in_a_scenario(view, "Row A");
    check(
        bad,
        "`?` on spot_label(\"Row A\") passes the click point through",
        found.as_ref().is_ok_and(|(x, y)| *x > 0.0 && *y > 0.0),
        || format!("{found:?}"),
    );
    for (label, check_name) in [
        ("Nope", "not-found"),
        ("Dup", "ambiguous-label"),
        ("Half", "not-visible"),
        ("Locked", "disabled"),
    ] {
        let result = question_mark_in_a_scenario(view, label);
        check(
            bad,
            &format!("`?` on spot_label({label:?}) becomes a `{check_name}` finding of window `-`"),
            result.as_ref().is_err_and(|finding| {
                finding.window == "-"
                    && finding.check == check_name
                    && finding.element.is_empty()
                    && finding.detail.contains(label)
            }),
            || format!("{result:?}"),
        );
    }
    // The explicit conversion with a named window agrees with the one `?` uses, apart from the
    // window.
    let named = view
        .spot_label("Nope")
        .err()
        .map(|error| error.finding("MainWindow"));
    check(
        bad,
        "TargetError::finding(\"MainWindow\") names the window",
        named
            .as_ref()
            .is_some_and(|finding| finding.window == "MainWindow" && finding.check == "not-found"),
        || format!("{named:?}"),
    );
}

/// Prefix queries.
fn prefix_cases(bad: &mut Vec<String>, view: &WindowView) {
    let one = view.spot_label_start("Ro");
    check(bad, "spot_label_start(\"Ro\") is Ok", one.is_ok(), || {
        describe(&one)
    });
    let two = view.spot_label_start("Du");
    check(
        bad,
        "spot_label_start(\"Du\") is Ambiguous with count 2",
        matches!(&two, Err(TargetError::Ambiguous { count: 2, .. })),
        || describe(&two),
    );
    let three = view.spot_label_start("Ed");
    check(
        bad,
        "spot_label_start(\"Ed\") is Ambiguous with count 3",
        matches!(&three, Err(TargetError::Ambiguous { count: 3, .. })),
        || describe(&three),
    );
    let off = view.spot_label_start("Lock");
    check(
        bad,
        "spot_label_start(\"Lock\") is Disabled",
        matches!(off, Err(TargetError::Disabled { .. })),
        || describe(&off),
    );
}

/// Role and ancestor scoping.
fn scope_cases(bad: &mut Vec<String>, view: &WindowView) {
    let caption_and_field = view.spot_label("Name");
    check(
        bad,
        "spot_label(\"Name\") is Ambiguous (caption and field)",
        matches!(
            &caption_and_field,
            Err(TargetError::Ambiguous { count: 2, .. })
        ),
        || describe(&caption_and_field),
    );
    let field = view.spot_role(AccessibleRole::TextInput, "Name");
    check(
        bad,
        "spot_role(TextInput, \"Name\") is the field, not the caption",
        field.as_ref().is_ok_and(|spot| spot.x > 240.0),
        || describe(&field),
    );

    let both = view.spot_label("Apply");
    check(
        bad,
        "spot_label(\"Apply\") is Ambiguous with count 2",
        matches!(&both, Err(TargetError::Ambiguous { count: 2, .. })),
        || describe(&both),
    );
    let inside = view.spot_within("Panel", "Apply");
    check(
        bad,
        "spot_within(\"Panel\", \"Apply\") is the one inside the panel",
        inside
            .as_ref()
            .is_ok_and(|spot| spot.y >= 149.0 && spot.y < 200.0),
        || describe(&inside),
    );
    let no_scope = view.spot_within("NoSuchComponent", "Apply");
    check(
        bad,
        "spot_within(\"NoSuchComponent\", ..) is NotFound",
        matches!(no_scope, Err(TargetError::NotFound { .. })),
        || describe(&no_scope),
    );
}

/// Picking by row.
fn row_cases(bad: &mut Vec<String>, view: &WindowView) {
    // The Edit tiles are centred at 137.5, 167.5 and 197.5.
    let middle = view.spot_label_near_y("Edit", 165.0, 12.0);
    check(
        bad,
        "spot_label_near_y(\"Edit\", 165, 12) is the middle one",
        clicks_at(&middle, None, Some(167.5), 1.0),
        || describe(&middle),
    );
    let far = view.spot_label_near_y("Edit", 400.0, 12.0);
    check(
        bad,
        "spot_label_near_y(\"Edit\", 400, 12) is NotFound",
        matches!(far, Err(TargetError::NotFound { .. })),
        || describe(&far),
    );
    let last = view.spot_label_start_near_y("Ed", 195.0, 15.0);
    check(
        bad,
        "spot_label_start_near_y(\"Ed\", 195, 15) is the last one",
        clicks_at(&last, None, Some(197.5), 1.0),
        || describe(&last),
    );
}

/// The scroll plan for the half-hidden tile, performed with wheel events, then re-queried.
fn scroll_cases(bad: &mut Vec<String>, app: &Attached, ctx_view: impl Fn() -> WindowView) {
    let view = ctx_view();
    let half = view.try_find("Half").map(|handle| view.spot(&handle));
    check(
        bad,
        "Spot::in_view(Half) is clipped to about half and not clickable",
        half.as_ref().is_some_and(|spot| {
            (spot.visible_fraction() - 0.5).abs() < 0.1 && spot.click_point().is_none()
        }),
        || "the Half element is missing or its clipping is wrong".to_owned(),
    );
    let plan = half.as_ref().and_then(scroll_into_view_plan);
    check(
        bad,
        "scroll_into_view_plan(Half) scrolls the content up",
        plan.as_ref()
            .is_some_and(|(container, delta)| *delta < -25.0 && container.handle.is_some()),
        || {
            plan.as_ref().map_or_else(
                || "no plan".to_owned(),
                |(_, delta)| format!("delta {delta}"),
            )
        },
    );
    let row_a = view.spot_label("Row A").ok();
    check(
        bad,
        "scroll_into_view_plan(Row A) is None (fully visible)",
        row_a
            .as_ref()
            .is_some_and(|spot| scroll_into_view_plan(spot).is_none()),
        || "Row A needs a scroll or is missing".to_owned(),
    );
    let outside = view.spot_label("Std OK").ok();
    check(
        bad,
        "scroll_into_view_plan(Std OK) is None (not in a scroll container)",
        outside
            .as_ref()
            .is_some_and(|spot| scroll_into_view_plan(spot).is_none()),
        || "Std OK has a scroll plan or is missing".to_owned(),
    );

    let Some((container, delta)) = plan else {
        return;
    };
    let (cx, cy) = container.center();
    let position = LogicalPosition::new(cx, cy);
    let window = app.window();
    window.dispatch_event(WindowEvent::PointerMoved { position });
    for step in wheel_steps(delta) {
        window.dispatch_event(WindowEvent::PointerScrolled {
            position,
            delta_x: 0.0,
            delta_y: step,
        });
        pump(20);
    }
    pump(100);
    let after = ctx_view().spot_label("Half");
    check(
        bad,
        "after the planned wheel turn Half is clickable",
        after.is_ok(),
        || describe(&after),
    );
}

fn main() -> ExitCode {
    let harness = Harness::new();
    let mismatches: Mismatches = Rc::default();
    let sink = Rc::clone(&mismatches);
    let report = harness
        .report_dir(uiprobe::target_dir!().join("uiprobe-selftest-targeting"))
        .scenario("targeting", move |ctx| {
            let app = ctx.launch(Fixture::new)?;
            let mut bad = sink.borrow_mut();
            query_cases(&mut bad, &ctx.view(&app));
            scroll_cases(&mut bad, &app, || ctx.view(&app));
            drop(bad);
            ctx.shutdown(app);
            Ok(())
        })
        .run_to_report();

    let mut failed = false;
    for mismatch in mismatches.borrow().iter() {
        eprintln!("targeting: MISMATCH: {mismatch}");
        failed = true;
    }
    for finding in report.findings() {
        eprintln!(
            "targeting: unexpected finding {}: {}",
            finding.check, finding.detail
        );
        failed = true;
    }
    if failed {
        ExitCode::FAILURE
    } else {
        println!("targeting: ok");
        ExitCode::SUCCESS
    }
}
