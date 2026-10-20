//! Terminal titles and Codex terminal notifications.
//!
//! Halv starts interactive Codex with `tui.notification_method="osc9"`. Codex
//! then writes `ESC ] 9 ; <text> BEL` when it asks for an approval, asks a
//! question, or shows a plan prompt. Each agent also sets the terminal title
//! with `ESC ] 0 ; <text> BEL` or `ESC ] 2 ; <text> BEL`. The scanner finds
//! these sequences in the live PTY output. `ESC \` can end a sequence in
//! place of BEL. The scanner keeps its state across chunk borders, because a
//! sequence can arrive in two reads.

use super::machine::TerminalNotice;
use halv_supervisor::agent_hook::{preview, PREVIEW_CHARS};

const ESC: char = '\u{1b}';
const BEL: char = '\u{7}';
/// The C1 string terminator.
const ST: char = '\u{9c}';
/// A longer OSC body is not a notification or an agent title. The scanner
/// drops it, so the body buffer of a pane never grows past this size.
const MAX_OSC_BYTES: usize = 4 * 1024;

/// One complete OSC sequence that agent status reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OscEvent {
    /// The text of an OSC 9 notification.
    Notification(String),
    /// The text of an OSC 0 or OSC 2 title.
    Title(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    Osc,
    /// An ESC inside an OSC body. `\` ends the body.
    OscEscape,
}

#[derive(Debug)]
pub(crate) struct OscScanner {
    state: State,
    body: String,
    overflow: bool,
}

impl Default for OscScanner {
    fn default() -> Self {
        Self {
            state: State::Ground,
            body: String::new(),
            overflow: false,
        }
    }
}

impl OscScanner {
    /// Returns each complete notification and title in `data`, in order.
    pub(crate) fn feed(&mut self, data: &str) -> Vec<OscEvent> {
        let mut found = Vec::new();
        if self.state == State::Ground && !data.contains(ESC) {
            return found;
        }
        for character in data.chars() {
            match self.state {
                State::Ground => {
                    if character == ESC {
                        self.state = State::Escape;
                    }
                }
                State::Escape => self.begin(character),
                State::Osc => match character {
                    BEL | ST => self.finish(&mut found),
                    ESC => self.state = State::OscEscape,
                    _ => self.push(character),
                },
                State::OscEscape => {
                    if character == '\\' {
                        self.finish(&mut found);
                    } else {
                        // A new escape sequence cuts the body short.
                        self.begin(character);
                    }
                }
            }
        }
        found
    }

    /// Handles the character after an ESC.
    fn begin(&mut self, character: char) {
        self.state = match character {
            ']' => {
                self.body.clear();
                self.overflow = false;
                State::Osc
            }
            ESC => State::Escape,
            _ => State::Ground,
        };
    }

    fn push(&mut self, character: char) {
        if self.body.len() + character.len_utf8() > MAX_OSC_BYTES {
            self.overflow = true;
        } else if !self.overflow {
            self.body.push(character);
        }
    }

    fn finish(&mut self, found: &mut Vec<OscEvent>) {
        self.state = State::Ground;
        let body = std::mem::take(&mut self.body);
        if self.overflow {
            return;
        }
        // `9;4;` is the progress bar form of OSC 9, not a notification.
        if let Some(text) = body.strip_prefix("9;") {
            if !text.starts_with("4;") {
                found.push(OscEvent::Notification(text.to_string()));
            }
        } else if let Some(text) = body.strip_prefix("0;").or_else(|| body.strip_prefix("2;")) {
            found.push(OscEvent::Title(text.to_string()));
        }
    }
}

/// The last complete title in `data`, such as the scrollback of a pane. The
/// scan goes in slices, so a long scrollback with many spinner frames never
/// holds all of its titles at once.
pub(crate) fn last_title(data: &str) -> Option<String> {
    const SLICE_BYTES: usize = 64 * 1024;
    let mut scanner = OscScanner::default();
    let mut last = None;
    let mut rest = data;
    while !rest.is_empty() {
        let mut end = rest.len().min(SLICE_BYTES);
        while !rest.is_char_boundary(end) {
            end += 1;
        }
        let (slice, tail) = rest.split_at(end);
        for event in scanner.feed(slice) {
            if let OscEvent::Title(title) = event {
                last = Some(title);
            }
        }
        rest = tail;
    }
    last
}

/// Maps the text of a Codex notification to a wait. Other texts, such as
/// the turn-complete text, give `None`: the rollout reports the turn end.
pub(crate) fn classify_codex_notification(text: &str) -> Option<TerminalNotice> {
    let text = text.trim();
    let short = |value: &str| preview(value, PREVIEW_CHARS);
    if let Some(command) = text.strip_prefix("Approval requested: ") {
        return Some(TerminalNotice::Approval {
            command: short(command),
            file_path: None,
            message: None,
        });
    }
    if text.starts_with("Approval requested by ") {
        return Some(TerminalNotice::Approval {
            command: None,
            file_path: None,
            message: short(text),
        });
    }
    if let Some(file) = text.strip_prefix("Codex wants to edit ") {
        return Some(TerminalNotice::Approval {
            command: None,
            file_path: short(file),
            message: None,
        });
    }
    if text == "Approval requested" || text == "Approval requested:" {
        return Some(TerminalNotice::Approval {
            command: None,
            file_path: None,
            message: None,
        });
    }
    if let Some(question) = text.strip_prefix("Question:") {
        return Some(TerminalNotice::Question(
            short(question).unwrap_or_default(),
        ));
    }
    if let Some(plan) = text.strip_prefix("Plan mode prompt:") {
        return Some(TerminalNotice::Plan(short(plan).unwrap_or_default()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(text: &str) -> OscEvent {
        OscEvent::Notification(text.to_string())
    }

    fn title(text: &str) -> OscEvent {
        OscEvent::Title(text.to_string())
    }

    #[test]
    fn finds_notifications_with_both_terminators() {
        let mut scanner = OscScanner::default();
        let found = scanner.feed(
            "work\u{1b}]9;Approval requested: npm test\u{7}more\u{1b}]9;Question: Which?\u{1b}\\end",
        );
        assert_eq!(
            found,
            [
                notice("Approval requested: npm test"),
                notice("Question: Which?")
            ]
        );
    }

    #[test]
    fn keeps_state_across_chunks() {
        let mut scanner = OscScanner::default();
        assert!(scanner.feed("text \u{1b}").is_empty());
        assert!(scanner.feed("]9;Plan mode").is_empty());
        assert!(scanner.feed(" prompt: Ship it?\u{1b}").is_empty());
        assert_eq!(scanner.feed("\\"), [notice("Plan mode prompt: Ship it?")]);
    }

    #[test]
    fn finds_titles_with_both_terminators_in_order() {
        let mut scanner = OscScanner::default();
        let found = scanner.feed(
            "\u{1b}]0;\u{2733} Claude Code\u{7}text\u{1b}]2;\u{25d0} Fix the parser\u{1b}\\\u{1b}]9;Question: b\u{7}\u{1b}]0;\u{2733} Fix the parser\u{9c}",
        );
        assert_eq!(
            found,
            [
                title("\u{2733} Claude Code"),
                title("\u{25d0} Fix the parser"),
                notice("Question: b"),
                title("\u{2733} Fix the parser"),
            ]
        );
    }

    #[test]
    fn keeps_a_split_title_across_chunks() {
        let mut scanner = OscScanner::default();
        // A chunk border can fall in the introducer, the body, or the ST.
        assert!(scanner.feed("output \u{1b}").is_empty());
        assert!(scanner.feed("]").is_empty());
        assert!(scanner.feed("0;\u{280b} scr").is_empty());
        assert_eq!(scanner.feed("atch\u{7}more"), [title("\u{280b} scratch")]);
        assert!(scanner
            .feed("\u{1b}]2;[ ! ] Action Required | scratch\u{1b}")
            .is_empty());
        assert_eq!(
            scanner.feed("\\"),
            [title("[ ! ] Action Required | scratch")]
        );
        // A multibyte glyph in one chunk is whole, as the PTY reader decodes
        // the bytes before it sends the text.
        assert_eq!(
            scanner.feed("\u{1b}]0;\u{2733} Idle\u{7}"),
            [title("\u{2733} Idle")]
        );
    }

    #[test]
    fn ignores_icon_names_progress_and_other_sequences() {
        let mut scanner = OscScanner::default();
        let found = scanner.feed(
            "\u{1b}]1;icon\u{7}\u{1b}]7;file:///tmp\u{7}\u{1b}]9;4;1;50\u{7}\u{1b}[31mred\u{1b}[0m\u{7}",
        );
        assert!(found.is_empty());
        // A plain BEL outside an OSC is not a notification.
        assert!(scanner.feed("\u{7}").is_empty());
        // An empty title is still a title: the agent cleared it.
        assert_eq!(scanner.feed("\u{1b}]2;\u{7}"), [title("")]);
    }

    #[test]
    fn drops_an_oversized_body_and_recovers() {
        let mut scanner = OscScanner::default();
        let long = format!("\u{1b}]9;{}\u{7}", "x".repeat(MAX_OSC_BYTES + 10));
        assert!(scanner.feed(&long).is_empty());
        assert_eq!(
            scanner.feed("\u{1b}]9;Question: ok\u{7}"),
            [notice("Question: ok")]
        );
        let long_title = format!("\u{1b}]0;{}\u{7}", "x".repeat(MAX_OSC_BYTES + 10));
        assert!(scanner.feed(&long_title).is_empty());
        assert_eq!(scanner.feed("\u{1b}]0;ok\u{7}"), [title("ok")]);
    }

    #[test]
    fn a_new_escape_cuts_a_body_short() {
        let mut scanner = OscScanner::default();
        assert!(scanner.feed("\u{1b}]9;Approval\u{1b}[0m").is_empty());
        assert_eq!(
            scanner.feed("\u{1b}]9;Question: b\u{7}"),
            [notice("Question: b")]
        );
        // An abandoned title before a new one gives only the new one.
        assert_eq!(
            scanner.feed("\u{1b}]0;abandoned\u{1b}]0;Fresh title\u{7}"),
            [title("Fresh title")]
        );
    }

    #[test]
    fn the_last_title_of_a_scrollback_wins() {
        assert_eq!(
            last_title(
                "\u{1b}]0;\u{2733} Claude Code\u{7}hello\u{1b}]0;\u{25d1} Fix it\u{7}\u{1b}]9;Question: x\u{7}"
            )
            .as_deref(),
            Some("\u{25d1} Fix it")
        );
        assert_eq!(last_title("plain output"), None);
        // An unfinished title at the end is not a title yet.
        assert_eq!(
            last_title("\u{1b}]0;one\u{7}\u{1b}]0;two").as_deref(),
            Some("one")
        );
        // A title across a slice border of a long scrollback stays whole.
        // The 64 KiB border falls inside the three bytes of the spinner.
        let long = format!(
            "{}\u{1b}]0;\u{2733} early\u{7}{}\u{1b}]0;\u{25d0} across the border\u{7}tail",
            "\u{2500}".repeat(20_000),
            "x".repeat(64 * 1024 - 60_000 - 14 - 5),
        );
        assert!(!long.is_char_boundary(64 * 1024));
        assert_eq!(
            last_title(&long).as_deref(),
            Some("\u{25d0} across the border")
        );
    }

    #[test]
    fn classifies_codex_texts() {
        assert_eq!(
            classify_codex_notification("Approval requested: cargo build --release"),
            Some(TerminalNotice::Approval {
                command: Some("cargo build --release".to_string()),
                file_path: None,
                message: None,
            })
        );
        assert_eq!(
            classify_codex_notification("Codex wants to edit src/main.rs"),
            Some(TerminalNotice::Approval {
                command: None,
                file_path: Some("src/main.rs".to_string()),
                message: None,
            })
        );
        assert!(matches!(
            classify_codex_notification("Approval requested by deploy-server"),
            Some(TerminalNotice::Approval {
                message: Some(_),
                ..
            })
        ));
        assert_eq!(
            classify_codex_notification("Question: Which branch?"),
            Some(TerminalNotice::Question("Which branch?".to_string()))
        );
        assert_eq!(
            classify_codex_notification("Plan mode prompt: Implement this plan?"),
            Some(TerminalNotice::Plan("Implement this plan?".to_string()))
        );
        assert_eq!(classify_codex_notification("Agent turn complete"), None);
        assert_eq!(classify_codex_notification("All tests pass."), None);
    }
}
