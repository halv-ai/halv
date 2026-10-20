import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import { createTerminalSizeRegistry } from "../src/terminalSizeOwner.ts";

type Size = { cols: number; rows: number };

function harness() {
  const registry = createTerminalSizeRegistry();
  const sent: string[] = [];
  const errors: unknown[] = [];
  let fail = false;
  const pane = (sessionId: string, name: string, canSend?: () => boolean) =>
    registry.claim(
      sessionId,
      async ({ cols, rows }: Size) => {
        sent.push(`${name} ${sessionId} ${cols}x${rows}`);
        if (fail) throw new Error("resize failed");
      },
      (cause: unknown) => errors.push(cause),
      canSend,
    );
  return {
    pane,
    sent,
    errors,
    failNext(value: boolean) {
      fail = value;
    },
  };
}

test("sends a grid only when it differs from the last size sent", () => {
  const { pane, sent } = harness();
  const surface = pane("s1", "surface");
  surface.resize({ cols: 80, rows: 24 });
  surface.setVisible(true);
  surface.resize({ cols: 80, rows: 24 });
  surface.resize({ cols: 81, rows: 24 });
  surface.resize({ cols: 81, rows: 24 });
  surface.resize({ cols: 81, rows: 25 });
  assert.deepEqual(sent, [
    "surface s1 80x24",
    "surface s1 81x24",
    "surface s1 81x25",
  ]);
});

test("a hidden pane sends nothing and sends only a changed grid when it shows", () => {
  const { pane, sent } = harness();
  const surface = pane("s1", "surface");
  surface.setVisible(true);
  surface.resize({ cols: 26, rows: 37 });
  surface.setVisible(false);
  surface.resize({ cols: 28, rows: 29 });
  surface.resize({ cols: 26, rows: 37 });
  surface.setVisible(true);
  assert.deepEqual(sent, ["surface s1 26x37"]);

  surface.setVisible(false);
  surface.resize({ cols: 30, rows: 37 });
  surface.setVisible(true);
  assert.deepEqual(sent, ["surface s1 26x37", "surface s1 30x37"]);
});

test("a second pane owns the size and gives it back when it closes", () => {
  const { pane, sent } = harness();
  const surface = pane("s1", "surface");
  surface.setVisible(true);
  surface.resize({ cols: 26, rows: 37 });

  const overlay = pane("s1", "overlay");
  overlay.setVisible(true);
  overlay.resize({ cols: 24, rows: 44 });
  // The surface stays laid out under the overlay. Its fits do not send.
  surface.resize({ cols: 27, rows: 37 });
  overlay.dispose();

  assert.deepEqual(sent, [
    "surface s1 26x37",
    "overlay s1 24x44",
    "surface s1 27x37",
  ]);
});

test("closing a pane that does not own the size sends nothing", () => {
  const { pane, sent } = harness();
  const surface = pane("s1", "surface");
  surface.setVisible(true);
  surface.resize({ cols: 26, rows: 37 });
  const overlay = pane("s1", "overlay");
  overlay.resize({ cols: 24, rows: 44 });
  overlay.setVisible(true);
  surface.dispose();
  overlay.resize({ cols: 24, rows: 44 });
  assert.deepEqual(sent, ["surface s1 26x37", "overlay s1 24x44"]);
});

test("the owner gives the size back only when the grids differ", () => {
  const { pane, sent } = harness();
  const surface = pane("s1", "surface");
  surface.setVisible(true);
  surface.resize({ cols: 26, rows: 37 });
  const overlay = pane("s1", "overlay");
  overlay.setVisible(true);
  overlay.resize({ cols: 26, rows: 37 });
  overlay.setVisible(false);
  assert.deepEqual(sent, ["surface s1 26x37"]);
});

test("sessions keep separate sizes", () => {
  const { pane, sent } = harness();
  const first = pane("s1", "pane");
  const second = pane("s2", "pane");
  first.setVisible(true);
  second.setVisible(true);
  first.resize({ cols: 26, rows: 37 });
  second.resize({ cols: 26, rows: 37 });
  assert.deepEqual(sent, ["pane s1 26x37", "pane s2 26x37"]);
});

test("a failed resize sends the same grid again on the next fit", async () => {
  const { pane, sent, errors, failNext } = harness();
  const surface = pane("s1", "surface");
  surface.setVisible(true);
  failNext(true);
  surface.resize({ cols: 26, rows: 37 });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(errors.length, 1);
  failNext(false);
  surface.resize({ cols: 26, rows: 37 });
  assert.deepEqual(sent, ["surface s1 26x37", "surface s1 26x37"]);
});

test("a new pane sends its first grid after the last pane closed", () => {
  const { pane, sent } = harness();
  const first = pane("s1", "first");
  first.setVisible(true);
  first.resize({ cols: 26, rows: 37 });
  first.dispose();
  first.resize({ cols: 30, rows: 37 });

  const second = pane("s1", "second");
  second.setVisible(true);
  second.resize({ cols: 26, rows: 37 });
  assert.deepEqual(sent, ["first s1 26x37", "second s1 26x37"]);
});

test("a pane of an ended session sends nothing when it gets the size back", () => {
  const { pane, sent } = harness();
  let ended = false;
  const surface = pane("s1", "surface", () => !ended);
  surface.resize({ cols: 26, rows: 37 });
  surface.setVisible(true);
  const overlay = pane("s1", "overlay", () => !ended);
  overlay.resize({ cols: 24, rows: 44 });
  overlay.setVisible(true);
  // The user kills the session from the overlay, then closes the overlay.
  ended = true;
  overlay.dispose();
  surface.setVisible(false);
  surface.setVisible(true);
  assert.deepEqual(sent, ["surface s1 26x37", "overlay s1 24x44"]);
});

test("a pane sends only a grid that differs from the known PTY size", () => {
  const { pane, sent } = harness();
  const surface = pane("s1", "surface");
  // The pane mounts hidden and takes the PTY size from the attach.
  surface.ptySize({ cols: 108, rows: 44 });
  surface.resize({ cols: 108, rows: 44 });
  surface.setVisible(true);
  surface.resize({ cols: 26, rows: 37 });
  // A size that a pane sent stays the known size.
  surface.ptySize({ cols: 108, rows: 44 });
  surface.resize({ cols: 26, rows: 37 });
  assert.deepEqual(sent, ["surface s1 26x37"]);
});
