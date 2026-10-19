//! Screencast pacing helpers: how long to read a step, how long a cursor glide takes, caption
//! cues from a title and a body, the wheel ticks of a scroll, and a real-time wait.
//!
//! Ported from the tutorial videos of the Indicatrix cutter (`tests/tutorial_videos/pacing.rs`),
//! with the same defaults: a step is read at 3 words per second, between 2.5 and 8 seconds; a
//! glide takes 0.45 s plus 1 s per 2400 logical pixels, between 0.5 s and 0.9 s; caption lines
//! are at most 80 characters, two lines to a cue; a wheel tick is at most 120 pixels and a scroll
//! at most 12 ticks.

use crate::{backend, timing};
use std::time::{Duration, Instant};

/// The widest caption line, in characters.
pub const CAPTION_WIDTH: usize = 80;

/// The lines of one caption cue.
pub const CAPTION_LINES: usize = 2;

/// The shortest time a step is read.
const READ_MIN_SECS: f32 = 2.5;

/// The longest time a step is read.
const READ_MAX_SECS: f32 = 8.0;

/// Reading speed: words per second.
const WORDS_PER_SECOND: f32 = 3.0;

/// The wheel ticks that add up to a scroll delta (the one implementation, shared with
/// [`crate::view::geometry`]): none for a zero or non-finite delta, otherwise
/// `ceil(|delta_y| / 120)` equal ticks, at most 12 of them.
pub use crate::view::geometry::wheel_steps;

/// How long a step with `words` words of text is held for reading: `words / 3` seconds, clamped
/// to 2.5 to 8 seconds.
#[must_use]
pub fn reading_time(words: usize) -> Duration {
    // Beyond 65 535 words the answer is the 8 second cap anyway.
    let words = f32::from(u16::try_from(words).unwrap_or(u16::MAX));
    Duration::from_secs_f32((words / WORDS_PER_SECOND).clamp(READ_MIN_SECS, READ_MAX_SECS))
}

/// How long a cursor glide over `distance_logical` pixels takes: 0.45 s plus 1 s per 2400 px,
/// kept between 0.5 s and 0.9 s (a non-finite distance gives 0.9 s).
#[must_use]
pub fn glide_duration(distance_logical: f32) -> Duration {
    let distance = if distance_logical.is_finite() {
        distance_logical.abs()
    } else {
        2400.0
    };
    Duration::from_secs_f32((0.45 + distance / 2400.0).clamp(0.5, 0.9))
}

/// The text without the light markup a guide may use (`**bold**`, `` `code` ``), on one line
/// with single spaces.
#[must_use]
pub fn plain(text: &str) -> String {
    let stripped: String = text.chars().filter(|c| !matches!(c, '*' | '`')).collect();
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` broken into lines of at most `width` characters (a longer word has a line of its own).
#[must_use]
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let joined = line.chars().count() + 1 + word.chars().count();
        if !line.is_empty() && joined > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// The caption cues of a step: the title (as one line, markup stripped) and the body's pieces
/// (markup stripped, wrapped to `width` characters), `lines` lines to a cue.
///
/// The title opens the first cue. The tutorial videos use [`CAPTION_WIDTH`] and [`CAPTION_LINES`].
#[must_use]
pub fn cues(title: &str, body: &[String], width: usize, lines: usize) -> Vec<String> {
    let mut all = vec![plain(title)];
    for piece in body {
        all.extend(wrap(&plain(piece), width));
    }
    all.chunks(lines.max(1))
        .map(|group| group.join("\n"))
        .collect()
}

/// The number of words in `title` and `body`.
#[must_use]
pub fn word_count(title: &str, body: &[String]) -> usize {
    title.split_whitespace().count()
        + body
            .iter()
            .map(|piece| piece.split_whitespace().count())
            .sum::<usize>()
}

/// Pumps the mock time that passed in real time since `*last` (clamped to 5 to 100 ms, so a
/// slow poll does not jump and a fast one still moves) and moves `*last` to now.
pub(super) fn pump_real(last: &mut Instant) {
    let now = Instant::now();
    let ms = u64::try_from(now.duration_since(*last).as_millis()).unwrap_or(10);
    *last = now;
    backend::pump(ms.clamp(5, 100));
}

/// Pumps until `condition` holds or `timeout` of real time has passed, with mock time advancing
/// by the real time that passed (not a fixed 10 ms per round).
///
/// So an app's timers and a recipe's tick budget run at the speed they do in real life while
/// worker threads solve. Shows nothing: in a video this is a cut. Returns whether `condition` held. Recorded under the
/// timing label `wait`.
pub fn wait_real(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    let mut last = start;
    let held = loop {
        pump_real(&mut last);
        if condition() {
            break true;
        }
        if start.elapsed() >= timeout {
            break false;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    timing::record_str("wait", start.elapsed(), !held);
    held
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn reading_time_is_three_words_a_second_between_two_and_a_half_and_eight_seconds() {
        assert_eq!(reading_time(0), Duration::from_secs_f32(2.5));
        assert_eq!(reading_time(6), Duration::from_secs_f32(2.5));
        assert_eq!(reading_time(12), Duration::from_secs(4));
        assert_eq!(reading_time(24), Duration::from_secs(8));
        assert_eq!(reading_time(1000), Duration::from_secs(8));
        assert_eq!(reading_time(usize::MAX), Duration::from_secs(8));
    }

    #[test]
    fn a_glide_takes_half_a_second_to_nine_tenths() {
        assert_eq!(glide_duration(0.0), Duration::from_secs_f32(0.5));
        assert!((glide_duration(240.0).as_secs_f32() - 0.55).abs() < 1e-5);
        assert!((glide_duration(-240.0).as_secs_f32() - 0.55).abs() < 1e-5);
        assert_eq!(glide_duration(2400.0), Duration::from_secs_f32(0.9));
        assert_eq!(glide_duration(f32::NAN), Duration::from_secs_f32(0.9));
        assert_eq!(glide_duration(f32::INFINITY), Duration::from_secs_f32(0.9));
    }

    #[test]
    fn plain_strips_the_markup_and_the_line_breaks() {
        assert_eq!(plain("**Bold** and `code`\n  text"), "Bold and code text");
        assert_eq!(plain(""), "");
    }

    #[test]
    fn wrap_breaks_at_the_width_and_keeps_a_long_word_whole() {
        assert_eq!(
            wrap("one two three four", 9),
            vec!["one two", "three", "four"]
        );
        assert_eq!(wrap("abcdefghijkl x", 5), vec!["abcdefghijkl", "x"]);
        assert_eq!(wrap("   ", 5).len(), 0);
    }

    #[test]
    fn cues_open_with_the_title_two_lines_to_a_cue() {
        let body = lines(&["**First** piece of text", "second"]);
        let cues = cues("A `title`", &body, 12, 2);
        assert_eq!(
            cues,
            vec![
                "A title\nFirst piece".to_owned(),
                "of text\nsecond".to_owned()
            ]
        );
        assert_eq!(
            super::cues("T", &lines(&["x"]), CAPTION_WIDTH, CAPTION_LINES),
            vec!["T\nx".to_owned()]
        );
        assert_eq!(super::cues("T", &[], 80, 2), vec!["T".to_owned()]);
        assert_eq!(super::cues("T", &lines(&["a", "b"]), 80, 0).len(), 3);
    }

    #[test]
    fn word_count_adds_the_title_and_every_piece() {
        assert_eq!(word_count("Two words", &lines(&["a b c", "d"])), 6);
        assert_eq!(word_count("", &[]), 0);
    }

    /// The ticks themselves are tested once, with `view::geometry`; this pins that the path
    /// the CHANGELOG names is that one function.
    #[test]
    fn the_wheel_steps_here_are_the_geometry_ones() {
        let here: fn(f32) -> Vec<f32> = wheel_steps;
        let there: fn(f32) -> Vec<f32> = crate::view::geometry::wheel_steps;
        for delta in [100.0, -240.0, -300.0, 3000.0, 0.0, f32::NAN] {
            assert_eq!(here(delta), there(delta));
        }
        assert_eq!(here(-240.0), vec![-120.0, -120.0]);
    }
}
