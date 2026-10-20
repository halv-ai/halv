//! Transport framing (section 7.6).
//!
//! Each Noise transport plaintext starts with a flag byte:
//!
//! | Flag | Meaning |
//! | --- | --- |
//! | `0x00` | The rest is one complete application message. |
//! | `0x01` | The rest is a chunk. More chunks follow. |
//! | `0x02` | The rest is the last chunk. |
//!
//! A Noise message holds at most 65,535 bytes, so a plaintext holds at most 65,519 bytes
//! and a chunk holds at most 65,518 bytes.
//! Each Noise message travels in its own WebSocket frame.

use std::fmt;

use zeroize::{Zeroize, Zeroizing};

use crate::error::{Error, Result};
use crate::relay_wire::MAX_NOISE_MESSAGE;

/// The length of the ChaChaPoly authentication tag of each Noise message.
pub const NOISE_TAG_LEN: usize = 16;

/// The largest plaintext of one Noise transport message (65,519 bytes).
pub const MAX_TRANSPORT_PLAINTEXT: usize = MAX_NOISE_MESSAGE - NOISE_TAG_LEN;

/// The largest part of an application message in one Noise message (65,518 bytes).
pub const MAX_CHUNK_LEN: usize = MAX_TRANSPORT_PLAINTEXT - 1;

/// The largest application message (8 MiB). A larger message is a protocol error.
pub const MAX_APP_MESSAGE: usize = 8 * 1024 * 1024;

/// The flag of a complete application message.
pub const FLAG_COMPLETE: u8 = 0x00;

/// The flag of a chunk that more chunks follow.
pub const FLAG_CHUNK: u8 = 0x01;

/// The flag of the last chunk.
pub const FLAG_LAST_CHUNK: u8 = 0x02;

/// Version 1 has no rekey, so a connection closes after 24 hours (section 7.7).
pub const MAX_CONNECTION_LIFETIME_MS: u64 = 24 * 60 * 60 * 1000;

/// Returns the number of Noise messages that [`Transport::seal`] makes for a message of `len` bytes.
pub const fn noise_message_count(len: usize) -> usize {
    if len <= MAX_CHUNK_LEN {
        1
    } else {
        len.div_ceil(MAX_CHUNK_LEN)
    }
}

/// An encrypted channel after a successful handshake.
///
/// It encrypts application messages into Noise messages and decrypts them again.
/// A decryption failure, a framing error, or a message above 8 MiB is fatal:
/// the transport is then poisoned, and every later call fails with [`Error::Poisoned`].
/// The connection must close. A new connection makes a new handshake (section 7.7).
///
/// The Noise messages of each direction must arrive in the order that they were made.
/// A lost, replayed, reordered, changed, or truncated message fails.
pub struct Transport {
    state: snow::TransportState,
    partial: Option<Vec<u8>>,
    poisoned: bool,
}

impl Transport {
    pub(crate) fn new(state: snow::TransportState) -> Transport {
        Transport {
            state,
            partial: None,
            poisoned: false,
        }
    }

    /// Encrypts one application message into one or more Noise messages.
    ///
    /// Send each Noise message in its own WebSocket frame, in order, before the next call.
    /// A message of more than 8 MiB fails with [`Error::TooLarge`] and leaves the transport usable.
    pub fn seal(&mut self, message: &[u8]) -> Result<Vec<Vec<u8>>> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        if message.len() > MAX_APP_MESSAGE {
            return Err(Error::TooLarge);
        }
        // One buffer for every chunk. It zeroizes on drop, so no plaintext stays in freed memory.
        let mut plaintext = Zeroizing::new(Vec::with_capacity(
            MAX_TRANSPORT_PLAINTEXT.min(message.len() + 1),
        ));
        let mut sealed = Vec::with_capacity(noise_message_count(message.len()));
        if message.len() <= MAX_CHUNK_LEN {
            sealed.push(self.encrypt(&mut plaintext, FLAG_COMPLETE, message)?);
        } else {
            let mut chunks = message.chunks(MAX_CHUNK_LEN).peekable();
            while let Some(chunk) = chunks.next() {
                let flag = if chunks.peek().is_some() {
                    FLAG_CHUNK
                } else {
                    FLAG_LAST_CHUNK
                };
                sealed.push(self.encrypt(&mut plaintext, flag, chunk)?);
            }
        }
        Ok(sealed)
    }

    /// Decrypts one Noise message.
    ///
    /// It returns `Some(message)` when the message completes an application message,
    /// and `None` when it is a chunk that more chunks follow.
    /// Any error poisons the transport. The caller must then close the connection.
    pub fn open(&mut self, noise_message: &[u8]) -> Result<Option<Vec<u8>>> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        let result = self.open_inner(noise_message);
        if result.is_err() {
            self.poison();
        }
        result
    }

    /// Returns true after a fatal error.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Returns true for the initiator (the device) and false for the responder (the host).
    pub fn is_initiator(&self) -> bool {
        self.state.is_initiator()
    }

    fn encrypt(&mut self, plaintext: &mut Vec<u8>, flag: u8, body: &[u8]) -> Result<Vec<u8>> {
        plaintext.clear();
        plaintext.push(flag);
        plaintext.extend_from_slice(body);
        let mut out = vec![0u8; plaintext.len() + NOISE_TAG_LEN];
        match self.state.write_message(plaintext, &mut out) {
            Ok(len) => {
                out.truncate(len);
                Ok(out)
            }
            Err(_) => {
                // Some chunks of this message can have used nonces already.
                self.poison();
                Err(Error::Internal("Noise encryption failed"))
            }
        }
    }

    fn open_inner(&mut self, noise_message: &[u8]) -> Result<Option<Vec<u8>>> {
        if noise_message.len() > MAX_NOISE_MESSAGE {
            return Err(Error::Framing(
                "the Noise message is larger than 65,535 bytes",
            ));
        }
        let mut plaintext = Zeroizing::new(vec![0u8; noise_message.len()]);
        let len = self
            .state
            .read_message(noise_message, &mut plaintext)
            .map_err(|_| Error::Decrypt)?;
        plaintext.truncate(len);
        let Some(&flag) = plaintext.first() else {
            return Err(Error::Framing("the transport message has no flag byte"));
        };
        match flag {
            FLAG_COMPLETE => {
                if self.partial.is_some() {
                    return Err(Error::Framing("a complete message arrived between chunks"));
                }
                let mut message = std::mem::take(&mut *plaintext);
                message.remove(0);
                Ok(Some(message))
            }
            FLAG_CHUNK | FLAG_LAST_CHUNK => {
                let body = &plaintext[1..];
                let partial = match self.partial.as_mut() {
                    Some(partial) => partial,
                    None if flag == FLAG_CHUNK => self.partial.insert(Vec::new()),
                    None => {
                        return Err(Error::Framing(
                            "a last chunk arrived without earlier chunks",
                        ))
                    }
                };
                if partial.len() + body.len() > MAX_APP_MESSAGE {
                    return Err(Error::TooLarge);
                }
                append_wiping(partial, body);
                if flag == FLAG_LAST_CHUNK {
                    Ok(self.partial.take())
                } else {
                    Ok(None)
                }
            }
            _ => Err(Error::Framing("unknown flag byte")),
        }
    }

    fn poison(&mut self) {
        self.poisoned = true;
        if let Some(mut partial) = self.partial.take() {
            partial.zeroize();
        }
    }
}

/// Appends `body` to `buffer`. When the buffer must grow, the old allocation is zeroized,
/// so a reallocation leaves no copy of the plaintext in freed memory.
fn append_wiping(buffer: &mut Vec<u8>, body: &[u8]) {
    let needed = buffer.len() + body.len();
    if needed > buffer.capacity() {
        let capacity = needed.max(buffer.capacity().saturating_mul(2));
        let mut grown = Vec::with_capacity(capacity);
        grown.extend_from_slice(buffer);
        let mut old = std::mem::replace(buffer, grown);
        old.zeroize();
    }
    buffer.extend_from_slice(body);
}

impl fmt::Debug for Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Transport")
            .field("initiator", &self.is_initiator())
            .field("poisoned", &self.poisoned)
            .field("partial_len", &self.partial.as_ref().map(Vec::len))
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Makes a connected transport pair with a plain `Noise_NN` handshake.
    /// The tests of the real handshakes are in the `noise` module.
    fn pair() -> (Transport, Transport) {
        let params: snow::params::NoiseParams =
            "Noise_NN_25519_ChaChaPoly_BLAKE2s".parse().unwrap();
        let mut initiator = snow::Builder::new(params.clone())
            .build_initiator()
            .unwrap();
        let mut responder = snow::Builder::new(params).build_responder().unwrap();
        let mut message = [0u8; 1024];
        let mut payload = [0u8; 1024];
        let len = initiator.write_message(&[], &mut message).unwrap();
        responder
            .read_message(&message[..len], &mut payload)
            .unwrap();
        let len = responder.write_message(&[], &mut message).unwrap();
        initiator
            .read_message(&message[..len], &mut payload)
            .unwrap();
        (
            Transport::new(initiator.into_transport_mode().unwrap()),
            Transport::new(responder.into_transport_mode().unwrap()),
        )
    }

    /// Makes a raw Noise message with any plaintext, so a test can break the framing rules.
    fn raw(sender: &mut Transport, plaintext: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; plaintext.len() + NOISE_TAG_LEN];
        let len = sender.state.write_message(plaintext, &mut out).unwrap();
        out.truncate(len);
        out
    }

    fn pattern(len: usize) -> Vec<u8> {
        (0..len).map(|index| (index % 251) as u8).collect()
    }

    fn deliver(receiver: &mut Transport, sealed: &[Vec<u8>]) -> Vec<u8> {
        let (last, first) = sealed.split_last().unwrap();
        for message in first {
            assert_eq!(receiver.open(message).unwrap(), None);
        }
        receiver
            .open(last)
            .unwrap()
            .expect("the last Noise message completes the message")
    }

    #[test]
    fn chunk_sizes_of_the_contract() {
        let (mut device, mut host) = pair();
        for (len, count) in [
            (0, 1),
            (1, 1),
            (65_518, 1),
            (65_519, 2),
            (131_036, 2),
            (1 << 20, 17),
        ] {
            let message = pattern(len);
            let sealed = device.seal(&message).unwrap();
            assert_eq!(sealed.len(), count, "message of {len} bytes");
            assert_eq!(noise_message_count(len), count);
            assert!(sealed.iter().all(|noise| noise.len() <= MAX_NOISE_MESSAGE));
            assert_eq!(
                deliver(&mut host, &sealed),
                message,
                "message of {len} bytes"
            );
        }
        // A message of 65,518 bytes fills one Noise message exactly.
        assert_eq!(device.seal(&pattern(65_518)).unwrap()[0].len(), 65_535);
        // An empty message is the flag byte and the tag.
        assert_eq!(host.seal(&[]).unwrap()[0].len(), 17);
    }

    #[test]
    fn chunks_carry_the_flags_in_order() {
        let (mut device, mut host) = pair();
        let sealed = device.seal(&pattern(2 * MAX_CHUNK_LEN + 10)).unwrap();
        let mut flags = Vec::new();
        for message in &sealed {
            let mut plaintext = vec![0u8; message.len()];
            let len = host.state.read_message(message, &mut plaintext).unwrap();
            flags.push(plaintext[0]);
            assert_eq!(
                len,
                if plaintext[0] == FLAG_LAST_CHUNK {
                    11
                } else {
                    65_519
                }
            );
        }
        assert_eq!(flags, [FLAG_CHUNK, FLAG_CHUNK, FLAG_LAST_CHUNK]);
    }

    #[test]
    fn both_directions_work() {
        let (mut device, mut host) = pair();
        assert!(device.is_initiator());
        assert!(!host.is_initiator());
        for round in 0..3u8 {
            let up = device.seal(&[round; 10]).unwrap();
            assert_eq!(host.open(&up[0]).unwrap(), Some(vec![round; 10]));
            let down = host.seal(&[round; 20]).unwrap();
            assert_eq!(device.open(&down[0]).unwrap(), Some(vec![round; 20]));
        }
    }

    #[test]
    fn size_limit_of_8_mib() {
        let (mut device, mut host) = pair();
        assert_eq!(
            device.seal(&vec![0u8; MAX_APP_MESSAGE + 1]).unwrap_err(),
            Error::TooLarge
        );
        // A rejected seal does not change the transport.
        assert!(!device.is_poisoned());
        let largest = pattern(MAX_APP_MESSAGE);
        let sealed = device.seal(&largest).unwrap();
        assert_eq!(deliver(&mut host, &sealed), largest);

        // A peer that sends more than 8 MiB in chunks ends the transport.
        let chunk = [&[FLAG_CHUNK][..], &[0u8; MAX_CHUNK_LEN][..]].concat();
        let mut result = Ok(None);
        for _ in 0..=MAX_APP_MESSAGE / MAX_CHUNK_LEN {
            let message = raw(&mut device, &chunk);
            result = host.open(&message);
            if result.is_err() {
                break;
            }
        }
        assert_eq!(result.unwrap_err(), Error::TooLarge);
        assert!(host.is_poisoned());
    }

    #[test]
    fn changed_message_fails_and_poisons() {
        let (mut device, mut host) = pair();
        let mut sealed = device.seal(b"hello").unwrap();
        sealed[0][3] ^= 0x40;
        assert_eq!(host.open(&sealed[0]).unwrap_err(), Error::Decrypt);
        assert!(host.is_poisoned());
        // A later valid message also fails.
        let next = device.seal(b"next").unwrap();
        assert_eq!(host.open(&next[0]).unwrap_err(), Error::Poisoned);
        assert_eq!(host.seal(b"reply").unwrap_err(), Error::Poisoned);
    }

    #[test]
    fn replayed_message_fails() {
        let (mut device, mut host) = pair();
        let sealed = device.seal(b"once").unwrap();
        assert_eq!(host.open(&sealed[0]).unwrap(), Some(b"once".to_vec()));
        assert_eq!(host.open(&sealed[0]).unwrap_err(), Error::Decrypt);
        assert!(host.is_poisoned());
    }

    #[test]
    fn reordered_messages_fail() {
        let (mut device, mut host) = pair();
        let first = device.seal(b"first").unwrap();
        let second = device.seal(b"second").unwrap();
        assert_eq!(host.open(&second[0]).unwrap_err(), Error::Decrypt);
        assert_eq!(host.open(&first[0]).unwrap_err(), Error::Poisoned);
    }

    #[test]
    fn reordered_chunks_fail() {
        let (mut device, mut host) = pair();
        let sealed = device.seal(&pattern(3 * MAX_CHUNK_LEN)).unwrap();
        assert_eq!(host.open(&sealed[0]).unwrap(), None);
        assert_eq!(host.open(&sealed[2]).unwrap_err(), Error::Decrypt);
    }

    #[test]
    fn lost_message_fails() {
        let (mut device, mut host) = pair();
        let _lost = device.seal(b"lost").unwrap();
        let next = device.seal(b"next").unwrap();
        assert_eq!(host.open(&next[0]).unwrap_err(), Error::Decrypt);
    }

    #[test]
    fn truncated_messages_fail() {
        let (mut device, mut host) = pair();
        let sealed = device.seal(b"truncate me").unwrap();
        let cut = &sealed[0][..sealed[0].len() - 1];
        assert_eq!(host.open(cut).unwrap_err(), Error::Decrypt);

        let (mut device, mut host) = pair();
        let sealed = device.seal(b"x").unwrap();
        assert_eq!(host.open(&sealed[0][..10]).unwrap_err(), Error::Decrypt);

        let (_, mut host) = pair();
        assert_eq!(host.open(&[]).unwrap_err(), Error::Decrypt);

        let (_, mut host) = pair();
        assert!(matches!(
            host.open(&vec![0u8; MAX_NOISE_MESSAGE + 1]).unwrap_err(),
            Error::Framing(_)
        ));
        assert!(host.is_poisoned());
    }

    #[test]
    fn framing_errors_poison() {
        let cases: [&[u8]; 4] = [&[], &[0x03, 1, 2], &[FLAG_LAST_CHUNK, 1, 2], &[0xff]];
        for plaintext in cases {
            let (mut device, mut host) = pair();
            let message = raw(&mut device, plaintext);
            assert!(
                matches!(host.open(&message).unwrap_err(), Error::Framing(_)),
                "plaintext {plaintext:?}"
            );
            assert!(host.is_poisoned());
        }

        // A complete message inside a chunked message is a framing error.
        let (mut device, mut host) = pair();
        let chunk = raw(&mut device, &[FLAG_CHUNK, 1]);
        let complete = raw(&mut device, &[FLAG_COMPLETE, 2]);
        assert_eq!(host.open(&chunk).unwrap(), None);
        assert!(matches!(
            host.open(&complete).unwrap_err(),
            Error::Framing(_)
        ));
    }

    #[test]
    fn small_and_empty_chunks_join() {
        let (mut device, mut host) = pair();
        for plaintext in [
            &[FLAG_CHUNK, 1, 2][..],
            &[FLAG_CHUNK][..],
            &[FLAG_LAST_CHUNK, 3][..],
        ] {
            let message = raw(&mut device, plaintext);
            let opened = host.open(&message).unwrap();
            if plaintext[0] == FLAG_LAST_CHUNK {
                assert_eq!(opened, Some(vec![1, 2, 3]));
            } else {
                assert_eq!(opened, None);
            }
        }
        let empty_last = [
            raw(&mut device, &[FLAG_CHUNK]),
            raw(&mut device, &[FLAG_LAST_CHUNK]),
        ];
        assert_eq!(host.open(&empty_last[0]).unwrap(), None);
        assert_eq!(host.open(&empty_last[1]).unwrap(), Some(Vec::new()));
    }

    #[test]
    fn debug_output_has_no_content() {
        let (mut device, mut host) = pair();
        let sealed = device.seal(&pattern(MAX_CHUNK_LEN + 5)).unwrap();
        assert_eq!(host.open(&sealed[0]).unwrap(), None);
        let debug = format!("{host:?}");
        assert!(debug.contains("partial_len: Some(65518)"));
        assert!(!debug.contains("[0, 1, 2"));
    }
}
