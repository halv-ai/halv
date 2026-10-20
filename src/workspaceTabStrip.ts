// Pure helpers of the workspace tab strip (ADR 0010, stage S7). They copy
// Orca's tab strip: the drag threshold, the insertion bar, the overflow
// state with its fade mask, the scroll step of the chevrons, and the delays
// of the hover card. The strip component reads the DOM and calls these.

/** A press that moves this far is a drag. A shorter press is a click. */
export const TAB_DRAG_DISTANCE_PX = 12;
/** A chevron scrolls three quarters of the strip, at least 120 px. */
export const TAB_STRIP_SCROLL_FRACTION = 0.75;
export const TAB_STRIP_MIN_SCROLL_STEP_PX = 120;
/** A tab drag over a chevron scrolls the strip at this interval. */
export const TAB_STRIP_DRAG_SCROLL_INTERVAL_MS = 180;
/** The hover card opens after this delay. */
export const TAB_HOVER_DELAY_MS = 500;
/** A card that closed less than this time ago lets the next card open at
 *  once, so the user can slide along the strip. */
export const TAB_HOVER_SKIP_DELAY_MS = 300;
/** A drag that leaves the strip by more than this distance drops nothing. */
export const TAB_DROP_VERTICAL_SLACK_PX = 48;

const OVERFLOW_EPSILON_PX = 1;

// ---------------------------------------------------------------- drag

export function isDragDistance(dx: number, dy: number) {
  return Math.hypot(dx, dy) >= TAB_DRAG_DISTANCE_PX;
}

export type TabSpan = { key: string; left: number; right: number };

export type DropSide = "left" | "right";

/** The tab under the pointer and its half. A pointer before the first tab
 *  or after the last tab counts as that tab. The dragged tab itself is no
 *  target. */
export function dropTargetAt(
  spans: readonly TabSpan[],
  draggedKey: string,
  x: number,
): { key: string; side: DropSide } | null {
  if (spans.length === 0) return null;
  const first = spans[0];
  const last = spans[spans.length - 1];
  const over =
    x < first.left
      ? first
      : x >= last.right
        ? last
        : spans.find((span) => x >= span.left && x < span.right) ??
          // A pointer in the gap between two tabs counts as the next tab.
          spans.find((span) => x < span.left) ??
          last;
  if (over.key === draggedKey) return null;
  const middle = over.left + (over.right - over.left) / 2;
  return { key: over.key, side: x < middle ? "left" : "right" };
}

/** Where the insertion bar shows: the left edge of the tab at the insertion
 *  point, else the right edge of the last tab (Orca). */
export function dropIndicatorEdge(
  keys: readonly string[],
  target: { key: string; side: DropSide } | null,
): { key: string; side: DropSide } | null {
  if (!target) return null;
  const index = keys.indexOf(target.key);
  if (index < 0) return null;
  const insertion = index + (target.side === "right" ? 1 : 0);
  if (insertion < keys.length) return { key: keys[insertion], side: "left" };
  return { key: keys[keys.length - 1], side: "right" };
}

/**
 * The result of a drop: the tab before which the dragged tab goes, null for
 * the end of the strip, or undefined when the order does not change.
 */
export function dropBeforeKey(
  keys: readonly string[],
  draggedKey: string,
  target: { key: string; side: DropSide } | null,
): string | null | undefined {
  if (!target) return undefined;
  const from = keys.indexOf(draggedKey);
  const over = keys.indexOf(target.key);
  if (from < 0 || over < 0) return undefined;
  const insertion = over + (target.side === "right" ? 1 : 0);
  if (insertion === from || insertion === from + 1) return undefined;
  return keys[insertion] ?? null;
}

/** The index for `moveTab`: the position among the other tabs of the
 *  strip. The strip can hide some rows, so the caller passes every key. */
export function moveIndexBefore(
  keys: readonly string[],
  draggedKey: string,
  beforeKey: string | null,
) {
  const others = keys.filter((key) => key !== draggedKey);
  if (beforeKey === null) return others.length;
  const index = others.indexOf(beforeKey);
  return index < 0 ? others.length : index;
}

// ---------------------------------------------------------------- overflow

export type ScrollBox = { scrollWidth: number; clientWidth: number; scrollLeft: number };

export type TabStripOverflow = {
  hasOverflow: boolean;
  canScrollStart: boolean;
  canScrollEnd: boolean;
};

export const NO_TAB_STRIP_OVERFLOW: TabStripOverflow = {
  hasOverflow: false,
  canScrollStart: false,
  canScrollEnd: false,
};

export function tabStripOverflow(box: ScrollBox): TabStripOverflow {
  const maxScrollLeft = Math.max(0, box.scrollWidth - box.clientWidth);
  const hasOverflow = maxScrollLeft > OVERFLOW_EPSILON_PX;
  return {
    hasOverflow,
    canScrollStart: hasOverflow && box.scrollLeft > OVERFLOW_EPSILON_PX,
    canScrollEnd: hasOverflow && box.scrollLeft < maxScrollLeft - OVERFLOW_EPSILON_PX,
  };
}

export function sameTabStripOverflow(left: TabStripOverflow, right: TabStripOverflow) {
  return (
    left.hasOverflow === right.hasOverflow &&
    left.canScrollStart === right.canScrollStart &&
    left.canScrollEnd === right.canScrollEnd
  );
}

/** The fade mask classes: an edge with hidden tabs fades out. */
export function tabStripFadeClasses(overflow: TabStripOverflow) {
  return [
    overflow.canScrollStart ? "workspace-tab-scroll--fade-start" : "",
    overflow.canScrollEnd ? "workspace-tab-scroll--fade-end" : "",
  ]
    .filter(Boolean)
    .join(" ");
}

export function tabStripScrollStep(clientWidth: number) {
  return Math.max(TAB_STRIP_MIN_SCROLL_STEP_PX, clientWidth * TAB_STRIP_SCROLL_FRACTION);
}

/**
 * The scroll position that shows a tab with the least movement, or null when
 * the tab shows already. `left` and `right` are the tab edges in the
 * coordinates of the scroll content.
 */
export function revealScrollLeft(
  box: Pick<ScrollBox, "clientWidth" | "scrollLeft">,
  tab: { left: number; right: number },
): number | null {
  if (tab.left < box.scrollLeft) return Math.max(0, tab.left);
  if (tab.right > box.scrollLeft + box.clientWidth) {
    return Math.min(tab.left, tab.right - box.clientWidth);
  }
  return null;
}

// ---------------------------------------------------------------- hover card

/** The delay before a hover card opens: none while the strip is warm. */
export function hoverCardDelay(lastClosedAt: number | null, now: number) {
  return lastClosedAt !== null && now - lastClosedAt < TAB_HOVER_SKIP_DELAY_MS
    ? 0
    : TAB_HOVER_DELAY_MS;
}

/** The card sits below the tab, aligned to its left edge, and stays in the
 *  window with an 8 px margin. */
export function hoverCardPosition(
  tab: { left: number; bottom: number },
  card: { width: number },
  viewportWidth: number,
) {
  const margin = 8;
  const left = Math.max(margin, Math.min(tab.left, viewportWidth - card.width - margin));
  return { left, top: tab.bottom };
}
