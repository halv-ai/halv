import assert from "node:assert/strict";
import test from "node:test";
import {
  agentPaneViewFor,
  createTerminalActivityStore,
} from "../src/terminalActivityStore.ts";
import type { AgentStatusEntry } from "../src/agentPaneStatus.ts";
import { TERMINAL_RESPONSE_IDLE_MS, TERMINAL_TURN_GAP_MS } from "../src/terminalActivity.ts";

test("activity continues without a mounted pane and remount keeps completion read", (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const store = createTerminalActivityStore();
  const tracker = store.trackerForSession("cli");
  const unsubscribe = store.subscribe(() => {});
  tracker.notePromptSubmitted();
  unsubscribe(); // Navigate away while the answer is running.
  store.noteEvent({ sessionId: "cli", sequence: 1, kind: "data", data: "reply".repeat(100) });
  context.mock.timers.tick(TERMINAL_RESPONSE_IDLE_MS);
  context.mock.timers.tick(TERMINAL_TURN_GAP_MS);
  assert.equal(store.getSnapshot().statuses.cli, "waiting_input");
  assert.equal(agentPaneViewFor(store.getSnapshot(), "cli")?.status, "done");
  assert.equal(agentPaneViewFor(store.getSnapshot(), "cli")?.unread, true);

  assert.equal(store.trackerForSession("cli"), tracker);
  store.markViewed("cli");
  const completed = store.getSnapshot();
  store.noteEvent({ sessionId: "cli", sequence: 2, kind: "data", data: "redraw".repeat(10_000) });
  context.mock.timers.tick(TERMINAL_TURN_GAP_MS);
  assert.equal(store.getSnapshot(), completed);
  assert.equal(store.trackerForSession("cli"), tracker);
  assert.equal(agentPaneViewFor(store.getSnapshot(), "cli")?.unread, false);

  tracker.notePromptSubmitted();
  assert.equal(store.getSnapshot().statuses.cli, "running");
  assert.equal(agentPaneViewFor(store.getSnapshot(), "cli")?.status, "working");
  assert.equal(agentPaneViewFor(store.getSnapshot(), "cli")?.unread, false);
  store.dispose();
});

test("replayed output and duplicate events do not change activity", (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const store = createTerminalActivityStore();
  store.trackerForSession("cli");
  store.noteEvent({ sessionId: "cli", sequence: 1, kind: "data", data: "history", replay: true });
  assert.equal(store.getSnapshot().statuses.cli, "waiting_input");
  store.noteEvent({ sessionId: "cli", sequence: 1, kind: "data", data: "duplicate" });
  assert.equal(store.getSnapshot().statuses.cli, "waiting_input");
  store.dispose();
});

test("opening an idle restored session cannot start work through its first redraw", (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const store = createTerminalActivityStore();
  store.trackerForSession("cli");
  store.markViewed("cli");
  const idle = store.getSnapshot();
  store.noteEvent({ sessionId: "cli", sequence: 1, kind: "data", data: "redraw".repeat(10_000) });
  context.mock.timers.tick(TERMINAL_TURN_GAP_MS);
  assert.equal(store.getSnapshot(), idle);
  store.trackerForSession("cli").notePromptSubmitted();
  assert.equal(store.getSnapshot().statuses.cli, "running");
  store.dispose();
});

test("an attach cannot overwrite a prompt submitted before the pane mounts", (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const store = createTerminalActivityStore();
  store.trackerForSession("cli").notePromptSubmitted();
  store.trackerForSession("cli");
  store.noteEvent({ sessionId: "cli", sequence: 1, kind: "data", data: "old output", replay: true });
  store.markViewed("cli");
  assert.equal(store.getSnapshot().statuses.cli, "running");
  assert.equal(agentPaneViewFor(store.getSnapshot(), "cli")?.status, "working");
  store.dispose();
});

test("separate panes retain independent activity and acknowledgement", (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const store = createTerminalActivityStore();
  const first = store.trackerForSession("first");
  const second = store.trackerForSession("second");
  first.noteOutput();
  context.mock.timers.tick(TERMINAL_RESPONSE_IDLE_MS);
  second.notePromptSubmitted();
  store.markViewed("first");
  assert.equal(store.getSnapshot().statuses.first, "waiting_input");
  assert.equal(store.getSnapshot().statuses.second, "running");
  // A burst of output without a prompt is not a finished turn.
  assert.equal(agentPaneViewFor(store.getSnapshot(), "first")?.status, "idle");
  assert.equal(agentPaneViewFor(store.getSnapshot(), "second")?.status, "working");
  store.noteEvent({ sessionId: "second", sequence: 1, kind: "exit", exitCode: 0 });
  context.mock.timers.tick(60_000);
  assert.equal(store.getSnapshot().statuses.second, undefined);
  assert.equal(store.getSnapshot().statuses.first, "waiting_input");
  store.forget("first");
  assert.equal(store.getSnapshot().statuses.first, undefined);
  assert.equal(agentPaneViewFor(store.getSnapshot(), "first"), null);
  store.dispose();
});


test("input drafted before a pane remount still starts a turn on Enter", () => {
  const store = createTerminalActivityStore();
  const input = store.promptSubmissionForSession("cli");
  assert.equal(input.noteInput("continue"), false);
  assert.equal(store.promptSubmissionForSession("cli"), input);
  assert.equal(store.promptSubmissionForSession("cli").noteInput("\r"), true);
  assert.equal(input.noteInput("\r"), false);
  store.forget("cli");
  assert.notEqual(store.promptSubmissionForSession("cli"), input);
  store.dispose();
});

test("embedded turn events settle immediately and keep aborted outcome distinct", (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const store = createTerminalActivityStore();

  assert.equal(store.noteAgentEvent("cli", {
    type: "embeddedTurnStarted",
    turnId: "turn-1",
  }), true);
  store.noteEvent({
    sessionId: "cli",
    sequence: 1,
    kind: "data",
    data: "reply".repeat(100),
  });
  context.mock.timers.tick(TERMINAL_TURN_GAP_MS * 2);
  assert.equal(store.getSnapshot().statuses.cli, "responding");

  const aborted = {
    type: "embeddedTurnFinished" as const,
    turnId: "turn-1",
    outcome: "aborted" as const,
  };
  assert.equal(store.noteAgentEvent("cli", aborted), true);
  assert.equal(aborted.outcome, "aborted");
  assert.equal(store.getSnapshot().statuses.cli, "waiting_input");
  store.dispose();
});

test("historic and mismatched transcript finishes are rejected", () => {
  const store = createTerminalActivityStore();
  assert.equal(
    store.noteAgentEvent("cli", {
      type: "embeddedTurnFinished",
      turnId: "historic-turn",
      outcome: "completed",
    }),
    false,
  );
  assert.equal(
    store.noteAgentEvent("cli", {
      type: "embeddedTurnStarted",
      turnId: "current-turn",
    }),
    true,
  );
  assert.equal(
    store.noteAgentEvent("cli", {
      type: "embeddedTurnFinished",
      turnId: "other-turn",
      outcome: "completed",
    }),
    false,
  );
  assert.equal(store.getSnapshot().statuses.cli, "running");
  store.dispose();
});

function entry(
  sessionId: string,
  overrides: Partial<AgentStatusEntry> = {},
): AgentStatusEntry {
  return {
    sessionId,
    status: "working",
    updatedAt: 1,
    unread: false,
    waitSeq: 0,
    turnSeq: 1,
    source: "presence",
    ...overrides,
  };
}

function finishTurn(
  store: ReturnType<typeof createTerminalActivityStore>,
  id: string,
  sequence: number,
  tick: (ms: number) => void,
) {
  store.trackerForSession(id).notePromptSubmitted();
  store.noteEvent({ sessionId: id, sequence, kind: "data", data: "reply".repeat(100) });
  tick(TERMINAL_RESPONSE_IDLE_MS);
  tick(TERMINAL_TURN_GAP_MS);
}

test("a new or adopted pane starts idle and read (S2)", () => {
  const store = createTerminalActivityStore();
  store.trackerForSession("adopted");
  const view = agentPaneViewFor(store.getSnapshot(), "adopted");
  assert.equal(view?.status, "idle");
  assert.equal(view?.unread, false);
  // The raw status keeps its seed for the guided first task.
  assert.equal(store.getSnapshot().statuses.adopted, "waiting_input");
  store.dispose();
});

test("a turn that finishes in the pane the user watches is read (S9)", (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const store = createTerminalActivityStore();
  store.setWindowFocused(true);
  store.setVisiblePane("cli");
  finishTurn(store, "cli", 1, (ms) => context.mock.timers.tick(ms));
  assert.equal(agentPaneViewFor(store.getSnapshot(), "cli")?.status, "done");
  assert.equal(agentPaneViewFor(store.getSnapshot(), "cli")?.unread, false);

  // The same turn in a blurred window stays unread.
  store.setWindowFocused(false);
  finishTurn(store, "cli", 2, (ms) => context.mock.timers.tick(ms));
  assert.equal(agentPaneViewFor(store.getSnapshot(), "cli")?.unread, true);
  // Focus on the window reads the visible pane.
  store.setWindowFocused(true);
  assert.equal(agentPaneViewFor(store.getSnapshot(), "cli")?.unread, false);
  store.dispose();
});

test("viewing a pane mid-turn does not read the turn result", (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const store = createTerminalActivityStore();
  const tracker = store.trackerForSession("cli");
  tracker.notePromptSubmitted();
  store.noteEvent({ sessionId: "cli", sequence: 1, kind: "data", data: "reply".repeat(100) });
  context.mock.timers.tick(TERMINAL_RESPONSE_IDLE_MS);
  store.markViewed("cli");
  context.mock.timers.tick(TERMINAL_TURN_GAP_MS);
  assert.equal(agentPaneViewFor(store.getSnapshot(), "cli")?.unread, true);
  store.dispose();
});

test("an agent signal outranks the heuristic and its source none does not", () => {
  const store = createTerminalActivityStore();
  store.trackerForSession("claude");
  store.noteAgentStatus(entry("claude", { status: "working", source: "none" }));
  assert.equal(agentPaneViewFor(store.getSnapshot(), "claude")?.status, "idle");
  store.noteAgentStatus(entry("claude", { status: "needsApproval", reason: "permission", waitSeq: 1 }));
  const view = agentPaneViewFor(store.getSnapshot(), "claude");
  assert.equal(view?.status, "needsApproval");
  assert.equal(view?.source, "presence");
  assert.equal(store.paneIsBusy("claude"), true);

  // Process exit belongs to the terminal list. The entry goes away.
  store.noteAgentStatus(entry("claude", { status: "exited", source: "process" }));
  assert.equal(store.getSnapshot().agentStatuses.claude, undefined);
  assert.equal(agentPaneViewFor(store.getSnapshot(), "claude")?.status, "idle");
  assert.equal(store.paneIsBusy("claude"), false);
  store.dispose();
});

test("reading a pane clears a finished turn but never a wait", () => {
  const store = createTerminalActivityStore();
  const viewed: string[] = [];
  store.setAgentStatusBridge({
    markViewed: (id) => viewed.push(id),
    setFocus: () => undefined,
  });
  store.noteAgentStatus(entry("done", { status: "done", unread: true }));
  store.noteAgentStatus(entry("wait", { status: "needsInput", reason: "question", waitSeq: 2 }));
  store.markViewed("done");
  store.markViewed("done");
  store.markViewed("wait");
  assert.equal(agentPaneViewFor(store.getSnapshot(), "done")?.unread, false);
  assert.equal(agentPaneViewFor(store.getSnapshot(), "wait")?.status, "needsInput");
  // Only an unread entry costs a Rust call.
  assert.deepEqual(viewed, ["done"]);
  store.dispose();
});

test("the focused pane goes to Rust and follows the window focus", () => {
  const store = createTerminalActivityStore();
  const focus: (string | null)[] = [];
  store.setAgentStatusBridge({
    markViewed: () => undefined,
    setFocus: (id) => focus.push(id),
  });
  store.setVisiblePane("cli");
  assert.equal(store.getSnapshot().focusedSessionId, null);
  store.setWindowFocused(true);
  assert.equal(store.getSnapshot().focusedSessionId, "cli");
  store.setVisiblePane("other");
  store.setWindowFocused(false);
  store.setWindowFocused(false);
  assert.deepEqual(focus, [null, "cli", "other", null]);
  store.dispose();
});

test("a surface over the panes clears the focused pane", () => {
  const store = createTerminalActivityStore();
  const focus: (string | null)[] = [];
  store.setAgentStatusBridge({
    markViewed: () => undefined,
    setFocus: (id) => focus.push(id),
  });
  store.setWindowFocused(true);
  store.setVisiblePane("cli");
  store.setPaneCovered(true);
  assert.equal(store.getSnapshot().focusedSessionId, null);
  // A result that comes while Dot covers the pane stays unread.
  store.noteAgentStatus(entry("cli", { status: "done", unread: true, turnSeq: 1 }));
  assert.equal(agentPaneViewFor(store.getSnapshot(), "cli")?.unread, true);
  store.setPaneCovered(false);
  assert.equal(store.getSnapshot().focusedSessionId, "cli");
  assert.equal(agentPaneViewFor(store.getSnapshot(), "cli")?.unread, false);
  assert.deepEqual(focus, [null, "cli", null, "cli"]);
  store.dispose();
});

test("the snapshot does not overwrite entries from newer events", () => {
  const store = createTerminalActivityStore();
  store.noteAgentStatus(entry("fresh", { status: "done", unread: true, turnSeq: 4 }));
  store.noteAgentStatus(entry("gone", { status: "working" }));
  store.hydrateAgentStatuses(
    [
      entry("fresh", { status: "working", turnSeq: 4 }),
      entry("other", { status: "needsApproval", waitSeq: 1 }),
    ],
    new Set(["fresh"]),
  );
  const statuses = store.getSnapshot().agentStatuses;
  assert.equal(statuses.fresh?.status, "done");
  assert.equal(statuses.other?.status, "needsApproval");
  assert.equal(statuses.gone, undefined);
  store.dispose();
});

test("pane listeners hear each display change once", () => {
  const store = createTerminalActivityStore();
  const changes: string[] = [];
  store.subscribePanes((id, view, previous) =>
    changes.push(`${id}:${previous?.status ?? "none"}>${view?.status ?? "none"}`),
  );
  store.noteAgentStatus(entry("cli", { status: "working" }));
  store.noteAgentStatus(entry("cli", { status: "working" }));
  store.noteAgentStatus(entry("cli", { status: "needsApproval", waitSeq: 1 }));
  store.forget("cli");
  assert.deepEqual(changes, [
    "cli:none>working",
    "cli:working>needsApproval",
    "cli:needsApproval>none",
  ]);
  store.dispose();
});
