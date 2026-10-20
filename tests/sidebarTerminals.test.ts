import assert from "node:assert/strict";
import test from "node:test";
import { standaloneSidebarTerminals } from "../src/sidebarTerminals.ts";

const terminal = (
  id: string,
  conversationId: string | null,
  customTitle: string | null = null,
) => ({ id, conversationId, customTitle });

test("removes the terminal already exposed by a visible conversation", () => {
  const terminals = [
    terminal("visible-terminal", "visible-conversation"),
    terminal("orphan-terminal", null),
  ];

  assert.deepEqual(
    standaloneSidebarTerminals(terminals, [{ id: "visible-conversation" }])
      .map(({ id }) => id),
    ["orphan-terminal"],
  );
});

test("preserves terminals bound only to archived conversations", () => {
  const terminals = [
    terminal("active-terminal", "active-conversation"),
    terminal("archived-terminal", "archived-conversation"),
  ];

  assert.deepEqual(
    standaloneSidebarTerminals(terminals, [{ id: "active-conversation" }])
      .map(({ id }) => id),
    ["archived-terminal"],
  );
});

test("preserves additional panes that the conversation row cannot open", () => {
  const terminals = [
    terminal("fallback-terminal", "conversation"),
    terminal("named-terminal", "conversation", "Release audit"),
    terminal("second-terminal", "conversation", "Verification"),
  ];

  assert.deepEqual(
    standaloneSidebarTerminals(terminals, [{ id: "conversation" }]).map(
      ({ id }) => id,
    ),
    ["fallback-terminal", "second-terminal"],
  );
});

test("a continuation shell and its ended agent share one sidebar row", () => {
  const terminals = [
    terminal("0f6d6c1e-agent", "conversation"),
    {
      ...terminal("terminal-shell", "conversation"),
      continuedFrom: "0f6d6c1e-agent",
    },
    terminal("terminal-extra", "conversation"),
  ];

  // The conversation row opens the agent pane, which now shows the shell.
  assert.deepEqual(
    standaloneSidebarTerminals(terminals, [{ id: "conversation" }]).map(
      ({ id }) => id,
    ),
    ["terminal-extra"],
  );
  // An archived conversation keeps one row for the pair: the shell.
  assert.deepEqual(
    standaloneSidebarTerminals(terminals, []).map(({ id }) => id),
    ["terminal-shell", "terminal-extra"],
  );
});

test("a renamed continuation shell hides its ended source", () => {
  const terminals = [
    terminal("0f6d6c1e-agent", "conversation"),
    {
      ...terminal("terminal-shell", "conversation", "Deploy shell"),
      continuedFrom: "0f6d6c1e-agent",
    },
  ];

  assert.deepEqual(
    standaloneSidebarTerminals(terminals, [{ id: "conversation" }]),
    [],
  );
});
