import assert from "node:assert/strict";
import test from "node:test";
import {
  completedTurnCount,
  isCompactionBoundaryMessage,
} from "../src/compaction.ts";
import type { ConversationCompaction, StoredMessage } from "../src/types.ts";

const compaction: ConversationCompaction = {
  id: 1,
  conversationId: "conversation-1",
  uptoMessageId: 42,
  summary: "Earlier work is complete.",
  createdAt: "2026-07-16T12:00:00.000Z",
};

function message(
  sequence: number | undefined,
  role: StoredMessage["role"] = "assistant",
  error = false,
  streaming = false,
): StoredMessage & { streaming?: boolean } {
  return {
    id: `message-${sequence ?? "pending"}`,
    sequence,
    conversationId: "conversation-1",
    role,
    content: "content",
    createdAt: "2026-07-16T12:00:00.000Z",
    error,
    streaming,
  };
}

test("compaction divider matches only the persisted watermark sequence", () => {
  assert.equal(isCompactionBoundaryMessage(message(41), compaction), false);
  assert.equal(isCompactionBoundaryMessage(message(42), compaction), true);
  assert.equal(isCompactionBoundaryMessage(message(undefined), compaction), false);
  assert.equal(isCompactionBoundaryMessage(message(42), null), false);
});

test("completed turn count ignores incomplete and errored assistant messages", () => {
  assert.equal(
    completedTurnCount([
      message(1, "user"),
      message(2),
      message(3, "user"),
      message(4, "assistant", true),
      message(5, "user"),
      message(6, "assistant", false, true),
    ]),
    1,
  );
});
