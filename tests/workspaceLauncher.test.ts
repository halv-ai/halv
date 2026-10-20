import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  launcherFocusIndex,
  launcherItemKey,
  launcherItems,
  workspaceLauncherShown,
} from "../src/workspaceLauncher.ts";
// @ts-expect-error Node's test runner needs the source extension.
import { plusMenuItems } from "../src/workspaceTabs.ts";

const AGENTS = [
  { kind: "codex", ready: true },
  { kind: "claude", ready: true },
  { kind: "kimi", ready: false },
];

test("the launcher lists the ready agents in the order of the '+' menu, then Terminal, then Text conversation", () => {
  const items = launcherItems({
    platform: "mac",
    agents: AGENTS,
    defaultAgent: "claude",
    shellTabs: true,
  });
  assert.deepEqual(items.map(launcherItemKey), [
    "agent:claude",
    "agent:codex",
    "terminal",
    "text",
  ]);
  // The default agent and the terminal show their chords.
  assert.deepEqual(items[0], { kind: "agent", agentKind: "claude", shortcut: "⌥⌘T" });
  assert.deepEqual(items[1], { kind: "agent", agentKind: "codex", shortcut: null });
  assert.deepEqual(items[2], { kind: "terminal", shortcut: "⌘T" });

  // The agents keep the order of the "+" menu.
  const plusAgents = plusMenuItems({
    platform: "mac",
    agents: AGENTS,
    defaultAgent: "claude",
    shellTabs: true,
    reopen: { shown: false, available: false },
  }).flatMap((item: { type: string; agentKind?: string }) =>
    item.type === "agent" ? [`agent:${item.agentKind}`] : [],
  );
  assert.deepEqual(items.filter((item) => item.kind === "agent").map(launcherItemKey), plusAgents);
});

test("the launcher chords follow the platform, and Terminal needs shell tabs", () => {
  const linux = launcherItems({
    platform: "linux",
    agents: AGENTS,
    defaultAgent: "claude",
    shellTabs: true,
  });
  assert.deepEqual(linux[0], { kind: "agent", agentKind: "claude", shortcut: null });
  assert.deepEqual(linux[2], { kind: "terminal", shortcut: "Ctrl+Shift+T" });

  const noShells = launcherItems({
    platform: "mac",
    agents: [],
    defaultAgent: null,
    shellTabs: false,
  });
  assert.deepEqual(noShells.map(launcherItemKey), ["text"], "a text conversation is always there");
});

test("the launcher shows only in Pro mode, on an empty strip, while nothing composes", () => {
  assert.equal(workspaceLauncherShown({ proMode: true, stripKeys: [], composing: false }), true);
  assert.equal(
    workspaceLauncherShown({ proMode: false, stripKeys: [], composing: false }),
    false,
    "Pro mode off keeps the composer",
  );
  assert.equal(
    workspaceLauncherShown({ proMode: true, stripKeys: ["shell:a"], composing: false }),
    false,
  );
  assert.equal(
    workspaceLauncherShown({ proMode: true, stripKeys: [], composing: true }),
    false,
    "a text conversation draft shows the composer",
  );
  assert.equal(
    workspaceLauncherShown({ proMode: true, stripKeys: null, composing: false }),
    false,
    "no strip, no launcher",
  );
});

test("the arrow keys move through the launcher and wrap", () => {
  assert.equal(launcherFocusIndex("ArrowDown", -1, 4), 0);
  assert.equal(launcherFocusIndex("ArrowDown", 3, 4), 0);
  assert.equal(launcherFocusIndex("ArrowUp", 0, 4), 3);
  assert.equal(launcherFocusIndex("ArrowUp", -1, 4), 3);
  assert.equal(launcherFocusIndex("Home", 2, 4), 0);
  assert.equal(launcherFocusIndex("End", 0, 4), 3);
  assert.equal(launcherFocusIndex("Enter", 1, 4), null, "Enter stays with the button");
  assert.equal(launcherFocusIndex("ArrowDown", -1, 0), null);
});
