import type { ConversationSummary } from "./types";
import {
  currentConversationOf,
  visibleTabKey,
  type TabFacts,
  type TabKey,
  type TabNavigation,
  type TabsState,
} from "./workspaceTabs.ts";

// App navigation (ADR 0010): what the main area shows. One App function,
// `navigate()`, writes it, and the current conversation is derived from it.
// So the App keeps no second copy that can drift.
//
// Kinds:
// - `legacy`: the workspaceTabs flag is off. The main area shows one
//   conversation, or the new-task composer when the id is null.
// - `workspace`: the strip of one workspace and its active tab, or the
//   draft tab while the user composes a new task.
// - `loose`: a conversation without a strip (no project, or archived).

export type Navigation =
  | { kind: "legacy"; conversationId: string | null }
  | TabNavigation;

export const INITIAL_NAVIGATION: Navigation = { kind: "legacy", conversationId: null };

export const NAVIGATION_STORAGE_KEY = "halv.navigation.v1";

export function legacyNavigation(conversationId: string | null): Navigation {
  return { kind: "legacy", conversationId };
}

export function workspaceNavigation(
  workspaceId: string,
  composing = false,
  draftConversationId: string | null = null,
): Navigation {
  return composing && draftConversationId
    ? { kind: "workspace", workspaceId, composing, draftConversationId }
    : { kind: "workspace", workspaceId, composing };
}

export function looseNavigation(conversationId: string | null): Navigation {
  return { kind: "loose", conversationId };
}

/** The tab store view that resolves the active tab of a strip. */
export type NavigationTabs = { tabs: TabsState; facts: TabFacts };

/** The current conversation. Null: the new-task composer shows. Without
 *  the tab view, a workspace shows no conversation. */
export function navigationConversationId(
  navigation: Navigation,
  view?: NavigationTabs,
): string | null {
  if (navigation.kind === "legacy") return navigation.conversationId;
  if (navigation.kind === "workspace" && !navigation.composing && !view) return null;
  return currentConversationOf(
    navigation,
    view?.tabs ?? { rows: {}, activeByWorkspace: {} },
    view?.facts ?? { complete: false, conversations: new Map(), workspaces: new Map() },
  );
}

/** The tab that the main area shows. Null while composing, in the loose
 *  view, and with the flag off. */
export function navigationTabKey(navigation: Navigation, view: NavigationTabs): TabKey | null {
  return navigation.kind === "legacy" ? null : visibleTabKey(navigation, view.tabs, view.facts);
}

export function navigationWorkspaceId(navigation: Navigation) {
  return navigation.kind === "workspace" ? navigation.workspaceId : null;
}

export function sameNavigation(left: Navigation, right: Navigation) {
  if (left.kind !== right.kind) return false;
  if (left.kind === "workspace" && right.kind === "workspace") {
    return (
      left.workspaceId === right.workspaceId &&
      left.composing === right.composing &&
      (left.draftConversationId ?? null) === (right.draftConversationId ?? null)
    );
  }
  return (
    (left as { conversationId: string | null }).conversationId ===
    (right as { conversationId: string | null }).conversationId
  );
}

type NavigationStorage = Pick<Storage, "getItem" | "setItem">;

/** The workspace of the last session. Only the workspace kind is stored:
 *  its strip knows its own active tab. */
export function readStoredNavigation(storage: NavigationStorage | null): Navigation | null {
  if (!storage) return null;
  try {
    const parsed = JSON.parse(storage.getItem(NAVIGATION_STORAGE_KEY) ?? "null") as {
      version?: unknown;
      workspaceId?: unknown;
    } | null;
    if (!parsed || parsed.version !== 1) return null;
    return typeof parsed.workspaceId === "string" && parsed.workspaceId.trim()
      ? workspaceNavigation(parsed.workspaceId)
      : null;
  } catch {
    return null;
  }
}

export function writeStoredNavigation(
  navigation: Navigation,
  storage: NavigationStorage | null,
) {
  if (!storage || navigation.kind !== "workspace") return false;
  try {
    storage.setItem(
      NAVIGATION_STORAGE_KEY,
      JSON.stringify({ version: 1, workspaceId: navigation.workspaceId }),
    );
    return true;
  } catch {
    return false;
  }
}

/** The options of `openConversation`, the one entry point that shows a
 *  conversation. */
export type OpenConversationOptions = {
  /** A conversation that the caller just created. The conversation list can
   *  lag behind it, so the tab store learns it from here. */
  summary?: ConversationSummary;
  /** False: show the conversation now. The caller streams or loads its
   *  messages itself. */
  load?: boolean;
  /** A navigation that the caller started already (fork). */
  navigationGeneration?: number;
  /** False: add the tab without showing it (a draft row that arrives after
   *  the user left the draft). */
  activate?: boolean;
};
