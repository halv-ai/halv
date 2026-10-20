import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  buildTerminalRestoreWritePayload,
  isTerminalResumeFailure,
  TERMINAL_RESUME_FAILURE_WINDOW_MS,
  type TerminalPaneState,
  type TerminalResumeAttempt,
} from "../src/terminalRestore.ts";
import type { TerminalSession } from "../src/types.ts";

const createdAtMs = Date.parse("2026-08-22T12:00:00Z");

const attempt: TerminalResumeAttempt = {
  createdAtMs,
  createdByResume: true,
  userWroteInput: false,
};

test("code zero within 20 seconds marks a resume failure", () => {
  assert.equal(
    isTerminalResumeFailure(attempt, {
      exitCode: 0,
      exitedAtMs: createdAtMs + TERMINAL_RESUME_FAILURE_WINDOW_MS,
    }),
    true,
  );
});

test("user input prevents a resume failure", () => {
  assert.equal(
    isTerminalResumeFailure(
      { ...attempt, userWroteInput: true },
      { exitCode: 1, exitedAtMs: createdAtMs + 1_000 },
    ),
    false,
  );
});

test("an exit after 20 seconds is not a resume failure", () => {
  assert.equal(
    isTerminalResumeFailure(attempt, {
      exitCode: 0,
      exitedAtMs: createdAtMs + TERMINAL_RESUME_FAILURE_WINDOW_MS + 1,
    }),
    false,
  );
});

test("restore write keeps failed pane metadata", () => {
  const failedPane: TerminalPaneState = {
    paneId: "pane-failed",
    sessionId: "terminal-failed",
    projectId: 7,
    conversationId: "conversation-a",
    projectName: "Halv",
    cwd: "/tmp/halv",
    executionProfile: "full_access",
    agentKind: "codex",
    laneKind: "codexChatGpt",
    credentialProfileId: null,
    customTitle: "Release checks",
    defaultTitle: "Codex",
    generatedTitle: "Verify release",
    aiVaultTitle: null,
    generation: 3,
    cliSessionId: "01a02076-resume",
    cols: 120,
    rows: 32,
    scrollback: "resume output",
    updatedAt: "2026-08-22T12:00:10Z",
  };
  const failedSession: TerminalSession = {
    id: failedPane.sessionId,
    title: failedPane.customTitle,
    projectId: failedPane.projectId,
    conversationId: failedPane.conversationId,
    projectName: failedPane.projectName,
    cwd: failedPane.cwd,
    shell: "codex",
    pid: 123,
    executionProfile: "full_access",
    agentKind: "codex",
    status: "exited",
    cols: failedPane.cols,
    rows: failedPane.rows,
    startedAt: "2026-08-22T12:00:00Z",
    exitedAt: failedPane.updatedAt,
    exitCode: 0,
  };
  const layout = {
    version: 1 as const,
    activeTabId: failedPane.paneId,
    activeLeafId: failedPane.paneId,
    tabs: [
      {
        tabId: failedPane.paneId,
        root: { type: "leaf" as const, leafId: failedPane.paneId },
      },
    ],
  };

  const payload = buildTerminalRestoreWritePayload({
    scope: "conversation:conversation-a",
    layout,
    sessions: [failedSession],
    paneSessionIds: { [failedPane.paneId]: failedPane.sessionId },
    agentEntries: [],
    restoredPanes: { [failedPane.paneId]: failedPane },
    resumeFallbackPanes: {},
    laneOptions: [],
  });

  assert.equal(payload.panes.length, 1);
  assert.deepEqual(payload.layout, layout);
  assert.deepEqual(payload.panes[0], {
    paneId: failedPane.paneId,
    sessionId: failedPane.sessionId,
    projectId: failedPane.projectId,
    conversationId: failedPane.conversationId,
    projectName: failedPane.projectName,
    cwd: failedPane.cwd,
    executionProfile: failedPane.executionProfile,
    agentKind: failedPane.agentKind,
    laneKind: failedPane.laneKind,
    credentialProfileId: failedPane.credentialProfileId,
    customTitle: failedPane.customTitle,
    defaultTitle: failedPane.defaultTitle,
    generatedTitle: failedPane.generatedTitle,
    aiVaultTitle: failedPane.aiVaultTitle,
    generation: failedPane.generation,
    cliSessionId: failedPane.cliSessionId,
    cols: failedPane.cols,
    rows: failedPane.rows,
  });
});
