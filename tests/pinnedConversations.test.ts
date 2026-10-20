import assert from "node:assert/strict";
import test from "node:test";
import { nextPinnedConversation, pinnedConversations, pinnedConversationInitial } from "../src/pinnedConversations.ts";
import type { ConversationSummary } from "../src/types";

const rows = [
  { id: "old", pinned: true, updatedAt: "2026-09-01", createdAt: "2026-09-01" },
  { id: "unpinned", pinned: false, updatedAt: "2026-09-04", createdAt: "2026-09-01" },
  { id: "archived", pinned: true, archivedAt: "2026-09-03", updatedAt: "2026-09-03", createdAt: "2026-09-01" },
  { id: "new", pinned: true, updatedAt: "2026-09-02", createdAt: "2026-09-01" },
] as ConversationSummary[];

test("pinned cycling follows the visible order and wraps in both directions", () => {
  assert.deepEqual(pinnedConversations(rows).map((row) => row.id), ["new", "old"]);
  assert.equal(nextPinnedConversation(rows, "new", 1), "old");
  assert.equal(nextPinnedConversation(rows, "old", 1), "new");
  assert.equal(nextPinnedConversation(rows, "new", -1), "old");
  assert.equal(nextPinnedConversation(rows, "old", -1), "new");
});

test("cycling from an unpinned conversation enters the pins; empty and single pins are safe", () => {
  assert.equal(nextPinnedConversation(rows, "unpinned", 1), "new");
  assert.equal(nextPinnedConversation(rows, null, -1), "old");
  assert.equal(nextPinnedConversation([], "new", 1), null);
  assert.equal(nextPinnedConversation([rows[0]], "old", -1), "old");
});

test("pin initials use the title's first letter, including non-English titles", () => {
  assert.equal(pinnedConversationInitial("Halv"), "H");
  assert.equal(pinnedConversationInitial(" 🚀 éclair"), "É");
  assert.equal(pinnedConversationInitial("项目"), "项");
  assert.equal(pinnedConversationInitial(""), "?");
});
