//! Unit tests of the target resolution that need no window: clipping of a `Spot`, the click
//! point, ambiguity, `NotVisible`, `NotFound` and row selection, on spots built from bare
//! rectangles. The element-backed cases (disabled, scope, scroll plan) are in
//! `tests/targeting.rs`.

use super::{Rect, Spot, TargetError, TargetSpec, choose};

/// A 190 x 100 scroll viewport at the origin.
const VIEWPORT: Rect = Rect::new(0.0, 0.0, 190.0, 100.0);

/// A row of a scrolled list: `top` is its y, 30 high.
fn row(top: f32) -> Spot {
    Spot::from_rect(Rect::new(5.0, top, 100.0, 30.0))
        .in_window((300.0, 200.0))
        .clipped_by(VIEWPORT)
}

#[test]
fn a_spot_clips_against_its_viewport_and_the_window() {
    let spot = row(80.0);
    assert_eq!(spot.visible_rect(), Some(Rect::new(5.0, 80.0, 100.0, 20.0)));
    let outside = Spot::from_rect(Rect::new(280.0, 10.0, 40.0, 20.0)).in_window((300.0, 200.0));
    assert_eq!(
        outside.visible_rect(),
        Some(Rect::new(280.0, 10.0, 20.0, 20.0))
    );
}

#[test]
fn a_spot_without_clips_is_fully_visible() {
    let spot = Spot::from_rect(Rect::new(5.0, 80.0, 100.0, 30.0));
    assert_eq!(spot.visible_rect(), Some(spot.rect()));
    assert_eq!(spot.click_point(), Some((55.0, 95.0)));
}

#[test]
fn the_click_point_is_the_centre_of_the_visible_part() {
    let spot = row(80.0);
    let (cx, cy) = spot.click_point().expect("20 of 30 pixels show");
    assert!((cx - 55.0).abs() < 1.0e-3);
    assert!(
        (cy - 90.0).abs() < 1.0e-3,
        "centre of 80..100, not of 80..110: {cy}"
    );
}

#[test]
fn a_row_mostly_scrolled_out_has_no_click_point() {
    let spot = row(88.0);
    assert!(spot.center().1 > VIEWPORT.bottom());
    assert!((spot.visible_fraction() - 0.4).abs() < 1.0e-3);
    assert_eq!(spot.click_point(), None);
}

#[test]
fn two_visible_controls_with_one_label_are_ambiguous() {
    let spec = TargetSpec::exact("Apply");
    assert_eq!(
        choose(&spec, vec![row(0.0), row(40.0)]).err(),
        Some(TargetError::Ambiguous {
            label: "Apply".to_owned(),
            count: 2,
        })
    );
}

#[test]
fn a_hidden_duplicate_does_not_make_the_label_ambiguous() {
    let spec = TargetSpec::exact("Apply");
    let picked = choose(&spec, vec![row(0.0), row(90.0)]).expect("only the first shows");
    assert!(picked.y.abs() < 1.0e-3);
}

#[test]
fn a_control_that_exists_but_does_not_show_is_not_visible() {
    let spec = TargetSpec::exact("Apply");
    assert_eq!(
        choose(&spec, vec![row(90.0)]).err(),
        Some(TargetError::NotVisible {
            label: "Apply".to_owned(),
        })
    );
}

#[test]
fn no_control_is_not_found() {
    let spec = TargetSpec::prefix("Ap");
    assert_eq!(
        choose(&spec, Vec::new()).err(),
        Some(TargetError::NotFound {
            label: "Ap".to_owned(),
        })
    );
}

#[test]
fn near_y_picks_the_row_instead_of_complaining() {
    let spec = TargetSpec::exact("Edit").near_y(55.0, 20.0);
    let picked = choose(&spec, vec![row(0.0), row(40.0), row(70.0)]).expect("row two");
    assert!((picked.y - 40.0).abs() < 1.0e-3);
}

#[test]
fn near_y_beyond_the_tolerance_is_not_found() {
    let spec = TargetSpec::exact("Edit").near_y(500.0, 20.0);
    assert!(matches!(
        choose(&spec, vec![row(0.0), row(40.0)]),
        Err(TargetError::NotFound { .. })
    ));
}

#[test]
fn near_y_on_a_hidden_row_is_not_visible_not_another_row() {
    let spec = TargetSpec::exact("Edit").near_y(110.0, 20.0);
    assert!(matches!(
        choose(&spec, vec![row(0.0), row(95.0)]),
        Err(TargetError::NotVisible { .. })
    ));
}

#[test]
fn the_union_of_two_spots_has_no_element() {
    let joined = row(0.0).union(&row(60.0));
    assert!(joined.handle.is_none());
    assert_eq!(joined.rect(), Rect::new(5.0, 0.0, 100.0, 90.0));
}

#[test]
fn a_target_error_names_its_check_and_label() {
    let error = TargetError::Ambiguous {
        label: "OK".to_owned(),
        count: 3,
    };
    assert_eq!(error.check(), "ambiguous-label");
    assert_eq!(error.label(), "OK");
    assert!(error.to_string().contains("3 different visible controls"));
    let finding = error.finding("MainWindow");
    assert_eq!(finding.window, "MainWindow");
    assert_eq!(finding.check, "ambiguous-label");
}

#[test]
fn a_label_match_is_exact_or_a_prefix() {
    let exact = TargetSpec::exact("Save");
    assert!(exact.label.matches("Save"));
    assert!(!exact.label.matches("Save As"));
    let prefix = TargetSpec::prefix("Save");
    assert!(prefix.label.matches("Save As"));
    assert!(!prefix.label.matches("Re-save"));
}
