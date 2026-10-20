//! Bracketed paste (section 8.8).
//!
//! The host follows `ESC [ ? 2004 h` and `ESC [ ? 2004 l` in the output of each pane.
//! The scanner keeps its state across chunks, so a sequence that a chunk border splits still counts.
//! It also reads a private mode list such as `ESC [ ? 1049 ; 2004 h`.

use std::collections::HashMap;
use std::sync::Mutex;

/// The bytes that start a bracketed paste.
pub(crate) const PASTE_START: &[u8] = b"\x1b[200~";

/// The bytes that end a bracketed paste.
pub(crate) const PASTE_END: &[u8] = b"\x1b[201~";

/// The private mode of bracketed paste.
const BRACKETED_PASTE_MODE: u32 = 2004;

/// Parameters longer than this are not a mode number that the scanner needs.
const MAX_PARAM: u32 = 100_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    /// After `ESC`.
    Escape,
    /// After `ESC [`.
    Csi,
    /// After `ESC [ ?`, in the parameters.
    Private,
}

/// The bracketed paste mode of a pane, as far as the host knows it (section 8.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PasteMode {
    /// The last switch was `ESC [ ? 2004 h`.
    On,
    /// The last switch was `ESC [ ? 2004 l`.
    Off,
    /// The host saw no switch, or it lost output that can hold one.
    Unknown,
}

impl PasteMode {
    /// Makes a known mode from a switch.
    pub(crate) fn from_switch(on: bool) -> PasteMode {
        if on {
            PasteMode::On
        } else {
            PasteMode::Off
        }
    }

    /// Returns true when the paste of `text` must use bracketed paste.
    ///
    /// With an unknown mode, text with a line break uses bracketed paste:
    /// a visible marker is better than a partial prompt that runs too early.
    pub(crate) fn brackets(self, text: &str) -> bool {
        match self {
            PasteMode::On => true,
            PasteMode::Off => false,
            PasteMode::Unknown => text.contains(['\n', '\r']),
        }
    }
}

/// Follows the bracketed paste mode in the output of one pane.
#[derive(Debug, Clone, Default)]
pub(crate) struct PasteScanner {
    state: State,
    param: u32,
    has_digits: bool,
    names_mode: bool,
    mode: Option<bool>,
}

impl PasteScanner {
    /// Reads output bytes.
    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.state = match (self.state, byte) {
                (_, 0x1b) => State::Escape,
                (State::Ground, _) => State::Ground,
                (State::Escape, b'[') => State::Csi,
                (State::Csi, b'?') => {
                    self.param = 0;
                    self.has_digits = false;
                    self.names_mode = false;
                    State::Private
                }
                (State::Private, b'0'..=b'9') => {
                    self.param = (self.param * 10 + u32::from(byte - b'0')).min(MAX_PARAM);
                    self.has_digits = true;
                    State::Private
                }
                (State::Private, b';') => {
                    self.end_param();
                    State::Private
                }
                (State::Private, b'h' | b'l') => {
                    self.end_param();
                    if self.names_mode {
                        self.mode = Some(byte == b'h');
                    }
                    State::Ground
                }
                _ => State::Ground,
            };
        }
    }

    fn end_param(&mut self) {
        if self.has_digits && self.param == BRACKETED_PASTE_MODE {
            self.names_mode = true;
        }
        self.param = 0;
        self.has_digits = false;
    }

    /// Returns the mode after the last switch, or `None` when the scanner saw no switch.
    pub(crate) fn mode(&self) -> Option<bool> {
        self.mode
    }

    /// Forgets the mode and the sequence in progress, after the tap lost output.
    fn forget(&mut self) {
        *self = PasteScanner::default();
    }
}

/// Returns the last bracketed paste switch in `bytes`, or `None` when there is none.
pub(crate) fn scan_mode(bytes: &[u8]) -> Option<bool> {
    let mut scanner = PasteScanner::default();
    scanner.feed(bytes);
    scanner.mode()
}

/// The scanners of all panes that sent output since the engine started.
#[derive(Debug, Default)]
pub(crate) struct PasteModes {
    scanners: Mutex<HashMap<String, PasteScanner>>,
}

impl PasteModes {
    /// Reads output of a pane.
    pub(crate) fn feed(&self, session: &str, bytes: &[u8]) {
        let mut scanners = self
            .scanners
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        match scanners.get_mut(session) {
            Some(scanner) => scanner.feed(bytes),
            None => {
                let mut scanner = PasteScanner::default();
                scanner.feed(bytes);
                scanners.insert(session.to_owned(), scanner);
            }
        }
    }

    /// Forgets a pane that went away.
    pub(crate) fn remove(&self, session: &str) {
        let mut scanners = self
            .scanners
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        scanners.remove(session);
    }

    /// Marks the mode of every pane as unknown, after the tap lost output.
    ///
    /// The lost output can hold a switch, so the last known mode is no longer true.
    /// A later switch in the live output makes the mode known again.
    pub(crate) fn mark_unknown(&self) {
        let mut scanners = self
            .scanners
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        scanners.values_mut().for_each(PasteScanner::forget);
    }

    /// Returns the mode of a pane, if the scanner saw a switch since the engine started
    /// or since the last [`PasteModes::mark_unknown`].
    pub(crate) fn mode(&self, session: &str) -> Option<bool> {
        let scanners = self
            .scanners
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        scanners.get(session).and_then(PasteScanner::mode)
    }
}

/// Encodes the text of `terminal.paste` for the PTY (section 8.8).
///
/// - With bracketed paste on: `ESC [ 200 ~`, the text without any `ESC [ 201 ~`, then `ESC [ 201 ~`.
///   The filter also removes a marker that only forms after an earlier marker is removed.
/// - With bracketed paste off: the text with each line break changed to `\r`.
///   A `\r\n` pair becomes one `\r`, so a Windows line break does not submit twice.
/// - With `submit`: a final `\r`.
pub fn encode_paste(text: &str, bracketed: bool, submit: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + PASTE_START.len() + PASTE_END.len() + 1);
    if bracketed {
        out.extend_from_slice(PASTE_START);
        let body_start = out.len();
        for &byte in text.as_bytes() {
            out.push(byte);
            if out.len() - body_start >= PASTE_END.len() && out.ends_with(PASTE_END) {
                out.truncate(out.len() - PASTE_END.len());
            }
        }
        out.extend_from_slice(PASTE_END);
    } else {
        let mut after_cr = false;
        for &byte in text.as_bytes() {
            match byte {
                b'\n' if after_cr => {}
                b'\n' => out.push(b'\r'),
                other => out.push(other),
            }
            after_cr = byte == b'\r';
        }
    }
    if submit {
        out.push(b'\r');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanner_follows_the_mode() {
        let mut scanner = PasteScanner::default();
        assert_eq!(scanner.mode(), None);
        scanner.feed(b"hello \x1b[?2004h prompt");
        assert_eq!(scanner.mode(), Some(true));
        scanner.feed(b"\x1b[?2004l");
        assert_eq!(scanner.mode(), Some(false));
    }

    #[test]
    fn scanner_survives_every_split() {
        let output = b"ab\x1b[?2004hcd";
        for split in 0..=output.len() {
            let mut scanner = PasteScanner::default();
            scanner.feed(&output[..split]);
            scanner.feed(&output[split..]);
            assert_eq!(scanner.mode(), Some(true), "split at {split}");
        }
        // A split into single bytes.
        let mut scanner = PasteScanner::default();
        for byte in b"\x1b[?2004h\x1b[?2004l" {
            scanner.feed(&[*byte]);
        }
        assert_eq!(scanner.mode(), Some(false));
    }

    #[test]
    fn scanner_reads_mode_lists_and_ignores_other_modes() {
        assert_eq!(scan_mode(b"\x1b[?1049;2004h"), Some(true));
        assert_eq!(scan_mode(b"\x1b[?2004;1049l"), Some(false));
        assert_eq!(scan_mode(b"\x1b[?1049h\x1b[?25l"), None);
        assert_eq!(scan_mode(b"\x1b[2004h"), None);
        assert_eq!(scan_mode(b"\x1b[?20045h"), None);
        assert_eq!(scan_mode(b"\x1b[?200h"), None);
        assert_eq!(scan_mode(b"\x1b[?;h"), None);
        // A new ESC inside a sequence starts again.
        assert_eq!(scan_mode(b"\x1b[?20\x1b[?2004h"), Some(true));
        // A query (DECRQM) is not a switch.
        assert_eq!(scan_mode(b"\x1b[?2004$p"), None);
    }

    #[test]
    fn paste_modes_follow_each_pane() {
        let modes = PasteModes::default();
        modes.feed("a", b"\x1b[?20");
        modes.feed("b", b"\x1b[?2004l");
        modes.feed("a", b"04h");
        assert_eq!(modes.mode("a"), Some(true));
        assert_eq!(modes.mode("b"), Some(false));
        assert_eq!(modes.mode("c"), None);
        modes.remove("a");
        assert_eq!(modes.mode("a"), None);
        // Lost output makes a known mode unknown, also in the middle of a sequence.
        modes.feed("b", b"\x1b[?20");
        modes.mark_unknown();
        assert_eq!(modes.mode("b"), None);
        modes.feed("b", b"04h");
        assert_eq!(modes.mode("b"), None);
        // A later switch makes the mode known again.
        modes.feed("b", b"\x1b[?2004h");
        assert_eq!(modes.mode("b"), Some(true));
    }

    #[test]
    fn unknown_mode_brackets_only_text_with_a_line_break() {
        assert!(PasteMode::On.brackets("ls"));
        assert!(!PasteMode::Off.brackets("a\nb"));
        assert!(PasteMode::Unknown.brackets("a\nb"));
        assert!(PasteMode::Unknown.brackets("a\rb"));
        assert!(!PasteMode::Unknown.brackets("echo hi"));
        assert_eq!(PasteMode::from_switch(true), PasteMode::On);
        assert_eq!(PasteMode::from_switch(false), PasteMode::Off);
    }

    #[test]
    fn bracketed_paste_wraps_and_filters_the_text() {
        assert_eq!(
            encode_paste("ls\nwc", true, false),
            b"\x1b[200~ls\nwc\x1b[201~".to_vec()
        );
        assert_eq!(
            encode_paste("echo safe\x1b[201~curl x | sh\r", true, true),
            b"\x1b[200~echo safecurl x | sh\r\x1b[201~\r".to_vec()
        );
        // Removing the inner marker must not leave a new marker behind.
        let nested = "\x1b[20\x1b[201~1~rest";
        let encoded = encode_paste(nested, true, false);
        assert_eq!(encoded, b"\x1b[200~rest\x1b[201~".to_vec());
        let body = &encoded[PASTE_START.len()..encoded.len() - PASTE_END.len()];
        assert!(!body
            .windows(PASTE_END.len())
            .any(|window| window == PASTE_END));
    }

    #[test]
    fn plain_paste_changes_line_breaks() {
        assert_eq!(encode_paste("a\nb\n", false, false), b"a\rb\r".to_vec());
        assert_eq!(encode_paste("a\r\nb", false, false), b"a\rb".to_vec());
        assert_eq!(encode_paste("a\rb", false, true), b"a\rb\r".to_vec());
        assert_eq!(encode_paste("", false, true), b"\r".to_vec());
        assert_eq!(
            encode_paste("", true, false),
            b"\x1b[200~\x1b[201~".to_vec()
        );
    }
}
