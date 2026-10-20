import assert from "node:assert/strict";
import test from "node:test";

import {
  createConversationTurnStore,
  emptyConversationTurnStoreState,
  reduceChatDelta,
  type ConversationTurnEffects,
  type PendingNewTurn,
  type TurnState,
} from "../src/conversationTurnStore.ts";
import type {
  BranchStatus,
  ChatDeltaPayload,
  UiMessage,
} from "../src/types.ts";
import { NEW_THREAD } from "../src/types.ts";

const conversationId = "conversation-1";

function context() {
  return {
    currentConversationId: conversationId,
    deletedConversationIds: new Set<string>(),
    navigationGeneration: 1,
    chatRequestFailedText: "Chat request failed",
    now: "2026-09-01T12:00:00.000Z",
  };
}

test("folds a realistic chat delta stream and preserves effect order", async () => {
  let emit: ((delta: ChatDeltaPayload) => void) | null = null;
  let messages: UiMessage[] = [];
  let branchStatus: BranchStatus | null = null;
  const calls: string[] = [];
  const effects: ConversationTurnEffects = {
    setMessages(updater) {
      messages = updater(messages);
      calls.push("setMessages");
    },
    openConversation(nextConversationId) {
      calls.push(`openConversation:${nextConversationId}`);
    },
    setChatError(message) {
      calls.push(`setChatError:${message ?? "null"}`);
    },
    setBranchStatus(updater) {
      branchStatus = updater(branchStatus);
      calls.push("setBranchStatus");
    },
    setConversationRunning(nextConversationId, running) {
      calls.push(`setConversationRunning:${nextConversationId}:${running}`);
    },
    loadConversations() {
      calls.push("loadConversations");
    },
    refreshStatus() {
      calls.push("refreshStatus");
    },
    refreshSavingsTotals() {
      calls.push("refreshSavingsTotals");
    },
    loadSubagents(nextConversationId) {
      calls.push(`loadSubagents:${nextConversationId}`);
    },
    loadProjects() {
      calls.push("loadProjects");
    },
    refreshAgentSessions() {
      calls.push("refreshAgentSessions");
    },
    notifyTerminal(delta) {
      calls.push(`notifyTerminal:${delta.kind}`);
    },
    notifyApproval(delta) {
      calls.push(`notifyApproval:${delta.callId}`);
    },
    handleGoalSignal(delta) {
      calls.push(`handleGoalSignal:${delta.goalSignal ?? "none"}`);
    },
    migrateConversationTurnState(fromConversationId, toConversationId) {
      calls.push(
        `migrateConversationTurnState:${fromConversationId ?? "new"}:${toConversationId}`,
      );
    },
    switchComposerDraft(nextConversationId) {
      calls.push(`switchComposerDraft:${nextConversationId}`);
    },
    requestChatCancel(nextConversationId) {
      calls.push(`requestChatCancel:${nextConversationId}`);
    },
    requestChatCancelAndRevert(nextConversationId) {
      calls.push(`requestChatCancelAndRevert:${nextConversationId}`);
    },
    dispatchQueuedNewThreadMessage() {
      calls.push("dispatchQueuedNewThreadMessage");
    },
  };
  const store = createConversationTurnStore({
    subscribe: async (handler) => {
      emit = handler;
      return () => {
        emit = null;
      };
    },
    effects,
    context,
  });

  store.setConversationRunning(conversationId, true);
  await store.connect();
  assert.ok(emit);
  const push = emit as (delta: ChatDeltaPayload) => void;
  const stream: ChatDeltaPayload[] = [
    { conversationId, kind: "delta", text: "Hello" },
    { conversationId, kind: "delta", text: " world" },
    { conversationId, kind: "reasoning", text: "Inspect the event route." },
    {
      conversationId,
      kind: "tool_call",
      callId: "call-1",
      command: "pnpm test:unit",
      toolKind: "shell",
      cwd: "/tmp/project",
      status: "running",
    },
    {
      conversationId,
      kind: "tool_output",
      callId: "call-1",
      tail: "tests started\n",
    },
    {
      conversationId,
      kind: "tool_result",
      callId: "call-1",
      status: "completed",
      exitCode: 0,
      wallTimeMs: 86,
      truncatedOutput: false,
    },
    {
      conversationId,
      kind: "tool_call",
      callId: "approval-1",
      command: "git status --short",
      toolKind: "shell",
      cwd: "/tmp/project",
      status: "awaiting_approval",
    },
    {
      conversationId,
      kind: "turn_diff",
      turnId: "turn-1",
      diff: "diff --git a/a.ts b/a.ts\n+export const ready = true;",
      cwd: "/tmp/project",
      revertible: true,
    },
    {
      conversationId,
      kind: "task_list",
      explanation: "Verify the extraction.",
      plan: [{ step: "Run unit tests", status: "completed" }],
    },
    {
      conversationId,
      kind: "done",
      usage: {
        inputTokens: 3_842,
        outputTokens: 786,
        totalTokens: 4_628,
        cachedTokens: 1_280,
        reasoningTokens: 312,
      },
    },
  ];
  for (const delta of stream) {
    push(delta);
  }

  const snapshot = store.getSnapshot();
  assert.equal(snapshot.streamBuffersByConversation.get(conversationId), "Hello world");
  assert.equal(snapshot.reasoningByConversation[conversationId], "Inspect the event route.");
  assert.deepEqual(snapshot.toolCallsByConversation[conversationId], [
    {
      callId: "call-1",
      toolKind: "shell",
      command: "pnpm test:unit",
      cwd: "/tmp/project",
      status: "completed",
      tail: "tests started\n",
      exitCode: 0,
      wallTimeMs: 86,
      truncatedOutput: false,
      rtkRoute: undefined,
      fileChanges: undefined,
      warning: undefined,
      justification: undefined,
      elicitationMode: undefined,
      elicitationSchema: undefined,
      elicitationUrl: undefined,
    },
    {
      callId: "approval-1",
      toolKind: "shell",
      command: "git status --short",
      cwd: "/tmp/project",
      status: "awaiting_approval",
      tail: "",
      exitCode: undefined,
      wallTimeMs: undefined,
      truncatedOutput: undefined,
      rtkRoute: undefined,
      fileChanges: undefined,
      warning: undefined,
      justification: undefined,
      elicitationMode: undefined,
      elicitationSchema: undefined,
      elicitationUrl: undefined,
    },
  ]);
  assert.equal(snapshot.pendingApprovals[conversationId]?.callId, "approval-1");
  assert.equal(snapshot.turnDiffsByConversation[conversationId]?.[0]?.turnId, "turn-1");
  assert.equal(snapshot.turnDiffsByConversation[conversationId]?.[0]?.createdAt, context().now);
  assert.deepEqual(snapshot.taskPlansByConversation[conversationId], {
    explanation: "Verify the extraction.",
    plan: [{ step: "Run unit tests", status: "completed" }],
  });
  assert.equal(snapshot.runningConversationIds.has(conversationId), false);
  assert.equal(messages.length, 1);
  assert.equal(messages[0]?.content, "Hello world");
  assert.equal(messages[0]?.streaming, false);
  assert.deepEqual(messages[0]?.usage, stream.at(-1)?.usage);
  assert.deepEqual(
    calls.slice(-7),
    [
      "notifyTerminal:done",
      "handleGoalSignal:none",
      `setConversationRunning:${conversationId}:false`,
      "setMessages",
      "loadConversations",
      "refreshStatus",
      "refreshSavingsTotals",
    ],
  );
  assert.equal(calls.at(-1), "refreshSavingsTotals");
  assert.ok(
    calls.indexOf(`setConversationRunning:${conversationId}:false`) <
      calls.lastIndexOf("loadConversations"),
  );

  store.disconnect();
  assert.equal(emit, null);
});

test("reduces every chat delta kind", async (t) => {
  await t.test("delta", () => {
    const result = reduceChatDelta(
      emptyConversationTurnStoreState(),
      { conversationId, kind: "delta", text: "A" },
      context(),
    );

    assert.equal(result.state.streamBuffersByConversation.get(conversationId), "A");
    assert.deepEqual(result.effects.map((effect) => effect.type), [
      "appendStreamingAssistant",
    ]);
  });

  await t.test("reasoning", () => {
    const result = reduceChatDelta(
      emptyConversationTurnStoreState(),
      { conversationId, kind: "reasoning", text: "Think" },
      context(),
    );

    assert.equal(result.state.reasoningByConversation[conversationId], "Think");
    assert.deepEqual(result.effects, []);
  });

  await t.test("done", () => {
    const state = emptyConversationTurnStoreState();
    state.runningConversationIds.add(conversationId);
    const result = reduceChatDelta(
      state,
      { conversationId, kind: "done" },
      context(),
    );

    assert.equal(result.state.runningConversationIds.has(conversationId), false);
    assert.deepEqual(result.effects.map((effect) => effect.type), [
      "notifyTerminal",
      "handleGoalSignal",
      "setConversationRunning",
      "finishStreamingAssistant",
      "loadConversations",
      "refreshStatus",
      "refreshSavingsTotals",
    ]);
  });

  await t.test("error", () => {
    const result = reduceChatDelta(
      emptyConversationTurnStoreState(),
      { conversationId, kind: "error", text: "Failure" },
      context(),
    );

    assert.deepEqual(result.effects.map((effect) => effect.type), [
      "notifyTerminal",
      "setConversationRunning",
      "finishStreamingAssistant",
      "setChatError",
      "loadConversations",
      "refreshStatus",
      "refreshSavingsTotals",
    ]);
  });

  await t.test("tool_call", () => {
    const result = reduceChatDelta(
      emptyConversationTurnStoreState(),
      {
        conversationId,
        kind: "tool_call",
        callId: "call-1",
        toolKind: "subagent",
        status: "awaiting_approval",
      },
      context(),
    );

    assert.equal(result.state.pendingApprovals[conversationId]?.callId, "call-1");
    assert.deepEqual(result.effects.map((effect) => effect.type), [
      "notifyApproval",
      "loadSubagents",
    ]);
  });

  await t.test("tool_output", () => {
    const start = reduceChatDelta(
      emptyConversationTurnStoreState(),
      {
        conversationId,
        kind: "tool_call",
        callId: "call-1",
        toolKind: "shell",
      },
      context(),
    ).state;
    const result = reduceChatDelta(
      start,
      {
        conversationId,
        kind: "tool_output",
        callId: "call-1",
        tail: "output",
      },
      context(),
    );

    assert.equal(result.state.toolCallsByConversation[conversationId]?.[0]?.tail, "output");
  });

  await t.test("tool_result", () => {
    const start = reduceChatDelta(
      emptyConversationTurnStoreState(),
      {
        conversationId,
        kind: "tool_call",
        callId: "call-1",
        toolKind: "shell",
        status: "awaiting_approval",
      },
      context(),
    ).state;
    const result = reduceChatDelta(
      start,
      {
        conversationId,
        kind: "tool_result",
        callId: "call-1",
        status: "completed",
        exitCode: 0,
      },
      context(),
    );

    assert.equal(result.state.pendingApprovals[conversationId], undefined);
    assert.equal(
      result.state.toolCallsByConversation[conversationId]?.[0]?.status,
      "completed",
    );
    assert.deepEqual(result.effects.map((effect) => effect.type), [
      "loadSubagents",
    ]);
  });

  await t.test("turn_diff", () => {
    const result = reduceChatDelta(
      emptyConversationTurnStoreState(),
      {
        conversationId,
        kind: "turn_diff",
        turnId: "turn-1",
        diff: "+line",
      },
      context(),
    );

    assert.equal(
      result.state.turnDiffsByConversation[conversationId]?.[0]?.createdAt,
      context().now,
    );
  });

  await t.test("branch_status", () => {
    const result = reduceChatDelta(
      emptyConversationTurnStoreState(),
      { conversationId, kind: "branch_status", branch: "feature" },
      context(),
    );

    assert.deepEqual(result.effects.map((effect) => effect.type), [
      "setBranchStatus",
      "loadProjects",
    ]);
  });

  await t.test("conversation_updated", () => {
    const result = reduceChatDelta(
      emptyConversationTurnStoreState(),
      { conversationId, kind: "conversation_updated" },
      context(),
    );

    assert.deepEqual(result.effects.map((effect) => effect.type), [
      "loadConversations",
      "refreshAgentSessions",
    ]);
  });

  await t.test("context_compacted", () => {
    const compaction = {
      id: 1,
      conversationId,
      uptoMessageId: 4,
      summary: "Earlier context",
      createdAt: context().now,
    };
    const result = reduceChatDelta(
      emptyConversationTurnStoreState(),
      { conversationId, kind: "context_compacted", compaction },
      context(),
    );

    assert.deepEqual(result.state.compactionsByConversation[conversationId], compaction);
  });

  await t.test("task_list", () => {
    const plan = [{ step: "Verify", status: "in_progress" as const }];
    const result = reduceChatDelta(
      emptyConversationTurnStoreState(),
      { conversationId, kind: "task_list", explanation: "Check", plan },
      context(),
    );

    assert.deepEqual(result.state.taskPlansByConversation[conversationId], {
      explanation: "Check",
      plan,
    });
  });
});

test("adopts the first unknown delta for a pending new turn", () => {
  const state = emptyConversationTurnStoreState();
  const pendingNewTurn: PendingNewTurn = {
    conversationId: null,
    freshViewRequested: false,
    cancelDispatched: false,
    navigationGeneration: 4,
    knownConversationIds: new Set(["known-conversation"]),
  };
  const turnState: TurnState = {
    startedAt: 1,
    pendingNewTurn,
    cancelRequested: false,
    normalCancelRequested: false,
    cancelAndRevertRequested: false,
    cancelAndRevertDispatched: false,
    cancelAndRevertPending: false,
    cancelAndRevertTurn: false,
    cancelAndRevertDrain: false,
    stopAndRevertAvailable: true,
  };
  state.conversationTurnStates.set(NEW_THREAD, turnState);
  state.runningConversationIds.add(NEW_THREAD);

  const result = reduceChatDelta(
    state,
    { conversationId: "new-conversation", kind: "delta", text: "Hello" },
    {
      ...context(),
      currentConversationId: null,
      navigationGeneration: 4,
    },
  );

  assert.equal(result.state.conversationTurnStates.has(NEW_THREAD), false);
  assert.equal(
    result.state.conversationTurnStates.get("new-conversation")?.pendingNewTurn
      ?.conversationId,
    "new-conversation",
  );
  assert.equal(result.state.runningConversationIds.has(NEW_THREAD), false);
  assert.equal(result.state.runningConversationIds.has("new-conversation"), true);
  assert.equal(result.state.lastNewThreadTurnId, "new-conversation");
  assert.equal(pendingNewTurn.conversationId, null);
  assert.deepEqual(result.effects.map((effect) => effect.type), [
    "migrateConversationTurnState",
    "loadConversations",
    "switchComposerDraft",
    "openConversation",
    "dispatchQueuedNewThreadMessage",
    "appendStreamingAssistant",
  ]);
});

test("ignores non-terminal deltas for a deleted conversation", () => {
  const state = emptyConversationTurnStoreState();
  const result = reduceChatDelta(
    state,
    { conversationId, kind: "delta", text: "ignored" },
    {
      ...context(),
      deletedConversationIds: new Set([conversationId]),
    },
  );

  assert.equal(result.state, state);
  assert.deepEqual(result.effects, []);
});
