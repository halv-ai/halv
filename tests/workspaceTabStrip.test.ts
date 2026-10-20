import assert from "node:assert/strict";
import test from "node:test";

// Stage S4a of ADR 0010: the pieces of the workspace strip that need no
// browser: the park payload, the tab-close stop order, the mount registry,
// retention, the tab body rule, and the tab chords.

// @ts-expect-error Node's test runner needs source extensions.
import { parkedTerminalRestorePayload, stopTerminalSessionForTabClose } from "../src/terminalClose.ts";
// @ts-expect-error Node's test runner needs source extensions.
import { createTerminalPanelRegistry } from "../src/terminalPanelRegistry.ts";
import {
  conversationBodyKind,
  liveConversationIds,
  mountedTabKeys,
  // @ts-expect-error Node's test runner needs source extensions.
} from "../src/workspaceTabBodies.ts";
// @ts-expect-error Node's test runner needs source extensions.
import { workspaceTabKeyAction, workspaceTabKeyTarget } from "../src/useWorkspaceTabKeys.ts";
import {
  defaultShortcut,
  effectiveShortcut,
  recordedShortcutBinding,
  shortcutConflict,
  shortcutDefinitions,
  shortcutDigitForEvent,
  shortcutMatchesEvent,
  // @ts-expect-error Node's test runner needs source extensions.
} from "../src/shortcuts.ts";
import type { TerminalSession } from "../src/types.ts";

function session(
  id: string,
  agentKind: TerminalSession["agentKind"] | undefined,
  status: TerminalSession["status"] = "running",
): TerminalSession {
  return {
    id,
    projectId: 1,
    conversationId: "c1",
    projectName: "Halv",
    cwd: "/tmp/halv",
    shell: "/bin/zsh",
    pid: 42,
    executionProfile: "full_access",
    status,
    cols: 100,
    rows: 24,
    startedAt: "2026-10-15T10:00:00Z",
    agentKind,
  };
}

function agentEntry(id: string, cliSessionId: string | null, agentKind = "claude") {
  return {
    session: {
      id,
      agentKind,
      laneKind: "claudeSubscription",
      cliSessionId,
      conversationId: "c1",
      mode: "terminal_embedded",
      status: "running",
      customTitle: null,
      defaultTitle: "Claude 1",
      generatedTitle: null,
      aiVaultTitle: null,
      generation: 0,
    },
  };
}

const splitLayout = {
  version: 1 as const,
  activeTabId: "tab",
  activeLeafId: "pane-agent",
  tabs: [
    {
      tabId: "tab",
      root: {
        type: "split" as const,
        direction: "row" as const,
        first: { type: "leaf" as const, leafId: "pane-agent" },
        second: {
          type: "split" as const,
          direction: "column" as const,
          first: { type: "leaf" as const, leafId: "pane-shell" },
          second: { type: "leaf" as const, leafId: "pane-fresh" },
        },
      },
    },
  ],
};

test("park keeps each agent pane with a CLI session id and prunes the layout to them", () => {
  const payload = parkedTerminalRestorePayload({
    scope: "halv.terminal-layout.v1:conversation:c1",
    layout: splitLayout,
    sessions: [session("agent", "claude"), session("shell", undefined), session("fresh", "codex")],
    paneSessionIds: { "pane-agent": "agent", "pane-shell": "shell", "pane-fresh": "fresh" },
    agentEntries: [agentEntry("agent", "cli-1"), agentEntry("fresh", null, "codex")] as never,
    laneOptions: [],
    restoredPanes: {},
    resumeFallbackPanes: {},
  });
  assert.deepEqual(
    payload.panes.map((pane: { paneId: string; cliSessionId: string | null }) => [pane.paneId, pane.cliSessionId]),
    [["pane-agent", "cli-1"]],
  );
  assert.equal(payload.panes[0].agentKind, "claude");
  assert.equal(payload.panes[0].laneKind, "claudeSubscription");
  assert.equal(payload.panes[0].cwd, "/tmp/halv");
  assert.deepEqual(payload.layout.tabs[0].root, { type: "leaf", leafId: "pane-agent" });
  assert.equal(payload.layout.activeLeafId, "pane-agent");
});

test("park of a tab without a resumable agent writes no panes, so the scope drops", () => {
  const payload = parkedTerminalRestorePayload({
    scope: "halv.terminal-layout.v1:conversation:c1",
    layout: splitLayout,
    sessions: [session("shell", undefined)],
    paneSessionIds: { "pane-shell": "shell" },
    agentEntries: [],
    laneOptions: [],
    restoredPanes: {},
    resumeFallbackPanes: {},
  });
  assert.deepEqual(payload.panes, []);
  assert.deepEqual(payload.layout.tabs, []);
});

test("a tab close stops an agent through its agent session and forgets both rows", async () => {
  const calls: string[] = [];
  let running = true;
  await stopTerminalSessionForTabClose(session("a1", "claude"), {
    killAgent: async (id: string) => {
      calls.push(`kill ${id}`);
      running = false;
    },
    closeTerminal: async (id: string) => void calls.push(`close ${id}`),
    forgetTerminal: async (id: string) => {
      if (running) throw new Error("close the terminal before removing it");
      calls.push(`forget ${id}`);
    },
    forgetAgent: async (id: string) => void calls.push(`forget-agent ${id}`),
    refreshAgentSessions: async () => void calls.push("refresh"),
    waitForExit: async () => undefined,
  });
  assert.deepEqual(calls, ["kill a1", "forget a1", "forget-agent a1", "refresh"]);
});

test("a tab close waits for a slow agent exit before it forgets the terminal row", async () => {
  let attempts = 0;
  const calls: string[] = [];
  await stopTerminalSessionForTabClose(session("a1", "claude"), {
    killAgent: async () => undefined,
    closeTerminal: async () => undefined,
    forgetTerminal: async () => {
      attempts += 1;
      if (attempts < 3) throw new Error("close the terminal before removing it");
      calls.push("forget");
    },
    forgetAgent: async () => void calls.push("forget-agent"),
    refreshAgentSessions: async () => undefined,
    waitForExit: async () => undefined,
  });
  assert.equal(attempts, 3);
  assert.deepEqual(calls, ["forget", "forget-agent"]);
});

test("a tab close stops a shell with terminal_close, and an agent terminal without an agent row as a shell", async () => {
  const calls: string[] = [];
  const dependencies = {
    killAgent: async () => {
      throw new Error("agent session `x` not found");
    },
    closeTerminal: async (id: string) => void calls.push(`close ${id}`),
    forgetTerminal: async (id: string) => void calls.push(`forget ${id}`),
    forgetAgent: async () => void calls.push("forget-agent"),
    refreshAgentSessions: async () => undefined,
    waitForExit: async () => undefined,
  };
  await stopTerminalSessionForTabClose(session("s1", undefined), dependencies);
  await stopTerminalSessionForTabClose(session("x", "codex"), dependencies);
  assert.deepEqual(calls, ["close s1", "forget s1", "close x", "forget x"]);
});

test("a failed stop rejects, so the tab stays", async () => {
  await assert.rejects(
    stopTerminalSessionForTabClose(session("a1", "claude"), {
      killAgent: async () => {
        throw new Error("supervisor is gone");
      },
      closeTerminal: async () => undefined,
      forgetTerminal: async () => undefined,
      forgetAgent: async () => undefined,
      refreshAgentSessions: async () => undefined,
    }),
    /supervisor is gone/,
  );
});

test("one mounted panel per scope: a second panel waits and takes the scope on release", () => {
  const registry = createTerminalPanelRegistry();
  const first = Symbol("first");
  const second = Symbol("second");
  let notified = 0;
  registry.subscribe(() => {
    notified += 1;
  });
  assert.equal(registry.claim("scope", first), true);
  assert.equal(registry.claim("scope", first), true, "a claim is idempotent");
  assert.equal(registry.claim("scope", second), false);
  assert.equal(registry.holds("scope", second), false);
  // Only the holder publishes a handle.
  const handle = {
    closeAll: async () => undefined,
    focusActivePane: () => true,
    openShell: () => undefined,
    panes: () => [],
  };
  registry.setHandle("scope", second, handle);
  assert.equal(registry.request("scope"), null);
  registry.setHandle("scope", first, handle);
  assert.equal(registry.request("scope"), handle);
  // A release by another token changes nothing.
  registry.release("scope", second);
  assert.equal(registry.holds("scope", first), true);
  registry.release("scope", first);
  assert.equal(registry.mounted("scope"), false);
  assert.equal(registry.request("scope"), null);
  assert.equal(registry.claim("scope", second), true);
  assert.ok(notified >= 3);
});

test("retention keeps the visible tab, live tabs and closing tabs; nothing else mounts", () => {
  const keys = ["conversation:a", "conversation:b", "conversation:c", "conversation:d"];
  const live = liveConversationIds({
    agentEntries: [
      { session: { conversationId: "b", mode: "terminal_embedded", status: "running" } },
      { session: { conversationId: "c", mode: "gui", status: "running" } },
      { session: { conversationId: "d", mode: "terminal_embedded", status: "exited" } },
    ],
    terminals: [
      { conversationId: "d", status: "exited" },
      { conversationId: null, status: "running" },
    ],
  });
  assert.deepEqual([...live], ["b"]);
  assert.deepEqual(
    mountedTabKeys({
      keys,
      visibleKey: "conversation:a",
      liveConversationIds: live,
      closingKeys: new Set(["conversation:d"]),
    }),
    ["conversation:a", "conversation:b", "conversation:d"],
  );
  // A loose conversation mounts too, once.
  assert.deepEqual(
    mountedTabKeys({
      keys,
      visibleKey: null,
      liveConversationIds: new Set(),
      closingKeys: new Set(),
      looseConversationId: "x",
    }),
    ["conversation:x"],
  );
});

test("the tab body follows the mode; an agent tab shows its terminal once it has one", () => {
  const base = { agentMode: null, runningOwnedTerminal: false, savedTerminals: false, opening: false };
  assert.equal(conversationBodyKind({ ...base, mode: "chat", runningOwnedTerminal: true }), "chat");
  assert.equal(conversationBodyKind({ ...base, mode: "agent" }), "chat");
  assert.equal(conversationBodyKind({ ...base, mode: "agent", opening: true }), "terminal");
  assert.equal(conversationBodyKind({ ...base, mode: "agent", savedTerminals: true }), "terminal");
  assert.equal(conversationBodyKind({ ...base, mode: "agent", runningOwnedTerminal: true }), "terminal");
  assert.equal(conversationBodyKind({ ...base, mode: "agent", agentMode: "terminal_embedded" }), "terminal");
  assert.equal(conversationBodyKind({ ...base, mode: "agent", agentMode: "terminal_ghostty" }), "ghostty");
  assert.equal(conversationBodyKind({ ...base, mode: "agent", agentMode: "gui" }), "chat");
});

function key(
  value: string,
  options: Partial<{ code: string; metaKey: boolean; ctrlKey: boolean; altKey: boolean; shiftKey: boolean }> = {},
) {
  return {
    key: value,
    code: options.code ?? (/^[0-9]$/.test(value) ? `Digit${value}` : `Key${value.toUpperCase()}`),
    metaKey: options.metaKey ?? false,
    ctrlKey: options.ctrlKey ?? false,
    altKey: options.altKey ?? false,
    shiftKey: options.shiftKey ?? false,
  };
}

test("the digit token: Cmd+1 and Cmd+9 match; Cmd+0, Shift+Cmd+1 and a bare digit do not", () => {
  const binding = effectiveShortcut("workspace.select_tab_by_number", {}, "MacIntel");
  assert.equal(binding, "mod+digit");
  assert.equal(shortcutDigitForEvent(key("1", { metaKey: true }), binding, "MacIntel"), 1);
  assert.equal(shortcutDigitForEvent(key("9", { metaKey: true }), binding, "MacIntel"), 9);
  assert.equal(shortcutDigitForEvent(key("0", { metaKey: true }), binding, "MacIntel"), null);
  assert.equal(shortcutDigitForEvent(key("!", { code: "Digit1", metaKey: true, shiftKey: true }), binding, "MacIntel"), null);
  assert.equal(shortcutDigitForEvent(key("1"), binding, "MacIntel"), null);
  assert.equal(shortcutDigitForEvent(key("2", { ctrlKey: true }), binding, "Linux x86_64"), 2);
  assert.equal(shortcutDigitForEvent(key("2", { metaKey: true }), binding, "Linux x86_64"), null);
  // Another layout still gives the physical digit.
  assert.equal(shortcutDigitForEvent(key("&", { code: "Digit1", metaKey: true }), binding, "MacIntel"), 1);
  assert.equal(shortcutMatchesEvent(key("3", { metaKey: true }), binding, "MacIntel"), true);
  // A recorded chord for a digit action keeps the token.
  assert.equal(recordedShortcutBinding("workspace.select_tab_by_number", "mod+alt+4"), "mod+alt+digit");
  assert.equal(recordedShortcutBinding("task.new", "mod+4"), "mod+4");
});

test("per-platform defaults: Option+Cmd+T on macOS only", () => {
  const definition = shortcutDefinitions.find((item: { id: string }) => item.id === "workspace.new_agent_tab");
  assert.equal(defaultShortcut(definition, "MacIntel"), "mod+alt+t");
  assert.equal(defaultShortcut(definition, "Win32"), "disabled");
  assert.equal(effectiveShortcut("workspace.new_agent_tab", {}, "Linux x86_64"), "disabled");
  assert.equal(effectiveShortcut("workspace.new_agent_tab", { "workspace.new_agent_tab": "ctrl+alt+a" }, "Linux x86_64"), "ctrl+alt+a");
  assert.equal(defaultShortcut({ defaultBinding: "mod+k" }, "Win32"), "mod+k");
});

test("the cycle chords share one chord with the task cycle on purpose", () => {
  assert.equal(shortcutConflict("workspace.next_terminal_tab", "mod+shift+bracketright", {}), null);
  assert.equal(shortcutConflict("task.previous", "mod+shift+bracketleft", {}), null);
  assert.equal(shortcutConflict("task.new", "mod+shift+bracketright", {})?.id, "workspace.next_terminal_tab");
});

test("the tab chords route to the strip and skip dialogs and menus", () => {
  assert.deepEqual(workspaceTabKeyAction(key("2", { metaKey: true }), {}, "MacIntel"), { type: "select", digit: 2 });
  assert.deepEqual(
    workspaceTabKeyAction(key("]", { code: "BracketRight", metaKey: true, shiftKey: true }), {}, "MacIntel"),
    { type: "cycle", offset: 1 },
  );
  assert.deepEqual(
    workspaceTabKeyAction(key("[", { code: "BracketLeft", metaKey: true, shiftKey: true }), {}, "MacIntel"),
    { type: "cycle", offset: -1 },
  );
  assert.deepEqual(workspaceTabKeyAction(key("t", { metaKey: true, altKey: true }), {}, "MacIntel"), { type: "new-agent" });
  assert.equal(workspaceTabKeyAction(key("t", { ctrlKey: true, altKey: true }), {}, "Linux x86_64"), null);
  // Stage S5b: Cmd+T opens a shell tab and Cmd+W runs the close chain.
  assert.deepEqual(workspaceTabKeyAction(key("t", { metaKey: true }), {}, "MacIntel"), { type: "new-shell" });
  assert.deepEqual(workspaceTabKeyAction(key("w", { metaKey: true }), {}, "MacIntel"), { type: "close" });
  assert.equal(workspaceTabKeyTarget(null), true);
});
