import assert from "node:assert/strict";
import test from "node:test";

import {
  effectiveShortcut,
  formatShortcut,
  shortcutConflict,
  shortcutDefinitions,
  shortcutFromKeyboardEvent,
  shortcutMatchesEvent,
} from "../src/shortcuts.ts";

function keyEvent(
  key: string,
  options: Partial<{
    code: string;
    metaKey: boolean;
    ctrlKey: boolean;
    altKey: boolean;
    shiftKey: boolean;
  }> = {},
) {
  return {
    key,
    code: options.code ?? `Key${key.toUpperCase()}`,
    metaKey: options.metaKey ?? false,
    ctrlKey: options.ctrlKey ?? false,
    altKey: options.altKey ?? false,
    shiftKey: options.shiftKey ?? false,
  };
}

test("captures portable macOS and non-macOS shortcut chords", () => {
  assert.equal(
    shortcutFromKeyboardEvent(keyEvent("k", { metaKey: true }), "MacIntel"),
    "mod+k",
  );
  assert.equal(
    shortcutFromKeyboardEvent(keyEvent("k", { ctrlKey: true }), "Linux x86_64"),
    "mod+k",
  );
  assert.equal(
    shortcutFromKeyboardEvent(
      keyEvent("D", { ctrlKey: true, shiftKey: true }),
      "MacIntel",
    ),
    "ctrl+shift+d",
  );
  assert.equal(shortcutFromKeyboardEvent(keyEvent("a"), "MacIntel"), null);
});

test("matches bindings without accepting accidental extra modifiers", () => {
  assert.equal(
    shortcutMatchesEvent(keyEvent("k", { metaKey: true }), "mod+k", "MacIntel"),
    true,
  );
  assert.equal(
    shortcutMatchesEvent(
      keyEvent("k", { metaKey: true, shiftKey: true }),
      "mod+k",
      "MacIntel",
    ),
    false,
  );
  assert.equal(
    shortcutMatchesEvent(
      keyEvent("+", { code: "Equal", metaKey: true, shiftKey: true }),
      "mod+plus",
      "MacIntel",
    ),
    true,
  );
});

test("uses overrides, formats them, and detects conflicts with defaults", () => {
  const overrides = { "application.settings": "mod+s" };
  assert.equal(effectiveShortcut("application.settings", overrides), "mod+s");
  assert.equal(formatShortcut("mod+shift+slash", "MacIntel"), "⌘⇧/");
  assert.equal(formatShortcut("ctrl+shift+d", "Linux"), "Ctrl+Shift+D");
  assert.equal(
    shortcutConflict("application.settings", "mod+k", overrides)?.id,
    "application.command_menu",
  );
});

test("defines terminal tab cycling with macOS bracket shortcuts", () => {
  assert.equal(
    shortcutDefinitions.find(
      (definition) => definition.id === "workspace.previous_terminal_tab",
    )?.defaultBinding,
    "mod+shift+bracketleft",
  );
  assert.equal(
    shortcutDefinitions.find(
      (definition) => definition.id === "workspace.next_terminal_tab",
    )?.defaultBinding,
    "mod+shift+bracketright",
  );
});

test("pinned conversation shortcuts do not overlap terminal bindings or plain shell keys", () => {
  for (const [id, key] of [["task.previous_pinned", "ArrowLeft"], ["task.next_pinned", "ArrowRight"]]) {
    const binding = effectiveShortcut(id, {});
    assert.equal(shortcutConflict(id, binding, {}), null);
    assert.equal(shortcutMatchesEvent(keyEvent(key, { metaKey: true, altKey: true }), binding, "MacIntel"), true);
    assert.equal(shortcutMatchesEvent(keyEvent(key, { altKey: true }), binding, "MacIntel"), false);
    assert.equal(shortcutMatchesEvent(keyEvent(key, { ctrlKey: true }), binding, "Linux"), false);
  }
  assert.equal(formatShortcut(effectiveShortcut("task.next_pinned", {}), "MacIntel"), "⌘⌥→");
});
