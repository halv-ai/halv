import assert from "node:assert/strict";
import test from "node:test";

// Stage S7 of ADR 0010: the closed-tab stack of Shift+Cmd+T, the Ctrl+Tab
// switcher order, the workspace cycle, and the chords that S7 adds.

import {
  CLOSED_TAB_STACK_LIMIT,
  cycledWorkspaceId,
  hasClosedTab,
  nextSwitcherIndex,
  pushClosedTab,
  recentTabOrder,
  takeClosedTab,
  type ClosedTabEntry,
} from "../src/workspaceTabHistory.ts";
import { formatShortcut } from "../src/shortcuts.ts";
import { workspaceTabKeyAction } from "../src/useWorkspaceTabKeys.ts";
import { defaultShortcut, shortcutConflict, shortcutDefinitions } from "../src/shortcuts.ts";

const MAC = "MacIntel";

function conversation(id: string, workspaceId = "primary-1", index = 0): ClosedTabEntry {
  return { kind: "conversation", key: `conversation:${id}`, conversationId: id, workspaceId, index };
}

function shell(id: string, workspaceId = "primary-1", title: string | null = null): ClosedTabEntry {
  return { kind: "shell", key: `shell:${id}`, workspaceId, title, index: 1 };
}

test("the stack keeps the 20 newest closes, newest first, and one entry per tab", () => {
  let stack: ClosedTabEntry[] = [];
  for (let index = 0; index < 25; index += 1) stack = pushClosedTab(stack, conversation(`c${index}`));
  assert.equal(stack.length, CLOSED_TAB_STACK_LIMIT);
  assert.equal(stack[0].key, "conversation:c24");
  assert.equal(stack.at(-1)?.key, "conversation:c5");
  stack = pushClosedTab(stack, conversation("c10", "primary-1", 3));
  assert.equal(stack.length, CLOSED_TAB_STACK_LIMIT);
  assert.equal(stack[0].key, "conversation:c10");
  assert.equal(stack.filter((entry) => entry.key === "conversation:c10").length, 1);
});

test("reopen takes the newest close of the visible workspace; other workspaces keep theirs", () => {
  const stack = [conversation("a"), shell("s1", "worktree-2"), conversation("b")];
  const taken = takeClosedTab(stack, "worktree-2", () => true);
  assert.equal(taken.entry?.key, "shell:s1");
  assert.deepEqual(
    taken.stack.map((entry) => entry.key),
    ["conversation:a", "conversation:b"],
  );
  assert.equal(takeClosedTab(stack, "primary-1", () => true).entry?.key, "conversation:a");
  assert.equal(takeClosedTab([], "primary-1", () => true).entry, null);
});

test("an entry that cannot reopen drops, and the next entry of the workspace reopens", () => {
  const stack = [conversation("archived"), shell("s1", "worktree-2"), conversation("kept")];
  const taken = takeClosedTab(stack, "primary-1", (entry) => entry.key !== "conversation:archived");
  assert.equal(taken.entry?.key, "conversation:kept");
  assert.deepEqual(taken.stack.map((entry) => entry.key), ["shell:s1"]);
  const none = takeClosedTab(stack, "primary-1", () => false);
  assert.equal(none.entry, null);
  assert.deepEqual(none.stack.map((entry) => entry.key), ["shell:s1"]);
});

test("the '+' menu item is available only with a reopenable close in the workspace", () => {
  const stack = [conversation("a"), shell("s1", "worktree-2", "dev server")];
  assert.equal(hasClosedTab(stack, "primary-1"), true);
  assert.equal(hasClosedTab(stack, "worktree-3"), false);
  assert.equal(hasClosedTab(stack, "primary-1", () => false), false);
});

function key(value: string, modifiers: { meta?: boolean; ctrl?: boolean; shift?: boolean; alt?: boolean } = {}) {
  return {
    key: value,
    code: /^[a-z]$/i.test(value) ? `Key${value.toUpperCase()}` : value,
    metaKey: modifiers.meta ?? false,
    ctrlKey: modifiers.ctrl ?? false,
    shiftKey: modifiers.shift ?? false,
    altKey: modifiers.alt ?? false,
  };
}

test("Shift+Cmd+T reopens and Cmd+R renames on macOS; Windows and Linux keep their chords", () => {
  assert.deepEqual(workspaceTabKeyAction(key("T", { meta: true, shift: true }), {}, MAC), { type: "reopen" });
  assert.deepEqual(workspaceTabKeyAction(key("t", { meta: true }), {}, MAC), { type: "new-shell" });
  assert.deepEqual(workspaceTabKeyAction(key("r", { meta: true }), {}, MAC), { type: "rename" });
  for (const platform of ["Win32", "Linux x86_64"]) {
    // Ctrl+Shift+T opens a shell tab there; Ctrl+R stays reverse search.
    assert.deepEqual(
      workspaceTabKeyAction(key("T", { ctrl: true, shift: true }), {}, platform),
      { type: "new-shell" },
    );
    assert.equal(workspaceTabKeyAction(key("r", { ctrl: true }), {}, platform), null);
  }
});

test("the new chords conflict with no other shortcut", () => {
  for (const id of ["workspace.reopen_closed_tab", "workspace.rename_tab"]) {
    const definition = shortcutDefinitions.find((item) => item.id === id);
    assert.ok(definition, id);
    for (const platform of [MAC, "Win32"]) {
      const binding = defaultShortcut(definition, platform);
      assert.equal(shortcutConflict(id, binding, {}, platform)?.id ?? null, null);
    }
  }
});

test("Ctrl+Tab lists the active tab first, then the most recent tabs", () => {
  const tabs = [
    { key: "a", activatedAt: 10 },
    { key: "b", activatedAt: 30 },
    { key: "c", activatedAt: null },
    { key: "d", activatedAt: 20 },
    { key: "e", activatedAt: null },
  ];
  assert.deepEqual(recentTabOrder(tabs, "a"), ["a", "b", "d", "c", "e"]);
  // The draft tab shows: no tab is active.
  assert.deepEqual(recentTabOrder(tabs, null), ["b", "d", "a", "c", "e"]);
});

test("each Tab moves down and Shift+Tab moves up, with a wrap", () => {
  // The first press selects the previous tab (index 1).
  assert.equal(nextSwitcherIndex(3, 0, 1), 1);
  assert.equal(nextSwitcherIndex(3, 2, 1), 0);
  assert.equal(nextSwitcherIndex(3, 0, -1), 2);
  assert.equal(nextSwitcherIndex(3, -1, 1), 0);
  assert.equal(nextSwitcherIndex(3, -1, -1), 2);
  assert.equal(nextSwitcherIndex(0, 0, 1), -1);
});

test("Option+Cmd+Up and Down cycle through the shown workspaces", () => {
  const ids = ["primary-1", "worktree-a", "worktree-b", "primary-2"];
  assert.equal(cycledWorkspaceId(ids, "worktree-a", "down"), "worktree-b");
  assert.equal(cycledWorkspaceId(ids, "worktree-a", "up"), "primary-1");
  assert.equal(cycledWorkspaceId(ids, "primary-2", "down"), "primary-1");
  assert.equal(cycledWorkspaceId(ids, "primary-1", "up"), "primary-2");
  // A workspace in a folded project enters from the far end.
  assert.equal(cycledWorkspaceId(ids, "hidden", "down"), "primary-1");
  assert.equal(cycledWorkspaceId(ids, null, "up"), "primary-2");
  assert.equal(cycledWorkspaceId([], "primary-1", "down"), null);
});

test("the workspace chords and Ctrl+Tab route on each platform", () => {
  const arrow = (value: string, modifiers: Parameters<typeof key>[1]) => ({ ...key(value, modifiers), code: value });
  assert.deepEqual(workspaceTabKeyAction(arrow("ArrowUp", { meta: true, alt: true }), {}, MAC), {
    type: "workspace",
    direction: "up",
  });
  assert.deepEqual(workspaceTabKeyAction(arrow("ArrowDown", { meta: true, alt: true }), {}, MAC), {
    type: "workspace",
    direction: "down",
  });
  // Cmd+Up stays "go to the start" in a text field.
  assert.equal(workspaceTabKeyAction(arrow("ArrowUp", { meta: true }), {}, MAC), null);
  for (const platform of ["Win32", "Linux x86_64"]) {
    assert.deepEqual(workspaceTabKeyAction(arrow("ArrowUp", { ctrl: true, shift: true }), {}, platform), {
      type: "workspace",
      direction: "up",
    });
    assert.equal(workspaceTabKeyAction(arrow("ArrowUp", { ctrl: true, alt: true }), {}, platform), null);
  }
  for (const id of ["workspace.recent_tab", "workspace.previous_workspace", "workspace.next_workspace"]) {
    const definition = shortcutDefinitions.find((item) => item.id === id);
    assert.ok(definition, id);
    for (const platform of [MAC, "Win32"]) {
      const binding = defaultShortcut(definition, platform);
      assert.equal(shortcutConflict(id, binding, {}, platform)?.id ?? null, null, `${id} ${platform}`);
    }
  }
  assert.equal(formatShortcut("ctrl+tab", MAC), "⌃Tab");
  assert.equal(formatShortcut("mod+alt+arrowup", MAC), "⌘⌥↑");
});
