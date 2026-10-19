//! Comment- and string-blanking scanner and statement splitter for `.slint` sources.
//!
//! The coverage inventory ([`super::inventory`]) reads `.slint` text, not the compiler's object
//! tree (plan §1.6: the compiler crates are internal and `=` pinned). Everything it does needs
//! two services, both here and both independent of any app:
//!
//! * [`scan`] blanks comments and the contents of string literals (same length, same newlines,
//!   so line numbers survive) and returns the literals it blanked, so a `{` or `;` inside
//!   `"a { b ; c"` or `// callback commented();` cannot be mistaken for syntax;
//! * [`events`] splits the masked text into statement boundaries (`{`, `;`, `}`), each with the
//!   line where its statement starts.
//!
//! Lifted verbatim from Indicatrix `tests/ui_static/scanner.rs`.

/// A string literal found by [`scan`].
pub struct RawString {
    pub text: String,
}

/// A source with comments and string contents blanked (same length, same newlines) plus the
/// string literals that were blanked.
pub struct Scanned {
    pub masked: Vec<char>,
    pub strings: Vec<RawString>,
}

/// A blank of the same line structure: newlines stay, everything else becomes a space.
const fn blank(ch: char) -> char {
    if ch == '\n' { '\n' } else { ' ' }
}

/// Skips a `\{ ... }` interpolation inside a string; `i` is just after the `{`. The expression
/// is blanked in `masked` and not added to the string text.
fn skip_interpolation(
    chars: &[char],
    mut i: usize,
    line: &mut usize,
    masked: &mut Vec<char>,
) -> usize {
    let mut depth = 1;
    while i < chars.len() && depth > 0 {
        let ch = chars[i];
        match ch {
            '{' => depth += 1,
            '}' => depth -= 1,
            '"' => {
                masked.push(' ');
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\n' {
                        *line += 1;
                    }
                    masked.push(blank(chars[i]));
                    i += 1;
                }
            }
            _ => {}
        }
        if ch == '\n' {
            *line += 1;
        }
        masked.push(blank(ch));
        i += 1;
    }
    i
}

/// Reads a string body starting just after its opening quote; returns the index after the
/// closing quote.
fn read_string(
    chars: &[char],
    mut i: usize,
    line: &mut usize,
    masked: &mut Vec<char>,
    text: &mut String,
) -> usize {
    while i < chars.len() {
        let ch = chars[i];
        if ch == '"' {
            masked.push('"');
            return i + 1;
        }
        if ch == '\n' {
            *line += 1;
        }
        text.push(ch);
        masked.push(blank(ch));
        i += 1;
        if ch == '\\' && i < chars.len() {
            let next = chars[i];
            if next == '\n' {
                *line += 1;
            }
            if next == '{' {
                masked.push(' ');
                i = skip_interpolation(chars, i + 1, line, masked);
            } else {
                text.push(next);
                masked.push(blank(next));
                i += 1;
            }
        }
    }
    i
}

/// Blanks comments and string contents of `source`.
pub fn scan(source: &str) -> Scanned {
    let chars: Vec<char> = source.chars().collect();
    let mut masked = Vec::with_capacity(chars.len());
    let mut strings = Vec::new();
    let (mut i, mut line) = (0, 1);
    while i < chars.len() {
        let ch = chars[i];
        let next = chars.get(i + 1).copied();
        if ch == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                masked.push(' ');
                i += 1;
            }
        } else if ch == '/' && next == Some('*') {
            masked.extend([' ', ' ']);
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                if chars[i] == '\n' {
                    line += 1;
                }
                masked.push(blank(chars[i]));
                i += 1;
            }
            if i < chars.len() {
                masked.extend([' ', ' ']);
                i += 2;
            }
        } else if ch == '"' {
            masked.push('"');
            let mut text = String::new();
            i = read_string(&chars, i + 1, &mut line, &mut masked, &mut text);
            strings.push(RawString { text });
        } else {
            if ch == '\n' {
                line += 1;
            }
            masked.push(ch);
            i += 1;
        }
    }
    Scanned { masked, strings }
}

/// Kind of a [`Event`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Open,
    Close,
    Semi,
}

/// One statement boundary of the masked text: `text` is the statement before a `{` (Open), a `;`
/// or a closing `}` (Semi), or empty (Close); `line` is where the statement starts.
pub struct Event {
    pub kind: Kind,
    pub text: String,
    pub line: usize,
}

/// Splits the masked text into statement events.
pub fn events(masked: &[char]) -> Vec<Event> {
    let mut out = Vec::new();
    let (mut stmt, mut stmt_line, mut line) = (String::new(), 0, 1);
    let mut emit = |kind: Kind, stmt: &mut String, stmt_line: &mut usize, line: usize| {
        let text = std::mem::take(stmt);
        let at = if *stmt_line == 0 { line } else { *stmt_line };
        *stmt_line = 0;
        if kind != Kind::Close || !text.trim().is_empty() {
            let kind_for_text = if kind == Kind::Close {
                Kind::Semi
            } else {
                kind
            };
            out.push(Event {
                kind: kind_for_text,
                text,
                line: at,
            });
        }
        if kind == Kind::Close {
            out.push(Event {
                kind,
                text: String::new(),
                line,
            });
        }
    };
    for &ch in masked {
        match ch {
            '{' => emit(Kind::Open, &mut stmt, &mut stmt_line, line),
            ';' => emit(Kind::Semi, &mut stmt, &mut stmt_line, line),
            '}' => emit(Kind::Close, &mut stmt, &mut stmt_line, line),
            _ => {
                if ch == '\n' {
                    line += 1;
                }
                if stmt_line == 0 && !ch.is_whitespace() {
                    stmt_line = line;
                }
                stmt.push(ch);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Kind, events, scan};

    /// A source with comments and strings that must not confuse the scanner (the scanner part
    /// of Indicatrix `tests/ui_static.rs` `SAMPLE`).
    const SAMPLE: &str = r#"
// callback commented() ;
export global Demo {
    callback handled(string);
    callback never(); /* callback hidden(); */
    in-out property <string> text: "a { b ; c";
    Text { text: "Colour \{root.colour} here"; }
}
"#;

    #[test]
    fn scanner_blanks_comments_and_strings() {
        let scanned = scan(SAMPLE);
        let masked: String = scanned.masked.iter().collect();
        assert!(!masked.contains("commented"));
        assert!(!masked.contains("hidden"));
        assert!(!masked.contains("a { b"));
        assert_eq!(masked.lines().count(), SAMPLE.lines().count());
        assert!(
            scanned
                .strings
                .iter()
                .any(|s| s.text == "Colour " || s.text.starts_with("Colour "))
        );
    }

    #[test]
    fn events_split_statements_and_keep_start_lines() {
        let scanned = scan(SAMPLE);
        let evs = events(&scanned.masked);
        let opens: Vec<&str> = evs
            .iter()
            .filter(|e| e.kind == Kind::Open)
            .map(|e| e.text.trim())
            .collect();
        assert_eq!(opens, ["export global Demo", "Text"]);
        let callback = evs
            .iter()
            .find(|e| e.text.trim().starts_with("callback handled"))
            .expect("the handled callback statement");
        assert_eq!(callback.line, 4);
        // The string "a { b ; c" produced no extra boundary: the property is one statement.
        let property = evs
            .iter()
            .filter(|e| e.text.contains("in-out property"))
            .count();
        assert_eq!(property, 1);
    }
}
