import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  agentRowMenu,
  fileManagerLabel,
  groupRowMenu,
  menuPlatform,
  projectRowMenu,
  tidyMenu,
  workspaceRowMenu,
  type TreeMenuItem,
} from "../src/projectTreeMenus.ts";
// @ts-expect-error Node's test runner needs the source extension.
import { createTreeOperationsStore } from "../src/projectTreeOperations.ts";

const t = (source: string, values?: Record<string, unknown>) =>
  values
    ? source.replace(/\{(\w+)\}/g, (_, key: string) => String(values[key]))
    : source;

/** The menu as ids, with "|" for a separator and "!" for an item that is off. */
function shape(items: TreeMenuItem[]) {
  return items.map((entry) =>
    entry.kind === "separator" ? "|" : `${entry.disabled ? "!" : ""}${entry.id}`,
  );
}

test("separators never lead, trail or repeat", () => {
  const item = (id: string): TreeMenuItem => ({ kind: "item", id, label: id });
  const separator: TreeMenuItem = { kind: "separator" };
  assert.deepEqual(
    shape(tidyMenu([separator, item("a"), separator, separator, item("b"), separator])),
    ["a", "|", "b"],
  );
  assert.deepEqual(tidyMenu([separator]), []);
});

test("an agent row follows Orca's order and adds Rename, Close tab and Archive", () => {
  const tabbed = agentRowMenu(
    { kind: "conversation", unread: true, tabbed: true, path: "/u/app" },
    t,
  );
  assert.deepEqual(shape(tabbed), [
    "open",
    "rename",
    "|",
    "copy-title",
    "copy-path",
    "|",
    "mark-read",
    "|",
    "close-tab",
    "archive",
  ]);
  const read = agentRowMenu(
    { kind: "conversation", unread: false, tabbed: false, path: null },
    t,
  );
  assert.deepEqual(shape(read), [
    "open",
    "rename",
    "|",
    "copy-title",
    "|",
    "mark-unread",
    "|",
    "archive",
  ]);
  // An agent terminal without a conversation has no tab, no unread mark,
  // and no archive.
  const terminal = agentRowMenu(
    { kind: "terminal", unread: false, tabbed: false, path: "/u/app/src" },
    t,
  );
  assert.deepEqual(shape(terminal), ["open", "rename", "|", "copy-title", "copy-path"]);
  // A terminal row of the Chats area can move into a shell tab (ADR 0020).
  const loose = agentRowMenu(
    { kind: "terminal", unread: false, tabbed: false, path: "/u", openInTab: true },
    t,
  );
  assert.deepEqual(shape(loose), ["open", "open-in-tab", "rename", "|", "copy-title", "copy-path"]);
  // A conversation row never offers it.
  assert.ok(
    !shape(
      agentRowMenu({ kind: "conversation", unread: false, tabbed: false, path: null, openInTab: true }, t),
    ).includes("open-in-tab"),
  );
});

test("the primary workspace offers the project removal instead of its own", () => {
  const primary = workspaceRowMenu(
    { primary: true, managed: false, pinned: true, available: true, agentTabs: true },
    "mac",
    t,
  );
  assert.deepEqual(shape(primary), [
    "new-agent",
    "new-terminal",
    "|",
    "open-file-manager",
    "open-editor",
    "copy-path",
    "copy-branch",
    "|",
    "!remove-worktree",
    "remove-project",
  ]);
  const worktree = workspaceRowMenu(
    { primary: false, managed: true, pinned: false, available: true, agentTabs: false },
    "linux",
    t,
  );
  assert.deepEqual(shape(worktree), [
    "new-terminal",
    "|",
    "open-file-manager",
    "open-editor",
    "copy-path",
    "copy-branch",
    "|",
    "pin",
    "|",
    "remove-worktree",
  ]);
  const removeItem = worktree.at(-1);
  assert.equal(removeItem?.kind === "item" && removeItem.danger, true);
  assert.equal(worktree[2].kind === "item" && worktree[2].label, "Open in file manager");

  const pinnedForeign = workspaceRowMenu(
    { primary: false, managed: false, pinned: true, available: false, agentTabs: true },
    "windows",
    t,
  );
  assert.deepEqual(shape(pinnedForeign), [
    "!new-agent",
    "!new-terminal",
    "|",
    "!open-file-manager",
    "!open-editor",
    "copy-path",
    "copy-branch",
    "|",
    "unpin",
    "|",
    "!remove-worktree",
  ]);
});

test("a project row creates a worktree and toggles Collapse all and Expand all", () => {
  assert.deepEqual(shape(projectRowMenu({ allCollapsed: false, gitRepo: true }, "mac", t)), [
    "new-worktree",
    "|",
    "open-file-manager",
    "copy-path",
    "collapse-all",
    "|",
    "new-group",
    "|",
    "remove-project",
  ]);
  assert.deepEqual(
    shape(projectRowMenu({ allCollapsed: true, gitRepo: false }, "mac", t)).slice(0, 5),
    ["!new-worktree", "|", "open-file-manager", "copy-path", "expand-all"],
  );
});

test("a new worktree shows a pending row, and a failure stays until dismissed", () => {
  const store = createTreeOperationsStore();
  const first = store.startCreation(1, "feature/login");
  const second = store.startCreation(2, null);
  assert.notEqual(first, second);
  assert.deepEqual(
    store.getSnapshot().creations.map((item) => [item.projectId, item.branch, item.status]),
    [
      [1, "feature/login", "creating"],
      [2, null, "creating"],
    ],
  );
  store.dismissCreation(first);
  assert.equal(store.getSnapshot().creations.length, 2, "a running creation is not dismissed");
  store.failCreation(first, "a branch named 'feature/login' already exists");
  assert.equal(store.getSnapshot().creations[0].error, "a branch named 'feature/login' already exists");
  store.dismissCreation(first);
  store.finishCreation(second);
  assert.deepEqual(store.getSnapshot().creations, []);
});

test("the file manager item names the file manager of the platform", () => {
  assert.equal(menuPlatform("MacIntel"), "mac");
  assert.equal(menuPlatform("Win32"), "windows");
  assert.equal(menuPlatform("Linux x86_64"), "linux");
  assert.equal(fileManagerLabel("mac", t), "Open in Finder");
  assert.equal(fileManagerLabel("windows", t), "Open in File Explorer");
});

test("a removal shows while it runs and its failure stays until dismissed", () => {
  const store = createTreeOperationsStore();
  let calls = 0;
  store.subscribe(() => {
    calls += 1;
  });
  assert.equal(store.startRemoval("wt-1"), true);
  assert.equal(store.startRemoval("wt-1"), false, "one removal at a time");
  assert.equal(store.getSnapshot().removals["wt-1"]?.status, "removing");
  store.dismissRemoval("wt-1");
  assert.equal(store.getSnapshot().removals["wt-1"]?.status, "removing", "a run is not dismissed");
  store.failRemoval("wt-1", "workspace has uncommitted changes");
  assert.deepEqual(store.getSnapshot().removals["wt-1"], {
    workspaceId: "wt-1",
    status: "failed",
    error: "workspace has uncommitted changes",
  });
  assert.equal(store.startRemoval("wt-1"), true, "a failed removal can run again");
  store.finishRemoval("wt-1");
  assert.deepEqual(store.getSnapshot().removals, {});
  const before = calls;
  store.finishRemoval("wt-1");
  assert.equal(calls, before, "no change, no notice");
});

test("the group items follow Orca: a submenu of the groups and a way out", () => {
  const groups = [
    { id: "g1", name: "halv" },
    { id: "g2", name: "clients" },
  ];
  const grouped = projectRowMenu(
    { allCollapsed: false, gitRepo: true, groups, groupId: "g1" },
    "mac",
    t,
  );
  assert.deepEqual(shape(grouped).slice(6, 10), [
    "new-group",
    "move-to-group",
    "remove-from-group",
    "|",
  ]);
  const submenu = grouped.find((entry) => entry.kind === "item" && entry.id === "move-to-group");
  assert.deepEqual(
    submenu?.kind === "item" ? shape(submenu.children ?? []) : [],
    ["!move-to-group:g1", "move-to-group:g2"],
  );
  assert.deepEqual(shape(groupRowMenu(t)), ["rename-group", "|", "delete-group"]);
});
