/*
 * The binary split model adapts Orca terminal layout state.
 * Copyright (c) 2026 Lovecast Inc. Orca uses the MIT License.
 * See THIRD_PARTY_NOTICES.md for the license text.
 */

export type TerminalPaneSplitDirection = "vertical" | "horizontal";

export type TerminalPaneLayoutNode =
  // A leaf ID is a stable pane ID. Terminal session IDs can change.
  | { type: "leaf"; leafId: string }
  | {
      type: "split";
      direction: TerminalPaneSplitDirection;
      first: TerminalPaneLayoutNode;
      second: TerminalPaneLayoutNode;
      ratio?: number;
    };

export type TerminalSplitPath = Array<"first" | "second">;

export type TerminalTabLayout = {
  tabId: string;
  root: TerminalPaneLayoutNode;
};

export type TerminalPanelLayoutSnapshot = {
  version: 1;
  activeTabId: string | null;
  activeLeafId: string | null;
  tabs: TerminalTabLayout[];
};

export type TerminalFitMetrics = {
  paneWidth: number;
  paneHeight: number;
  paneHeaderHeight: number;
  paddingTop: number;
  paddingRight: number;
  paddingBottom: number;
  paddingLeft: number;
  borderTop: number;
  borderRight: number;
  borderBottom: number;
  borderLeft: number;
  scrollbarWidth: number;
  cellWidth: number;
  cellHeight: number;
  // True when the cell size does not depend on the grid (see
  // TerminalCellSize.exact). The fit then counts the cells that xterm can
  // draw inside the pane, because xterm rounds the screen to whole pixels.
  exactCellSize?: boolean;
};

export type TerminalFitDimensions = {
  cols: number;
  rows: number;
};

export type TerminalCellSize = {
  width: number;
  height: number;
  // True when the size comes from xterm's device-pixel cell. Then it does
  // not depend on the grid, and xterm draws n cells in
  // Math.round(n * size) CSS pixels.
  exact?: boolean;
};

export type TerminalCellSizeFallback = {
  screenWidth: number;
  screenHeight: number;
  cols: number;
  rows: number;
};

const MIN_RATIO = 0.05;
const MAX_RATIO = 0.95;

export function calculateTerminalFit(
  metrics: TerminalFitMetrics,
): TerminalFitDimensions | null {
  if (
    !positiveFinite(metrics.paneWidth) ||
    !positiveFinite(metrics.paneHeight) ||
    !positiveFinite(metrics.cellWidth) ||
    !positiveFinite(metrics.cellHeight)
  ) {
    return null;
  }
  const availableWidth =
    metrics.paneWidth -
    nonNegativeFinite(metrics.paddingLeft) -
    nonNegativeFinite(metrics.paddingRight) -
    nonNegativeFinite(metrics.borderLeft) -
    nonNegativeFinite(metrics.borderRight) -
    nonNegativeFinite(metrics.scrollbarWidth);
  const availableHeight =
    metrics.paneHeight -
    nonNegativeFinite(metrics.paneHeaderHeight) -
    nonNegativeFinite(metrics.paddingTop) -
    nonNegativeFinite(metrics.paddingBottom) -
    nonNegativeFinite(metrics.borderTop) -
    nonNegativeFinite(metrics.borderBottom);
  const exact = metrics.exactCellSize;
  return {
    cols: fitCellCount(availableWidth, metrics.cellWidth, 2, exact),
    rows: fitCellCount(availableHeight, metrics.cellHeight, 1, exact),
  };
}

// Xterm draws `count` exact cells in Math.round(count * cell) CSS pixels, so
// the cells that fit can be one more or one less than the plain division.
function fitCellCount(
  available: number,
  cell: number,
  minimum: number,
  exact = false,
) {
  let count = Math.floor(available / cell);
  if (exact) {
    if (Math.round((count + 1) * cell) <= available) count += 1;
    else if (Math.round(count * cell) > available) count -= 1;
  }
  return Math.max(minimum, count);
}

export function resolveTerminalCellSize(
  renderDimensions: unknown,
  fallback: TerminalCellSizeFallback,
  devicePixelRatio?: number,
): TerminalCellSize | null {
  if (isRecord(renderDimensions)) {
    // Xterm's DOM renderer divides a rounded screen width by the grid, so
    // css.cell changes with the grid. At some pane widths a fit with it
    // flips between n and n + 1 columns forever. The device cell does not
    // depend on the grid.
    const device = renderDimensions.device;
    const deviceCell = isRecord(device) ? device.cell : undefined;
    if (
      positiveFinite(devicePixelRatio) &&
      isRecord(deviceCell) &&
      positiveFinite(deviceCell.width) &&
      positiveFinite(deviceCell.height)
    ) {
      return {
        width: deviceCell.width / devicePixelRatio,
        height: deviceCell.height / devicePixelRatio,
        exact: true,
      };
    }
    const css = renderDimensions.css;
    if (isRecord(css)) {
      const cell = css.cell;
      if (
        isRecord(cell) &&
        positiveFinite(cell.width) &&
        positiveFinite(cell.height)
      ) {
        return { width: cell.width, height: cell.height };
      }
    }
  }

  const width = fallback.screenWidth / fallback.cols;
  const height = fallback.screenHeight / fallback.rows;
  return positiveFinite(width) && positiveFinite(height) ? { width, height } : null;
}

export function terminalLeaf(leafId: string): TerminalPaneLayoutNode {
  assertLeafId(leafId);
  return { type: "leaf", leafId };
}

export function collectTerminalLeafIds(
  node: TerminalPaneLayoutNode | null,
  leafIds: string[] = [],
): string[] {
  if (!node) return leafIds;
  if (node.type === "leaf") {
    leafIds.push(node.leafId);
    return leafIds;
  }
  collectTerminalLeafIds(node.first, leafIds);
  collectTerminalLeafIds(node.second, leafIds);
  return leafIds;
}

export function terminalLayoutHasLeaf(
  node: TerminalPaneLayoutNode | null,
  leafId: string,
): boolean {
  return collectTerminalLeafIds(node).includes(leafId);
}

export function splitTerminalLeaf(
  node: TerminalPaneLayoutNode,
  leafId: string,
  newLeafId: string,
  direction: TerminalPaneSplitDirection,
): TerminalPaneLayoutNode {
  assertLeafId(leafId);
  assertLeafId(newLeafId);
  if (leafId === newLeafId || terminalLayoutHasLeaf(node, newLeafId)) {
    throw new Error(`terminal leaf \`${newLeafId}\` already exists`);
  }
  const result = replaceLeaf(node, leafId, (leaf) => ({
    type: "split",
    direction,
    first: leaf,
    second: terminalLeaf(newLeafId),
    ratio: 0.5,
  }));
  if (!result.replaced) {
    throw new Error(`terminal leaf \`${leafId}\` was not found`);
  }
  return result.node;
}

/// Swaps the two children of the leaf's nearest parent split. The ratio
/// flips with the children, so each pane keeps its size.
export function swapTerminalSiblings(
  node: TerminalPaneLayoutNode,
  leafId: string,
): TerminalPaneLayoutNode {
  if (node.type !== "split") return node;
  const directChild =
    (node.first.type === "leaf" && node.first.leafId === leafId) ||
    (node.second.type === "leaf" && node.second.leafId === leafId);
  if (directChild) {
    return {
      ...node,
      first: node.second,
      second: node.first,
      ratio: node.ratio === undefined ? undefined : 1 - node.ratio,
    };
  }
  if (terminalLayoutHasLeaf(node.first, leafId)) {
    return { ...node, first: swapTerminalSiblings(node.first, leafId) };
  }
  if (terminalLayoutHasLeaf(node.second, leafId)) {
    return { ...node, second: swapTerminalSiblings(node.second, leafId) };
  }
  return node;
}

/// Toggles the split direction of the leaf's nearest parent split:
/// side-by-side becomes stacked and back.
export function rotateTerminalSplitAt(
  node: TerminalPaneLayoutNode,
  leafId: string,
): TerminalPaneLayoutNode {
  if (node.type !== "split") return node;
  const directChild =
    (node.first.type === "leaf" && node.first.leafId === leafId) ||
    (node.second.type === "leaf" && node.second.leafId === leafId);
  if (directChild) {
    return {
      ...node,
      direction: node.direction === "vertical" ? "horizontal" : "vertical",
    };
  }
  if (terminalLayoutHasLeaf(node.first, leafId)) {
    return { ...node, first: rotateTerminalSplitAt(node.first, leafId) };
  }
  if (terminalLayoutHasLeaf(node.second, leafId)) {
    return { ...node, second: rotateTerminalSplitAt(node.second, leafId) };
  }
  return node;
}

export function closeTerminalLeaf(
  node: TerminalPaneLayoutNode,
  leafId: string,
): TerminalPaneLayoutNode | null {
  assertLeafId(leafId);
  if (node.type === "leaf") {
    return node.leafId === leafId ? null : node;
  }
  const first = closeTerminalLeaf(node.first, leafId);
  const second = closeTerminalLeaf(node.second, leafId);
  if (!first) return second;
  if (!second) return first;
  if (first === node.first && second === node.second) return node;
  return { ...node, first, second };
}

export function setTerminalSplitRatio(
  node: TerminalPaneLayoutNode,
  splitPath: TerminalSplitPath,
  ratio: number,
): TerminalPaneLayoutNode {
  if (!Number.isFinite(ratio)) {
    throw new Error("terminal split ratio must be finite");
  }
  if (splitPath.length === 0) {
    if (node.type !== "split") {
      throw new Error("terminal split path points to a leaf");
    }
    return { ...node, ratio: clampRatio(ratio) };
  }
  if (node.type !== "split") {
    throw new Error("terminal split path is invalid");
  }
  const [branch, ...rest] = splitPath;
  return branch === "first"
    ? { ...node, first: setTerminalSplitRatio(node.first, rest, ratio) }
    : { ...node, second: setTerminalSplitRatio(node.second, rest, ratio) };
}

export function pruneTerminalLayout(
  node: TerminalPaneLayoutNode,
  validLeafIds: ReadonlySet<string>,
): TerminalPaneLayoutNode | null {
  if (node.type === "leaf") {
    return validLeafIds.has(node.leafId) ? node : null;
  }
  const first = pruneTerminalLayout(node.first, validLeafIds);
  const second = pruneTerminalLayout(node.second, validLeafIds);
  if (!first) return second;
  if (!second) return first;
  if (first === node.first && second === node.second) return node;
  return { ...node, first, second };
}

export function serializeTerminalLayout(
  node: TerminalPaneLayoutNode,
): string {
  return JSON.stringify({ version: 1, root: node });
}

export function deserializeTerminalLayout(
  serialized: string,
): TerminalPaneLayoutNode | null {
  try {
    const value: unknown = JSON.parse(serialized);
    if (!isRecord(value) || value.version !== 1) return null;
    return parseNode(value.root, new Set());
  } catch {
    return null;
  }
}

export function serializeTerminalPanelLayout(
  snapshot: TerminalPanelLayoutSnapshot,
): string {
  return JSON.stringify(snapshot);
}

export function deserializeTerminalPanelLayout(
  serialized: string | null,
): TerminalPanelLayoutSnapshot | null {
  if (!serialized) return null;
  try {
    const value: unknown = JSON.parse(serialized);
    if (!isRecord(value) || value.version !== 1 || !Array.isArray(value.tabs)) {
      return null;
    }
    const leafIds = new Set<string>();
    const tabs: TerminalTabLayout[] = [];
    for (const item of value.tabs) {
      if (!isRecord(item) || typeof item.tabId !== "string" || !item.tabId) {
        return null;
      }
      const root = parseNode(item.root, leafIds);
      if (!root) return null;
      tabs.push({ tabId: item.tabId, root });
    }
    return {
      version: 1,
      activeTabId:
        typeof value.activeTabId === "string" ? value.activeTabId : null,
      activeLeafId:
        typeof value.activeLeafId === "string" ? value.activeLeafId : null,
      tabs,
    };
  } catch {
    return null;
  }
}

function replaceLeafId(
  node: TerminalPaneLayoutNode,
  fromId: string,
  toId: string,
): TerminalPaneLayoutNode {
  if (node.type === "leaf") {
    return node.leafId === fromId ? { ...node, leafId: toId } : node;
  }
  return {
    ...node,
    first: replaceLeafId(node.first, fromId, toId),
    second: replaceLeafId(node.second, fromId, toId),
  };
}

export function reconcileTerminalPanelLayout(
  snapshot: TerminalPanelLayoutSnapshot | null,
  leafIds: readonly string[],
  preferredLeafId: string | null = null,
  // newLeafId -> the saved leaf it replaces (same CLI session). Appending a
  // replacement as its own tab is how duplicate tabs were born.
  leafAliases: ReadonlyMap<string, string> | null = null,
  // When provided, only these leaf ids may APPEND as new tabs. Every id in
  // leafIds stays valid in existing tabs - append eligibility must never
  // prune a leaf, or tabs flicker closed and open again.
  appendableLeafIds: ReadonlySet<string> | null = null,
): TerminalPanelLayoutSnapshot {
  const validLeafIds = new Set(leafIds);
  // Runtime edits can leave the same leaf id in two places; keep only the
  // first occurrence so one session never renders as several tabs.
  const seenLeafIds = new Set<string>();
  const tabs = (snapshot?.tabs ?? []).flatMap((tab) => {
    const pruned = pruneTerminalLayout(tab.root, validLeafIds);
    const root = pruned ? dropSeenLeaves(pruned, seenLeafIds) : null;
    return root ? [{ ...tab, root }] : [];
  });
  // Tabs are retired: a conversation is one terminal surface. Legacy
  // multi-tab snapshots merge into a single split tree, preserving order.
  if (tabs.length > 1) {
    const mergedRoot = tabs
      .slice(1)
      .reduce<TerminalPaneLayoutNode>(
        (root, tab) => ({
          type: "split",
          direction: "vertical",
          first: root,
          second: tab.root,
        }),
        tabs[0].root,
      );
    tabs.splice(0, tabs.length, { ...tabs[0], root: mergedRoot });
  }
  const assignedLeafIds = new Set(
    tabs.flatMap((tab) => collectTerminalLeafIds(tab.root)),
  );
  for (const leafId of leafIds) {
    if (assignedLeafIds.has(leafId)) continue;
    const aliasTarget = leafAliases?.get(leafId);
    if (aliasTarget && assignedLeafIds.has(aliasTarget)) {
      for (let index = 0; index < tabs.length; index += 1) {
        tabs[index] = {
          ...tabs[index],
          root: replaceLeafId(tabs[index].root, aliasTarget, leafId),
        };
      }
      // The target stays in the assigned set: it IS this replacement and
      // never comes back as its own tab.
      assignedLeafIds.add(leafId);
      continue;
    }
    if (appendableLeafIds && !appendableLeafIds.has(leafId)) continue;
    if (tabs.length === 0) {
      tabs.push({ tabId: leafId, root: terminalLeaf(leafId) });
    } else {
      // New sessions split into the surface instead of opening a tab.
      tabs[0] = {
        ...tabs[0],
        root: {
          type: "split",
          direction: "vertical",
          first: tabs[0].root,
          second: terminalLeaf(leafId),
        },
      };
    }
    assignedLeafIds.add(leafId);
  }
  const preferredTab = preferredLeafId
    ? tabs.find((tab) => terminalLayoutHasLeaf(tab.root, preferredLeafId))
    : null;
  const activeTab =
    preferredTab ??
    tabs.find((tab) => tab.tabId === snapshot?.activeTabId) ??
    tabs[0] ??
    null;
  const previousActiveLeafId = snapshot?.activeLeafId ?? null;
  const activeLeafId =
    preferredLeafId &&
    activeTab &&
    terminalLayoutHasLeaf(activeTab.root, preferredLeafId)
      ? preferredLeafId
      : previousActiveLeafId &&
          activeTab &&
          terminalLayoutHasLeaf(activeTab.root, previousActiveLeafId)
        ? previousActiveLeafId
        : collectTerminalLeafIds(activeTab?.root ?? null)[0] ?? null;
  return {
    version: 1,
    activeTabId: activeTab?.tabId ?? null,
    activeLeafId,
    tabs,
  };
}

function dropSeenLeaves(
  node: TerminalPaneLayoutNode,
  seenLeafIds: Set<string>,
): TerminalPaneLayoutNode | null {
  if (node.type === "leaf") {
    if (seenLeafIds.has(node.leafId)) return null;
    seenLeafIds.add(node.leafId);
    return node;
  }
  const first = dropSeenLeaves(node.first, seenLeafIds);
  const second = dropSeenLeaves(node.second, seenLeafIds);
  if (!first) return second;
  if (!second) return first;
  if (first === node.first && second === node.second) return node;
  return { ...node, first, second };
}

function replaceLeaf(
  node: TerminalPaneLayoutNode,
  leafId: string,
  replacement: (leaf: Extract<TerminalPaneLayoutNode, { type: "leaf" }>) =>
    TerminalPaneLayoutNode,
): { node: TerminalPaneLayoutNode; replaced: boolean } {
  if (node.type === "leaf") {
    return node.leafId === leafId
      ? { node: replacement(node), replaced: true }
      : { node, replaced: false };
  }
  const first = replaceLeaf(node.first, leafId, replacement);
  if (first.replaced) {
    return { node: { ...node, first: first.node }, replaced: true };
  }
  const second = replaceLeaf(node.second, leafId, replacement);
  return second.replaced
    ? { node: { ...node, second: second.node }, replaced: true }
    : { node, replaced: false };
}

function parseNode(
  value: unknown,
  leafIds: Set<string>,
): TerminalPaneLayoutNode | null {
  if (!isRecord(value)) return null;
  if (value.type === "leaf") {
    if (
      typeof value.leafId !== "string" ||
      !value.leafId ||
      leafIds.has(value.leafId)
    ) {
      return null;
    }
    leafIds.add(value.leafId);
    return terminalLeaf(value.leafId);
  }
  if (
    value.type !== "split" ||
    (value.direction !== "vertical" && value.direction !== "horizontal")
  ) {
    return null;
  }
  const first = parseNode(value.first, leafIds);
  const second = parseNode(value.second, leafIds);
  if (!first || !second) return null;
  const ratio =
    typeof value.ratio === "number" && Number.isFinite(value.ratio)
      ? clampRatio(value.ratio)
      : undefined;
  return { type: "split", direction: value.direction, first, second, ratio };
}

function assertLeafId(leafId: string) {
  if (!leafId.trim()) throw new Error("terminal leaf ID cannot be empty");
}

function clampRatio(ratio: number) {
  return Math.min(MAX_RATIO, Math.max(MIN_RATIO, ratio));
}

function positiveFinite(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value > 0;
}

function nonNegativeFinite(value: number) {
  return Number.isFinite(value) ? Math.max(0, value) : 0;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
