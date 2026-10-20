import assert from "node:assert/strict";
import test from "node:test";

// Stage S5b of ADR 0010: shell tabs, Cmd+T and the Cmd+W chain. These are
// the pieces that need no browser: the Cmd+W routing matrix, the chords on
// each platform, shell tab titles, retention, and the terminal rows of the
// Chats area.

import {
  closeChainStep,
  workspaceTabKeyAction,
  type CloseChainFocus,
  // @ts-expect-error Node's test runner needs source extensions.
} from "../src/useWorkspaceTabKeys.ts";
// @ts-expect-error Node's test runner needs source extensions.
import { shortcutConflict, shortcutDefinitions } from "../src/shortcuts.ts";
// @ts-expect-error Node's test runner needs source extensions.
import { plusMenuItems, shellTabTitle } from "../src/workspaceTabs.ts";
// @ts-expect-error Node's test runner needs source extensions.
import { liveShellTabIds, mountedTabKeys } from "../src/workspaceTabBodies.ts";
// @ts-expect-error Node's test runner needs source extensions.
import { tabbedShellSessionIds } from "../src/sidebarTerminals.ts";

const MAC = "MacIntel";
const WINDOWS = "Win32";
const LINUX = "Linux x86_64";

function key(
  keyName: string,
  modifiers: { meta?: boolean; ctrl?: boolean; alt?: boolean; shift?: boolean } = {},
) {
  return {
    key: keyName,
    code: `Key${keyName.toUpperCase()}`,
    metaKey: modifiers.meta ?? false,
    ctrlKey: modifiers.ctrl ?? false,
    altKey: modifiers.alt ?? false,
    shiftKey: modifiers.shift ?? false,
  };
}

const elsewhere: CloseChainFocus = { kind: "elsewhere" };

test("Cmd+W routing: without shell tabs, the window hides as before", () => {
  for (const focus of [
    elsewhere,
    { kind: "tab-pane", tabKey: "shell:a", sessionId: "s1", paneCount: 2 },
    { kind: "bottom-pane", scope: "panel:global", sessionId: "s1", paneCount: 1 },
  ] as CloseChainFocus[]) {
    assert.deepEqual(
      closeChainStep({
        shellTabs: false,
        focus,
        visibleTab: { key: "conversation:c1", paneCount: 2 },
        draftWithTabs: false,
      }),
      { type: "hide-window" },
    );
  }
});

test("Cmd+W routing: a focused pane closes, the only pane closes its tab", () => {
  assert.deepEqual(
    closeChainStep({
      shellTabs: true,
      focus: { kind: "tab-pane", tabKey: "shell:a", sessionId: "s2", paneCount: 2 },
      visibleTab: { key: "shell:a", paneCount: 2 },
      draftWithTabs: false,
    }),
    { type: "close-pane", scope: "tab", tabKey: "shell:a", sessionId: "s2" },
  );
  assert.deepEqual(
    closeChainStep({
      shellTabs: true,
      focus: { kind: "tab-pane", tabKey: "conversation:c1", sessionId: "s1", paneCount: 1 },
      visibleTab: { key: "conversation:c1", paneCount: 1 },
      draftWithTabs: false,
    }),
    { type: "close-tab", tabKey: "conversation:c1" },
  );
});

test("Cmd+W routing: a bottom panel pane closes, and the last one hides the panel", () => {
  assert.deepEqual(
    closeChainStep({
      shellTabs: true,
      focus: { kind: "bottom-pane", scope: "panel:global", sessionId: "s1", paneCount: 1 },
      visibleTab: { key: "conversation:c1", paneCount: 0 },
      draftWithTabs: false,
    }),
    { type: "close-pane", scope: "bottom", panelScope: "panel:global", sessionId: "s1", last: true },
  );
  assert.deepEqual(
    closeChainStep({
      shellTabs: true,
      focus: { kind: "bottom-pane", scope: "panel:global", sessionId: "s1", paneCount: 3 },
      visibleTab: null,
      draftWithTabs: false,
    }),
    { type: "close-pane", scope: "bottom", panelScope: "panel:global", sessionId: "s1", last: false },
  );
});

test("Cmd+W routing: elsewhere, the active pane of a split tab, else the tab", () => {
  assert.deepEqual(
    closeChainStep({
      shellTabs: true,
      focus: elsewhere,
      visibleTab: { key: "shell:a", paneCount: 3 },
      draftWithTabs: false,
    }),
    { type: "close-pane", scope: "tab", tabKey: "shell:a", sessionId: null },
  );
  for (const paneCount of [0, 1]) {
    assert.deepEqual(
      closeChainStep({
        shellTabs: true,
        focus: elsewhere,
        visibleTab: { key: "conversation:c1", paneCount },
        draftWithTabs: false,
      }),
      { type: "close-tab", tabKey: "conversation:c1" },
    );
  }
});

test("Cmd+W routing: the draft tab leaves composing; an empty strip hides the window", () => {
  assert.deepEqual(
    closeChainStep({ shellTabs: true, focus: elsewhere, visibleTab: null, draftWithTabs: true }),
    { type: "leave-draft" },
  );
  assert.deepEqual(
    closeChainStep({ shellTabs: true, focus: elsewhere, visibleTab: null, draftWithTabs: false }),
    { type: "hide-window" },
  );
});

test("Cmd+T and Cmd+W on macOS; Ctrl+Shift+T and Ctrl+Shift+W elsewhere", () => {
  assert.deepEqual(workspaceTabKeyAction(key("t", { meta: true }), {}, MAC), { type: "new-shell" });
  assert.deepEqual(workspaceTabKeyAction(key("w", { meta: true }), {}, MAC), { type: "close" });
  for (const platform of [WINDOWS, LINUX]) {
    assert.deepEqual(
      workspaceTabKeyAction(key("T", { ctrl: true, shift: true }), {}, platform),
      { type: "new-shell" },
    );
    assert.deepEqual(
      workspaceTabKeyAction(key("W", { ctrl: true, shift: true }), {}, platform),
      { type: "close" },
    );
    // Ctrl+T and Ctrl+W stay with the CLIs and the shell.
    assert.equal(workspaceTabKeyAction(key("t", { ctrl: true }), {}, platform), null);
    assert.equal(workspaceTabKeyAction(key("w", { ctrl: true }), {}, platform), null);
  }
  // Shift+Cmd+W is the menu "Close Window" item on macOS.
  assert.equal(workspaceTabKeyAction(key("w", { meta: true, shift: true }), {}, MAC), null);
  // Option+Cmd+T still starts an agent tab.
  assert.deepEqual(
    workspaceTabKeyAction(key("t", { meta: true, alt: true }), {}, MAC),
    { type: "new-agent" },
  );
});

test("the new chords conflict with no other default", () => {
  for (const platform of [MAC, WINDOWS, LINUX]) {
    for (const id of ["workspace.new_shell_tab", "workspace.close_pane_or_tab"]) {
      const definition = shortcutDefinitions.find((candidate) => candidate.id === id);
      assert.ok(definition, id);
      const binding =
        platform === MAC
          ? definition.defaultBinding
          : (definition.defaultBindingOther ?? definition.defaultBinding);
      assert.equal(shortcutConflict(id, binding, {}, platform), null, `${id} on ${platform}`);
    }
  }
});

test("a shell tab title: custom, else the live title of its pane, else Terminal N", () => {
  const row = { customTitle: null, defaultTitle: "Terminal 3" };
  assert.deepEqual(shellTabTitle(row, null), { terminalNumber: 3 });
  assert.deepEqual(shellTabTitle(row, "  "), { terminalNumber: 3 });
  assert.deepEqual(shellTabTitle(row, "vim notes.md"), { title: "vim notes.md" });
  assert.deepEqual(
    shellTabTitle({ ...row, customTitle: "dev server" }, "vim notes.md"),
    { title: "dev server" },
  );
});

test("the plus menu offers New Terminal first, with the platform chord", () => {
  const items = plusMenuItems({
    platform: "mac",
    agents: [{ kind: "claude", ready: true }],
    defaultAgent: "claude",
    shellTabs: true,
    reopen: { shown: false, available: false },
  });
  assert.deepEqual(items[0], { type: "item", id: "new-terminal", shortcut: "⌘T", disabled: false });
  const windows = plusMenuItems({
    platform: "windows",
    agents: [],
    defaultAgent: null,
    shellTabs: true,
    reopen: { shown: false, available: false },
  });
  assert.equal(windows[0].type === "item" && windows[0].shortcut, "Ctrl+Shift+T");
});

test("retention keeps a shell tab with a running shell mounted", () => {
  const live = liveShellTabIds([
    { shellTabId: "a", status: "running" },
    { shellTabId: "b", status: "exited" },
    { shellTabId: null, status: "running" },
  ]);
  assert.deepEqual([...live], ["a"]);
  const mounted = mountedTabKeys({
    keys: ["shell:a", "shell:b", "shell:c", "conversation:c1"],
    visibleKey: "shell:c",
    liveConversationIds: new Set(),
    liveShellTabIds: live,
    closingKeys: new Set(),
  });
  assert.deepEqual(mounted, ["shell:a", "shell:c"]);
});

test("a shell of an open shell tab never shows in the Chats area", () => {
  const terminals = [
    { id: "t1", shellTabId: "a" },
    { id: "t2", shellTabId: "gone" },
    { id: "t3", shellTabId: null },
  ];
  assert.deepEqual([...tabbedShellSessionIds(terminals, new Set(["a"]))], ["t1"]);
});
