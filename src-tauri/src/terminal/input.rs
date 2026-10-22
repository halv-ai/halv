//! Ordered terminal input.
//!
//! `terminal_write` is an async command. Tauri runs each call as its own task,
//! so two calls can reach the backend in either order. Each session gets one
//! input queue: a channel that one writer thread drains in order.
//!
//! A pane tags its writes with a writer id and a sequence number. The queue
//! writes the input of each writer in sequence order. It drops a sequence that
//! it already has, for example when Tauri sends a call again after a failed
//! IPC fetch. A call that arrives after the gap wait is still user input, so
//! the queue writes it late instead of dropping it. The writer thread uses its
//! own supervisor connection, so input never waits while a close or a
//! supervisor start holds the shared one.

use std::{
    collections::{BTreeMap, HashMap},
    panic::{self, AssertUnwindSafe},
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread,
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

/// How long a write waits for an earlier sequence of the same writer. Calls
/// usually arrive microseconds apart, but a resent IPC call or a busy async
/// runtime can hold one call back for longer. The wait ends when the missing
/// call arrives, so only a call that never arrives costs the full wait.
const SEQUENCE_GAP_WAIT: Duration = Duration::from_secs(2);
/// The writers that one session remembers. A pane gets a new writer id each
/// time that it mounts.
const MAX_WRITERS: usize = 16;
/// The skipped gaps that a writer remembers, so that a late call still goes
/// out once. One gap holds any number of sequences. Past this limit the writer
/// forgets its oldest gap.
const MAX_SKIPPED_GAPS: usize = 64;

pub(crate) type InputResult = Result<(), String>;

/// Writes one chunk of input to the session. Only the writer thread calls it.
pub(crate) type InputSink = Box<dyn FnMut(&str) -> InputResult + Send>;

/// The place of a write in the input stream of one pane. Sequences start at 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InputOrder {
    pub(crate) writer: String,
    pub(crate) sequence: u64,
}

pub(crate) struct InputRequest {
    pub(crate) data: String,
    pub(crate) order: Option<InputOrder>,
    pub(crate) done: oneshot::Sender<InputResult>,
}

impl InputRequest {
    fn answer(self, result: InputResult) {
        // The caller can stop waiting, for example when the webview reloads.
        let _ = self.done.send(result);
    }
}

/// The input queue of one session.
pub(crate) struct InputQueue {
    requests: Sender<InputRequest>,
}

impl InputQueue {
    pub(crate) fn start(sink: InputSink) -> Result<Self, String> {
        let (requests, received) = mpsc::channel();
        thread::Builder::new()
            .name("halv-terminal-input".to_string())
            .spawn(move || write_requests(received, sink))
            .map_err(|error| format!("failed to start the terminal input writer: {error}"))?;
        Ok(Self { requests })
    }

    /// Queues a request. A stopped writer thread gives the request back.
    pub(crate) fn push(&self, request: InputRequest) -> Result<(), InputRequest> {
        self.requests.send(request).map_err(|error| error.0)
    }

    /// A queue whose writer thread already stopped.
    #[cfg(test)]
    pub(crate) fn stopped() -> Self {
        let (requests, _) = mpsc::channel();
        Self { requests }
    }
}

fn write_requests(received: Receiver<InputRequest>, mut sink: InputSink) {
    let mut sequencer = InputSequencer::default();
    loop {
        let next = match sequencer.next_deadline() {
            Some(deadline) => {
                received.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            }
            None => received.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        let ready = match next {
            Ok(request) => sequencer.accept(request, Instant::now()),
            Err(RecvTimeoutError::Timeout) => sequencer.expire(Instant::now()),
            Err(RecvTimeoutError::Disconnected) => {
                // The session closed. Finish the writes that still wait.
                for request in sequencer.drain() {
                    write_request(&mut sink, request);
                }
                return;
            }
        };
        for request in ready {
            write_request(&mut sink, request);
        }
    }
}

fn write_request(sink: &mut InputSink, request: InputRequest) {
    // A panic fails this write only. The thread stays alive for the next one.
    let result = panic::catch_unwind(AssertUnwindSafe(|| sink(&request.data)))
        .unwrap_or_else(|_| Err("failed to write terminal input".to_string()));
    request.answer(result);
}

/// Puts the writes of each writer in sequence order. Writes without an order
/// go out in arrival order.
#[derive(Default)]
struct InputSequencer {
    writers: HashMap<String, WriterState>,
    uses: u64,
}

struct WriterState {
    next: u64,
    pending: BTreeMap<u64, PendingWrite>,
    /// The gaps that the writer stopped waiting for, in sequence order.
    skipped: Vec<SkippedGap>,
    /// The writer forgot the gaps below this sequence.
    forgotten_below: u64,
    last_use: u64,
}

/// The missing sequences `start..end` that a writer stopped waiting for.
#[derive(Clone, Copy)]
struct SkippedGap {
    start: u64,
    end: u64,
    skipped_at: Instant,
}

/// What a sequence below the next expected one is.
enum EarlierWrite {
    /// A call that arrived after the gap wait. It holds the time of the skip.
    Late(Instant),
    /// A call from a forgotten gap. It can also be a repeat.
    Unknown,
    /// A copy of a sequence that is already written.
    Repeat,
}

struct PendingWrite {
    arrived: Instant,
    request: InputRequest,
}

impl WriterState {
    fn new() -> Self {
        Self {
            next: 1,
            pending: BTreeMap::new(),
            skipped: Vec::new(),
            forgotten_below: 0,
            last_use: 0,
        }
    }

    fn take_ready(&mut self, ready: &mut Vec<InputRequest>) {
        while let Some(write) = self.pending.remove(&self.next) {
            ready.push(write.request);
            self.next += 1;
        }
    }

    /// Gives up on the missing sequences below the first pending write. They
    /// become one gap, so a large jump (a new queue for a pane far into its
    /// stream) costs no more than a small one.
    fn skip_to(&mut self, sequence: u64, now: Instant) {
        if sequence <= self.next {
            return;
        }
        self.skipped.push(SkippedGap {
            start: self.next,
            end: sequence,
            skipped_at: now,
        });
        self.forget_old_gaps();
        self.next = sequence;
    }

    /// Finds out what an earlier sequence is. A late sequence leaves its gap,
    /// so a second copy of it counts as a repeat.
    fn take_earlier(&mut self, sequence: u64) -> EarlierWrite {
        let index = self.skipped.partition_point(|gap| gap.start <= sequence);
        let Some(index) = index
            .checked_sub(1)
            .filter(|&index| sequence < self.skipped[index].end)
        else {
            if sequence < self.forgotten_below {
                return EarlierWrite::Unknown;
            }
            return EarlierWrite::Repeat;
        };
        let gap = self.skipped[index];
        let parts = [
            SkippedGap {
                end: sequence,
                ..gap
            },
            SkippedGap {
                start: sequence + 1,
                ..gap
            },
        ];
        self.skipped.splice(
            index..=index,
            parts.into_iter().filter(|part| part.start < part.end),
        );
        self.forget_old_gaps();
        EarlierWrite::Late(gap.skipped_at)
    }

    fn forget_old_gaps(&mut self) {
        let excess = self.skipped.len().saturating_sub(MAX_SKIPPED_GAPS);
        if let Some(oldest) = self.skipped.drain(..excess).next_back() {
            self.forgotten_below = oldest.end;
        }
    }

    fn deadline(&self) -> Option<Instant> {
        self.pending
            .values()
            .next()
            .map(|write| write.arrived + SEQUENCE_GAP_WAIT)
    }
}

impl InputSequencer {
    /// Accepts one request. Returns the requests to write now, in order.
    fn accept(&mut self, request: InputRequest, now: Instant) -> Vec<InputRequest> {
        let Some(order) = request.order.clone() else {
            return vec![request];
        };
        self.uses += 1;
        if !self.writers.contains_key(&order.writer) {
            self.forget_an_idle_writer();
            self.writers
                .insert(order.writer.clone(), WriterState::new());
        }
        let writer = self
            .writers
            .get_mut(&order.writer)
            .expect("the writer state exists");
        writer.last_use = self.uses;
        if writer.pending.contains_key(&order.sequence) {
            // A repeated call. The first copy waits.
            request.answer(Ok(()));
            return Vec::new();
        }
        if order.sequence < writer.next {
            match writer.take_earlier(order.sequence) {
                EarlierWrite::Late(skipped_at) => {
                    // The later writes already went out, but this is still
                    // user input, so it goes out now.
                    eprintln!(
                        "terminal input: write {} of writer {} arrived {} ms after \
                         the gap wait ended; it goes out after the later writes",
                        order.sequence,
                        order.writer,
                        now.saturating_duration_since(skipped_at).as_millis()
                    );
                    return vec![request];
                }
                EarlierWrite::Unknown => {
                    // Writing a possible repeat is better than losing input.
                    eprintln!(
                        "terminal input: write {} of writer {} is older than the \
                         gaps that the queue remembers; it goes out",
                        order.sequence, order.writer
                    );
                    return vec![request];
                }
                EarlierWrite::Repeat => {
                    // A repeated call. The first copy is written.
                    request.answer(Ok(()));
                    return Vec::new();
                }
            }
        }
        writer.pending.insert(
            order.sequence,
            PendingWrite {
                arrived: now,
                request,
            },
        );
        let mut ready = Vec::new();
        writer.take_ready(&mut ready);
        ready
    }

    /// Writes past the sequences that waited longer than the gap wait.
    fn expire(&mut self, now: Instant) -> Vec<InputRequest> {
        let mut ready = Vec::new();
        for (id, writer) in &mut self.writers {
            while writer.deadline().is_some_and(|deadline| deadline <= now) {
                let first = *writer
                    .pending
                    .keys()
                    .next()
                    .expect("a deadline needs a pending write");
                eprintln!(
                    "terminal input: writes {}..{first} of writer {id} did not arrive \
                     in {SEQUENCE_GAP_WAIT:?}; the later writes go out first",
                    writer.next
                );
                writer.skip_to(first, now);
                writer.take_ready(&mut ready);
            }
        }
        ready
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.writers
            .values()
            .filter_map(WriterState::deadline)
            .min()
    }

    /// Returns every pending write, in sequence order for each writer.
    fn drain(&mut self) -> Vec<InputRequest> {
        self.writers
            .drain()
            .flat_map(|(_, writer)| writer.pending.into_values())
            .map(|write| write.request)
            .collect()
    }

    fn forget_an_idle_writer(&mut self) {
        if self.writers.len() < MAX_WRITERS {
            return;
        }
        let idle = self
            .writers
            .iter()
            .filter(|(_, writer)| writer.pending.is_empty())
            .min_by_key(|(_, writer)| writer.last_use)
            .map(|(id, _)| id.clone());
        if let Some(id) = idle {
            self.writers.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn request(
        data: &str,
        order: Option<(&str, u64)>,
    ) -> (InputRequest, oneshot::Receiver<InputResult>) {
        let (done, answer) = oneshot::channel();
        (
            InputRequest {
                data: data.to_string(),
                order: order.map(|(writer, sequence)| InputOrder {
                    writer: writer.to_string(),
                    sequence,
                }),
                done,
            },
            answer,
        )
    }

    fn data(requests: Vec<InputRequest>) -> Vec<String> {
        requests.into_iter().map(|request| request.data).collect()
    }

    #[test]
    fn writes_of_one_writer_go_out_in_sequence_order() {
        let mut sequencer = InputSequencer::default();
        let now = Instant::now();

        let (third, _) = request("c", Some(("pane", 3)));
        assert!(sequencer.accept(third, now).is_empty());
        let (first, _) = request("a", Some(("pane", 1)));
        assert_eq!(data(sequencer.accept(first, now)), ["a"]);
        let (second, _) = request("b", Some(("pane", 2)));
        assert_eq!(data(sequencer.accept(second, now)), ["b", "c"]);
        assert_eq!(sequencer.next_deadline(), None);
    }

    #[test]
    fn a_repeated_sequence_is_answered_but_not_written_again() {
        let mut sequencer = InputSequencer::default();
        let now = Instant::now();

        let (first, _) = request("é", Some(("pane", 1)));
        assert_eq!(data(sequencer.accept(first, now)), ["é"]);
        let (repeat, mut answer) = request("é", Some(("pane", 1)));
        assert!(sequencer.accept(repeat, now).is_empty());
        assert_eq!(answer.try_recv().unwrap(), Ok(()));

        // A repeat of a write that still waits is dropped too.
        let (third, _) = request("o", Some(("pane", 3)));
        assert!(sequencer.accept(third, now).is_empty());
        let (third_again, mut third_answer) = request("o", Some(("pane", 3)));
        assert!(sequencer.accept(third_again, now).is_empty());
        assert_eq!(third_answer.try_recv().unwrap(), Ok(()));
        let (second, _) = request("a", Some(("pane", 2)));
        assert_eq!(data(sequencer.accept(second, now)), ["a", "o"]);
    }

    #[test]
    fn a_missing_sequence_stops_the_wait_after_the_gap_wait() {
        let mut sequencer = InputSequencer::default();
        let now = Instant::now();

        let (second, _) = request("b", Some(("pane", 2)));
        assert!(sequencer.accept(second, now).is_empty());
        assert_eq!(sequencer.next_deadline(), Some(now + SEQUENCE_GAP_WAIT));
        assert!(sequencer
            .expire(now + SEQUENCE_GAP_WAIT - Duration::from_millis(1))
            .is_empty());
        assert_eq!(data(sequencer.expire(now + SEQUENCE_GAP_WAIT)), ["b"]);
        assert_eq!(sequencer.next_deadline(), None);
    }

    #[test]
    fn a_write_that_arrives_after_the_gap_wait_still_goes_out() {
        let mut sequencer = InputSequencer::default();
        let now = Instant::now();
        let late_time = now + SEQUENCE_GAP_WAIT * 2;

        let (first, _) = request("a", Some(("pane", 1)));
        assert_eq!(data(sequencer.accept(first, now)), ["a"]);
        let (third, _) = request("c", Some(("pane", 3)));
        assert!(sequencer.accept(third, now).is_empty());
        assert_eq!(data(sequencer.expire(now + SEQUENCE_GAP_WAIT)), ["c"]);

        // The call for 2 was slow, not lost. The user typed it, so it goes
        // out after the later writes and its caller gets the write result.
        let (late, answer) = request("b", Some(("pane", 2)));
        let ready = sequencer.accept(late, late_time);
        let (mut sink, written) = collecting_sink();
        for request in ready {
            write_request(&mut sink, request);
        }
        assert_eq!(*written.lock().unwrap(), ["b"]);
        assert_eq!(answer.blocking_recv().unwrap(), Ok(()));

        // A repeat of the late call is not written again.
        let (repeat, mut repeat_answer) = request("b", Some(("pane", 2)));
        assert!(sequencer.accept(repeat, late_time).is_empty());
        assert_eq!(repeat_answer.try_recv().unwrap(), Ok(()));
        let (fourth, _) = request("d", Some(("pane", 4)));
        assert_eq!(data(sequencer.accept(fourth, late_time)), ["d"]);
    }

    #[test]
    fn a_new_queue_for_a_writer_far_into_its_stream_loses_nothing() {
        // A replaced queue, or a forgotten writer, meets a pane that already
        // sent many writes. The skip must not walk every missing sequence.
        let mut sequencer = InputSequencer::default();
        let now = Instant::now();
        let later = now + SEQUENCE_GAP_WAIT;
        let resumed: u64 = 1 << 40;

        let (write, _) = request("x", Some(("pane", resumed)));
        assert!(sequencer.accept(write, now).is_empty());
        assert_eq!(data(sequencer.expire(later)), ["x"]);
        let (next, _) = request("y", Some(("pane", resumed + 1)));
        assert_eq!(data(sequencer.accept(next, later)), ["y"]);

        // A write that was on its way when the queue changed still goes out.
        let (in_flight, _) = request("w", Some(("pane", resumed - 1)));
        assert_eq!(data(sequencer.accept(in_flight, later)), ["w"]);
        assert_eq!(sequencer.writers["pane"].skipped.len(), 1);
    }

    #[test]
    fn every_late_write_behind_one_long_gap_goes_out_once() {
        // A hundred calls fall behind one expired gap. That is more calls than
        // the queue could remember one by one, and each one is written once.
        let mut sequencer = InputSequencer::default();
        let now = Instant::now();
        let late_time = now + SEQUENCE_GAP_WAIT * 2;
        let last: u64 = 102;

        let (first, _) = request("1", Some(("pane", 1)));
        assert_eq!(data(sequencer.accept(first, now)), ["1"]);
        let (after, _) = request("102", Some(("pane", last)));
        assert!(sequencer.accept(after, now).is_empty());
        assert_eq!(data(sequencer.expire(now + SEQUENCE_GAP_WAIT)), ["102"]);

        // The late calls race, so they arrive in a mixed order.
        let arrivals = [52]
            .into_iter()
            .chain((2..last).filter(|sequence| sequence % 2 == 1).rev())
            .chain((2..last).filter(|sequence| sequence % 2 == 0 && *sequence != 52));
        for sequence in arrivals {
            let (late, _) = request(&sequence.to_string(), Some(("pane", sequence)));
            assert_eq!(
                data(sequencer.accept(late, late_time)),
                [sequence.to_string()],
                "late write {sequence}"
            );
        }
        assert!(sequencer.writers["pane"].skipped.is_empty());

        // A second copy of any of them is a repeat.
        for sequence in 1..=last {
            let (repeat, mut answer) = request("again", Some(("pane", sequence)));
            assert!(sequencer.accept(repeat, late_time).is_empty());
            assert_eq!(answer.try_recv().unwrap(), Ok(()));
        }
    }

    #[test]
    fn a_late_write_older_than_every_remembered_gap_still_goes_out() {
        let mut sequencer = InputSequencer::default();
        let mut now = Instant::now();
        let (first, _) = request("1", Some(("pane", 1)));
        assert_eq!(data(sequencer.accept(first, now)), ["1"]);

        // Every other call is slow, so each slow call leaves its own gap.
        let gaps = MAX_SKIPPED_GAPS as u64 + 5;
        for gap in 0..gaps {
            let (write, _) = request("x", Some(("pane", 3 + 2 * gap)));
            assert!(sequencer.accept(write, now).is_empty());
            now += SEQUENCE_GAP_WAIT;
            assert_eq!(sequencer.expire(now).len(), 1);
        }

        // The queue forgot the oldest gap, but its late call still goes out.
        let (oldest, _) = request("2", Some(("pane", 2)));
        assert_eq!(data(sequencer.accept(oldest, now)), ["2"]);
        let (newest, _) = request("n", Some(("pane", 2 * gaps)));
        assert_eq!(data(sequencer.accept(newest, now)), ["n"]);
        assert!(sequencer.writers["pane"].skipped.len() <= MAX_SKIPPED_GAPS);
    }

    #[test]
    fn writers_keep_separate_orders_and_unordered_writes_pass_at_once() {
        let mut sequencer = InputSequencer::default();
        let now = Instant::now();

        let (left_second, _) = request("L2", Some(("left", 2)));
        assert!(sequencer.accept(left_second, now).is_empty());
        let (right_first, _) = request("R1", Some(("right", 1)));
        assert_eq!(data(sequencer.accept(right_first, now)), ["R1"]);
        let (prompt, _) = request("prompt", None);
        assert_eq!(data(sequencer.accept(prompt, now)), ["prompt"]);
        let (left_first, _) = request("L1", Some(("left", 1)));
        assert_eq!(data(sequencer.accept(left_first, now)), ["L1", "L2"]);
    }

    #[test]
    fn an_idle_writer_is_forgotten_when_too_many_writers_exist() {
        let mut sequencer = InputSequencer::default();
        let now = Instant::now();
        for pane in 0..MAX_WRITERS + 4 {
            let (write, _) = request("x", Some((&format!("pane-{pane}"), 1)));
            assert_eq!(sequencer.accept(write, now).len(), 1);
        }
        assert_eq!(sequencer.writers.len(), MAX_WRITERS);
        assert!(sequencer
            .writers
            .contains_key(&format!("pane-{}", MAX_WRITERS + 3)));
        assert!(!sequencer.writers.contains_key("pane-0"));
    }

    fn collecting_sink() -> (InputSink, Arc<Mutex<Vec<String>>>) {
        let written = Arc::new(Mutex::new(Vec::new()));
        let sink_written = written.clone();
        let sink: InputSink = Box::new(move |data: &str| {
            sink_written.lock().unwrap().push(data.to_string());
            Ok(())
        });
        (sink, written)
    }

    #[test]
    fn many_concurrent_writes_arrive_in_order() {
        const WRITES: u64 = 1_000;
        const PRODUCERS: u64 = 8;
        let (sink, written) = collecting_sink();
        let queue = Arc::new(InputQueue::start(sink).unwrap());
        let start = Arc::new(std::sync::Barrier::new(PRODUCERS as usize));

        // Each producer sends every eighth sequence, so the threads race and
        // the queue receives the sequences out of order.
        let producers = (0..PRODUCERS)
            .map(|producer| {
                let queue = queue.clone();
                let start = start.clone();
                thread::spawn(move || {
                    start.wait();
                    let mut answers = Vec::new();
                    for sequence in (1..=WRITES).filter(|value| value % PRODUCERS == producer) {
                        let (write, answer) =
                            request(&format!("{sequence:04}|"), Some(("pane", sequence)));
                        assert!(queue.push(write).is_ok());
                        answers.push(answer);
                    }
                    answers
                })
            })
            .collect::<Vec<_>>();
        for producer in producers {
            for answer in producer.join().unwrap() {
                assert_eq!(answer.blocking_recv().unwrap(), Ok(()));
            }
        }

        let expected = (1..=WRITES)
            .map(|sequence| format!("{sequence:04}|"))
            .collect::<Vec<_>>();
        assert_eq!(*written.lock().unwrap(), expected);
    }

    #[test]
    fn a_repeated_call_reaches_the_sink_once() {
        let (sink, written) = collecting_sink();
        let queue = InputQueue::start(sink).unwrap();
        let mut answers = Vec::new();
        for (text, sequence) in [("Oi ", 1), ("é", 2), ("é", 2), (" isso", 3)] {
            let (write, answer) = request(text, Some(("pane", sequence)));
            assert!(queue.push(write).is_ok());
            answers.push(answer);
        }
        for answer in answers {
            assert_eq!(answer.blocking_recv().unwrap(), Ok(()));
        }
        assert_eq!(*written.lock().unwrap(), ["Oi ", "é", " isso"]);
    }

    #[test]
    fn a_panicking_write_fails_only_that_write() {
        let mut calls = 0;
        let sink: InputSink = Box::new(move |_: &str| {
            calls += 1;
            assert!(calls > 1, "the first write panics");
            Ok(())
        });
        let queue = InputQueue::start(sink).unwrap();
        let (first, first_answer) = request("a", None);
        assert!(queue.push(first).is_ok());
        assert!(first_answer.blocking_recv().unwrap().is_err());
        let (second, second_answer) = request("b", None);
        assert!(queue.push(second).is_ok());
        assert_eq!(second_answer.blocking_recv().unwrap(), Ok(()));
    }

    #[test]
    fn a_sink_error_reaches_only_its_caller() {
        let mut calls = 0;
        let sink: InputSink = Box::new(move |_: &str| {
            calls += 1;
            if calls == 1 {
                Err("terminal session is not running".to_string())
            } else {
                Ok(())
            }
        });
        let queue = InputQueue::start(sink).unwrap();
        let (first, first_answer) = request("a", Some(("pane", 1)));
        let (second, second_answer) = request("b", Some(("pane", 2)));
        assert!(queue.push(first).is_ok());
        assert!(queue.push(second).is_ok());
        assert_eq!(
            first_answer.blocking_recv().unwrap(),
            Err("terminal session is not running".to_string())
        );
        assert_eq!(second_answer.blocking_recv().unwrap(), Ok(()));
    }
}
