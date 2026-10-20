import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  createTerminalResumeGuard,
  filterRestoredPanesForAutoResume,
  reconcileListedTerminalPanes,
  releaseAutoResumePaneClaims,
} from "../src/terminalPaneReconcile.ts";
import type { TerminalPaneState } from "../src/terminalRestore.ts";
import type { TerminalSession } from "../src/types.ts";

const pane: TerminalPaneState = {
  paneId: "pane-running",
  sessionId: "terminal-running",
  projectId: 7,
  conversationId: "conversation-a",
  projectName: "Halv",
  cwd: "/tmp/halv",
  executionProfile: "full_access",
  agentKind: null,
  laneKind: null,
  credentialProfileId: null,
  customTitle: null,
  cliSessionId: null,
  cols: 100,
  rows: 24,
  scrollback: "saved output",
  updatedAt: "2026-08-19T12:00:00Z",
};

function terminalSession(
  id: string,
  status: TerminalSession["status"],
): TerminalSession {
  return {
    id,
    projectName: "Halv",
    cwd: "/tmp/halv",
    shell: "/bin/zsh",
    pid: status === "running" ? 42 : 0,
    executionProfile: "full_access",
    status,
    cols: 100,
    rows: 24,
    startedAt: "2026-08-19T12:00:00Z",
  };
}

test("resume guard stays active until concurrent resume transactions finish", () => {
  const guard = createTerminalResumeGuard();
  const endFirst = guard.begin();
  const endSecond = guard.begin();

  assert.equal(guard.active(), true);
  endFirst();
  endFirst();
  assert.equal(guard.active(), true);
  endSecond();
  assert.equal(guard.active(), false);
});

test("auto-resume drops live panes and releases their claims", () => {
  const agentPane: TerminalPaneState = {
    ...pane,
    paneId: "pane-agent",
    sessionId: "terminal-agent",
    agentKind: "codex",
    laneKind: "codexSubscription",
    cliSessionId: "cli-live",
  };
  const resumablePane: TerminalPaneState = {
    ...agentPane,
    paneId: "pane-resumable",
    sessionId: "terminal-resumable",
    cliSessionId: "cli-gone",
  };
  const panes = [pane, agentPane, resumablePane];
  const claims = new Set(panes.map((candidate) => candidate.paneId));

  const result = filterRestoredPanesForAutoResume(
    panes,
    [{ id: pane.sessionId, status: "running" }],
    [{ session: { cliSessionId: "cli-live", status: "waiting_input" } }],
  );
  releaseAutoResumePaneClaims(claims, result.droppedPaneIds);

  assert.deepEqual(result.droppedPaneIds, [pane.paneId, agentPane.paneId]);
  assert.deepEqual(result.resumablePanes, [resumablePane]);
  assert.deepEqual([...claims], [resumablePane.paneId]);
});

test("auto-resume does not replay an adopted agent terminal", () => {
  const adoptedPane: TerminalPaneState = {
    ...pane,
    paneId: "pane-adopted-agent",
    sessionId: "terminal-adopted-agent",
    agentKind: "codex",
    laneKind: "codexSubscription",
    cliSessionId: "cli-adopted-agent",
  };

  const result = filterRestoredPanesForAutoResume(
    [adoptedPane],
    [{ id: adoptedPane.sessionId, status: "running" }],
    [],
  );

  assert.deepEqual(result.droppedPaneIds, [adoptedPane.paneId]);
  assert.deepEqual(result.resumablePanes, []);
});

test("failed unmatched terminal does not create a pane", () => {
  let createdPaneCount = 0;
  const result = reconcileListedTerminalPanes(
    [terminalSession("terminal-failed", "failed")],
    {},
    {},
    [],
    new Set(),
    () => {
      createdPaneCount += 1;
      return "pane-created";
    },
  );

  assert.deepEqual(result.rows, []);
  assert.deepEqual(result.paneSessionIds, {});
  assert.equal(createdPaneCount, 0);
});

test("running unmatched terminal creates a pane", () => {
  const running = terminalSession("terminal-running-new", "running");
  const result = reconcileListedTerminalPanes(
    [running],
    {},
    {},
    [],
    new Set(),
    () => "pane-created",
  );

  assert.deepEqual(result.rows, [running]);
  assert.deepEqual(result.paneSessionIds, {
    "pane-created": running.id,
  });
  assert.deepEqual([...result.appendablePaneIds], ["pane-created"]);
});

test("expected terminal creates a pane before it is live", () => {
  const expected = terminalSession("terminal-expected", "failed");
  const result = reconcileListedTerminalPanes(
    [expected],
    {},
    {},
    [],
    new Set([expected.id]),
    () => "pane-expected",
  );

  assert.deepEqual(result.rows, [expected]);
  assert.deepEqual(result.paneSessionIds, {
    "pane-expected": expected.id,
  });
  assert.deepEqual([...result.appendablePaneIds], ["pane-expected"]);
});

test("terminal adopts a saved pane by CLI session id", () => {
  const adopted = terminalSession("terminal-adopted", "failed");
  let createdPaneCount = 0;
  const result = reconcileListedTerminalPanes(
    [adopted],
    {},
    { "pane-saved": { cliSessionId: "cli-adopted" } },
    [
      {
        session: { id: adopted.id, cliSessionId: "cli-adopted" },
      },
    ],
    new Set(),
    () => {
      createdPaneCount += 1;
      return "pane-created";
    },
  );

  assert.deepEqual(result.rows, [adopted]);
  assert.deepEqual(result.paneSessionIds, {
    "pane-saved": adopted.id,
  });
  assert.equal(createdPaneCount, 0);
});

test("four live terminals ignore four stale failed terminals", () => {
  const liveRows = Array.from({ length: 4 }, (_, index) =>
    terminalSession(`terminal-live-${index}`, "running"),
  );
  const failedRows = Array.from({ length: 4 }, (_, index) =>
    terminalSession(`terminal-stale-${index}`, "failed"),
  );
  const detachedRows = Array.from({ length: 4 }, (_, index) =>
    terminalSession(`terminal-old-${index}`, "exited"),
  );
  const restoredPanes = Object.fromEntries(
    liveRows.map((_, index) => [
      `pane-${index}`,
      { cliSessionId: `cli-${index}` },
    ]),
  );
  const existingBindings = Object.fromEntries(
    liveRows.map((_, index) => [`pane-${index}`, `terminal-old-${index}`]),
  );
  const agentEntries = liveRows.map((row, index) => ({
    session: { id: row.id, cliSessionId: `cli-${index}` },
  }));
  let createdPaneCount = 0;

  const result = reconcileListedTerminalPanes(
    [...liveRows, ...failedRows, ...detachedRows],
    existingBindings,
    restoredPanes,
    agentEntries,
    new Set(),
    () => {
      createdPaneCount += 1;
      return `pane-created-${createdPaneCount}`;
    },
  );

  assert.deepEqual(result.rows, liveRows);
  assert.equal(Object.keys(result.paneSessionIds).length, 4);
  assert.equal(createdPaneCount, 0);
});

test("a supervisor protocol mismatch keeps agent panes from resuming", () => {
  // The old supervisor still runs the agent, but Halv cannot list it, so
  // terminal_list and agent_session_list are empty (ADR 0008, rule 5).
  const agentPane: TerminalPaneState = {
    ...pane,
    paneId: "pane-old-supervisor",
    sessionId: "terminal-old-supervisor",
    agentKind: "claude",
    laneKind: "claudeSubscription",
    cliSessionId: "cli-1",
  };

  const mismatch = filterRestoredPanesForAutoResume([pane, agentPane], [], [], {
    supervisorProtocolMismatch: true,
  });
  assert.deepEqual(mismatch.resumablePanes, [pane]);
  assert.deepEqual(mismatch.droppedPaneIds, [agentPane.paneId]);

  const current = filterRestoredPanesForAutoResume([pane, agentPane], [], []);
  assert.deepEqual(current.resumablePanes, [pane, agentPane]);
});
