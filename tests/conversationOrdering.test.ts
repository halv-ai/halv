import assert from "node:assert/strict";
import test from "node:test";
import {
  conversationBuckets,
  sortConversations,
} from "../src/conversationOrdering.ts";
import type { ConversationSummary } from "../src/types";

function conversation(
  id: string,
  updatedAt: string,
  pinned = false,
): ConversationSummary {
  return {
    id,
    title: id,
    titleSource: "generated",
    mode: "chat",
    model: "gpt-5.6-sol",
    createdAt: updatedAt,
    updatedAt,
    pinned,
    draft: "",
    draftOnly: false,
    archivedAt: null,
    memoriesMode: null,
    fastMode: false,
    goal: null,
    agentMode: false,
    workflowMode: "default",
    credentialProfileId: null,
    agentBackend: null,
    agentSessionModel: null,
    projectId: null,
    projectName: null,
    projectRootPath: null,
    projectTrustLevel: null,
    approvalPolicyOverride: null,
    messageCount: 0,
  };
}

test("pinned conversations stay above newer activity", () => {
  const rows = sortConversations([
    conversation("new", "2026-08-20T12:00:00Z"),
    conversation("pinned", "2026-08-18T12:00:00Z", true),
    conversation("older", "2026-08-19T12:00:00Z"),
  ]);

  assert.deepEqual(
    rows.map((row) => row.id),
    ["pinned", "new", "older"],
  );
});

test("pinned conversations get their own bucket", () => {
  const buckets = conversationBuckets(
    [
      conversation("today", "2026-08-20T12:00:00Z"),
      conversation("pinned", "2026-08-18T12:00:00Z", true),
    ],
    new Date("2026-08-20T16:00:00Z"),
  );

  assert.deepEqual(
    buckets.map((bucket) => [bucket.label, bucket.conversations[0]?.id]),
    [
      ["PINNED", "pinned"],
      ["TODAY", "today"],
    ],
  );
});
