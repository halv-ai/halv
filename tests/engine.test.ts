import assert from "node:assert/strict";
import test from "node:test";

import { engineOff, engineReadyForSend } from "../src/engine.ts";
import { formatEngineStatus } from "../src/format.ts";
import type { SidecarStatus } from "../src/types.ts";

const running: SidecarStatus = {
  state: "running",
  spawnedByUs: true,
  managedByHalv: true,
  version: "0.9.0",
  pinnedVersion: "0.9.0",
  port: 8787,
  optimizationPaused: false,
};

test("a send can start when the engine runs or is off", () => {
  assert.equal(engineReadyForSend(running), true);
  assert.equal(engineReadyForSend({ state: "off", port: 8787 }), true);
});

test("a send waits while the engine starts, installs, or fails", () => {
  assert.equal(engineReadyForSend(null), false);
  assert.equal(engineReadyForSend({ state: "starting", port: 8787 }), false);
  assert.equal(engineReadyForSend({ state: "installing", port: 8787 }), false);
  assert.equal(
    engineReadyForSend({ state: "notInstalled", installHint: "", port: 8787 }),
    false,
  );
  assert.equal(
    engineReadyForSend({ state: "error", message: "down", port: 8787 }),
    false,
  );
});

test("only the off state reads as engine off", () => {
  assert.equal(engineOff({ state: "off", port: 8787 }), true);
  assert.equal(engineOff(running), false);
  assert.equal(engineOff({ state: "error", message: "down", port: 8787 }), false);
  assert.equal(engineOff(null), false);
});

test("the engine status text names the off state", () => {
  assert.equal(formatEngineStatus({ state: "off", port: 8787 }), "off");
});
