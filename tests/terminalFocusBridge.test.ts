import assert from "node:assert/strict";
import test from "node:test";
import {
  FOCUS_REQUEST_LIFETIME_MS,
  createTerminalFocusBridge,
} from "../src/terminalFocusBridge.ts";

test("the panel that holds the pane takes the request", () => {
  const bridge = createTerminalFocusBridge(() => 0);
  const seen: string[] = [];
  bridge.subscribe((id) => {
    seen.push(`other:${id}`);
    return false;
  });
  bridge.subscribe((id) => {
    seen.push(`owner:${id}`);
    return id === "pane";
  });
  bridge.request("pane");
  assert.deepEqual(seen, ["other:pane", "owner:pane"]);
  assert.equal(bridge.pendingSessionId(), null);
});

test("a request waits for a panel that mounts later", () => {
  let now = 0;
  const bridge = createTerminalFocusBridge(() => now);
  bridge.request("pane");
  assert.equal(bridge.pendingSessionId(), "pane");
  let panes: string[] = [];
  const handled: string[] = [];
  bridge.subscribe((id) => {
    if (!panes.includes(id)) return false;
    handled.push(id);
    return true;
  });
  assert.deepEqual(handled, []);
  panes = ["pane"];
  bridge.retry();
  assert.deepEqual(handled, ["pane"]);
  bridge.retry();
  assert.deepEqual(handled, ["pane"]);

  bridge.request("late");
  now = FOCUS_REQUEST_LIFETIME_MS + 1;
  panes = ["late"];
  bridge.retry();
  assert.deepEqual(handled, ["pane"]);
  assert.equal(bridge.pendingSessionId(), null);
});
