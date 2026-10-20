import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  bindTerminalPaneSession,
  replaceTerminalSessionId,
  restoreTerminalPanel,
  restoreTerminalPanels,
  resumeLaneForTerminalPane,
  supervisorReplayNotice,
  terminalPaneStateInput,
  terminalPaneTitleState,
  type TerminalPaneState,
} from "../src/terminalRestore.ts";
import type { AgentSessionLaneOptions } from "../src/agentSessions.ts";

const pane: TerminalPaneState = {
  paneId: "terminal-a",
  sessionId: "terminal-a",
  projectId: 7,
  conversationId: "conversation-a",
  projectName: "Halv",
  cwd: "/tmp/halv",
  executionProfile: "full_access",
  agentKind: "claude",
  laneKind: "claudeSubscription",
  credentialProfileId: null,
  customTitle: "Release checks",
  cliSessionId: "claude-session",
  cols: 100,
  rows: 24,
  scrollback: "saved output",
  updatedAt: "2026-08-17T12:00:00Z",
};

test("supervisor replay notice identifies saved agent sessions", () => {
  const unavailable = {
    available: false,
    socketPath: "",
    sessions: 0,
    runningSessions: 0,
    supervisorVersion: null,
    supervisorStale: false,
    appVersion: "0.4.15",
    message: "failed to locate halv-supervisor",
  };

  assert.deepEqual(
    supervisorReplayNotice([pane], unavailable),
    {
      message:
        "Halv could not start the terminal supervisor. Saved agent sessions will resume from their transcripts; in-flight work may be lost.",
      detail: "failed to locate halv-supervisor",
    },
  );
  assert.equal(
    supervisorReplayNotice([{ ...pane, agentKind: null }], unavailable),
    null,
  );
  assert.equal(
    supervisorReplayNotice([pane], {
      ...unavailable,
      available: true,
      supervisorVersion: "0.4.15",
      message: null,
    }),
    null,
  );
  assert.deepEqual(
    supervisorReplayNotice([pane], { ...unavailable, message: null }),
    {
      message:
        "Halv could not start the terminal supervisor. Saved agent sessions will resume from their transcripts; in-flight work may be lost.",
    },
  );
});

test("a supervisor from an older build shows in Settings, not in the panel", () => {
  const stale = {
    available: true,
    socketPath: "/tmp/supervisor.sock",
    sessions: 3,
    runningSessions: 3,
    supervisorVersion: "0.4.14",
    supervisorStale: true,
    appVersion: "0.4.15",
    message: "Supervisor is outdated (0.4.14). Restart it to use Halv 0.4.15.",
  };
  assert.equal(supervisorReplayNotice([pane], stale), null);
  assert.equal(
    supervisorReplayNotice([pane], {
      ...stale,
      supervisorVersion: "0.4.15",
      message: "Supervisor is from an older build. Restart it to update.",
    }),
    null,
  );
});

test("a protocol mismatch says the old agents still run and does not promise a resume", () => {
  const mismatch = {
    available: false,
    socketPath: "/tmp/supervisor.sock",
    sessions: 0,
    runningSessions: 0,
    supervisorVersion: null,
    supervisorStale: false,
    appVersion: "0.8.0",
    message: "failed to connect to supervisor: protocol 2 does not match 1",
    protocolMismatch: true,
  };
  assert.deepEqual(supervisorReplayNotice([pane], mismatch), {
    message:
      "Agents from an older Halv are still running. Halv cannot connect to them.",
    detail: "failed to connect to supervisor: protocol 2 does not match 1",
  });
  assert.equal(
    supervisorReplayNotice([{ ...pane, agentKind: null }], mismatch),
    null,
  );
});

test("legacy pane identity migrates to the session id", () => {
  const { paneId: _paneId, ...legacyPane } = pane;
  const restored = restoreTerminalPanel({
    layout: {
      version: 1,
      activeTabId: legacyPane.sessionId,
      activeLeafId: legacyPane.sessionId,
      tabs: [
        {
          tabId: legacyPane.sessionId,
          root: { type: "leaf", leafId: legacyPane.sessionId },
        },
      ],
    },
    panes: [legacyPane],
    updatedAt: legacyPane.updatedAt,
  });

  assert.equal(restored.panes[legacyPane.sessionId]?.paneId, legacyPane.sessionId);
  assert.equal(
    restored.paneSessionIds[legacyPane.sessionId],
    legacyPane.sessionId,
  );
  assert.deepEqual(restored.layout.tabs[0]?.root, {
    type: "leaf",
    leafId: legacyPane.sessionId,
  });
  assert.equal(restored.sessions[0]?.id, legacyPane.sessionId);
});

test("restore preserves Google provider pane kinds", () => {
  for (const agentKind of ["gemini", "antigravity"] as const) {
    const providerPane = {
      ...pane,
      paneId: `pane-${agentKind}`,
      sessionId: `terminal-${agentKind}`,
      agentKind,
      laneKind: "custom" as const,
    };
    const restored = restoreTerminalPanel({
      layout: {
        version: 1,
        activeTabId: providerPane.paneId,
        activeLeafId: providerPane.paneId,
        tabs: [
          {
            tabId: providerPane.paneId,
            root: { type: "leaf", leafId: providerPane.paneId },
          },
        ],
      },
      panes: [providerPane],
      updatedAt: providerPane.updatedAt,
    });

    assert.equal(restored.panes[providerPane.paneId]?.agentKind, agentKind);
    assert.equal(restored.sessions[0]?.agentKind, agentKind);
  }
});

test("resume changes only the pane session binding", () => {
  const stablePane = { ...pane, paneId: "pane-stable" };
  const restored = restoreTerminalPanel({
    layout: {
      version: 1,
      activeTabId: stablePane.paneId,
      activeLeafId: stablePane.paneId,
      tabs: [
        {
          tabId: stablePane.paneId,
          root: { type: "leaf", leafId: stablePane.paneId },
        },
      ],
    },
    panes: [stablePane],
    updatedAt: stablePane.updatedAt,
  });
  const layoutBeforeResume = structuredClone(restored.layout);
  const paneSessionIds = bindTerminalPaneSession(
    restored.paneSessionIds,
    stablePane.paneId,
    "terminal-resumed",
  );

  assert.equal(paneSessionIds[stablePane.paneId], "terminal-resumed");
  assert.equal(restored.panes[stablePane.paneId]?.paneId, stablePane.paneId);
  assert.deepEqual(restored.layout, layoutBeforeResume);
});

test("distinct pane ids preserve panes that share one CLI session id", () => {
  const first = { ...pane, paneId: "pane-first", sessionId: "terminal-first" };
  const second = { ...pane, paneId: "pane-second", sessionId: "terminal-second" };
  const restored = restoreTerminalPanel({
    layout: {
      version: 1,
      activeTabId: first.paneId,
      activeLeafId: second.paneId,
      tabs: [
        {
          tabId: first.paneId,
          root: {
            type: "split",
            direction: "vertical",
            first: { type: "leaf", leafId: first.paneId },
            second: { type: "leaf", leafId: second.paneId },
          },
        },
      ],
    },
    panes: [first, second],
    updatedAt: pane.updatedAt,
  });

  assert.deepEqual(Object.keys(restored.panes).sort(), [
    first.paneId,
    second.paneId,
  ]);
  assert.equal(restored.panes[first.paneId]?.cliSessionId, pane.cliSessionId);
  assert.equal(restored.panes[second.paneId]?.cliSessionId, pane.cliSessionId);
});

test("multi-scope restore keeps every pane session binding", () => {
  const second = {
    ...pane,
    paneId: "pane-second-scope",
    sessionId: "terminal-second-scope",
  };
  const panel = (candidate: TerminalPaneState) => ({
    layout: {
      version: 1,
      activeTabId: candidate.paneId,
      activeLeafId: candidate.paneId,
      tabs: [
        {
          tabId: candidate.paneId,
          root: { type: "leaf", leafId: candidate.paneId },
        },
      ],
    },
    panes: [candidate],
    updatedAt: candidate.updatedAt,
  });
  const restored = restoreTerminalPanels([
    { scope: "scope:first", panel: panel(pane) },
    { scope: "scope:second", panel: panel(second) },
  ]);

  assert.deepEqual(restored.paneSessionIds, {
    [pane.paneId]: pane.sessionId,
    [second.paneId]: second.sessionId,
  });
  assert.deepEqual(
    restored.sessions.map((session) => session.id).sort(),
    [pane.sessionId, second.sessionId].sort(),
  );
});

test("restore rebuilds split panes as exited sessions", () => {
  const second = {
    ...pane,
    paneId: "terminal-b",
    sessionId: "terminal-b",
    agentKind: "codex" as const,
  };
  const restored = restoreTerminalPanel({
    layout: {
      version: 1,
      activeTabId: "terminal-a",
      activeLeafId: "terminal-b",
      tabs: [
        {
          tabId: "terminal-a",
          root: {
            type: "split",
            direction: "vertical",
            first: { type: "leaf", leafId: "terminal-a" },
            second: { type: "leaf", leafId: "terminal-b" },
            ratio: 0.4,
          },
        },
      ],
    },
    panes: [pane, second],
    updatedAt: pane.updatedAt,
  });

  assert.deepEqual(
    restored.sessions.map((session) => [session.id, session.status, session.pid]),
    [
      ["terminal-a", "exited", 0],
      ["terminal-b", "exited", 0],
    ],
  );
  assert.equal(restored.panes["terminal-a"].scrollback, "saved output");
  assert.equal(restored.layout.activeLeafId, "terminal-b");
});

test("restore keeps layout leaves whose pane metadata is gone", () => {
  // A leaf without a saved pane can belong to a live session; pruning it
  // here rebuilt the layout from empty and flattened splits. The panel's
  // refresh removes leaves whose sessions are truly gone.
  const restored = restoreTerminalPanel({
    layout: {
      version: 1,
      activeTabId: "missing",
      activeLeafId: "missing",
      tabs: [
        {
          tabId: "missing",
          root: { type: "leaf", leafId: "missing" },
        },
      ],
    },
    panes: [pane],
    updatedAt: pane.updatedAt,
  });

  // One surface: the orphan leaf and the saved pane share a split now.
  assert.equal(restored.layout.tabs.length, 1);
  assert.deepEqual(
    restored.sessions.map((session) => session.id),
    ["terminal-a"],
  );
  assert.deepEqual(Object.keys(restored.panes), ["terminal-a"]);
});

test("restore returns exactly the panes bound to one conversation", () => {
  const second = {
    ...pane,
    paneId: "terminal-b",
    sessionId: "terminal-b",
    conversationId: "conversation-b",
  };
  const restored = restoreTerminalPanel(
    {
      layout: {
        version: 1,
        activeTabId: "terminal-a",
        activeLeafId: "terminal-b",
        tabs: [
          {
            tabId: "terminal-a",
            root: {
              type: "split",
              direction: "vertical",
              first: { type: "leaf", leafId: "terminal-a" },
              second: { type: "leaf", leafId: "terminal-b" },
            },
          },
        ],
      },
      panes: [pane, second],
      updatedAt: pane.updatedAt,
    },
    "conversation-a",
  );

  assert.deepEqual(
    restored.sessions.map((session) => session.id),
    ["terminal-a"],
  );
  assert.deepEqual(Object.keys(restored.panes), ["terminal-a"]);
  // The other conversation's leaf stays in the split (its session can be
  // live); only its pane metadata is excluded from this scope.
  assert.equal(restored.layout.tabs[0]!.root.type, "split");
});

test("pane persistence inherits the terminal conversation binding", () => {
  const input = terminalPaneStateInput(
    {
      id: "terminal-a",
      projectId: 7,
      conversationId: "conversation-a",
      projectName: "Halv",
      cwd: "/tmp/halv",
      shell: "/bin/zsh",
      pid: 123,
      executionProfile: "full_access",
      status: "running",
      cols: 100,
      rows: 24,
      startedAt: pane.updatedAt,
    },
    null,
    null,
    [],
  );

  assert.equal(input.conversationId, "conversation-a");
});

test("conversation restore adopts legacy unbound pane metadata", () => {
  const restored = restoreTerminalPanel(
    {
      layout: {
        version: 1,
        activeTabId: "terminal-a",
        activeLeafId: "terminal-a",
        tabs: [
          {
            tabId: "terminal-a",
            root: { type: "leaf", leafId: "terminal-a" },
          },
        ],
      },
      panes: [{ ...pane, conversationId: null }],
      updatedAt: pane.updatedAt,
    },
    "conversation-a",
  );

  assert.equal(restored.panes["terminal-a"].conversationId, "conversation-a");
});

test("resume replacement keeps the saved split position active", () => {
  const restored = restoreTerminalPanel({
    layout: {
      version: 1,
      activeTabId: "terminal-a",
      activeLeafId: "terminal-a",
      tabs: [
        {
          tabId: "terminal-a",
          root: { type: "leaf", leafId: "terminal-a" },
        },
      ],
    },
    panes: [pane],
    updatedAt: pane.updatedAt,
  });
  const replaced = replaceTerminalSessionId(
    restored.layout,
    "terminal-a",
    "terminal-live",
  );

  assert.equal(replaced.activeTabId, "terminal-live");
  assert.equal(replaced.activeLeafId, "terminal-live");
  assert.deepEqual(replaced.tabs[0].root, {
    type: "leaf",
    leafId: "terminal-live",
  });
});

test("replace removes a racing appended tab instead of duplicating the id", () => {
  // A session-list refresh appended the resumed session as its own tab
  // before the rename ran; the rename must not leave the id in two tabs.
  const replaced = replaceTerminalSessionId(
    {
      version: 1,
      activeTabId: "terminal-a",
      activeLeafId: "terminal-a",
      tabs: [
        {
          tabId: "terminal-a",
          root: {
            type: "split",
            direction: "vertical",
            first: { type: "leaf", leafId: "terminal-a" },
            second: { type: "leaf", leafId: "terminal-b" },
          },
        },
        {
          tabId: "terminal-live",
          root: { type: "leaf", leafId: "terminal-live" },
        },
      ],
    },
    "terminal-a",
    "terminal-live",
  );

  assert.equal(replaced.tabs.length, 1);
  assert.deepEqual(replaced.tabs[0].root, {
    type: "split",
    direction: "vertical",
    first: { type: "leaf", leafId: "terminal-live" },
    second: { type: "leaf", leafId: "terminal-b" },
  });
});

test("legacy restore keeps a shared CLI session id on only one pane", () => {
  const { paneId: _paneId, ...legacyPane } = pane;
  const second = { ...legacyPane, sessionId: "terminal-b" };
  const restored = restoreTerminalPanel({
    layout: {
      version: 1,
      activeTabId: "terminal-a",
      activeLeafId: "terminal-a",
      tabs: [
        {
          tabId: "terminal-a",
          root: {
            type: "split",
            direction: "vertical",
            first: { type: "leaf", leafId: "terminal-a" },
            second: { type: "leaf", leafId: "terminal-b" },
          },
        },
      ],
    },
    panes: [legacyPane, second],
    updatedAt: pane.updatedAt,
  });

  assert.equal(restored.panes["terminal-a"]!.cliSessionId, "claude-session");
  assert.equal(restored.panes["terminal-b"]!.cliSessionId, null);
});

test("saved title layers survive into the resume title state", () => {
  const titled: TerminalPaneState = {
    ...pane,
    defaultTitle: "Codex 3",
    generatedTitle: "Explain the codebase",
    generation: 2,
  };

  assert.deepEqual(terminalPaneTitleState(titled), {
    defaultTitle: "Codex 3",
    generatedTitle: "Explain the codebase",
    aiVaultTitle: undefined,
    customTitle: "Release checks",
    generation: 2,
  });
});

test("a pane without saved title layers falls back to the agent name", () => {
  const untitled: TerminalPaneState = { ...pane, customTitle: null };

  assert.equal(terminalPaneTitleState(untitled).defaultTitle, "Claude");
});

test("a restored Grok pane keeps its agent fallback title", () => {
  const grokPane: TerminalPaneState = {
    ...pane,
    agentKind: "grok",
    laneKind: "grokApiKey",
    customTitle: null,
  };

  assert.equal(terminalPaneTitleState(grokPane).defaultTitle, "Grok");
});

test("resume restores the saved subscription lane", () => {
  assert.deepEqual(resumeLaneForTerminalPane(pane, []), {
    kind: "claudeSubscription",
  });
});

test("resume restores an active saved credential profile", () => {
  const apiPane: TerminalPaneState = {
    ...pane,
    laneKind: "claudeApiKey",
    credentialProfileId: "profile-1",
  };
  const laneOptions: AgentSessionLaneOptions = [
    {
      agentKind: "claude",
      lanes: [
        {
          kind: "claudeApiKey",
          loggedIn: null,
          credentialProfiles: [
            {
              id: "profile-1",
              provider: "anthropic",
              name: "Work",
              status: "active",
              priority: 0,
              createdAt: "2026-08-17T00:00:00Z",
              updatedAt: "2026-08-17T00:00:00Z",
            },
          ],
        },
      ],
    },
  ];

  assert.deepEqual(resumeLaneForTerminalPane(apiPane, laneOptions), {
    kind: "claudeApiKey",
    credentialProfileId: "profile-1",
  });
});

test("restore collapses ghost-resume clones back to the saved split", () => {
  // Exact corrupted shape from 2026-08-19: the real split plus each pane
  // duplicated as a loose tab under the same CLI session id.
  const { paneId: _paneId, ...legacyPane } = pane;
  const split = { ...legacyPane, sessionId: "real-a", cliSessionId: "cli-1" };
  const splitTwo = { ...legacyPane, sessionId: "real-b", cliSessionId: "cli-2" };
  const cloneA = { ...legacyPane, sessionId: "ghost-a", cliSessionId: "cli-1" };
  const cloneB = { ...legacyPane, sessionId: "ghost-b", cliSessionId: "cli-2" };
  const restored = restoreTerminalPanel({
    layout: {
      version: 1,
      activeTabId: "real-a",
      activeLeafId: "real-a",
      tabs: [
        {
          tabId: "real-a",
          root: {
            type: "split",
            direction: "vertical",
            first: { type: "leaf", leafId: "real-a" },
            second: { type: "leaf", leafId: "real-b" },
          },
        },
        { tabId: "ghost-a", root: { type: "leaf", leafId: "ghost-a" } },
        { tabId: "ghost-b", root: { type: "leaf", leafId: "ghost-b" } },
      ],
    },
    panes: [cloneA, split, splitTwo, cloneB],
    updatedAt: pane.updatedAt,
  });

  assert.equal(restored.layout.tabs.length, 1);
  assert.deepEqual(restored.layout.tabs[0]!.root, {
    type: "split",
    direction: "vertical",
    first: { type: "leaf", leafId: "real-a" },
    second: { type: "leaf", leafId: "real-b" },
    ratio: undefined,
  });
  assert.deepEqual(Object.keys(restored.panes).sort(), ["real-a", "real-b"]);
});
