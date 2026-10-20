//! The terminal streaming rules of one subscription (section 8.8).
//!
//! - Batching: a `terminal.data` event goes out 50 ms after the first collected byte,
//!   or when the batch holds 32 KiB. A chunk is never split, so the `seq` of an event
//!   is always the sequence number of its last complete chunk.
//! - Flow control: the host counts the bytes that it sent after the last acknowledged `seq`.
//!   Above 512 KiB it stops the data. When an ack brings the count below 128 KiB,
//!   the host sends a new snapshot and continues.
//! - A snapshot starts a new window. Its own bytes do not count, because a snapshot
//!   larger than 512 KiB would otherwise stop the data at once and ask for the next snapshot.
//!   The count holds only the `terminal.data` bytes after the snapshot.
//!
//! This module has no I/O. The connection task sends the events that it returns.

use std::collections::VecDeque;
use std::time::Duration;

use tokio::time::Instant;

use crate::rpc::{
    TerminalData, FLOW_PAUSE_BYTES, FLOW_RESUME_BYTES, OUTPUT_BATCH_BYTES, OUTPUT_BATCH_DELAY_MS,
};

/// The batching delay of section 8.8.
pub(crate) const BATCH_DELAY: Duration = Duration::from_millis(OUTPUT_BATCH_DELAY_MS);

/// What [`Subscription::push`] did with a chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Push {
    /// The chunk is in the batch. The batch goes out at its deadline.
    Collected,
    /// The batch holds 32 KiB or more. Send it now.
    FlushNow,
    /// The subscription already has the chunk, or the data is stopped.
    Dropped,
}

/// The streaming state of one terminal subscription.
#[derive(Debug)]
pub(crate) struct Subscription {
    /// The subscription id of the session.
    pub(crate) id: u32,
    /// The terminal session id of the pane.
    pub(crate) session: String,
    /// The sequence number of the newest chunk that the device has, or that the host skipped.
    last_seq: u64,
    batch: Vec<u8>,
    batch_seq: u64,
    batch_deadline: Option<Instant>,
    /// The sent `terminal.data` events that the device did not acknowledge: `(seq, bytes)`.
    in_flight: VecDeque<(u64, usize)>,
    unacked: usize,
    paused: bool,
}

impl Subscription {
    /// Makes a subscription. The device already has every chunk up to `last_seq`.
    pub(crate) fn new(id: u32, session: String, last_seq: u64) -> Subscription {
        Subscription {
            id,
            session,
            last_seq,
            batch: Vec::new(),
            batch_seq: 0,
            batch_deadline: None,
            in_flight: VecDeque::new(),
            unacked: 0,
            paused: false,
        }
    }

    /// Adds a chunk of live output or of a replay.
    pub(crate) fn push(&mut self, seq: u64, data: &[u8], now: Instant) -> Push {
        if seq <= self.last_seq {
            return Push::Dropped;
        }
        self.last_seq = seq;
        if self.paused {
            return Push::Dropped;
        }
        if self.batch.is_empty() {
            self.batch_deadline = Some(now + BATCH_DELAY);
        }
        self.batch.extend_from_slice(data);
        self.batch_seq = seq;
        if self.batch.len() >= OUTPUT_BATCH_BYTES {
            Push::FlushNow
        } else {
            Push::Collected
        }
    }

    /// Takes the batch as a `terminal.data` event.
    ///
    /// It returns `None` for an empty batch, and when the data is stopped.
    /// When the unacknowledged bytes are above 512 KiB, it stops the data and drops the batch.
    pub(crate) fn take_batch(&mut self) -> Option<TerminalData> {
        self.batch_deadline = None;
        if self.batch.is_empty() {
            return None;
        }
        let data = std::mem::take(&mut self.batch);
        if self.paused {
            return None;
        }
        if self.unacked > FLOW_PAUSE_BYTES {
            self.paused = true;
            return None;
        }
        self.in_flight.push_back((self.batch_seq, data.len()));
        self.unacked += data.len();
        Some(TerminalData {
            sub: self.id,
            seq: self.batch_seq,
            data,
        })
    }

    /// Drops the batch, for example before a new snapshot.
    pub(crate) fn discard_batch(&mut self) {
        self.batch.clear();
        self.batch_deadline = None;
    }

    /// Records a snapshot that the host sent. The device resets its terminal and writes it.
    ///
    /// Live output up to `seq` is in the snapshot, so the subscription skips it.
    /// The snapshot replaces every earlier event, so the window starts again at zero.
    pub(crate) fn snapshot_sent(&mut self, seq: u64) {
        self.discard_batch();
        self.last_seq = seq;
        self.paused = false;
        self.in_flight.clear();
        self.unacked = 0;
    }

    /// Handles lost output of the tap. The batch is dropped, and the device needs a snapshot.
    ///
    /// It returns true when the window allows a snapshot now: the count is below 128 KiB.
    /// Otherwise the data stops, and the snapshot goes out when acks bring the count
    /// below 128 KiB. A stopped subscription already waits for that snapshot.
    pub(crate) fn lost_output(&mut self) -> bool {
        self.discard_batch();
        if self.paused {
            return false;
        }
        if self.unacked < FLOW_RESUME_BYTES {
            return true;
        }
        self.paused = true;
        false
    }

    /// Reads a `terminal.ack`. It returns true when the data was stopped and the count
    /// is now below 128 KiB: the caller then sends a new snapshot.
    pub(crate) fn ack(&mut self, seq: u64) -> bool {
        while let Some(&(sent_seq, len)) = self.in_flight.front() {
            if sent_seq > seq {
                break;
            }
            self.in_flight.pop_front();
            self.unacked -= len;
        }
        self.paused && self.unacked < FLOW_RESUME_BYTES
    }

    /// Returns the time when the batch must go out.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.batch_deadline
    }

    /// Returns true while the data is stopped.
    pub(crate) fn is_paused(&self) -> bool {
        self.paused
    }

    /// Returns the sequence number of the newest chunk that the subscription covers.
    #[cfg(test)]
    pub(crate) fn last_seq(&self) -> u64 {
        self.last_seq
    }

    /// Returns the bytes that the device did not acknowledge.
    #[cfg(test)]
    pub(crate) fn unacked(&self) -> usize {
        self.unacked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub() -> Subscription {
        Subscription::new(1, "term-1".to_owned(), 0)
    }

    #[test]
    fn small_chunks_wait_for_the_deadline() {
        let mut sub = sub();
        let now = Instant::now();
        assert_eq!(sub.push(1, b"ab", now), Push::Collected);
        assert_eq!(sub.deadline(), Some(now + BATCH_DELAY));
        // A later chunk does not move the deadline.
        assert_eq!(
            sub.push(2, b"cd", now + Duration::from_millis(20)),
            Push::Collected
        );
        assert_eq!(sub.deadline(), Some(now + BATCH_DELAY));
        let event = sub.take_batch().unwrap();
        assert_eq!(event.seq, 2);
        assert_eq!(event.data, b"abcd");
        assert_eq!(sub.deadline(), None);
        assert!(sub.take_batch().is_none());
    }

    #[test]
    fn a_full_batch_goes_out_at_once_and_keeps_chunks_whole() {
        let mut sub = sub();
        let now = Instant::now();
        let chunk = vec![b'x'; 20 * 1024];
        assert_eq!(sub.push(1, &chunk, now), Push::Collected);
        assert_eq!(sub.push(2, &chunk, now), Push::FlushNow);
        let event = sub.take_batch().unwrap();
        // The second chunk is not split, so the event holds 40 KiB with the seq of chunk 2.
        assert_eq!(event.data.len(), 40 * 1024);
        assert_eq!(event.seq, 2);
    }

    #[test]
    fn old_chunks_are_dropped() {
        let mut sub = Subscription::new(1, "term-1".to_owned(), 10);
        let now = Instant::now();
        assert_eq!(sub.push(9, b"old", now), Push::Dropped);
        assert_eq!(sub.push(10, b"old", now), Push::Dropped);
        assert_eq!(sub.push(11, b"new", now), Push::Collected);
        sub.snapshot_sent(20);
        assert_eq!(sub.push(15, b"in snapshot", now), Push::Dropped);
        assert!(sub.take_batch().is_none());
        assert_eq!(sub.push(21, b"after", now), Push::Collected);
        assert_eq!(sub.last_seq(), 21);
    }

    #[test]
    fn flow_control_pauses_and_resumes_with_a_snapshot() {
        let mut sub = sub();
        let now = Instant::now();
        let chunk = vec![0u8; 32 * 1024];
        let mut seq = 0;
        let mut sent = 0;
        // Send batches until the window stops the data.
        loop {
            seq += 1;
            assert_ne!(sub.push(seq, &chunk, now), Push::Dropped);
            match sub.take_batch() {
                Some(event) => sent += event.data.len(),
                None => break,
            }
        }
        assert!(sub.is_paused());
        assert!(sent > FLOW_PAUSE_BYTES && sent <= FLOW_PAUSE_BYTES + chunk.len());
        // While stopped, new output is dropped.
        assert_eq!(sub.push(seq + 1, &chunk, now), Push::Dropped);
        // An ack that leaves 128 KiB or more does not resume.
        let acked_seq = seq - 1 - (FLOW_RESUME_BYTES / chunk.len()) as u64;
        assert!(!sub.ack(acked_seq));
        assert!(sub.is_paused());
        // An ack below 128 KiB asks for a snapshot.
        assert!(sub.ack(seq));
        assert_eq!(sub.unacked(), 0);
        sub.snapshot_sent(seq + 1);
        assert!(!sub.is_paused());
        assert_eq!(sub.unacked(), 0);
        assert_eq!(sub.push(seq + 2, b"live", now), Push::Collected);
        assert!(sub.take_batch().is_some());
    }

    #[test]
    fn a_snapshot_above_the_window_does_not_stop_the_data() {
        let mut sub = sub();
        let now = Instant::now();
        // The host sends one snapshot of 1 MiB, then live output.
        sub.snapshot_sent(10);
        assert_eq!(sub.push(11, b"live", now), Push::Collected);
        let event = sub
            .take_batch()
            .expect("the data continues after the snapshot");
        assert_eq!(event.seq, 11);
        assert!(!sub.is_paused());
        // The ack of the snapshot asks for no new snapshot.
        assert!(!sub.ack(10));
        assert!(!sub.ack(11));
        assert_eq!(sub.unacked(), 0);
        // An ack from before the snapshot changes nothing.
        let mut sub = Subscription::new(1, "term-1".to_owned(), 0);
        sub.push(1, &[0u8; 100], now);
        sub.take_batch().unwrap();
        sub.snapshot_sent(5);
        assert!(!sub.ack(1));
        assert_eq!(sub.unacked(), 0);
    }

    #[test]
    fn lost_output_respects_the_window() {
        let now = Instant::now();
        // A small count: the snapshot can go out now.
        let mut sub = sub();
        sub.push(1, &[0u8; 1024], now);
        sub.take_batch().unwrap();
        sub.push(2, b"batch", now);
        assert!(sub.lost_output());
        assert!(sub.take_batch().is_none(), "the batch is dropped");
        assert!(!sub.is_paused());

        // A count of 128 KiB or more: the data stops until acks free the window.
        let mut sub = sub_with_unacked(FLOW_RESUME_BYTES);
        assert!(!sub.lost_output());
        assert!(sub.is_paused());
        assert_eq!(sub.push(100, b"lost", now), Push::Dropped);
        // A second loss sends nothing more.
        assert!(!sub.lost_output());
        // The ack that frees the window asks for the snapshot.
        assert!(sub.ack(99));
    }

    /// Makes a subscription with `bytes` unacknowledged bytes in batches of 32 KiB.
    fn sub_with_unacked(bytes: usize) -> Subscription {
        let mut sub = sub();
        let now = Instant::now();
        let mut seq = 0;
        while sub.unacked() < bytes {
            seq += 1;
            sub.push(seq, &vec![0u8; OUTPUT_BATCH_BYTES], now);
            sub.take_batch().unwrap();
        }
        sub
    }

    #[test]
    fn acks_release_only_acknowledged_events() {
        let mut sub = sub();
        let now = Instant::now();
        sub.push(1, &[0u8; 10], now);
        sub.take_batch().unwrap();
        sub.push(2, &[0u8; 20], now);
        sub.take_batch().unwrap();
        assert_eq!(sub.unacked(), 30);
        assert!(!sub.ack(1));
        assert_eq!(sub.unacked(), 20);
        // An old or repeated ack changes nothing.
        assert!(!sub.ack(1));
        assert_eq!(sub.unacked(), 20);
        assert!(!sub.ack(5));
        assert_eq!(sub.unacked(), 0);
    }
}
