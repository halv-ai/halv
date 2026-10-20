import assert from "node:assert/strict";
import test from "node:test";
import { subagentActivities, subagentStatusLabel } from "../src/subagents.ts";
import type { SubagentThread, ToolCallUi } from "../src/types.ts";

function toolCall(overrides: Partial<ToolCallUi>): ToolCallUi {
  return {
    callId: "call-1",
    toolKind: "subagent",
    command: "/root/audit",
    cwd: "/repo",
    status: "running",
    tail: "",
    ...overrides,
  };
}

test("extracts real agent records and ignores coordinator operations", () => {
  const result = subagentActivities([
    toolCall({ callId: "spawn-call", command: "spawn_agent", status: "completed" }),
    toolCall({
      callId: "agent-1",
      command: "/root/audit",
      justification: "Review the auth boundary",
      status: "running",
    }),
  ]);

  assert.deepEqual(result, [
    {
      agentId: "agent-1",
      taskName: "/root/audit",
      task: "Review the auth boundary",
      status: "running",
      output: "",
      active: true,
      failed: false,
    },
  ]);
});

test("keeps the latest state for a stable agent id", () => {
  const result = subagentActivities([
    toolCall({ callId: "agent-1", status: "pending" }),
    toolCall({
      callId: "agent-1",
      status: "completed",
      tail: "Implemented and verified.",
    }),
  ]);

  assert.equal(result.length, 1);
  assert.equal(result[0].status, "completed");
  assert.equal(result[0].active, false);
  assert.equal(result[0].output, "Implemented and verified.");
  assert.equal(subagentStatusLabel(result[0].status), "Completed");
});

test("normalizes failed and interrupted terminal states", () => {
  const result = subagentActivities([
    toolCall({ callId: "agent-1", status: "failed" }),
    toolCall({ callId: "agent-2", command: "/root/tests", status: "cancelled" }),
  ]);

  assert.deepEqual(
    result.map(({ status, active, failed }) => ({ status, active, failed })),
    [
      { status: "errored", active: false, failed: true },
      { status: "interrupted", active: false, failed: true },
    ],
  );
});

test("hydrates durable threads and lets live events override their state", () => {
  const thread: SubagentThread = {
    agentId: "agent-1",
    conversationId: "conversation-1",
    turnId: "turn-1",
    taskName: "/root/audit",
    task: "Review persistence",
    status: "completed",
    result: "Persisted result",
    error: null,
    generation: 1,
    createdAt: "2026-07-13T10:00:00Z",
    updatedAt: "2026-07-13T10:01:00Z",
    messages: [],
  };

  const result = subagentActivities(
    [toolCall({ callId: "agent-1", status: "running", tail: "Live output" })],
    [thread],
  );

  assert.equal(result.length, 1);
  assert.equal(result[0].task, "Review persistence");
  assert.equal(result[0].status, "running");
  assert.equal(result[0].output, "Live output");
});
