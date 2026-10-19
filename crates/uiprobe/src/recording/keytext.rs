//! Text for the keystroke badge, drawn by Slint's own software renderer.
//!
//! The crate has no font rasteriser, so a label is rendered by a tiny offscreen Slint component
//! (a window with one `Text`) into a [`Shot`], once per label (the recorder caches the result).
//! The text is white on an opaque black window, so the red channel of the picture is the
//! anti-aliased coverage of the glyphs: a mask the badge then paints in any colour, independent of
//! how the renderer treats alpha. The window is created, shown, captured and hidden within one
//! call, without pumping, so it never meets the harness's own windows.
//!
//! The glyphs come from the system fonts the software renderer uses; a symbol the first font
//! lacks (the arrows, the Mac key symbols) is taken from a fallback font if the machine has one.

use crate::screenshot;
use slint::ComponentHandle;

mod surface {
    slint::slint! {
        export component KeyText inherits Window {
            in property <string> label;
            in property <length> font-px;
            out property <length> text-width: glyphs.preferred-width;
            out property <length> text-height: glyphs.preferred-height;
            background: #000000;
            glyphs := Text {
                x: 0px;
                y: 0px;
                width: root.width;
                height: root.height;
                text: root.label;
                font-size: root.font-px;
                font-weight: 600;
                color: #ffffff;
                horizontal-alignment: center;
                vertical-alignment: center;
            }
        }
    }
}

/// The logical size of the window the text is measured in (large enough for any label).
const PROBE: (u32, u32) = (4096, 512);

/// Extra logical pixels around the measured text, so no glyph edge is clipped.
const PADDING: u32 = 2;

/// The coverage of one rendered text, in physical pixels.
#[derive(Debug, Clone)]
pub(super) struct Mask {
    pub(super) width: u32,
    pub(super) height: u32,
    /// `width * height` coverages, row by row (0 = no ink, 255 = full ink).
    pub(super) cover: Vec<u8>,
}

/// Renders `label` at `font_logical` logical pixels drawn at `scale` physical pixels per logical
/// pixel.
///
/// # Errors
///
/// The offscreen window could not be created, shown or captured (the run needs
/// `Renderer::Software`).
pub(super) fn render(label: &str, font_logical: f32, scale: f32) -> Result<Mask, String> {
    let component = surface::KeyText::new()
        .map_err(|e| format!("cannot create the keystroke badge text window: {e}"))?;
    component.set_label(label.into());
    component.set_font_px(font_logical);
    component
        .show()
        .map_err(|e| format!("cannot show the keystroke badge text window: {e}"))?;
    let window = component.window();
    screenshot::set_window_size(window, PROBE, scale);
    let text = (component.get_text_width(), component.get_text_height());
    let size = (
        super::num::ceil_u32(text.0).max(1) + 2 * PADDING,
        super::num::ceil_u32(text.1).max(1) + 2 * PADDING,
    );
    screenshot::set_window_size(window, size, scale);
    let shot = screenshot::capture(window);
    let _ = component.hide();
    let shot = shot.map_err(|e| format!("cannot draw the keystroke badge text: {e}"))?;
    Ok(Mask {
        width: shot.width(),
        height: shot.height(),
        cover: shot.pixels().iter().map(|p| p.r).collect(),
    })
}
