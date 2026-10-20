// The tab history of the workspace strip (ADR 0010, stage S7), as pure
// functions: the closed-tab stack of Shift+Cmd+T, the order of the Ctrl+Tab
// switcher, and the workspace cycle of Option+Cmd+Up and Down. The stack
// lives in memory only. A relaunch starts with an empty stack.
//
// Rules:
// - The stack keeps the 20 newest closes, newest first.
// - Shift+Cmd+T reopens the newest close of the visible workspace. An entry
//   that cannot reopen (its conversation is gone, archived or open again)
//   drops, and the next entry of that workspace is tried.
// - A conversation tab reopens the conversation. Its parked panes then
//   resume once (ADR 0010, "Close and park").
// - A shell tab reopens as a new shell tab in the same workspace, with the
//   same custom title, at the same place in the strip. Its shell starts in
//   the workspace root, where every shell of a shell tab starts.

import type { TabKey } from "./workspaceTabs.ts";

export const CLOSED_TAB_STACK_LIMIT = 20;

export type ClosedTabEntry =
  | {
      kind: "conversation";
      key: TabKey;
      conversationId: string;
      workspaceId: string;
      /** The index of the tab in its strip when it closed. */
      index: number;
    }
  | {
      kind: "shell";
      key: TabKey;
      workspaceId: string;
      /** The custom title of the closed tab, or null. */
      title: string | null;
      index: number;
    };

/** Adds a close to the top of the stack. A second close of the same tab
 *  replaces the first. */
export function pushClosedTab(
  stack: readonly ClosedTabEntry[],
  entry: ClosedTabEntry,
): ClosedTabEntry[] {
  return [entry, ...stack.filter((item) => item.key !== entry.key)].slice(
    0,
    CLOSED_TAB_STACK_LIMIT,
  );
}

/** True when the workspace has an entry that `canReopen` accepts. */
export function hasClosedTab(
  stack: readonly ClosedTabEntry[],
  workspaceId: string,
  canReopen: (entry: ClosedTabEntry) => boolean = () => true,
) {
  return stack.some((entry) => entry.workspaceId === workspaceId && canReopen(entry));
}

/** Takes the newest entry of the workspace that can reopen. The entries of
 *  that workspace before it drop. Other workspaces keep their entries. */
export function takeClosedTab(
  stack: readonly ClosedTabEntry[],
  workspaceId: string,
  canReopen: (entry: ClosedTabEntry) => boolean,
): { entry: ClosedTabEntry | null; stack: ClosedTabEntry[] } {
  const rest: ClosedTabEntry[] = [];
  let found: ClosedTabEntry | null = null;
  for (const entry of stack) {
    if (found || entry.workspaceId !== workspaceId) {
      rest.push(entry);
    } else if (canReopen(entry)) {
      found = entry;
    }
  }
  return { entry: found, stack: rest };
}

// ---------------------------------------------------------------- Ctrl+Tab

/**
 * The order of the Ctrl+Tab switcher (Orca's "most recent" mode): the
 * active tab first, then the other tabs by their last activation, newest
 * first. Tabs that the user never activated follow in strip order.
 */
export function recentTabOrder(
  tabs: ReadonlyArray<{ key: string; activatedAt: number | null }>,
  activeKey: string | null,
): string[] {
  const ranked = tabs
    .map((tab, index) => ({ ...tab, index }))
    .sort((left, right) => {
      if (left.key === activeKey) return -1;
      if (right.key === activeKey) return 1;
      const leftTime = left.activatedAt ?? Number.NEGATIVE_INFINITY;
      const rightTime = right.activatedAt ?? Number.NEGATIVE_INFINITY;
      return rightTime - leftTime || left.index - right.index;
    });
  return ranked.map((tab) => tab.key);
}

/** The next selection of the switcher. It wraps. With no selection, Tab
 *  starts at the top and Shift+Tab at the bottom. */
export function nextSwitcherIndex(count: number, current: number, direction: 1 | -1) {
  if (count <= 0) return -1;
  if (current < 0) return direction > 0 ? 0 : count - 1;
  return (current + direction + count) % count;
}

// ---------------------------------------------------------------- workspaces

/**
 * Option+Cmd+Up and Down: the previous or next workspace of the list that
 * the sidebar shows, with a wrap (Orca's worktree cycle). A workspace that
 * the list does not show enters from the end that the key points away from.
 */
export function cycledWorkspaceId(
  workspaceIds: readonly string[],
  activeWorkspaceId: string | null,
  direction: "up" | "down",
): string | null {
  if (workspaceIds.length === 0) return null;
  const index = activeWorkspaceId ? workspaceIds.indexOf(activeWorkspaceId) : -1;
  if (index < 0) {
    return (direction === "down" ? workspaceIds[0] : workspaceIds[workspaceIds.length - 1]) ?? null;
  }
  const step = direction === "down" ? 1 : -1;
  return workspaceIds[(index + step + workspaceIds.length) % workspaceIds.length] ?? null;
}
