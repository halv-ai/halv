import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs source extensions.
import {
  agentSessionWaitsForExit,
  closeRunningTerminalSession,
  closedTerminalRestorePayload,
  forgetTerminalSession,
  forgetPendingTerminalExit,
  markPendingTerminalForget,
  writeClosedTerminalRestore,
  type TerminalCloseSnapshot,
} from "../src/terminalClose.ts";
import type { TerminalPaneState } from "../src/terminalRestore.ts";
import type { TerminalSession } from "../src/types.ts";

const updatedAt = "2026-08-19T12:00:00Z";

test("only live agent states wait for a terminal exit event", () => {
  assert.equal(agentSessionWaitsForExit("starting"), true);
  assert.equal(agentSessionWaitsForExit("running"), true);
  assert.equal(agentSessionWaitsForExit("waiting_input"), true);
  assert.equal(agentSessionWaitsForExit("exited"), false);
  assert.equal(agentSessionWaitsForExit("failed"), false);
});

function terminalSession(
  id: string,
  agentKind: TerminalSession["agentKind"] = undefined,
): TerminalSession {
  return {
    id,
    projectId: 7,
    conversationId: "conversation-a",
    projectName: "Halv",
    cwd: "/tmp/halv",
    shell: "/bin/zsh",
    pid: 123,
    executionProfile: "full_access",
    status: id === "agent-a" ? "exited" : "running",
    cols: 100,
    rows: 24,
    startedAt: updatedAt,
    agentKind,
  };
}

function paneState(
  id: string,
  agentKind: TerminalPaneState["agentKind"] = null,
): TerminalPaneState {
  return {
    paneId: `pane-${id}`,
    sessionId: id,
    projectId: 7,
    conversationId: "conversation-a",
    projectName: "Halv",
    cwd: "/tmp/halv",
    executionProfile: "full_access",
    agentKind,
    laneKind: agentKind === "claude" ? "claudeSubscription" : null,
    credentialProfileId: null,
    customTitle: null,
    cliSessionId: agentKind ? "cli-a" : null,
    cols: 100,
    rows: 24,
    scrollback: "saved output",
    updatedAt,
  };
}

function closeSnapshot(): TerminalCloseSnapshot {
  return {
    scope: "terminal-panel:conversation-a",
    layout: {
      version: 1,
      activeTabId: "pane-agent-a",
      activeLeafId: "pane-agent-a",
      tabs: [
        {
          tabId: "pane-agent-a",
          root: {
            type: "split",
            direction: "vertical",
            first: { type: "leaf", leafId: "pane-agent-a" },
            second: { type: "leaf", leafId: "pane-shell-b" },
          },
        },
      ],
    },
    sessions: [terminalSession("agent-a", "claude"), terminalSession("shell-b")],
    paneSessionIds: {
      "pane-agent-a": "agent-a",
      "pane-shell-b": "shell-b",
    },
    agentEntries: [],
    laneOptions: [],
    restoredPanes: {
      "pane-agent-a": paneState("agent-a", "claude"),
    },
    resumeFallbackPanes: {
      "pane-agent-a": paneState("agent-a", "claude"),
    },
    excludedPaneIds: new Set(["pane-agent-a"]),
  };
}

test("closing an exited agent removes its saved pane and layout leaf", () => {
  const payload = closedTerminalRestorePayload(closeSnapshot());

  assert.equal(payload.scope, "terminal-panel:conversation-a");
  assert.deepEqual(payload.layout.tabs[0]!.root, {
    type: "leaf",
    leafId: "pane-shell-b",
  });
  assert.deepEqual(
    payload.panes.map((pane) => pane.sessionId),
    ["shell-b"],
  );
  assert.deepEqual(
    payload.panes.map((pane) => pane.paneId),
    ["pane-shell-b"],
  );
});

test("close writes the captured scope after the active scope changes", async () => {
  const snapshot = closeSnapshot();
  const restoreWrites: string[] = [];
  const layoutWrites: string[] = [];
  let activeScope = snapshot.scope;
  let releaseBackend: (() => void) | null = null;
  const backendWork = new Promise<void>((resolve) => {
    releaseBackend = resolve;
  });

  const close = async () => {
    await backendWork;
    return writeClosedTerminalRestore(snapshot, {
      writeLayout: (scope) => layoutWrites.push(scope),
      writeRestore: async (payload) => {
        restoreWrites.push(payload.scope);
      },
    });
  };
  const write = close();
  activeScope = "terminal-panel:conversation-b";
  releaseBackend?.();
  const payload = await write;

  assert.equal(activeScope, "terminal-panel:conversation-b");
  assert.equal(payload.scope, "terminal-panel:conversation-a");
  assert.deepEqual(layoutWrites, ["terminal-panel:conversation-a"]);
  assert.deepEqual(restoreWrites, ["terminal-panel:conversation-a"]);
});

test("an agent exit forgets both backend records", async () => {
  const terminalIds = new Set<string>();
  const agentIds = new Set<string>();
  const calls: string[] = [];
  let agentAttempts = 0;
  markPendingTerminalForget(terminalIds, agentIds, "agent-a", true);

  const handled = await forgetPendingTerminalExit(
    terminalIds,
    agentIds,
    "agent-a",
    {
      forgetTerminal: async (sessionId) => {
        calls.push(`terminal:${sessionId}`);
      },
      forgetAgent: async (sessionId) => {
        calls.push(`agent:${sessionId}`);
        agentAttempts += 1;
        if (agentAttempts === 1) {
          throw new Error(`agent session \`${sessionId}\` is running`);
        }
      },
      refreshAgentSessions: async () => {
        calls.push("refresh-agent");
      },
      refreshTerminalSessions: async () => {
        calls.push("refresh-terminal");
      },
      waitForExit: async () => {
        calls.push("wait-exit");
      },
    },
  );

  assert.equal(handled, true);
  assert.deepEqual(calls, [
    "terminal:agent-a",
    "agent:agent-a",
    "wait-exit",
    "agent:agent-a",
    "refresh-agent",
    "refresh-terminal",
  ]);
  assert.equal(terminalIds.size, 0);
  assert.equal(agentIds.size, 0);
});

test("a dead running terminal falls back to forget", async () => {
  const calls: string[] = [];

  const result = await closeRunningTerminalSession("shell-dead", {
    closeTerminal: async (sessionId) => {
      calls.push(`close:${sessionId}`);
      throw new Error(`terminal session \`${sessionId}\` was not found`);
    },
    forgetTerminal: async (sessionId) => {
      calls.push(`forget:${sessionId}`);
    },
  });

  assert.equal(result, "forgotten");
  assert.deepEqual(calls, ["close:shell-dead", "forget:shell-dead"]);
});

test("a terminal missing from both backend layers closes idempotently", async () => {
  const calls: string[] = [];

  const result = await closeRunningTerminalSession("ghost", {
    closeTerminal: async (sessionId) => {
      calls.push(`close:${sessionId}`);
      throw new Error(`terminal session \`${sessionId}\` was not found`);
    },
    forgetTerminal: async (sessionId) => {
      calls.push(`forget:${sessionId}`);
      throw new Error(`terminal session \`${sessionId}\` was not found`);
    },
  });

  assert.equal(result, "forgotten");
  assert.deepEqual(calls, ["close:ghost", "forget:ghost"]);
});

test("forgetting an already missing restored terminal is idempotent", async () => {
  await forgetTerminalSession("ghost", async (sessionId) => {
    throw new Error(`terminal session \`${sessionId}\` was not found`);
  });
});

test("forgetting a terminal still surfaces real backend failures", async () => {
  await assert.rejects(
    forgetTerminalSession("live", async () => {
      throw new Error("terminal supervisor request timed out");
    }),
    /timed out/,
  );
});
