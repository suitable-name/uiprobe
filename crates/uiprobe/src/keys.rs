//! Key chords for [`crate::view::press`]: parsing "Ctrl+Shift+P" into modifiers plus the key
//! text Slint's `key-pressed` handlers see.
//!
//! The parser is a pure function. Its unit tests run under `cargo test --lib`; the harness
//! integration targets use `harness = false` and so have no `#[test]`s of their own.

use slint::{SharedString, platform::Key};

/// A parsed chord.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the four modifier keys are independent flags and public fields of the API"
)]
pub struct Chord {
    /// Ctrl held.
    pub ctrl: bool,
    /// Shift held.
    pub shift: bool,
    /// Alt held.
    pub alt: bool,
    /// Meta (Cmd / Windows key) held.
    pub meta: bool,
    /// The `event.text` of the key press.
    pub text: SharedString,
}

impl Chord {
    /// The modifier keys held, in press order.
    #[must_use]
    pub fn modifier_keys(&self) -> Vec<Key> {
        let mut keys = Vec::new();
        if self.ctrl {
            keys.push(Key::Control);
        }
        if self.shift {
            keys.push(Key::Shift);
        }
        if self.alt {
            keys.push(Key::Alt);
        }
        if self.meta {
            keys.push(Key::Meta);
        }
        keys
    }
}

/// The special key called `name` (case-insensitive), if there is one.
fn named_key(name: &str) -> Option<Key> {
    let lower = name.to_lowercase();
    let key = match lower.as_str() {
        "escape" | "esc" => Key::Escape,
        "return" | "enter" => Key::Return,
        "tab" => Key::Tab,
        "backtab" => Key::Backtab,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "insert" => Key::Insert,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "up" | "uparrow" => Key::UpArrow,
        "down" | "downarrow" => Key::DownArrow,
        "left" | "leftarrow" => Key::LeftArrow,
        "right" | "rightarrow" => Key::RightArrow,
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,
        _ => return None,
    };
    Some(key)
}

/// Parses `chord` ("Ctrl+S", "F5", "Escape", "?", "Ctrl+Shift+P", "Ctrl+Alt+2", "Ctrl++").
///
/// A single letter is lower case unless Shift is part of the chord (Slint reports `s` for Ctrl+S
/// and `S` for Ctrl+Shift+S); a space is the key `" "`.
///
/// # Errors
///
/// A message naming the unknown modifier or key.
pub fn parse_chord(chord: &str) -> Result<Chord, String> {
    let trimmed = chord.trim();
    // "Ctrl++" ends in the plus key itself.
    let (modifier_part, key_part) = if trimmed == "+" {
        ("", "+")
    } else if let Some(rest) = trimmed.strip_suffix("++") {
        (rest, "+")
    } else if let Some((rest, key)) = trimmed.rsplit_once('+') {
        (rest, key)
    } else {
        ("", trimmed)
    };
    if key_part.is_empty() {
        return Err(format!("chord {chord:?} has no key"));
    }
    let mut parsed = Chord {
        ctrl: false,
        shift: false,
        alt: false,
        meta: false,
        text: SharedString::new(),
    };
    for modifier in modifier_part.split('+').filter(|m| !m.is_empty()) {
        match modifier.trim().to_lowercase().as_str() {
            "ctrl" | "control" => parsed.ctrl = true,
            "shift" => parsed.shift = true,
            "alt" => parsed.alt = true,
            "meta" | "cmd" | "super" | "win" => parsed.meta = true,
            other => return Err(format!("chord {chord:?}: unknown modifier {other:?}")),
        }
    }
    parsed.text = if let Some(key) = named_key(key_part) {
        key.into()
    } else if key_part.eq_ignore_ascii_case("space") {
        SharedString::from(" ")
    } else {
        let mut chars = key_part.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if c.is_alphabetic() => {
                let cased: String = if parsed.shift {
                    c.to_uppercase().collect()
                } else {
                    c.to_lowercase().collect()
                };
                SharedString::from(cased.as_str())
            }
            (Some(_), None) => SharedString::from(key_part),
            _ => return Err(format!("chord {chord:?}: unknown key {key_part:?}")),
        }
    };
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_s() {
        let ctrl_s = parse_chord("Ctrl+S").expect("Ctrl+S");
        assert!(ctrl_s.ctrl && !ctrl_s.shift && ctrl_s.text == "s", "Ctrl+S");
    }

    #[test]
    fn ctrl_shift_p() {
        let shifted = parse_chord("Ctrl+Shift+P").expect("Ctrl+Shift+P");
        assert!(
            shifted.ctrl && shifted.shift && shifted.text == "P",
            "Ctrl+Shift+P"
        );
    }

    #[test]
    fn ctrl_alt_digit() {
        let digits = parse_chord("Ctrl+Alt+2").expect("Ctrl+Alt+2");
        assert!(
            digits.ctrl && digits.alt && digits.text == "2",
            "Ctrl+Alt+2"
        );
    }

    #[test]
    fn named_keys() {
        let f5 = parse_chord("F5").expect("F5");
        assert!(!f5.ctrl && f5.text == SharedString::from(Key::F5), "F5");
        let escape = parse_chord("Escape").expect("Escape");
        assert!(escape.text == SharedString::from(Key::Escape), "Escape");
    }

    #[test]
    fn punctuation() {
        let question = parse_chord("?").expect("?");
        assert!(!question.shift && question.text == "?", "?");
        let plus = parse_chord("Ctrl++").expect("Ctrl++");
        assert!(plus.ctrl && plus.text == "+", "Ctrl++");
    }

    #[test]
    fn errors() {
        assert!(parse_chord("Hyper+X").is_err(), "unknown modifier");
        assert!(parse_chord("Ctrl+").is_err(), "missing key");
        assert!(parse_chord("Ctrl+Banana").is_err(), "unknown key");
    }
}
