import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  IDLE_TERMINAL_OPENING,
  openPlainTerminalSession,
  reduceTerminalOpening,
  terminalOpeningNotice,
} from "../src/terminalOpening.ts";
import type { TerminalSession } from "../src/types.ts";

test("opens a plain terminal with the shared command parameters", async () => {
  const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
  const session = { id: "terminal-1" } as TerminalSession;
  const result = await openPlainTerminalSession(
    async (command, args) => {
      calls.push({ command, args });
      return session;
    },
    {
      projectId: 7,
      workspaceId: "workspace-1",
      conversationId: "conversation-1",
    },
  );

  assert.equal(result, session);
  assert.deepEqual(calls, [
    {
      command: "terminal_open",
      args: {
        projectId: 7,
        workspaceId: "workspace-1",
        conversationId: "conversation-1",
        cols: 100,
        rows: 24,
        executionProfile: "full_access",
      },
    },
  ]);
});

test("a shell tab names itself as the owner of its shell", async () => {
  const calls: Array<Record<string, unknown>> = [];
  await openPlainTerminalSession(
    async (_command, args) => {
      calls.push(args);
      return { id: "terminal-2" } as TerminalSession;
    },
    {
      projectId: 7,
      workspaceId: "primary-7",
      conversationId: null,
      shellTabId: "tab-1",
    },
  );
  assert.equal(calls[0]?.shellTabId, "tab-1");
  assert.equal(calls[0]?.conversationId, null);
});

test("moves terminal opening feedback from pending to attached", () => {
  const opening = reduceTerminalOpening(IDLE_TERMINAL_OPENING, {
    type: "start",
  });
  assert.equal(terminalOpeningNotice(opening), "Opening terminal…");

  const attached = reduceTerminalOpening(opening, { type: "attach" });
  assert.equal(
    terminalOpeningNotice(attached),
    "Continued in terminal — resumed session",
  );
});

test("keeps a terminal opening error visible and ignores stale completion", () => {
  const opening = reduceTerminalOpening(IDLE_TERMINAL_OPENING, {
    type: "start",
  });
  const failed = reduceTerminalOpening(opening, {
    type: "fail",
    message: "Claude login is required.",
  });
  assert.equal(
    terminalOpeningNotice(failed),
    "Terminal failed to open — Claude login is required.",
  );
  assert.deepEqual(
    reduceTerminalOpening(failed, { type: "attach" }),
    failed,
  );
});
