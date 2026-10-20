//! Ordered terminal input.
//!
//! `terminal_write` is an async command. Tauri runs each call as its own task,
//! so two calls can reach the backend in either order. Each session gets one
//! input queue: a channel that one writer thread drains in order.
//!
//! A pane tags its writes with a writer id and a sequence number. The queue
//! writes the input of each writer in sequence order. It drops a sequence that
//! it already has, for example when Tauri sends a call again after a failed
//! IPC fetch. The writer thread uses its own supervisor connection, so input
//! never waits while a close or a supervisor start holds the shared one.

use std::{
    collections::{BTreeMap, HashMap},
    panic::{self, AssertUnwindSafe},
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread,
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

/// How long a write waits for an earlier sequence of the same writer. Calls
/// arrive microseconds apart, so a longer gap means that a call was lost.
const SEQUENCE_GAP_WAIT: Duration = Duration::from_millis(500);
/// The writers that one session remembers. A pane gets a new writer id each
/// time that it mounts.
const MAX_WRITERS: usize = 16;
/// The skipped sequences that a writer remembers, to report a late call.
const MAX_SKIPPED: usize = 64;

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
    skipped: Vec<u64>,
    last_use: u64,
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
            last_use: 0,
        }
    }

    fn take_ready(&mut self, ready: &mut Vec<InputRequest>) {
        while let Some(write) = self.pending.remove(&self.next) {
            ready.push(write.request);
            self.next += 1;
        }
    }

    /// Gives up on the missing sequences below the first pending write.
    fn skip_to(&mut self, sequence: u64) {
        for missing in self.next..sequence {
            if self.skipped.len() == MAX_SKIPPED {
                self.skipped.remove(0);
            }
            self.skipped.push(missing);
        }
        self.next = sequence;
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
        }
        let writer = self
            .writers
            .entry(order.writer)
            .or_insert_with(WriterState::new);
        writer.last_use = self.uses;
        if order.sequence < writer.next || writer.pending.contains_key(&order.sequence) {
            if writer.skipped.contains(&order.sequence) {
                request.answer(Err(
                    "terminal input arrived too late and was not sent".to_string()
                ));
            } else {
                // A repeated call. The first copy is written or waits.
                request.answer(Ok(()));
            }
            return Vec::new();
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
        for writer in self.writers.values_mut() {
            while writer.deadline().is_some_and(|deadline| deadline <= now) {
                let first = *writer
                    .pending
                    .keys()
                    .next()
                    .expect("a deadline needs a pending write");
                writer.skip_to(first);
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

        // The lost call arrives after all. It must not go out of order.
        let (late, mut answer) = request("a", Some(("pane", 1)));
        assert!(sequencer
            .accept(late, now + SEQUENCE_GAP_WAIT * 2)
            .is_empty());
        assert!(answer.try_recv().unwrap().is_err());
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
