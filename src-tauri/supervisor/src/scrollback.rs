use std::collections::VecDeque;

pub struct ScrollbackRing {
    max_bytes: usize,
    sequence: u64,
    scrollback: VecDeque<u8>,
    chunks: VecDeque<OutputChunk>,
    utf8_carry: Vec<u8>,
}

struct OutputChunk {
    sequence: u64,
    data: String,
}

impl ScrollbackRing {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            sequence: 0,
            scrollback: VecDeque::new(),
            chunks: VecDeque::new(),
            utf8_carry: Vec::new(),
        }
    }

    pub fn append(&mut self, data: &[u8]) -> Option<(u64, String)> {
        let mut bytes = std::mem::take(&mut self.utf8_carry);
        bytes.extend_from_slice(data);
        let complete_len = complete_utf8_prefix_len(&bytes);
        self.utf8_carry.extend_from_slice(&bytes[complete_len..]);
        debug_assert!(self.utf8_carry.len() <= 3);

        let decoded = String::from_utf8_lossy(&bytes[..complete_len]).into_owned();
        if decoded.is_empty() {
            return None;
        }

        let sequence = self.next_sequence();
        self.store_decoded(sequence, &decoded);
        Some((sequence, decoded))
    }

    pub fn append_decoded(&mut self, sequence: u64, decoded: &str) -> Option<(u64, String)> {
        if sequence <= self.sequence {
            return None;
        }
        self.sequence = sequence;
        if decoded.is_empty() {
            return None;
        }
        self.store_decoded(sequence, decoded);
        Some((sequence, decoded.to_string()))
    }

    pub fn replace_with_snapshot(&mut self, sequence: u64, scrollback: &str) {
        self.sequence = sequence;
        self.scrollback.clear();
        self.chunks.clear();
        self.utf8_carry.clear();
        if !scrollback.is_empty() {
            self.store_decoded(sequence, scrollback);
        }
    }

    pub fn snapshot(&self, from_sequence: Option<u64>) -> (u64, String) {
        let bytes = match from_sequence {
            Some(sequence) => self
                .chunks
                .iter()
                .filter(|chunk| chunk.sequence > sequence)
                .flat_map(|chunk| chunk.data.as_bytes().iter().copied())
                .collect::<Vec<_>>(),
            None => self.scrollback.iter().copied().collect::<Vec<_>>(),
        };
        let scrollback = String::from_utf8(bytes).expect("terminal scrollback must be valid UTF-8");
        (self.sequence, scrollback)
    }

    pub fn next_sequence(&mut self) -> u64 {
        self.sequence = self.sequence.saturating_add(1);
        self.sequence
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn set_sequence(&mut self, sequence: u64) {
        self.sequence = sequence;
    }

    pub fn trim(&mut self) {
        let mut removed = 0;
        while self.scrollback.len() > self.max_bytes {
            self.scrollback.pop_front();
            removed += 1;
        }
        while self
            .scrollback
            .front()
            .is_some_and(|byte| byte & 0b1100_0000 == 0b1000_0000)
        {
            self.scrollback.pop_front();
            removed += 1;
        }
        self.trim_chunks(removed);
    }

    fn store_decoded(&mut self, sequence: u64, decoded: &str) {
        self.scrollback.extend(decoded.as_bytes());
        self.chunks.push_back(OutputChunk {
            sequence,
            data: decoded.to_string(),
        });
        self.trim();
    }

    fn trim_chunks(&mut self, mut bytes: usize) {
        while bytes > 0 {
            let Some(front) = self.chunks.front_mut() else {
                break;
            };
            if bytes >= front.data.len() {
                bytes -= front.data.len();
                self.chunks.pop_front();
            } else {
                front.data.drain(..bytes);
                bytes = 0;
            }
        }
    }
}

fn complete_utf8_prefix_len(bytes: &[u8]) -> usize {
    let mut offset = 0;
    while offset < bytes.len() {
        match std::str::from_utf8(&bytes[offset..]) {
            Ok(_) => return bytes.len(),
            Err(error) => {
                offset += error.valid_up_to();
                match error.error_len() {
                    Some(invalid_len) => offset += invalid_len,
                    None => return offset,
                }
            }
        }
    }
    bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX_SCROLLBACK_BYTES: usize = 256 * 1024;

    fn ring() -> ScrollbackRing {
        ScrollbackRing::new(MAX_SCROLLBACK_BYTES)
    }

    #[test]
    fn scrollback_keeps_only_the_bounded_tail() {
        let mut output = ring();
        let _ = output.append(&vec![b'a'; MAX_SCROLLBACK_BYTES]);
        let _ = output.append(b"tail");
        let (sequence, scrollback) = output.snapshot(None);
        assert_eq!(sequence, 2);
        assert_eq!(scrollback.len(), MAX_SCROLLBACK_BYTES);
        assert!(scrollback.ends_with("tail"));
    }

    #[test]
    fn externally_sequenced_scrollback_keeps_sequence_and_bounded_tail() {
        let mut output = ring();
        assert_eq!(
            output.append_decoded(41, "first"),
            Some((41, "first".to_string()))
        );
        let tail = "x".repeat(MAX_SCROLLBACK_BYTES);
        let _ = output.append_decoded(97, &tail);

        let (sequence, scrollback) = output.snapshot(None);
        assert_eq!(sequence, 97);
        assert_eq!(scrollback.len(), MAX_SCROLLBACK_BYTES);
        assert_eq!(scrollback, tail);
    }

    #[test]
    fn snapshot_replacement_can_rewind_externally_sequenced_output() {
        let mut output = ring();
        let _ = output.append_decoded(41, "old");

        output.replace_with_snapshot(7, "snapshot");
        assert_eq!(output.snapshot(None), (7, "snapshot".to_string()));
        assert_eq!(
            output.append_decoded(8, "next"),
            Some((8, "next".to_string()))
        );
        assert_eq!(output.snapshot(None), (8, "snapshotnext".to_string()));
    }

    #[test]
    fn incomplete_utf8_tail_waits_for_the_next_append() {
        let mut output = ring();
        let encoded = "🦀".as_bytes();

        assert_eq!(output.append(&encoded[..2]), None);
        assert_eq!(output.snapshot(None), (0, String::new()));
        assert_eq!(output.append(&encoded[2..]), Some((1, "🦀".to_string())));
        assert_eq!(output.snapshot(None), (1, "🦀".to_string()));
    }

    #[test]
    fn utf8_characters_survive_every_two_chunk_boundary() {
        for character in ['⏺', '✻', '╭', '─', '🦀'] {
            let encoded = character.to_string().into_bytes();
            for split in 1..encoded.len() {
                let mut output = ring();
                let mut live = String::new();
                for chunk in [&encoded[..split], &encoded[split..]] {
                    if let Some((_, decoded)) = output.append(chunk) {
                        live.push_str(&decoded);
                    }
                }
                assert_eq!(live, character.to_string());
                assert_eq!(output.snapshot(None).1, character.to_string());
            }
        }
    }

    #[test]
    fn utf8_characters_survive_every_three_chunk_boundary() {
        for character in ['⏺', '✻', '╭', '─', '🦀'] {
            let encoded = character.to_string().into_bytes();
            for first in 1..encoded.len() - 1 {
                for second in first + 1..encoded.len() {
                    let mut output = ring();
                    let mut live = String::new();
                    for chunk in [
                        &encoded[..first],
                        &encoded[first..second],
                        &encoded[second..],
                    ] {
                        if let Some((_, decoded)) = output.append(chunk) {
                            live.push_str(&decoded);
                        }
                    }
                    assert_eq!(live, character.to_string());
                    assert_eq!(output.snapshot(None).1, character.to_string());
                }
            }
        }
    }

    #[test]
    fn scrollback_replay_preserves_chunked_utf8() {
        let expected = "Claude ⏺ ✻ ╭─ ready";
        let mut output = ring();
        for chunk in expected.as_bytes().chunks(2) {
            let _ = output.append(chunk);
        }

        let (_, scrollback) = output.snapshot(None);
        assert_eq!(scrollback, expected);
        assert!(!scrollback.contains('�'));
    }

    #[test]
    fn terminal_stream_preserves_escape_and_utf8_bytes_across_chunks() {
        let expected = concat!(
            "\x1b7saved\x1b8",
            "\x1b[?25h",
            "\x1b]10;?\x1b\\",
            "Claude 🦀 ready",
        );
        let bytes = expected.as_bytes();

        for split in 1..bytes.len() {
            let mut output = ring();
            let mut emitted = Vec::new();
            for chunk in [&bytes[..split], &bytes[split..]] {
                if let Some((_, decoded)) = output.append(chunk) {
                    emitted.extend_from_slice(decoded.as_bytes());
                }
            }

            assert_eq!(emitted, bytes, "two-chunk split at byte {split}");
            assert_eq!(output.snapshot(None).1.as_bytes(), bytes);
        }

        let mut output = ring();
        let mut emitted = Vec::new();
        for byte in bytes {
            if let Some((_, decoded)) = output.append(&[*byte]) {
                emitted.extend_from_slice(decoded.as_bytes());
            }
        }
        assert_eq!(emitted, bytes, "one-byte chunks");
        assert_eq!(output.snapshot(None).1.as_bytes(), bytes);
    }

    #[test]
    fn scrollback_trimming_keeps_a_valid_utf8_front_boundary() {
        let mut output = ring();
        let mut full = "🦀".as_bytes().to_vec();
        full.extend(vec![b'a'; MAX_SCROLLBACK_BYTES - full.len()]);
        let _ = output.append(&full);
        let _ = output.append(b"x");

        let (_, scrollback) = output.snapshot(None);
        assert_eq!(
            scrollback,
            format!("{}x", "a".repeat(MAX_SCROLLBACK_BYTES - 4))
        );
        assert!(!scrollback.contains('�'));
    }

    #[test]
    fn sequence_snapshot_returns_only_newer_decoded_chunks() {
        let mut output = ring();
        let first = output.append(b"first").unwrap().0;
        let _ = output.append(b"second");
        let _ = output.append(b"third");
        assert_eq!(output.snapshot(Some(first)).1, "secondthird");
    }

    #[cfg(unix)]
    #[test]
    fn inclusive_client_resume_returns_the_next_chunks_without_a_gap() {
        let mut output = ring();
        let first = output.append(b"first").unwrap().0;
        let second = output.append(b"second").unwrap().0;
        let third = output.append(b"third").unwrap().0;

        let next_sequence = first.saturating_add(1);
        let protocol_cursor = crate::client::resume_protocol_cursor(Some(next_sequence));
        assert_eq!(protocol_cursor, Some(first));
        let (snapshot_sequence, delta) = output.snapshot(protocol_cursor);
        assert_eq!(snapshot_sequence, third);
        assert_eq!(second, next_sequence);
        assert_eq!(delta, "secondthird");
    }
}
