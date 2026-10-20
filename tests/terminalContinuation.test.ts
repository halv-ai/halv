import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  applyTerminalContinuations,
  filterRestoredPanesForAutoResume,
  omitPaneIds,
  panelOwnsContinuationSource,
} from "../src/terminalPaneReconcile.ts";
// @ts-expect-error Node's test runner needs the source extension.
import { buildTerminalRestoreWritePayload } from "../src/terminalRestore.ts";
// @ts-expect-error Node's test runner needs the source extension.
import {
  closeOrphanTerminalSession,
  forgetContinuationSource,
} from "../src/terminalClose.ts";
// @ts-expect-error Node's test runner needs the source extension.
import {
  TERMINAL_EXIT_MODE_RESET,
  terminalAgentLabel,
  terminalContinuationPrelude,
} from "../src/terminalSurface.ts";
// @ts-expect-error Node's test runner needs the source extension.
import {
  registerTerminalShellRequest,
  requestTerminalShell,
} from "../src/terminalShellRequest.ts";
import type { TerminalPaneState } from "../src/terminalRestore.ts";
import type { TerminalSession } from "../src/types.ts";

const agentId = "0f6d6c1e-agent";
const shellId = "terminal-shell";

function terminal(
  id: string,
  overrides: Partial<TerminalSession> = {},
): TerminalSession {
  return {
    id,
    projectId: 7,
    conversationId: "conversation-1",
    projectName: "Halv",
    cwd: "/tmp/halv",
    shell: "/bin/zsh",
    pid: 42,
    executionProfile: "full_access",
    status: "running",
    cols: 100,
    rows: 24,
    startedAt: "2026-10-14T12:00:00Z",
    ...overrides,
  };
}

const savedAgentPane: TerminalPaneState = {
  paneId: "pane-1",
  sessionId: agentId,
  projectId: 7,
  conversationId: "conversation-1",
  projectName: "Halv",
  cwd: "/tmp/halv",
  executionProfile: "full_access",
  agentKind: "claude",
  laneKind: "claudeSubscription",
  credentialProfileId: null,
  customTitle: null,
  cliSessionId: "cli-ended",
  cols: 100,
  rows: 24,
  scrollback: "saved output",
  updatedAt: "2026-10-14T12:00:00Z",
};

test("a continuation shell takes over the pane of its ended agent", () => {
  const rows = [
    terminal(agentId, { agentKind: "claude", status: "exited", exitCode: 0 }),
    terminal(shellId, { continuedFrom: agentId }),
  ];
  const result = applyTerminalContinuations(rows, {
    "pane-1": agentId,
    "pane-2": "terminal-other",
  });

  assert.deepEqual(result.paneSessionIds, {
    "pane-1": shellId,
    "pane-2": "terminal-other",
  });
  assert.deepEqual(result.reboundPaneIds, ["pane-1"]);
  assert.deepEqual([...result.replacedSessionIds], [agentId]);
  assert.equal(result.continuationBySourceId.get(agentId), shellId);
  assert.deepEqual(result.orphanSessionIds, []);
});

test("a later refresh keeps the binding and still hides the ended agent", () => {
  const rows = [
    terminal(agentId, { agentKind: "claude", status: "exited", exitCode: 0 }),
    terminal(shellId, { continuedFrom: agentId }),
  ];
  const bound = { "pane-1": shellId };
  const result = applyTerminalContinuations(rows, bound);

  assert.equal(result.paneSessionIds, bound);
  assert.deepEqual(result.reboundPaneIds, []);
  assert.deepEqual([...result.replacedSessionIds], [agentId]);
});

test("a shell for a closed or failed-resume pane is an orphan", () => {
  const rows = [terminal(shellId, { continuedFrom: agentId })];

  const blocked = applyTerminalContinuations(
    rows,
    { "pane-1": agentId },
    new Set(["pane-1"]),
  );
  // The resume failure card keeps the pane. Only the shell goes.
  assert.deepEqual(blocked.paneSessionIds, { "pane-1": agentId });
  assert.deepEqual(blocked.orphanSessionIds, [shellId]);
  assert.deepEqual([...blocked.replacedSessionIds], []);

  const dismissed = applyTerminalContinuations(
    rows,
    {},
    new Set(),
    new Set([agentId]),
  );
  assert.deepEqual(dismissed.orphanSessionIds, [shellId]);

  // A shell that the panel already closes is not reported twice.
  const closing = applyTerminalContinuations(
    rows,
    {},
    new Set(),
    new Set([agentId, shellId]),
  );
  assert.deepEqual(closing.orphanSessionIds, []);
});

test("a closing continuation shell still hides its ended agent", () => {
  const rows = [
    terminal(agentId, { agentKind: "claude", status: "exited", exitCode: 0 }),
    terminal(shellId, { continuedFrom: agentId }),
  ];
  const result = applyTerminalContinuations(
    rows,
    { "pane-1": agentId },
    new Set(),
    new Set([shellId]),
  );

  assert.deepEqual(result.paneSessionIds, { "pane-1": agentId });
  assert.deepEqual(result.reboundPaneIds, []);
  assert.deepEqual([...result.replacedSessionIds], [agentId]);
});

test("a continuation event reaches the panel that showed or closed its source", () => {
  assert.equal(
    panelOwnsContinuationSource(agentId, { "pane-1": agentId }, new Set()),
    true,
  );
  assert.equal(
    panelOwnsContinuationSource(agentId, { "pane-1": "terminal-other" }, new Set()),
    false,
  );
  // The user closed the ended agent pane before the shell started. The
  // refresh then finds the shell as an orphan and closes it.
  const dismissed = new Set([agentId]);
  assert.equal(panelOwnsContinuationSource(agentId, {}, dismissed), true);
  assert.deepEqual(
    applyTerminalContinuations(
      [terminal(shellId, { continuedFrom: agentId })],
      {},
      new Set(),
      dismissed,
    ).orphanSessionIds,
    [shellId],
  );
});

test("auto-resume skips a saved agent pane that a shell continues", () => {
  const plainPane: TerminalPaneState = {
    ...savedAgentPane,
    paneId: "pane-2",
    sessionId: "terminal-ended-shell",
    agentKind: null,
    laneKind: null,
    cliSessionId: null,
  };
  const { droppedPaneIds, resumablePanes } = filterRestoredPanesForAutoResume(
    [savedAgentPane, plainPane],
    [{ id: shellId, status: "running", continuedFrom: agentId }],
    [],
  );

  assert.deepEqual(droppedPaneIds, ["pane-1"]);
  assert.deepEqual(
    resumablePanes.map((pane: TerminalPaneState) => pane.paneId),
    ["pane-2"],
  );
});

test("a rebound pane saves as a shell once its agent state is cleared", () => {
  const restoredPanes = { "pane-1": savedAgentPane };
  const resumeFallbackPanes = { "pane-1": savedAgentPane };
  const shell = terminal(shellId, { continuedFrom: agentId });
  const payload = buildTerminalRestoreWritePayload({
    scope: "conversation:conversation-1",
    layout: {
      version: 1,
      activeTabId: "tab-1",
      activeLeafId: "pane-1",
      tabs: [{ tabId: "tab-1", root: { type: "leaf", leafId: "pane-1" } }],
    },
    sessions: [shell],
    paneSessionIds: { "pane-1": shellId },
    agentEntries: [],
    restoredPanes: omitPaneIds(restoredPanes, ["pane-1"]),
    resumeFallbackPanes: omitPaneIds(resumeFallbackPanes, ["pane-1"]),
    laneOptions: [],
  });

  assert.equal(payload.panes.length, 1);
  assert.equal(payload.panes[0].paneId, "pane-1");
  assert.equal(payload.panes[0].sessionId, shellId);
  assert.equal(payload.panes[0].agentKind, null);
  assert.equal(payload.panes[0].cliSessionId, null);
  assert.equal(payload.panes[0].laneKind, null);
});

test("omitPaneIds keeps the same record when no pane matches", () => {
  const record = { "pane-1": 1, "pane-2": 2 };
  assert.equal(omitPaneIds(record, ["pane-3"]), record);
  assert.deepEqual(omitPaneIds(record, ["pane-1"]), { "pane-2": 2 });
});

test("an orphan shell closes and its row goes after the exit", async () => {
  const calls: string[] = [];
  let forgetAttempts = 0;
  await closeOrphanTerminalSession(shellId, {
    closeTerminal: async (sessionId: string) => {
      calls.push(`close:${sessionId}`);
    },
    forgetTerminal: async (sessionId: string) => {
      forgetAttempts += 1;
      calls.push(`forget:${sessionId}`);
      if (forgetAttempts < 3) {
        throw new Error("close the terminal before removing it");
      }
    },
    waitForExit: async () => {
      calls.push("wait");
    },
  });

  assert.deepEqual(calls, [
    `close:${shellId}`,
    `forget:${shellId}`,
    "wait",
    `forget:${shellId}`,
    "wait",
    `forget:${shellId}`,
  ]);

  // A shell that is already gone needs no forget.
  const gone: string[] = [];
  await closeOrphanTerminalSession(shellId, {
    closeTerminal: async () => {
      throw new Error("terminal session `terminal-shell` was not found");
    },
    forgetTerminal: async () => {
      gone.push("forget");
    },
  });
  assert.deepEqual(gone, []);
});

test("closing a continuation shell forgets the ended agent rows", async () => {
  const calls: string[] = [];
  await forgetContinuationSource(agentId, {
    forgetTerminal: async (sessionId: string) => {
      calls.push(`terminal:${sessionId}`);
      throw new Error(`terminal session \`${sessionId}\` was not found`);
    },
    forgetAgent: async (sessionId: string) => {
      calls.push(`agent:${sessionId}`);
    },
    refreshAgentSessions: async () => {
      calls.push("refresh");
    },
  });

  assert.deepEqual(calls, [
    `terminal:${agentId}`,
    `agent:${agentId}`,
    "refresh",
  ]);
});

test("the prelude shows the last agent screen and a dim marker", () => {
  // The cursor save comes before the mode reset: the reset of mode 1049
  // restores the saved cursor, and an older save would move the marker
  // and the prompt over the agent output.
  assert.equal(
    terminalContinuationPrelude({
      scrollback: "last screen",
      agentKind: "claude",
      exitCode: 0,
    }),
    `last screen\x18\x1b7${TERMINAL_EXIT_MODE_RESET}\r\n\x1b[2m[Claude Code exited 0 · shell]\x1b[0m\r\n`,
  );
  assert.ok(TERMINAL_EXIT_MODE_RESET.includes("\x1b[?1049l"));
  assert.match(
    terminalContinuationPrelude({ scrollback: "" }),
    /\[process exited \? · shell\]/,
  );
  // The marker uses the product name of the pane header, not the lane id.
  assert.match(
    terminalContinuationPrelude({ scrollback: "", agentKind: "zai", exitCode: 0 }),
    /\[GLM exited 0 · shell\]/,
  );
  assert.equal(terminalAgentLabel("zai"), "GLM");
  assert.equal(terminalAgentLabel(null), null);
});

test("the New shell request reaches the panel of the conversation", () => {
  const opened: string[] = [];
  const unregisterChat = registerTerminalShellRequest(null, () => {
    opened.push("chat");
  });
  const unregisterPro = registerTerminalShellRequest("conversation-1", () => {
    opened.push("conversation-1");
  });

  assert.equal(requestTerminalShell("conversation-1"), true);
  assert.equal(requestTerminalShell(null), true);
  assert.equal(requestTerminalShell("conversation-2"), false);
  assert.deepEqual(opened, ["conversation-1", "chat"]);

  // A newer panel for the same conversation owns the request. When it
  // unmounts, the older panel gets the request again.
  const unregisterNewer = registerTerminalShellRequest("conversation-1", () => {
    opened.push("newer");
  });
  assert.equal(requestTerminalShell("conversation-1"), true);
  assert.deepEqual(opened.at(-1), "newer");
  unregisterNewer();
  assert.equal(requestTerminalShell("conversation-1"), true);
  assert.deepEqual(opened.at(-1), "conversation-1");

  // The older panel's cleanup must not remove a newer panel.
  const unregisterLatest = registerTerminalShellRequest("conversation-1", () => {
    opened.push("latest");
  });
  unregisterPro();
  assert.equal(requestTerminalShell("conversation-1"), true);
  assert.deepEqual(opened.at(-1), "latest");
  unregisterLatest();
  unregisterChat();
  assert.equal(requestTerminalShell("conversation-1"), false);
  assert.equal(requestTerminalShell(null), false);
});

test("only agent panes pass a submitted line through the trial gate", () => {
  const source = readFileSync(
    new URL("../src/components/TerminalPane.tsx", import.meta.url),
    "utf8",
  );
  assert.match(
    source,
    /if \(session\.agentKind && promptSubmissionTracker\.noteInput\(data\)\) \{\s*if \(callbacksRef\.current\.onPromptSubmitGate\?\.\(\)\)/,
  );
  assert.equal(source.match(/onPromptSubmitGate\?\.\(\)/g)?.length, 1);
});

test("an ended live agent pane offers Open shell here", () => {
  const panel = readFileSync(
    new URL("../src/components/TerminalPanel.tsx", import.meta.url),
    "utf8",
  );
  assert.match(
    panel,
    /onOpenShell=\{[\s\S]*?session\.agentKind &&\s*session\.status !== "running" &&\s*!restoredPane &&\s*!props\.resumeFailures\[paneId\]/,
  );
  assert.match(
    panel,
    /async function continueInShell\([\s\S]*?ipc\.terminal\.continueShell\(\{ sessionId: source\.id \}\)/,
  );
  // The header button and the New shell command split the active pane.
  assert.match(
    panel,
    /async function openShellFromHeader\(\)[\s\S]*?splitPane\(\s*activeSession,\s*"vertical",\s*TERMINAL_SPLIT_TERMINAL_CHOICE_ID,/,
  );
});
