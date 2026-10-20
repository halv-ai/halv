import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  agentLaunchCwd,
  effectiveWorkspaceId,
  readLastActiveConversationId,
  readTranscriptScrollState,
  resolveLastActiveConversationId,
  terminalPanelStorageKey,
  writeLastActiveConversationId,
  writeTranscriptScrollState,
} from "../src/conversationWorkspace.ts";

function memoryStorage() {
  const values = new Map<string, string>();
  return {
    getItem(key: string) {
      return values.get(key) ?? null;
    },
    setItem(key: string, value: string) {
      values.set(key, value);
    },
  };
}

test("terminal persistence uses a separate key for each conversation", () => {
  const common = {
    agentSessionId: null,
    workspaceId: "workspace-1",
    projectId: 7,
  };
  const first = terminalPanelStorageKey({
    ...common,
    conversationId: "conversation-a",
  });
  const second = terminalPanelStorageKey({
    ...common,
    conversationId: "conversation-b",
  });
  const outsideConversation = terminalPanelStorageKey({
    ...common,
    conversationId: null,
  });

  assert.equal(first, "halv.terminal-layout.v1:conversation:conversation-a");
  assert.equal(second, "halv.terminal-layout.v1:conversation:conversation-b");
  assert.equal(outsideConversation, "halv.terminal-layout.v1:workspace:workspace-1");
  assert.notEqual(first, second);
});

test("a shell tab panel has its own scope, and a conversation wins over it", () => {
  const common = { workspaceId: "workspace-1", projectId: 7, agentSessionId: "agent-1" };
  assert.equal(
    terminalPanelStorageKey({ ...common, conversationId: null, shellTabId: "tab-1" }),
    "halv.terminal-layout.v1:shell:tab-1",
  );
  assert.equal(
    terminalPanelStorageKey({ ...common, conversationId: "c1", shellTabId: "tab-1" }),
    "halv.terminal-layout.v1:conversation:c1",
  );
  // A shell tab wins over the agent, workspace and project scopes.
  assert.equal(
    terminalPanelStorageKey({ ...common, conversationId: null, shellTabId: null }),
    "halv.terminal-layout.v1:agent:agent-1",
  );
  assert.equal(
    terminalPanelStorageKey({ conversationId: null, workspaceId: null, projectId: null }),
    "halv.terminal-layout.v1:panel:global",
  );
});

test("last-active relaunch selects only an existing conversation", () => {
  const storage = memoryStorage();
  writeLastActiveConversationId("conversation-b", storage);

  const saved = readLastActiveConversationId(storage);
  assert.equal(
    resolveLastActiveConversationId(saved, ["conversation-a", "conversation-b"]),
    "conversation-b",
  );
  assert.equal(resolveLastActiveConversationId(saved, ["conversation-a"]), null);
});

test("transcript scroll state is isolated by conversation", () => {
  const storage = memoryStorage();
  writeTranscriptScrollState(
    "conversation-a",
    { top: 320, pinnedToBottom: false },
    storage,
  );
  writeTranscriptScrollState(
    "conversation-b",
    { top: 900, pinnedToBottom: true },
    storage,
  );

  assert.deepEqual(readTranscriptScrollState("conversation-a", storage), {
    version: 1,
    top: 320,
    pinnedToBottom: false,
  });
  assert.deepEqual(readTranscriptScrollState("conversation-b", storage), {
    version: 1,
    top: 900,
    pinnedToBottom: true,
  });
});

test("a conversation lives in its bound workspace, else in the primary one", () => {
  assert.equal(
    effectiveWorkspaceId({ projectId: 7, workspaceId: "worktree-a" }),
    "worktree-a",
  );
  assert.equal(effectiveWorkspaceId({ projectId: 7, workspaceId: null }), "primary-7");
  assert.equal(effectiveWorkspaceId({ projectId: 7 }), "primary-7");
  assert.equal(effectiveWorkspaceId({ projectId: 7, workspaceId: "  " }), "primary-7");
  assert.equal(effectiveWorkspaceId({ projectId: null, workspaceId: "worktree-a" }), null);
  assert.equal(effectiveWorkspaceId({ workspaceId: null }), null);
});

test("an unknown bound workspace falls back to the primary workspace", () => {
  const known = new Set(["primary-7", "worktree-a"]);
  assert.equal(
    effectiveWorkspaceId({ projectId: 7, workspaceId: "worktree-a" }, known),
    "worktree-a",
  );
  assert.equal(
    effectiveWorkspaceId({ projectId: 7, workspaceId: "worktree-gone" }, known),
    "primary-7",
  );
});

test("a fresh agent starts in the root of the conversation workspace", () => {
  const project = { id: 7, rootPath: "/repo" };
  assert.equal(
    agentLaunchCwd({
      conversation: { projectId: 7, projectRootPath: "/data/worktrees/a/repo" },
      project,
    }),
    "/data/worktrees/a/repo",
  );
  assert.equal(
    agentLaunchCwd({ conversation: { projectId: 7, projectRootPath: "/repo" }, project }),
    "/repo",
  );
  assert.equal(agentLaunchCwd({ conversation: null, project }), "/repo");
  assert.equal(
    agentLaunchCwd({ conversation: { projectId: 7, projectRootPath: null }, project }),
    "/repo",
  );
  // A summary of another project never moves the agent out of its project.
  assert.equal(
    agentLaunchCwd({
      conversation: { projectId: 8, projectRootPath: "/other" },
      project,
    }),
    "/repo",
  );
});

test("the Pro send path starts a fresh agent in the workspace root", () => {
  const source = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
  const send = source.indexOf("async function sendIntoConversationTerminal(");
  const end = source.indexOf("async function watchTerminalOpening(", send);
  assert.notEqual(send, -1);
  assert.notEqual(end, -1);
  const body = source.slice(send, end);
  assert.match(body, /cwd: agentLaunchCwd\(\{ conversation: conversationSummary, project: project! \}\)/);
  assert.match(body, /conversationSummary = summary;/);
  assert.doesNotMatch(body, /cwd: project!\.rootPath/);
});
