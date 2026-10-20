import assert from "node:assert/strict";
import test from "node:test";
import { createTerminalDraftBridge, TerminalDraftError } from "../src/terminalDraft.ts";

test("preparing a task addresses only the requested terminal and never submits it", async () => {
  const bridge = createTerminalDraftBridge();
  const writes: string[] = [];
  bridge.register("other", async () => { assert.fail("wrong terminal"); });
  bridge.register("target", async (text) => { writes.push(text); });
  await bridge.prepare("target", " Review\nthis\r\nfile\tplease ");
  assert.deepEqual(writes, ["Review this file please"]);
  await assert.rejects(bridge.prepare("missing", "task"), { reason: "unavailable" });
});

test("draft preparation rejects controls and preserves errors from occupied terminals", async () => {
  const bridge = createTerminalDraftBridge();
  bridge.register("target", async () => { throw new TerminalDraftError("occupied"); });
  const endMarkers = ["task\u001b[201~rm -rf x", "task\u009b201~rm -rf x"];
  for (const text of [" ", "task\u001b[31m", "task\u0003", "task\u0085", ...endMarkers]) {
    await assert.rejects(bridge.prepare("target", text), { reason: "invalid" });
  }
  await assert.rejects(bridge.prepare("target", "task"), { reason: "occupied" });
});

test("an old pane cleanup cannot unregister a new pane for the same session", async () => {
  const bridge = createTerminalDraftBridge();
  const unregisterOld = bridge.register("session", async () => { assert.fail("old pane"); });
  let prepared = false;
  const unregisterNew = bridge.register("session", async () => { prepared = true; });
  unregisterOld();
  await bridge.prepare("session", "task");
  assert.equal(prepared, true);
  unregisterNew();
  await assert.rejects(bridge.prepare("session", "task"), { reason: "unavailable" });
});
