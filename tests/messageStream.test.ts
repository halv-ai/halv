import assert from "node:assert/strict";
import test from "node:test";

import { mergePersistedMessagesWithStream } from "../src/messageStream.ts";
import type { StoredMessage } from "../src/types.ts";

function message(
  id: string,
  role: StoredMessage["role"],
  content: string,
): StoredMessage {
  return {
    id,
    conversationId: "conversation-1",
    role,
    content,
    createdAt: "2026-07-13T12:00:00.000Z",
    error: false,
  };
}

test("does not replay a stream buffer after the assistant is persisted", () => {
  const persisted = [
    message("user-1", "user", "Hello"),
    message("assistant-1", "assistant", "Hello back"),
  ];

  const merged = mergePersistedMessagesWithStream(
    persisted,
    "conversation-1",
    "Hello back",
  );

  assert.deepEqual(merged, persisted);
  assert.equal(merged.filter((entry) => entry.role === "assistant").length, 1);
  assert.equal(merged.some((entry) => entry.streaming), false);
});

test("restores an in-flight stream when storage still ends at the user turn", () => {
  const persisted = [message("user-1", "user", "Hello")];

  const merged = mergePersistedMessagesWithStream(
    persisted,
    "conversation-1",
    "Partial answer",
  );

  assert.equal(merged.length, 2);
  assert.equal(merged[1]?.content, "Partial answer");
  assert.equal(merged[1]?.streaming, true);
});

test("keeps buffered text until the assistant row is persisted", () => {
  const persisted = [message("user-1", "user", "Hello")];

  const merged = mergePersistedMessagesWithStream(
    persisted,
    "conversation-1",
    "Finalizing answer",
  );

  assert.equal(merged.length, 2);
  assert.equal(merged[1]?.content, "Finalizing answer");
  assert.equal(merged[1]?.streaming, true);
});
