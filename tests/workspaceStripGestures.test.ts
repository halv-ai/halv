import assert from "node:assert/strict";
import test from "node:test";

// Stage S7 of ADR 0010: the pure helpers of the strip gestures. The drag
// threshold, the insertion bar, the drop result, the overflow state, the
// chevron step and the hover card timing copy Orca's tab strip.

import {
  dropBeforeKey,
  dropIndicatorEdge,
  dropTargetAt,
  hoverCardDelay,
  hoverCardPosition,
  isDragDistance,
  moveIndexBefore,
  revealScrollLeft,
  TAB_HOVER_DELAY_MS,
  tabStripFadeClasses,
  tabStripOverflow,
  tabStripScrollStep,
} from "../src/workspaceTabStrip.ts";
import { moveTab, type TabFacts, type TabRow, type TabsState } from "../src/workspaceTabs.ts";

const spans = [
  { key: "a", left: 0, right: 100 },
  { key: "b", left: 102, right: 202 },
  { key: "c", left: 204, right: 304 },
];
const keys = spans.map((span) => span.key);

test("a press becomes a drag at 12 px, as in Orca", () => {
  assert.equal(isDragDistance(11, 0), false);
  assert.equal(isDragDistance(0, 12), true);
  assert.equal(isDragDistance(9, 9), true);
});

test("the drop target is the tab under the pointer and its half", () => {
  assert.deepEqual(dropTargetAt(spans, "a", 120), { key: "b", side: "left" });
  assert.deepEqual(dropTargetAt(spans, "a", 180), { key: "b", side: "right" });
  // The dragged tab is no target.
  assert.equal(dropTargetAt(spans, "b", 150), null);
  // Before the first tab and after the last tab count as those tabs.
  assert.deepEqual(dropTargetAt(spans, "b", -40), { key: "a", side: "left" });
  assert.deepEqual(dropTargetAt(spans, "a", 900), { key: "c", side: "right" });
  // The gap between two tabs counts as the next tab.
  assert.deepEqual(dropTargetAt(spans, "a", 203), { key: "c", side: "left" });
  assert.equal(dropTargetAt([], "a", 10), null);
});

test("the insertion bar shows on the left edge of the next tab, else the right edge of the last", () => {
  assert.deepEqual(dropIndicatorEdge(keys, { key: "a", side: "right" }), { key: "b", side: "left" });
  assert.deepEqual(dropIndicatorEdge(keys, { key: "c", side: "right" }), { key: "c", side: "right" });
  assert.deepEqual(dropIndicatorEdge(keys, { key: "a", side: "left" }), { key: "a", side: "left" });
  assert.equal(dropIndicatorEdge(keys, null), null);
  assert.equal(dropIndicatorEdge(keys, { key: "x", side: "left" }), null);
});

test("a drop names the tab that follows, null for the end, undefined for no change", () => {
  assert.equal(dropBeforeKey(keys, "c", { key: "a", side: "left" }), "a");
  assert.equal(dropBeforeKey(keys, "a", { key: "c", side: "right" }), null);
  assert.equal(dropBeforeKey(keys, "a", { key: "b", side: "right" }), "c");
  // The two edges of the dragged tab keep the order.
  assert.equal(dropBeforeKey(keys, "b", { key: "a", side: "right" }), undefined);
  assert.equal(dropBeforeKey(keys, "b", { key: "c", side: "left" }), undefined);
  assert.equal(dropBeforeKey(keys, "b", null), undefined);
});

test("the move index counts every tab of the workspace, also hidden rows", () => {
  // The strip hides "s" (a shell tab with shell tabs off).
  const all = ["a", "s", "b", "c"];
  assert.equal(moveIndexBefore(all, "c", "a"), 0);
  assert.equal(moveIndexBefore(all, "a", null), 3);
  assert.equal(moveIndexBefore(all, "a", "c"), 2);
  assert.equal(moveIndexBefore(all, "a", "gone"), 3);
});

test("a drop result moves the tab in the store model", () => {
  const facts: TabFacts = {
    complete: true,
    conversations: new Map(
      ["a", "b", "c"].map((id) => [
        id,
        { id, projectId: 1, workspaceId: null, mode: "agent", archived: false, createdAt: 0, updatedAt: 0 },
      ]),
    ),
    workspaces: new Map([["primary-1", { id: "primary-1", projectId: 1, kind: "primary" }]]),
  };
  const row = (id: string, position: number): TabRow => ({
    key: `conversation:${id}`,
    conversationId: id,
    shellWorkspaceId: null,
    customTitle: null,
    defaultTitle: null,
    position,
    createdAt: 0,
    activatedAt: null,
  });
  const state: TabsState = {
    rows: { "conversation:a": row("a", 0), "conversation:b": row("b", 1), "conversation:c": row("c", 2) },
    activeByWorkspace: {},
  };
  const stripKeys = ["conversation:a", "conversation:b", "conversation:c"];
  const before = dropBeforeKey(stripKeys, "conversation:c", { key: "conversation:a", side: "left" });
  assert.equal(before, "conversation:a");
  const moved = moveTab(
    state,
    facts,
    "conversation:c",
    moveIndexBefore(stripKeys, "conversation:c", before ?? null),
  );
  assert.deepEqual(
    Object.values(moved.state.rows)
      .sort((left, right) => left.position - right.position)
      .map((item) => item.key),
    ["conversation:c", "conversation:a", "conversation:b"],
  );
  assert.equal(moved.ops.length, 1);
  assert.equal(moved.ops[0].type, "setPositions");
});

test("overflow: the chevrons and the fade follow the hidden edges", () => {
  assert.deepEqual(tabStripOverflow({ scrollWidth: 300, clientWidth: 300, scrollLeft: 0 }), {
    hasOverflow: false,
    canScrollStart: false,
    canScrollEnd: false,
  });
  const start = tabStripOverflow({ scrollWidth: 900, clientWidth: 300, scrollLeft: 0 });
  assert.deepEqual(start, { hasOverflow: true, canScrollStart: false, canScrollEnd: true });
  assert.equal(tabStripFadeClasses(start), "workspace-tab-scroll--fade-end");
  const middle = tabStripOverflow({ scrollWidth: 900, clientWidth: 300, scrollLeft: 200 });
  assert.equal(
    tabStripFadeClasses(middle),
    "workspace-tab-scroll--fade-start workspace-tab-scroll--fade-end",
  );
  const end = tabStripOverflow({ scrollWidth: 900, clientWidth: 300, scrollLeft: 600 });
  assert.deepEqual(end, { hasOverflow: true, canScrollStart: true, canScrollEnd: false });
  // One pixel of overflow is rounding, not overflow.
  assert.equal(tabStripOverflow({ scrollWidth: 301, clientWidth: 300, scrollLeft: 0 }).hasOverflow, false);
});

test("a chevron scrolls three quarters of the strip, at least 120 px", () => {
  assert.equal(tabStripScrollStep(400), 300);
  assert.equal(tabStripScrollStep(100), 120);
});

test("the active tab scrolls into view with the least movement", () => {
  const box = { clientWidth: 300, scrollLeft: 200 };
  assert.equal(revealScrollLeft(box, { left: 250, right: 400 }), null);
  assert.equal(revealScrollLeft(box, { left: 100, right: 280 }), 100);
  assert.equal(revealScrollLeft(box, { left: 450, right: 630 }), 330);
  // A tab wider than the strip shows its start.
  assert.equal(revealScrollLeft(box, { left: 600, right: 1000 }), 600);
});

test("the hover card waits 500 ms, and opens at once while the strip is warm", () => {
  assert.equal(hoverCardDelay(null, 1000), TAB_HOVER_DELAY_MS);
  assert.equal(hoverCardDelay(900, 1000), 0);
  assert.equal(hoverCardDelay(600, 1000), TAB_HOVER_DELAY_MS);
});

test("the hover card sits below the tab, left aligned, inside the window", () => {
  assert.deepEqual(hoverCardPosition({ left: 40, bottom: 50 }, { width: 256 }, 1280), { left: 40, top: 50 });
  assert.deepEqual(hoverCardPosition({ left: 1200, bottom: 50 }, { width: 256 }, 1280), {
    left: 1016,
    top: 50,
  });
  assert.deepEqual(hoverCardPosition({ left: -20, bottom: 50 }, { width: 256 }, 1280), { left: 8, top: 50 });
});
