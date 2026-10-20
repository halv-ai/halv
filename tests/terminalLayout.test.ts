import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  calculateTerminalFit,
  swapTerminalSiblings,
  rotateTerminalSplitAt,
  closeTerminalLeaf,
  collectTerminalLeafIds,
  deserializeTerminalLayout,
  deserializeTerminalPanelLayout,
  reconcileTerminalPanelLayout,
  resolveTerminalCellSize,
  serializeTerminalLayout,
  serializeTerminalPanelLayout,
  setTerminalSplitRatio,
  splitTerminalLeaf,
  terminalLeaf,
  type TerminalPanelLayoutSnapshot,
} from "../src/terminalLayout.ts";

const BASE_FIT_METRICS = {
  paneWidth: 1000,
  paneHeight: 600,
  paneHeaderHeight: 30,
  paddingTop: 12,
  paddingRight: 12,
  paddingBottom: 14,
  paddingLeft: 12,
  borderTop: 1,
  borderRight: 1,
  borderBottom: 1,
  borderLeft: 1,
  scrollbarWidth: 10,
  cellWidth: 8,
  cellHeight: 17,
};

test("fits a single terminal pane inside its header, padding, and borders", () => {
  assert.deepEqual(calculateTerminalFit(BASE_FIT_METRICS), {
    cols: 120,
    rows: 31,
  });
});

test("fits an agent pane after the panel and pane headers", () => {
  const panelHeight = 720;
  const panelBorderHeight = 2;
  const panelHeaderHeight = 48;
  const tabStripHeight = 36;
  const paneHeight =
    panelHeight - panelBorderHeight - panelHeaderHeight - tabStripHeight;

  assert.deepEqual(
    calculateTerminalFit({
      ...BASE_FIT_METRICS,
      paneHeight,
      paneHeaderHeight: 30,
    }),
    { cols: 120, rows: 33 },
  );
});

test("fits both terminal split directions with whole-cell rows", () => {
  assert.deepEqual(
    calculateTerminalFit({ ...BASE_FIT_METRICS, paneWidth: 500 }),
    { cols: 58, rows: 31 },
  );
  assert.deepEqual(
    calculateTerminalFit({ ...BASE_FIT_METRICS, paneHeight: 300 }),
    { cols: 120, rows: 14 },
  );
});

test("refits after divider and window size changes", () => {
  const dividerResize = calculateTerminalFit({
    ...BASE_FIT_METRICS,
    paneWidth: 630,
    paneHeight: 410,
  });
  const windowResize = calculateTerminalFit({
    ...BASE_FIT_METRICS,
    paneWidth: 840,
    paneHeight: 520,
  });
  assert.deepEqual(dividerResize, { cols: 74, rows: 20 });
  assert.deepEqual(windowResize, { cols: 100, rows: 27 });
  assert.equal(Number.isInteger(dividerResize?.rows), true);
  assert.equal(Number.isInteger(windowResize?.rows), true);
});

test("prefers xterm render dimensions over a stale screen grid", () => {
  const staleScreenCell = resolveTerminalCellSize(null, {
    screenWidth: 800,
    screenHeight: 408,
    cols: 80,
    rows: 24,
  });
  const renderCell = resolveTerminalCellSize(
    { css: { cell: { width: 8, height: 17 } } },
    {
      screenWidth: 800,
      screenHeight: 408,
      cols: 80,
      rows: 24,
    },
  );
  assert.deepEqual(staleScreenCell, { width: 10, height: 17 });
  assert.deepEqual(renderCell, { width: 8, height: 17 });
  assert.deepEqual(
    calculateTerminalFit({
      ...BASE_FIT_METRICS,
      paneWidth: 900,
      cellWidth: staleScreenCell!.width,
    }),
    { cols: 86, rows: 31 },
  );
  assert.deepEqual(
    calculateTerminalFit({
      ...BASE_FIT_METRICS,
      paneWidth: 900,
      cellWidth: renderCell!.width,
    }),
    { cols: 108, rows: 31 },
  );
});

test("falls back when xterm render dimensions are not ready", () => {
  assert.deepEqual(
    resolveTerminalCellSize(
      { css: { cell: { width: 0, height: Number.NaN } } },
      {
        screenWidth: 720,
        screenHeight: 340,
        cols: 80,
        rows: 20,
      },
    ),
    { width: 9, height: 17 },
  );
});

// Xterm 6.0.0 DomRenderer._updateDimensions for one grid. The CSS cell is a
// rounded screen size divided by the grid, so it changes with the grid.
function xtermRenderDimensions(
  charWidth: number,
  charHeight: number,
  pixelRatio: number,
  cols: number,
  rows: number,
) {
  const deviceCellWidth = charWidth * pixelRatio;
  const deviceCellHeight = Math.ceil(charHeight * pixelRatio);
  return {
    device: { cell: { width: deviceCellWidth, height: deviceCellHeight } },
    css: {
      cell: {
        width: Math.round((deviceCellWidth * cols) / pixelRatio) / cols,
        height: Math.round((deviceCellHeight * rows) / pixelRatio) / rows,
      },
    },
  };
}

// WebKit, SF Mono 13 px, weight 500, line height 1, pixel ratio 2.
const SF_MONO_CHAR_WIDTH = 8.0361328125;
const SF_MONO_CHAR_HEIGHT = 16;

function fitAt(
  availableWidth: number,
  availableHeight: number,
  grid: { cols: number; rows: number },
  pixelRatio = 2,
  charWidth = SF_MONO_CHAR_WIDTH,
  charHeight = SF_MONO_CHAR_HEIGHT,
) {
  const renderDimensions = xtermRenderDimensions(
    charWidth,
    charHeight,
    pixelRatio,
    grid.cols,
    grid.rows,
  );
  const fallback = { screenWidth: 0, screenHeight: 0, ...grid };
  const cell = resolveTerminalCellSize(renderDimensions, fallback, pixelRatio)!;
  const previous = resolveTerminalCellSize(renderDimensions, fallback)!;
  const metrics = {
    ...BASE_FIT_METRICS,
    // BASE_FIT_METRICS takes 36 px of width and 58 px of height.
    paneWidth: availableWidth + 36,
    paneHeight: availableHeight + 58,
  };
  return {
    fit: calculateTerminalFit({
      ...metrics,
      cellWidth: cell.width,
      cellHeight: cell.height,
      exactCellSize: cell.exact,
    })!,
    // The fit before this change used the grid-dependent CSS cell.
    previousFit: calculateTerminalFit({
      ...metrics,
      cellWidth: previous.width,
      cellHeight: previous.height,
    })!,
  };
}

test("fits one grid inside the xterm column bands", () => {
  // Real layout sweep: Claude panes flipped 41 <-> 42 at 337.03-337.78 px
  // and the Codex pane flipped 69 <-> 70 at 562.31 and 562.81 px.
  const bands = [
    { cols: 41, widths: [337.03, 337.4, 337.78] },
    { cols: 69, widths: [562.31, 562.81] },
  ];
  for (const { cols, widths } of bands) {
    for (const width of widths) {
      const fromSmaller = fitAt(width, 600, { cols, rows: 37 });
      const fromLarger = fitAt(width, 600, { cols: cols + 1, rows: 37 });
      assert.equal(fromSmaller.previousFit.cols, cols + 1, `${width} px`);
      assert.equal(fromLarger.previousFit.cols, cols, `${width} px`);
      assert.equal(fromSmaller.fit.cols, cols, `${width} px`);
      assert.equal(fromLarger.fit.cols, cols, `${width} px`);
      // Xterm draws the chosen grid inside the pane.
      assert.ok(Math.round(SF_MONO_CHAR_WIDTH * cols) <= width);
    }
  }
});

test("a stable pane width causes at most one resize", () => {
  const cases = [
    // SF Mono at pixel ratio 2: even device cell height.
    { pixelRatio: 2, charWidth: SF_MONO_CHAR_WIDTH, charHeight: 16 },
    // Odd device cell height: rows also have bands.
    { pixelRatio: 2, charWidth: 7.8251953125, charHeight: 16.25 },
    { pixelRatio: 1, charWidth: 7.8251953125, charHeight: 15.5 },
  ];
  for (const { pixelRatio, charWidth, charHeight } of cases) {
    for (let width = 160; width < 760; width += 0.0625) {
      const height = 120 + ((width * 7) % 480);
      for (const start of [{ cols: 80, rows: 24 }, { cols: 20, rows: 50 }]) {
        let grid = start;
        let resizes = 0;
        for (let pass = 0; pass < 6; pass += 1) {
          const { fit } = fitAt(width, height, grid, pixelRatio, charWidth, charHeight);
          if (fit.cols === grid.cols && fit.rows === grid.rows) break;
          grid = fit;
          resizes += 1;
        }
        const label = `${pixelRatio}x ${charWidth} ${width}x${height}`;
        assert.ok(resizes <= 1, label);
        const deviceHeight = Math.ceil(charHeight * pixelRatio);
        assert.ok(Math.round(charWidth * grid.cols) <= width, label);
        assert.ok(
          Math.round((deviceHeight * grid.rows) / pixelRatio) <= height,
          label,
        );
      }
    }
  }
});

test("TerminalPane refits after renderer and font layout changes", () => {
  const source = readFileSync(
    new URL("../src/components/TerminalPane.tsx", import.meta.url),
    "utf8",
  );
  assert.match(source, /_renderService\?\.dimensions/);
  assert.match(source, /terminal\.onRender\(\(\) =>/);
  assert.match(source, /document\.fonts\?\.ready\.then/);
  assert.match(source, /resizeObserver\.observe\(observedScreen\)/);
  assert.match(
    source,
    /terminal\.resize\([\s\S]*?requestAnimationFrame\(\(\) =>[\s\S]*?fit\(remainingFollowUps - 1\)/,
  );
});

test("supervisor reconnect status does not change terminal geometry", () => {
  const paneSource = readFileSync(
    new URL("../src/components/TerminalPane.tsx", import.meta.url),
    "utf8",
  );
  const panelSource = readFileSync(
    new URL("../src/components/TerminalPanel.tsx", import.meta.url),
    "utf8",
  );
  const styles = readFileSync(
    new URL("../src/styles.css", import.meta.url),
    "utf8",
  );

  assert.match(
    paneSource,
    /<span\s+className="terminal-supervisor-reconnect"[\s\S]*?<\/span>/,
  );
  assert.doesNotMatch(
    paneSource,
    /<div\s+className="terminal-supervisor-reconnect"/,
  );
  assert.doesNotMatch(panelSource, /SUPERVISOR_RECONNECTED_NOTICE_DELAY_MS/);

  const reconnectStyle = styles.match(
    /\.terminal-supervisor-reconnect\s*\{([\s\S]*?)\}/,
  )?.[1];
  assert.ok(reconnectStyle);
  assert.match(reconnectStyle, /max-width:/);
  assert.match(reconnectStyle, /white-space:\s*nowrap/);
  assert.doesNotMatch(reconnectStyle, /border-bottom:/);
});

test("splits a leaf only with the explicit user direction", () => {
  const vertical = splitTerminalLeaf(
    terminalLeaf("terminal-a"),
    "terminal-a",
    "terminal-b",
    "vertical",
  );
  assert.deepEqual(vertical, {
    type: "split",
    direction: "vertical",
    first: { type: "leaf", leafId: "terminal-a" },
    second: { type: "leaf", leafId: "terminal-b" },
    ratio: 0.5,
  });

  const horizontal = splitTerminalLeaf(
    vertical,
    "terminal-b",
    "terminal-c",
    "horizontal",
  );
  assert.equal(horizontal.type, "split");
  assert.equal(horizontal.second.type, "split");
  assert.equal(horizontal.second.direction, "horizontal");
});

test("closes a leaf and collapses its parent to the sibling", () => {
  const layout = splitTerminalLeaf(
    terminalLeaf("terminal-a"),
    "terminal-a",
    "terminal-b",
    "vertical",
  );
  assert.deepEqual(closeTerminalLeaf(layout, "terminal-a"), {
    type: "leaf",
    leafId: "terminal-b",
  });
  assert.equal(
    closeTerminalLeaf({ type: "leaf", leafId: "terminal-b" }, "terminal-b"),
    null,
  );
});

test("updates a nested split ratio by path", () => {
  const layout = splitTerminalLeaf(
    splitTerminalLeaf(
      terminalLeaf("terminal-a"),
      "terminal-a",
      "terminal-b",
      "vertical",
    ),
    "terminal-b",
    "terminal-c",
    "horizontal",
  );
  const resized = setTerminalSplitRatio(layout, ["second"], 0.7);
  assert.equal(resized.type, "split");
  assert.equal(resized.second.type, "split");
  assert.equal(resized.second.ratio, 0.7);
});

test("round-trips a valid layout and rejects missing directions", () => {
  const layout = splitTerminalLeaf(
    terminalLeaf("terminal-a"),
    "terminal-a",
    "terminal-b",
    "horizontal",
  );
  assert.deepEqual(
    deserializeTerminalLayout(serializeTerminalLayout(layout)),
    layout,
  );
  assert.equal(
    deserializeTerminalLayout(
      JSON.stringify({
        version: 1,
        root: {
          type: "split",
          first: terminalLeaf("terminal-a"),
          second: terminalLeaf("terminal-b"),
        },
      }),
    ),
    null,
  );
});

test("round-trips panel tabs without synthesizing a direction", () => {
  const snapshot: TerminalPanelLayoutSnapshot = {
    version: 1,
    activeTabId: "tab-a",
    activeLeafId: "terminal-b",
    tabs: [
      {
        tabId: "tab-a",
        root: splitTerminalLeaf(
          terminalLeaf("terminal-a"),
          "terminal-a",
          "terminal-b",
          "vertical",
        ),
      },
    ],
  };
  const restored = deserializeTerminalPanelLayout(
    serializeTerminalPanelLayout(snapshot),
  );
  assert.deepEqual(restored, snapshot);
  assert.deepEqual(
    collectTerminalLeafIds(restored?.tabs[0]?.root ?? null),
    ["terminal-a", "terminal-b"],
  );
});

test("reconciles sessions without changing an existing split direction", () => {
  const existing = reconcileTerminalPanelLayout(null, ["terminal-a"]);
  const split = {
    ...existing,
    tabs: [
      {
        tabId: "terminal-a",
        root: splitTerminalLeaf(
          existing.tabs[0]!.root,
          "terminal-a",
          "terminal-b",
          "vertical" as const,
        ),
      },
    ],
  };
  const reconciled = reconcileTerminalPanelLayout(
    split,
    ["terminal-a", "terminal-b", "terminal-c"],
    "terminal-b",
  );
  assert.equal(reconciled.tabs.length, 1);
  // Tabs are retired: the new session splits into the single surface.
  const root = reconciled.tabs[0]!.root;
  assert.equal(root.type, "split");
  assert.equal(
    root.type === "split" && root.second.type === "leaf" && root.second.leafId,
    "terminal-c",
  );
});

test("reconcile keeps only the first occurrence of a duplicated leaf id", () => {
  const reconciled = reconcileTerminalPanelLayout(
    {
      version: 1,
      activeTabId: "terminal-a",
      activeLeafId: "terminal-a",
      tabs: [
        {
          tabId: "terminal-a",
          root: {
            type: "split",
            direction: "vertical",
            first: { type: "leaf", leafId: "terminal-a" },
            second: { type: "leaf", leafId: "terminal-b" },
          },
        },
        { tabId: "dupe-a", root: { type: "leaf", leafId: "terminal-a" } },
        { tabId: "dupe-b", root: { type: "leaf", leafId: "terminal-b" } },
      ],
    },
    ["terminal-a", "terminal-b"],
  );

  assert.equal(reconciled.tabs.length, 1);
  assert.deepEqual(
    collectTerminalLeafIds(reconciled.tabs[0]!.root),
    ["terminal-a", "terminal-b"],
  );
});

test("swap flips the siblings and keeps each pane's size", () => {
  const root = {
    type: "split" as const,
    direction: "vertical" as const,
    first: { type: "leaf" as const, leafId: "a" },
    second: { type: "leaf" as const, leafId: "b" },
    ratio: 0.3,
  };
  const swapped = swapTerminalSiblings(root, "b");
  assert.deepEqual(swapped, {
    type: "split",
    direction: "vertical",
    first: { type: "leaf", leafId: "b" },
    second: { type: "leaf", leafId: "a" },
    ratio: 0.7,
  });
});

test("rotate toggles the nearest parent split direction", () => {
  const root = {
    type: "split" as const,
    direction: "vertical" as const,
    first: { type: "leaf" as const, leafId: "a" },
    second: {
      type: "split" as const,
      direction: "horizontal" as const,
      first: { type: "leaf" as const, leafId: "b" },
      second: { type: "leaf" as const, leafId: "c" },
    },
  };
  const rotated = rotateTerminalSplitAt(root, "c");
  assert.equal(rotated.type === "split" && rotated.direction, "vertical");
  assert.equal(
    rotated.type === "split" &&
      rotated.second.type === "split" &&
      rotated.second.direction,
    "vertical",
  );
});

test("reconcile replaces an aliased leaf instead of appending a tab", () => {
  const snapshot = {
    version: 1 as const,
    activeTabId: "old",
    activeLeafId: "old",
    tabs: [
      {
        tabId: "old",
        root: {
          type: "split" as const,
          direction: "vertical" as const,
          first: { type: "leaf" as const, leafId: "old" },
          second: { type: "leaf" as const, leafId: "other" },
        },
      },
    ],
  };
  const next = reconcileTerminalPanelLayout(
    snapshot,
    ["fresh", "other", "old"],
    null,
    new Map([["fresh", "old"]]),
  );
  assert.equal(next.tabs.length, 1);
  assert.deepEqual(next.tabs[0].root, {
    type: "split",
    direction: "vertical",
    first: { type: "leaf", leafId: "fresh" },
    second: { type: "leaf", leafId: "other" },
  });
});
