import assert from "node:assert/strict";
import test from "node:test";
// @ts-expect-error Node's test runner needs the extension.
import {
  TERMINAL_DETACHED_MODE_RESET,
  TERMINAL_EXIT_MODE_RESET,
  createTerminalReplayGuard,
  deriveTerminalPalette,
  eventsAfterTerminalSnapshot,
  resolveTerminalMinimumContrastRatio,
  type TerminalCssTokens,
} from "../src/terminalSurface.ts";

test("replays each event once after the attach snapshot", () => {
  const events = [
    { sequence: 4, data: "new-b" },
    { sequence: 2, data: "snapshot-tail" },
    { sequence: 3, data: "new-a-old-copy" },
    { sequence: 3, data: "new-a" },
  ];

  assert.deepEqual(eventsAfterTerminalSnapshot(2, events), [
    { sequence: 3, data: "new-a" },
    { sequence: 4, data: "new-b" },
  ]);
});

test("scopes replay guard to each queued xterm write", () => {
  const writes: string[] = [];
  const callbacks: Array<() => void> = [];
  const guard = createTerminalReplayGuard((chunk, callback) => {
    writes.push(chunk);
    callbacks.push(callback);
  });

  guard.write("replay-one", true);
  guard.write("live", false);
  guard.write("replay-two", true);

  assert.equal(guard.replayDepth(), 1);
  callbacks.shift()?.();
  assert.equal(guard.replayDepth(), 0);
  callbacks.shift()?.();
  assert.equal(guard.replayDepth(), 1);
  callbacks.shift()?.();
  assert.equal(guard.replayDepth(), 0);
  assert.deepEqual(writes, ["replay-one", "live", "replay-two"]);
});

test("derives the full xterm palette from CSS token channels", () => {
  const tokens: TerminalCssTokens = {
    background: "1 2 3",
    foreground: "4 5 6",
    cursor: "7 8 9",
    selection: "10 11 12",
    selectionForeground: "61 62 63",
    black: "13 14 15",
    red: "16 17 18",
    green: "19 20 21",
    yellow: "22 23 24",
    blue: "25 26 27",
    magenta: "28 29 30",
    cyan: "31 32 33",
    white: "34 35 36",
    brightBlack: "37 38 39",
    brightRed: "40 41 42",
    brightGreen: "43 44 45",
    brightYellow: "46 47 48",
    brightBlue: "49 50 51",
    brightMagenta: "52 53 54",
    brightCyan: "55 56 57",
    brightWhite: "58 59 60",
  };
  assert.deepEqual(deriveTerminalPalette(tokens), {
    background: "rgb(1 2 3)",
    foreground: "rgb(4 5 6)",
    cursor: "rgb(7 8 9)",
    cursorAccent: "rgb(1 2 3)",
    selectionBackground: "rgb(10 11 12 / 0.40)",
    selectionForeground: "rgb(4 5 6)",
    selectionInactiveBackground: "rgb(10 11 12 / 0.24)",
    black: "rgb(13 14 15)",
    red: "rgb(16 17 18)",
    green: "rgb(19 20 21)",
    yellow: "rgb(22 23 24)",
    blue: "rgb(25 26 27)",
    magenta: "rgb(28 29 30)",
    cyan: "rgb(31 32 33)",
    white: "rgb(34 35 36)",
    brightBlack: "rgb(37 38 39)",
    brightRed: "rgb(40 41 42)",
    brightGreen: "rgb(43 44 45)",
    brightYellow: "rgb(46 47 48)",
    brightBlue: "rgb(49 50 51)",
    brightMagenta: "rgb(52 53 54)",
    brightCyan: "rgb(55 56 57)",
    brightWhite: "rgb(58 59 60)",
  });
  assert.notEqual(
    deriveTerminalPalette(tokens).selectionForeground,
    deriveTerminalPalette(tokens).selectionBackground,
  );
});

test("tunes xterm contrast from the terminal background luminance", () => {
  assert.equal(resolveTerminalMinimumContrastRatio("238 235 226"), 4.5);
  assert.equal(resolveTerminalMinimumContrastRatio("11 10 7"), 3);
  assert.equal(resolveTerminalMinimumContrastRatio("rgb(255 255 255)"), 4.5);
  assert.equal(resolveTerminalMinimumContrastRatio("rgb(0 0 0)"), 3);
});

test("resets dead terminal modes and returns to the normal screen", () => {
  assert.equal(TERMINAL_EXIT_MODE_RESET.startsWith("\x18\x1b[?1049l"), true);
  assert.equal(TERMINAL_EXIT_MODE_RESET.includes("\x1b[0m"), true);
  assert.equal(TERMINAL_EXIT_MODE_RESET.includes("\x1b[?25h"), true);
  assert.equal(TERMINAL_EXIT_MODE_RESET.includes("\x1b[?1003l"), true);
  assert.equal(TERMINAL_EXIT_MODE_RESET.includes("\x1b[?1004l"), true);
  assert.equal(TERMINAL_EXIT_MODE_RESET.endsWith("\x1b[?2004l"), true);
});

test("detached terminal reset hides the read-only cursor", () => {
  assert.equal(
    TERMINAL_DETACHED_MODE_RESET,
    `${TERMINAL_EXIT_MODE_RESET}\x1b[?25l`,
  );
});
