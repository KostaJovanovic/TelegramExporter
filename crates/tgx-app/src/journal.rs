//! The export log.
//!
//! It was an unbounded `Vec<SharedString>` of which only `.rev().take(6)` was
//! ever painted, in a panel with no scrolling. Two things follow from that, and
//! both were real: the memory grew for the life of the run with nothing reading
//! it, and **the one line the code goes out of its way to distinguish — the
//! INCOMPLETE-export warning — became unreachable after six more chats
//! finished.** A warning you cannot scroll back to is a warning that was not
//! given.
//!
//! So: a bounded ring, oldest first, and a line can say it matters.

use std::collections::VecDeque;

/// How many lines are kept. The Python original's `setMaximumBlockCount(2000)`,
/// carried across for the same reason: a long queue with a chatty chat can
/// produce thousands of lines, and an unbounded transcript nobody reads is
/// still memory somebody paid for.
pub const CAPACITY: usize = 2000;

/// One line of the transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    /// Painted in the accent. **Set by whoever wrote the line, never sniffed
    /// out of its text** — a substring match for "failed" catches "0 failed",
    /// which is the opposite of a warning.
    pub warning: bool,
}

#[derive(Debug, Default)]
pub struct Journal {
    lines: VecDeque<Line>,
    /// Lines dropped to the cap. Kept so the panel can say what it no longer
    /// holds instead of silently presenting a truncated run as the whole run.
    dropped: usize,
}

impl Journal {
    pub fn push(&mut self, text: impl Into<String>) {
        self.write(text.into(), false);
    }

    pub fn warn(&mut self, text: impl Into<String>) {
        self.write(text.into(), true);
    }

    fn write(&mut self, text: String, warning: bool) {
        if self.lines.len() == CAPACITY {
            self.lines.pop_front();
            self.dropped += 1;
        }
        self.lines.push_back(Line { text, warning });
    }

    /// Oldest first, which is the order a transcript is read in.
    pub fn lines(&self) -> impl ExactSizeIterator<Item = &Line> {
        self.lines.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn dropped(&self) -> usize {
        self.dropped
    }

    /// The whole transcript as one string, for the clipboard.
    ///
    /// **GPUI paints text; it does not let you select it.** A plain element has
    /// no selection behaviour at all, so the log was readable on screen and
    /// reachable nowhere else — which is a problem precisely for the log, whose
    /// whole purpose is to be handed to someone who was not watching. The
    /// sign-in error solved the same problem the same way, one line at a time.
    ///
    /// Two things the panel says in colour and position have to survive being
    /// pasted somewhere with neither: a warning is marked `!`, and the dropped
    /// count is stated at the top, or a truncated run reads as a whole one.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        if self.dropped > 0 {
            // Leading, not trailing: a reader who stops at the first line has
            // still been told, and someone pasting a tail has not lost it.
            out.push_str(&format!("[{} earlier lines dropped]\r\n", self.dropped));
        }
        for line in &self.lines {
            if line.warning {
                out.push_str("! ");
            }
            out.push_str(&line.text);
            // CRLF: the destination is Windows' clipboard, and Notepad renders
            // a bare LF as one very long line.
            out.push_str("\r\n");
        }
        out
    }

    /// How many warnings are still held.
    ///
    /// Painted in the log's own header, because the reason a warning is marked
    /// at all is that it must not be lost among the ordinary lines — and a
    /// thousand-line transcript with one red line in it is exactly where it
    /// gets lost.
    pub fn warnings(&self) -> usize {
        self.lines.iter().filter(|l| l.warning).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_warning_is_marked_by_its_writer_not_sniffed_from_its_text() {
        // "0 failed" is the opposite of a warning, and a substring match for
        // "failed" would paint it red.
        let mut j = Journal::default();
        j.push("chat: 12 files (0 failed)");
        j.warn("chat: INCOMPLETE — Telegram counted 6,643, 5,608 came through");
        let lines: Vec<&Line> = j.lines().collect();
        assert!(!lines[0].warning);
        assert!(lines[1].warning);
        assert_eq!(j.warnings(), 1);
    }

    #[test]
    fn the_transcript_reads_oldest_first() {
        let mut j = Journal::default();
        j.push("first");
        j.push("second");
        let texts: Vec<&str> = j.lines().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["first", "second"]);
    }

    #[test]
    fn the_cap_drops_the_oldest_and_says_how_many() {
        // Silently presenting a truncated run as the whole run is the failure
        // being avoided; the count is what makes it not silent.
        let mut j = Journal::default();
        for i in 0..CAPACITY + 5 {
            j.push(format!("line {i}"));
        }
        assert_eq!(j.lines().len(), CAPACITY);
        assert_eq!(j.dropped(), 5);
        assert_eq!(j.lines().next().unwrap().text, "line 5");
    }

    #[test]
    fn the_copied_text_keeps_what_the_panel_says_in_colour_and_position() {
        // Pasted into a mail, the transcript loses the accent that marks a
        // warning and the header that says lines were dropped. Both are the
        // difference between a partial run and a complete one.
        let mut j = Journal::default();
        j.push("chat: 12 files (0 failed)");
        j.warn("chat: INCOMPLETE");
        let text = j.to_text();
        assert_eq!(text, "chat: 12 files (0 failed)\r\n! chat: INCOMPLETE\r\n");

        let mut long = Journal::default();
        for i in 0..CAPACITY + 2 {
            long.push(format!("line {i}"));
        }
        assert!(
            long.to_text().starts_with("[2 earlier lines dropped]\r\n"),
            "a truncated run must not paste as a whole one"
        );
    }

    #[test]
    fn an_empty_journal_is_empty_and_holds_no_warnings() {
        let j = Journal::default();
        assert!(j.is_empty());
        assert_eq!(j.lines().len(), 0);
        assert_eq!(j.warnings(), 0);
        assert_eq!(j.dropped(), 0);
    }
}
