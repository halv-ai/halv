import assert from "node:assert/strict";
import test from "node:test";
// @ts-expect-error Node's test runner needs the extension.
import {
  createTerminalActivityTracker,
  effectiveTerminalStatus,
  guidedTerminalTurnCompleted,
  TERMINAL_PROMPT_GRACE_MS,
  TERMINAL_RESPONSE_IDLE_MS,
  TERMINAL_TURN_GAP_MS,
} from "../src/terminalActivity.ts";

test("guided terminal task completes only after an active turn settles", () => {
  assert.equal(
    guidedTerminalTurnCompleted("waiting", true, true, "waiting_input"),
    true,
  );
  assert.equal(
    guidedTerminalTurnCompleted("waiting", true, true, "exited"),
    true,
  );
  assert.equal(
    guidedTerminalTurnCompleted("waiting", false, true, "waiting_input"),
    false,
  );
  assert.equal(
    guidedTerminalTurnCompleted("prepared", true, true, "waiting_input"),
    false,
  );
  assert.equal(
    guidedTerminalTurnCompleted("waiting", true, true, "running"),
    false,
  );
});

test("starting then waiting does not complete without response output", () => {
  const statuses = ["starting", "waiting_input"] as const;
  const sawRunning = statuses.some(
    (status) => (status as string) === "running",
  );

  assert.equal(
    guidedTerminalTurnCompleted(
      "waiting",
      sawRunning,
      false,
      statuses.at(-1) ?? null,
    ),
    false,
  );
  assert.equal(
    guidedTerminalTurnCompleted("waiting", true, false, "waiting_input"),
    false,
  );
});

test("terminal output settles from working to waiting for input", (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const statuses: Array<string | null> = [];
  const tracker = createTerminalActivityTracker((status) =>
    statuses.push(status),
  );

  tracker.noteOutput();
  assert.deepEqual(statuses, ["running"]);
  context.mock.timers.tick(TERMINAL_RESPONSE_IDLE_MS);
  assert.deepEqual(statuses, ["running", "waiting_input"]);
  tracker.dispose();
});

test("new output resets the response-settled timer", (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const statuses: Array<string | null> = [];
  const tracker = createTerminalActivityTracker(
    (status) => statuses.push(status),
    { idleMs: 15 },
  );

  tracker.noteOutput();
  context.mock.timers.tick(8);
  tracker.noteOutput();
  context.mock.timers.tick(8);
  assert.deepEqual(statuses, ["running", "running"]);
  context.mock.timers.tick(7);
  assert.deepEqual(statuses, ["running", "running", "waiting_input"]);
  tracker.dispose();
});

test("xterm Enter arms response tracking", (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const statuses: Array<string | null> = [];
  const tracker = createTerminalActivityTracker((status) =>
    statuses.push(status),
  );

  tracker.notePromptSubmitted();
  tracker.noteOutput();
  context.mock.timers.tick(TERMINAL_RESPONSE_IDLE_MS);
  assert.equal(statuses.at(-1), "responding");
  tracker.dispose();
});

test("submitting a prompt returns an idle terminal to working", (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const statuses: Array<string | null> = [];
  const tracker = createTerminalActivityTracker(
    (status) => statuses.push(status),
    { idleMs: 5 },
  );

  tracker.noteOutput();
  context.mock.timers.tick(5);
  tracker.notePromptSubmitted();
  assert.deepEqual(statuses, ["running", "waiting_input", "running"]);
  tracker.dispose();
});

test("a submitted prompt settles after the prompt grace period", (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const statuses: Array<string | null> = [];
  const tracker = createTerminalActivityTracker((status) =>
    statuses.push(status),
  );

  tracker.notePromptSubmitted();
  context.mock.timers.tick(TERMINAL_PROMPT_GRACE_MS - 1);
  assert.deepEqual(statuses, ["running"]);
  context.mock.timers.tick(1);
  assert.deepEqual(statuses, ["running", "waiting_input"]);
  tracker.dispose();
});

test("a short output pause does not settle an active turn", (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const statuses: Array<string | null> = [];
  const tracker = createTerminalActivityTracker((status) =>
    statuses.push(status),
  );

  tracker.notePromptSubmitted();
  tracker.noteOutput();
  context.mock.timers.tick(TERMINAL_RESPONSE_IDLE_MS);
  assert.deepEqual(statuses, ["running", "running", "responding"]);
  context.mock.timers.tick(TERMINAL_RESPONSE_IDLE_MS);
  assert.equal(statuses.at(-1), "responding");
  context.mock.timers.tick(TERMINAL_TURN_GAP_MS - TERMINAL_RESPONSE_IDLE_MS);
  assert.deepEqual(statuses, [
    "running",
    "running",
    "responding",
    "waiting_input",
  ]);
  tracker.dispose();
});

test("small periodic repaint bursts do not keep a completed turn running", (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const statuses: Array<string | null> = [];
  const tracker = createTerminalActivityTracker(
    (status) => statuses.push(status),
    { idleMs: 1_000, sustainedOutputBytes: 100 },
  );

  tracker.notePromptSubmitted();
  tracker.noteOutput(400);
  context.mock.timers.tick(1_000);
  context.mock.timers.tick(TERMINAL_TURN_GAP_MS);
  assert.equal(statuses.at(-1), "waiting_input");

  for (let index = 0; index < 5; index += 1) {
    tracker.noteOutput(10);
    context.mock.timers.tick(1_000);
  }
  assert.equal(statuses.at(-1), "waiting_input");
  assert.equal(statuses.filter((status) => status === "running").length, 2);
  tracker.dispose();
});

test("spinner repaints do not finish a turn before reply output", (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const statuses: Array<string | null> = [];
  const tracker = createTerminalActivityTracker(
    (status) => statuses.push(status),
    {
      idleMs: 1_000,
      promptGraceMs: 45_000,
      sustainedOutputBytes: 100,
    },
  );

  tracker.notePromptSubmitted();
  for (let second = 0; second < 30; second += 1) {
    tracker.noteOutput(10);
    context.mock.timers.tick(1_000);
    assert.equal(statuses.at(-1), "running");
    assert.equal(
      guidedTerminalTurnCompleted(
        "waiting",
        true,
        false,
        statuses.at(-1) ?? null,
      ),
      false,
    );
  }

  tracker.noteOutput(100);
  context.mock.timers.tick(1_000);
  assert.equal(statuses.at(-1), "responding");
  tracker.noteOutput(10);
  context.mock.timers.tick(TERMINAL_TURN_GAP_MS);
  assert.equal(statuses.at(-1), "waiting_input");
  assert.equal(
    guidedTerminalTurnCompleted(
      "waiting",
      true,
      true,
      statuses.at(-1) ?? null,
    ),
    true,
  );
  tracker.dispose();
});

test("large focus and resize redraws cannot restart a completed turn", (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const statuses: Array<string | null> = [];
  const tracker = createTerminalActivityTracker((status) => statuses.push(status));
  tracker.notePromptSubmitted();
  tracker.noteOutput(2_000);
  context.mock.timers.tick(TERMINAL_RESPONSE_IDLE_MS);
  context.mock.timers.tick(TERMINAL_TURN_GAP_MS);
  assert.equal(statuses.at(-1), "waiting_input");
  const completed = [...statuses];
  for (let i = 0; i < 5; i++) {
    tracker.noteOutput(40_000);
    context.mock.timers.tick(TERMINAL_TURN_GAP_MS);
  }
  assert.deepEqual(statuses, completed);
  tracker.notePromptSubmitted();
  assert.equal(statuses.at(-1), "running");
  tracker.dispose();
});

test("response bursts across quiet samples keep one active turn", (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const statuses: Array<string | null> = [];
  const tracker = createTerminalActivityTracker((status) => statuses.push(status));
  tracker.notePromptSubmitted();
  for (let burst = 0; burst < 5; burst++) {
    tracker.noteOutput(2_000);
    context.mock.timers.tick(TERMINAL_RESPONSE_IDLE_MS);
    context.mock.timers.tick(4_000);
    assert.ok(!statuses.includes("waiting_input"));
  }
  context.mock.timers.tick(TERMINAL_TURN_GAP_MS);
  assert.equal(statuses.filter((status) => status === "waiting_input").length, 1);
  tracker.dispose();
});

test("Codex transcript lifecycle replaces quiet completion inference", (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const statuses: Array<string | null> = [];
  const tracker = createTerminalActivityTracker((status) => statuses.push(status));

  tracker.notePromptSubmitted();
  assert.equal(tracker.noteEmbeddedTurnStarted("turn-1"), true);
  tracker.noteOutput(2_000);
  context.mock.timers.tick(TERMINAL_PROMPT_GRACE_MS + TERMINAL_TURN_GAP_MS);
  assert.equal(statuses.at(-1), "responding");
  assert.ok(!statuses.includes("waiting_input"));

  assert.equal(tracker.noteEmbeddedTurnFinished("turn-1"), true);
  assert.equal(statuses.at(-1), "waiting_input");
  tracker.dispose();
});

test("known semantic lifecycle does not fall back on the next prompt", (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const statuses: Array<string | null> = [];
  const tracker = createTerminalActivityTracker((status) => statuses.push(status));

  tracker.noteEmbeddedTurnStarted("turn-1");
  tracker.noteEmbeddedTurnFinished("turn-1");
  tracker.notePromptSubmitted();
  context.mock.timers.tick(TERMINAL_PROMPT_GRACE_MS + TERMINAL_TURN_GAP_MS);
  assert.equal(statuses.at(-1), "running");
  tracker.noteEmbeddedTurnStarted("turn-2");
  tracker.noteEmbeddedTurnFinished("turn-2");
  assert.equal(statuses.at(-1), "waiting_input");
  tracker.dispose();
});

test("semantic completion must match an observed active turn", () => {
  const statuses: Array<string | null> = [];
  const tracker = createTerminalActivityTracker((status) => statuses.push(status));

  assert.equal(tracker.noteEmbeddedTurnFinished("historic-turn"), false);
  assert.deepEqual(statuses, []);
  assert.equal(tracker.noteEmbeddedTurnStarted("current-turn"), true);
  assert.equal(tracker.noteEmbeddedTurnFinished("other-turn"), false);
  assert.equal(statuses.at(-1), "running");
  assert.equal(tracker.noteEmbeddedTurnFinished("current-turn"), true);
  assert.equal(statuses.at(-1), "waiting_input");
  tracker.dispose();
});

test("process completion overrides inferred response activity", () => {
  assert.equal(
    effectiveTerminalStatus("running", "running", "waiting_input"),
    "waiting_input",
  );
  assert.equal(
    effectiveTerminalStatus("running", "waiting_input", "running"),
    "running",
  );
  assert.equal(
    effectiveTerminalStatus("failed", "waiting_input", "running"),
    "failed",
  );
  assert.equal(
    effectiveTerminalStatus("exited", "running", "waiting_input"),
    "exited",
  );
  assert.equal(
    effectiveTerminalStatus("running", null, "responding"),
    "running",
  );
});

test("terminal and GUI modes use their reliable status source", () => {
  assert.equal(
    effectiveTerminalStatus(
      "running",
      "running",
      undefined,
      "terminal_embedded",
    ),
    "running",
  );
  assert.equal(
    effectiveTerminalStatus(
      "running",
      "waiting_input",
      "running",
      "gui",
    ),
    "waiting_input",
  );
});
