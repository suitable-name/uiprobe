//! Caption cues and chapters, and their text formats (SRT, `YouTube` chapter list).

use std::{fmt::Write as _, time::Duration};

/// The shortest a cue stays on screen unless the next cue starts earlier.
const MIN_CUE_MS: u64 = 500;

/// `00:01:02,345`: the SRT form of a video time.
#[must_use]
pub fn srt_timestamp(t: Duration) -> String {
    let ms = t.as_millis();
    let (hours, minutes) = (ms / 3_600_000, ms / 60_000 % 60);
    let (seconds, millis) = (ms / 1000 % 60, ms % 1000);
    format!("{hours:02}:{minutes:02}:{seconds:02},{millis:03}")
}

/// `0:00`, `1:02`, `1:02:03`: the `YouTube` chapter form of a video time.
#[must_use]
pub fn chapter_timestamp(t: Duration) -> String {
    let secs = t.as_secs();
    let (hours, minutes, seconds) = (secs / 3600, secs / 60 % 60, secs % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// One caption: shown from `start` to `end` (milliseconds of video time); `end` is `None`
/// while the cue is still open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cue {
    pub(crate) start: u64,
    pub(crate) end: Option<u64>,
    pub(crate) text: String,
}

/// The captions and chapters of one recording.
#[derive(Debug, Default)]
pub struct Track {
    cues: Vec<Cue>,
    chapters: Vec<(u64, String)>,
}

impl Track {
    /// Ends the open cue at `now` and, for a non-empty `text`, starts a new one.
    pub(crate) fn caption(&mut self, now: u64, text: &str) {
        if let Some(open) = self.cues.last_mut().filter(|c| c.end.is_none()) {
            open.end = Some(now);
        }
        let text = clean_lines(text);
        if !text.is_empty() {
            self.cues.push(Cue {
                start: now,
                end: None,
                text,
            });
        }
    }

    /// Adds a chapter at `now`.
    pub(crate) fn chapter(&mut self, now: u64, title: &str) {
        let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
        self.chapters.push((now, title));
    }

    /// The SRT file: numbered cues, `start --> end`, text, blank line. A cue still open ends at
    /// `total` (the final video time).
    pub(crate) fn render_srt(&self, total: u64) -> String {
        let mut out = String::new();
        for (number, (start, end, text)) in cue_ranges(&self.cues, total).into_iter().enumerate() {
            let _ = write!(
                out,
                "{}\n{} --> {}\n{}\n\n",
                number + 1,
                srt_timestamp(Duration::from_millis(start)),
                srt_timestamp(Duration::from_millis(end)),
                text
            );
        }
        out
    }

    /// The chapter list, one `m:ss Title` line per chapter. `YouTube` needs the first chapter at
    /// `0:00`: when the first one is later, `0:00 Intro` is put in front.
    pub(crate) fn render_chapters(&self) -> String {
        let mut out = String::new();
        if self.chapters.first().is_none_or(|(at, _)| *at > 0) {
            out.push_str("0:00 Intro\n");
        }
        for (at, title) in &self.chapters {
            let _ = writeln!(
                out,
                "{} {}",
                chapter_timestamp(Duration::from_millis(*at)),
                title
            );
        }
        out
    }
}

/// The text with every line trimmed and the blank lines dropped (a blank line would end the
/// cue in an SRT file).
fn clean_lines(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The `(start, end, text)` of every cue that is shown for a positive time: an open cue ends at
/// `total`; a cue shorter than half a second is extended to half a second, but not past the
/// start of the next cue.
pub fn cue_ranges(cues: &[Cue], total: u64) -> Vec<(u64, u64, &str)> {
    let mut ranges = Vec::with_capacity(cues.len());
    for (index, cue) in cues.iter().enumerate() {
        let mut end = cue.end.unwrap_or(total);
        let wanted = cue.start.saturating_add(MIN_CUE_MS);
        if end < wanted {
            let limit = cues.get(index + 1).map_or(u64::MAX, |next| next.start);
            end = end.max(wanted.min(limit));
        }
        if end > cue.start {
            ranges.push((cue.start, end, cue.text.as_str()));
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srt_timestamps() {
        assert_eq!(srt_timestamp(Duration::ZERO), "00:00:00,000");
        assert_eq!(srt_timestamp(Duration::from_millis(62_345)), "00:01:02,345");
        assert_eq!(
            srt_timestamp(Duration::from_millis(3_723_004)),
            "01:02:03,004"
        );
    }

    #[test]
    fn chapter_timestamps() {
        assert_eq!(chapter_timestamp(Duration::ZERO), "0:00");
        assert_eq!(chapter_timestamp(Duration::from_millis(62_999)), "1:02");
        assert_eq!(chapter_timestamp(Duration::from_secs(3723)), "1:02:03");
        assert_eq!(chapter_timestamp(Duration::from_secs(9)), "0:09");
    }

    #[test]
    fn short_cues_are_extended_unless_the_next_one_starts_first() {
        let mut track = Track::default();
        track.caption(1000, "one");
        track.caption(1200, "two");
        track.caption(5000, "three");
        track.caption(5100, "");
        track.caption(9000, "four");
        let ranges = cue_ranges(&track.cues, 9100);
        assert_eq!(
            ranges,
            vec![
                (1000, 1200, "one"),
                (1200, 5000, "two"),
                (5000, 5500, "three"),
                (9000, 9500, "four"),
            ]
        );
    }

    #[test]
    fn an_open_cue_ends_at_the_total_and_empty_cues_vanish() {
        let mut track = Track::default();
        track.caption(0, "  first line \n\n second line ");
        track.caption(2000, "x");
        track.caption(2000, "y");
        let srt = track.render_srt(4000);
        assert_eq!(
            srt,
            "1\n00:00:00,000 --> 00:00:02,000\nfirst line\nsecond line\n\n\
             2\n00:00:02,000 --> 00:00:04,000\ny\n\n"
        );
    }

    #[test]
    fn chapters_start_at_zero() {
        let mut track = Track::default();
        assert_eq!(track.render_chapters(), "0:00 Intro\n");
        track.chapter(5000, "Open the\nbrowser");
        track.chapter(65_000, "Done");
        assert_eq!(
            track.render_chapters(),
            "0:00 Intro\n0:05 Open the browser\n1:05 Done\n"
        );
        let mut starts_at_zero = Track::default();
        starts_at_zero.chapter(0, "Intro");
        assert_eq!(starts_at_zero.render_chapters(), "0:00 Intro\n");
    }
}
