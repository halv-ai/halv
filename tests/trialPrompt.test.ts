import assert from "node:assert/strict";
import test from "node:test";
import {
  createTerminalPromptSubmissionTracker,
  trialGateForSend,
} from "../src/trialPrompt.ts";

test("terminal input identifies only prompt submissions", () => {
  const tracker = createTerminalPromptSubmissionTracker();
  assert.equal(tracker.noteInput("hello"), false);
  assert.equal(tracker.noteInput("\r"), true);
  assert.equal(tracker.noteInput("\r"), false);
  assert.equal(tracker.noteInput("pasted\ntext"), true);
  assert.equal(tracker.noteInput("\n"), true);
});

test("terminal controls do not count as prompt submissions", () => {
  const tracker = createTerminalPromptSubmissionTracker();
  assert.equal(tracker.noteInput("\u001b[C"), false);
  assert.equal(tracker.noteInput("\u001b[<0;12;4M\r"), false);
  assert.equal(tracker.noteInput("\u001b]0;printable title\u0007\r"), false);
  assert.equal(tracker.noteInput("\u0003\t\r"), false);
});

test("terminal escape sequences can span input chunks", () => {
  const tracker = createTerminalPromptSubmissionTracker();
  assert.equal(tracker.noteInput("\u001b[<0;"), false);
  assert.equal(tracker.noteInput("12;4M"), false);
  assert.equal(tracker.noteInput("\r"), false);
  assert.equal(tracker.noteInput("hello"), false);
  assert.equal(tracker.noteInput("\r"), true);
});

test("a rejected submission keeps the draft across repeated Enter presses", () => {
  const tracker = createTerminalPromptSubmissionTracker();
  tracker.noteInput("Keep this draft");
  for (let attempt = 0; attempt < 3; attempt += 1) {
    assert.equal(tracker.noteInput("\r"), true);
    tracker.rejectSubmission();
    assert.equal(tracker.hasDraft(), true);
  }
  tracker.noteInput("\u0003");
  assert.equal(tracker.hasDraft(), false);
  assert.equal(tracker.noteInput("\r"), false);
});

test("history recall cannot bypass the next-send gate", () => {
  for (const history of ["\u001b[A", "\u0010"]) {
    const tracker = createTerminalPromptSubmissionTracker();
    assert.equal(tracker.noteInput(history), false);
    assert.equal(tracker.hasDraft(), true);
    assert.equal(tracker.noteInput("\r"), true);
  }
});

test("deleting or clearing a draft does not submit a prompt", () => {
  const tracker = createTerminalPromptSubmissionTracker();
  tracker.noteInput("ab\u007f\u007f");
  assert.equal(tracker.noteInput("\r"), false);
  tracker.noteInput("new\u0015");
  assert.equal(tracker.noteInput("\r"), false);
});

test("the trial gate opens only after value was delivered", () => {
  assert.equal(
    trialGateForSend({ signedIn: false, valueDelivered: true }),
    true,
  );
  assert.equal(
    trialGateForSend({ signedIn: false, valueDelivered: false }),
    false,
  );
});

test("the trial gate never blocks signed-in users", () => {
  assert.equal(
    trialGateForSend({ signedIn: true, valueDelivered: true }),
    false,
  );
  assert.equal(
    trialGateForSend({ signedIn: true, valueDelivered: false }),
    false,
  );
});
