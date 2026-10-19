//! `overlap`: partial overlap between two enabled controls that a user can see and click,
//! measured on their visible parts (after clipping against scroll viewports).
//!
//! Not an overlap:
//!
//! 1. full containment (nesting, for example a `TouchArea` inside a `Button`);
//! 2. a control under a modal layer: a role-less `TouchArea` that covers a quarter of the window
//!    or more ([`SweepOptions::modal_cover`]) and is drawn LATER than the control, with the
//!    control's centre inside it. For example, in Indicatrix: the backdrop `TouchArea {}` of
//!    `WelcomeDialog`, `ConfirmActionDialog`, `NewDesignDialog` and the command palette in
//!    `ui/components/*_dialog.slint`. Slint draws in document order, so everything before the
//!    backdrop is covered and cannot be clicked. The `EmptyState` card grid has no backdrop
//!    `TouchArea` and so is not hidden by this: a card grid that covers the editor command bar
//!    (`EmptyState::*` against `FileActionsGroup::*` at 720x480) is a real overlap;
//! 3. a control laid on a header-size `TouchArea` backdrop on purpose: the `TouchArea` is a
//!    control of [`SweepOptions::header_max_h`] pixels or less in height that is declared
//!    BEFORE the other control, whose centre lies inside it and which lies at least
//!    [`SweepOptions::header_inside`] inside it. For example, in Indicatrix
//!    `SectionHeader::touch` declares the hit area first and the right-side `@children` slot
//!    after it, so the Tier/Preform/Optimize pills draw on top and win the click
//!    (`ui/components/section_header.slint` lines 23-25, 50 and 99). Two plain buttons that
//!    overlap (the two `HelpButtons` 2 px apart, "Solid view" and "starting a design") do not
//!    qualify: neither is a `TouchArea`, so they stay reported.
//!
//! 4. a control inside a floating layer against a control outside it or in another layer
//!    ([`SweepOptions::floating_layers`]). For example, in Indicatrix: the `Toast` ("Dismiss this
//!    message" sits above the status strip on purpose) and every `PopupWindow` (`FilterPanel`,
//!    `ImportDialog`, `RemoteWorkerDialog`, `VerdictPopover`) are drawn above the content and
//!    cover it by design, as do the guide panel (`GuideOverlayLayer`, draggable, collapses to a
//!    pill) and the floating `ParametersPanel` over the viewport toolbar. Overlaps WITHIN one
//!    layer are still reported. A full-window modal `Rectangle` WITHOUT a backdrop `TouchArea`
//!    (`PreviewBatchDialog`: its only `TouchArea {}` is card-sized, so clicks beside the card
//!    reach the editor) is NOT a layer: that is a real defect.
//!
//! Invisible (opacity at or below [`SweepOptions::invisible_opacity`]), disabled and
//! scrolled-out controls take no part.

use super::{SweepOptions, finding, rect_text};
use crate::{
    report::Finding,
    scene::{Placed, Scene},
    view::WindowView,
};

/// `overlap`: partial overlap between two enabled controls that a user can see and click
/// (containment, modal layers and header backdrops are not overlaps; see the module doc).
pub(super) fn check_overlaps(
    view: &WindowView,
    scene: &Scene,
    options: &SweepOptions,
    findings: &mut Vec<Finding>,
) {
    let live: Vec<&Placed> = scene
        .placed
        .iter()
        .filter(|p| {
            p.interactive
                && !p.blocked
                && p.control.enabled
                && p.control.opacity > options.invisible_opacity
                && !p.visible.is_empty()
        })
        .collect();
    let tol = options.overlap_tolerance;
    for (i, a) in live.iter().enumerate() {
        for b in &live[i + 1..] {
            let (ra, rb) = (&a.visible, &b.visible);
            let overlap_w = ra.right().min(rb.right()) - ra.x.max(rb.x);
            let overlap_h = ra.bottom().min(rb.bottom()) - ra.y.max(rb.y);
            if overlap_w <= tol || overlap_h <= tol {
                continue;
            }
            if a.layer != b.layer
                || ra.contains(rb, tol)
                || rb.contains(ra, tol)
                || on_backdrop(a, b, options)
            {
                continue;
            }
            findings.push(finding(
                view,
                &a.control,
                "overlap",
                format!(
                    "overlaps {} by {overlap_w:.0}x{overlap_h:.0}px ({} vs {})",
                    b.control.describe(),
                    rect_text(ra.x, ra.y, ra.w, ra.h),
                    rect_text(rb.x, rb.y, rb.w, rb.h)
                ),
            ));
        }
    }
}

/// Whether `later` is drawn on purpose on top of the header-size `TouchArea` backdrop `first`
/// (module doc, case 3). `first` precedes `later` in drawing order.
fn on_backdrop(first: &Placed, later: &Placed, options: &SweepOptions) -> bool {
    if first.index >= later.index
        || first.control.type_name != "TouchArea"
        || first.visible.h > options.header_max_h
    {
        return false;
    }
    let (cx, cy) = later.visible.centre();
    let common = first.visible.intersect(&later.visible);
    first.visible.contains_point(cx, cy)
        && common.area() >= options.header_inside * later.visible.area()
}
