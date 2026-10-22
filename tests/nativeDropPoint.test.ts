import assert from "node:assert/strict";
import test from "node:test";

import { nativeDropPointToCss } from "../src/nativeDropPoint.ts";
import { createTerminalDropBridge } from "../src/terminalDrop.ts";

test("keeps a macOS drop point as CSS pixels on a Retina display", () => {
  assert.deepEqual(nativeDropPointToCss({ x: 300, y: 600 }, "MacIntel", 2), { x: 300, y: 600 });
});

test("keeps a Linux drop point as CSS pixels on a scaled display", () => {
  assert.deepEqual(nativeDropPointToCss({ x: 300, y: 600 }, "Linux x86_64", 2), { x: 300, y: 600 });
});

test("divides a Windows drop point by the device pixel ratio", () => {
  assert.deepEqual(nativeDropPointToCss({ x: 600, y: 1200 }, "Win32", 2), { x: 300, y: 600 });
  assert.deepEqual(nativeDropPointToCss({ x: 450, y: 900 }, "Win32", 1.5), { x: 300, y: 600 });
});

test("uses a ratio of 1 when the Windows ratio is not valid", () => {
  assert.deepEqual(nativeDropPointToCss({ x: 30, y: 60 }, "Win32", 0), { x: 30, y: 60 });
  assert.deepEqual(nativeDropPointToCss({ x: 30, y: 60 }, "Win32", Number.NaN), { x: 30, y: 60 });
});

test("a macOS Retina drop on the lower terminal pane reaches that pane", () => {
  const bridge = createTerminalDropBridge();
  const pasted: string[] = [];
  // The terminal pane fills the lower part of a 1000x800 CSS pixel window.
  bridge.register("pane", {
    bounds: () => ({ left: 0, top: 500, right: 1000, bottom: 800 }),
    shell: "/bin/zsh",
    paste: (text) => pasted.push(text),
  });
  const point = nativeDropPointToCss({ x: 400, y: 650 }, "MacIntel", 2);
  assert.equal(bridge.drop(point.x, point.y, ["/Users/me/shot.png"]), true);
  assert.deepEqual(pasted, ["/Users/me/shot.png "]);
});
