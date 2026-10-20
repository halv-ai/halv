import assert from "node:assert/strict";
import test from "node:test";

import {
  clearConversationUnviewed,
  markConversationUnviewed,
  migrateConversationUnviewed,
} from "../src/conversationUnviewed.ts";
import { NEW_THREAD } from "../src/types.ts";

test("marks a finished background conversation as unviewed", () => {
  const result = markConversationUnviewed(
    new Set(["conversation-a"]),
    "conversation-b",
    "conversation-a",
  );

  assert.deepEqual([...result].sort(), ["conversation-a", "conversation-b"]);
});

test("does not mark a finished selected conversation as unviewed", () => {
  const result = markConversationUnviewed(
    new Set(["conversation-a"]),
    "conversation-a",
    "conversation-a",
  );

  assert.deepEqual([...result], []);
});

test("selection clears an unviewed conversation", () => {
  const result = clearConversationUnviewed(
    new Set(["conversation-a", "conversation-b"]),
    "conversation-a",
  );

  assert.deepEqual([...result], ["conversation-b"]);
});

test("new-thread migration moves unviewed state to the real id", () => {
  const result = migrateConversationUnviewed(
    new Set([NEW_THREAD, "conversation-a"]),
    NEW_THREAD,
    "conversation-b",
  );

  assert.deepEqual([...result].sort(), ["conversation-a", "conversation-b"]);
});
