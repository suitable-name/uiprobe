//! The filmed input layer: one call is one visible action and one real input to the app.
//!
//! `click_at`, `type_text`, `scroll_at` and `press_keys` glide the cursor, show the click ripple
//! or the keystroke badge, and dispatch real Slint pointer and key events to a window. That
//! dispatch IS the activation: a pointer press and release makes the control's own `clicked`
//! callback run once, so the caller must not invoke the callback again (a video would then fire
//! the action twice). Events are handled when they are dispatched; the next frame the recorder
//! emits shows their effect, so nothing is pumped here except the event queue.

use super::{Recorder, pacing};
use crate::keys;
use slint::{
    LogicalPosition, SharedString,
    platform::{PointerEventButton, WindowEvent},
};
use std::time::Duration;

/// The rest between the cursor arriving on a control and the click ripple.
const BEFORE_CLICK: Duration = Duration::from_millis(150);

/// The hold after each wheel tick.
const WHEEL_HOLD: Duration = Duration::from_millis(40);

/// The chord as a badge label: modifiers in the order Ctrl, Shift, Alt, Meta, then the key, joined
/// by ` + ` (`"Ctrl+Up"` becomes `"Ctrl + ↑"`).
///
/// # Errors
///
/// The chord does not parse (the message of [`keys::parse_chord`]).
pub(super) fn pretty_chord(chord: &str) -> Result<String, String> {
    let parsed = keys::parse_chord(chord)?;
    let mut parts: Vec<String> = Vec::new();
    for (held, name) in [
        (parsed.ctrl, "Ctrl"),
        (parsed.shift, "Shift"),
        (parsed.alt, "Alt"),
        (parsed.meta, "Meta"),
    ] {
        if held {
            parts.push(name.to_owned());
        }
    }
    parts.push(pretty_key(key_token(chord)));
    Ok(parts.join(" + "))
}

/// The key part of a chord (the text after the last `+`; a trailing `++` or a lone `+` is the
/// plus key).
fn key_token(chord: &str) -> &str {
    let trimmed = chord.trim();
    if trimmed == "+" || trimmed.ends_with("++") {
        "+"
    } else if let Some((_, key)) = trimmed.rsplit_once('+') {
        key.trim()
    } else {
        trimmed
    }
}

/// How a key is written on its keycap.
fn pretty_key(token: &str) -> String {
    let name = match token.to_lowercase().as_str() {
        "up" | "uparrow" => "↑",
        "down" | "downarrow" => "↓",
        "left" | "leftarrow" => "←",
        "right" | "rightarrow" => "→",
        "escape" | "esc" => "Esc",
        "return" | "enter" => "Enter",
        "backspace" => "Backspace",
        "delete" | "del" => "Del",
        "insert" => "Ins",
        "pageup" => "Page Up",
        "pagedown" => "Page Down",
        "space" => "Space",
        _ => "",
    };
    if !name.is_empty() {
        return name.to_owned();
    }
    let mut chars = token.chars();
    match chars.next() {
        // A single character, a function key or another name: upper case first letter.
        Some(first) if token.chars().count() == 1 => first.to_uppercase().collect(),
        Some(first) if first.eq_ignore_ascii_case(&'f') => token.to_uppercase(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

impl Recorder<'_> {
    /// Clicks the point `at_logical` of `window`: glides the cursor there (the duration follows
    /// from the distance, [`pacing::glide_duration`]), rests 150 ms, dispatches exactly ONE real
    /// `PointerMoved`, `PointerPressed`, `PointerReleased` sequence to `window` and draws the
    /// click ripple (its first frame already shows the app's reaction to the click). The press
    /// and release is the activation; do not invoke the callback again.
    ///
    /// `at_logical` is local to `window`. `origin` is the window's position in the frame (logical
    /// pixels): `(0.0, 0.0)` for the main window, the position given to
    /// [`Recorder::attach_window`] for an attached window. The cursor is drawn at
    /// `origin + at_logical`.
    ///
    /// # Errors
    ///
    /// As [`Recorder::hold`].
    pub fn click_at(
        &mut self,
        window: &slint::Window,
        at_logical: (f32, f32),
        origin: (f32, f32),
    ) -> Result<(), String> {
        self.glide_to((origin.0 + at_logical.0, origin.1 + at_logical.1))?;
        self.hold(BEFORE_CLICK)?;
        // The press and release go to the app first: the ripple's first frame (rendered by
        // `click`) then already shows the reaction, as in a real screen recording.
        let position = LogicalPosition::new(at_logical.0, at_logical.1);
        window.dispatch_event(WindowEvent::PointerMoved { position });
        window.dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        window.dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
        crate::backend::drain_events();
        self.reuse.invalidate();
        self.click()
    }

    /// Types `text` into the focused control of `window`: per character one real `KeyPressed`
    /// and one `KeyReleased`, then a hold of `per_key` of video. Focus the control first
    /// ([`Recorder::click_at`] on it); nothing is selected or cleared here.
    ///
    /// # Errors
    ///
    /// As [`Recorder::hold`].
    pub fn type_text(
        &mut self,
        window: &slint::Window,
        text: &str,
        per_key: Duration,
    ) -> Result<(), String> {
        for character in text.chars() {
            let key: SharedString = character.to_string().into();
            window.dispatch_event(WindowEvent::KeyPressed { text: key.clone() });
            window.dispatch_event(WindowEvent::KeyReleased { text: key });
            crate::backend::drain_events();
            self.reuse.invalidate();
            self.hold(per_key)?;
        }
        Ok(())
    }

    /// Turns the mouse wheel over the point `at` of `window` (the main window: `at` is both the
    /// cursor position in the frame and the position in the window): the cursor glides there,
    /// then `delta_y` is dispatched as real `PointerScrolled` ticks of at most 120 px (at most
    /// 12 ticks, [`pacing::wheel_steps`]) with a 40 ms hold after each, so the content visibly
    /// scrolls. `delta_y` is in Slint's wheel convention (positive scrolls the content down
    /// towards its top).
    ///
    /// # Errors
    ///
    /// As [`Recorder::hold`].
    pub fn scroll_at(
        &mut self,
        window: &slint::Window,
        at: (f32, f32),
        delta_y: f32,
    ) -> Result<(), String> {
        self.scroll_at_in(window, at, (0.0, 0.0), delta_y)
    }

    /// [`Recorder::scroll_at`] for a window shown at `origin` in the frame (an attached window):
    /// `at` is local to `window`, the cursor is drawn at `origin + at`.
    ///
    /// # Errors
    ///
    /// As [`Recorder::hold`].
    pub fn scroll_at_in(
        &mut self,
        window: &slint::Window,
        at: (f32, f32),
        origin: (f32, f32),
        delta_y: f32,
    ) -> Result<(), String> {
        self.glide_to((origin.0 + at.0, origin.1 + at.1))?;
        let position = LogicalPosition::new(at.0, at.1);
        window.dispatch_event(WindowEvent::PointerMoved { position });
        for step in pacing::wheel_steps(delta_y) {
            window.dispatch_event(WindowEvent::PointerScrolled {
                position,
                delta_x: 0.0,
                delta_y: step,
            });
            crate::backend::drain_events();
            self.reuse.invalidate();
            self.hold(WHEEL_HOLD)?;
        }
        Ok(())
    }

    /// Presses the key chord `chord` (`"Ctrl+S"`, `"F5"`, `"Ctrl+Shift+P"`) on `window` exactly as
    /// [`crate::view::press`] does (modifiers first, released last), but without its 50 ms pump:
    /// video time is the only clock, so the chord takes effect in the next frame the recorder
    /// emits. Then shows the keystroke badge with a readable label (`"Ctrl + S"`,
    /// `"Ctrl + ↑"`). The press is the activation; emits no frames itself.
    ///
    /// # Errors
    ///
    /// The chord does not parse; nothing is sent and no badge is shown then.
    pub fn press_keys(&mut self, window: &slint::Window, chord: &str) -> Result<(), String> {
        let label = pretty_chord(chord)?;
        crate::view::press_events(window, chord).map_err(|finding| finding.detail)?;
        self.reuse.invalidate();
        self.show_keys(&label);
        Ok(())
    }

    /// Glides the cursor to `to` (logical frame pixels) for the time [`pacing::glide_duration`]
    /// gives for the distance.
    fn glide_to(&mut self, to: (f32, f32)) -> Result<(), String> {
        let distance = (to.0 - self.cursor.0).hypot(to.1 - self.cursor.1);
        self.move_to(to, pacing::glide_duration(distance))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_get_readable_labels() {
        assert_eq!(pretty_chord("Ctrl+Up").as_deref(), Ok("Ctrl + ↑"));
        assert_eq!(
            pretty_chord("ctrl+shift+p").as_deref(),
            Ok("Ctrl + Shift + P")
        );
        assert_eq!(pretty_chord("F5").as_deref(), Ok("F5"));
        assert_eq!(pretty_chord("Escape").as_deref(), Ok("Esc"));
        assert_eq!(pretty_chord("Ctrl++").as_deref(), Ok("Ctrl + +"));
        assert_eq!(pretty_chord("Ctrl+Alt+2").as_deref(), Ok("Ctrl + Alt + 2"));
        assert_eq!(pretty_chord("Shift+Tab").as_deref(), Ok("Shift + Tab"));
        assert_eq!(
            pretty_chord("Ctrl+PageDown").as_deref(),
            Ok("Ctrl + Page Down")
        );
        assert_eq!(pretty_chord("Alt+Left").as_deref(), Ok("Alt + ←"));
        assert_eq!(pretty_chord("Space").as_deref(), Ok("Space"));
        assert!(pretty_chord("Hyper+X").is_err());
        assert!(pretty_chord("Ctrl+").is_err());
    }
}
