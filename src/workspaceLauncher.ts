import {
  newAgentShortcut,
  newTerminalShortcut,
  readyAgentsInOrder,
  type MenuPlatform,
  type PlusMenuAgent,
} from "./workspaceTabs.ts";

// The launcher of an empty workspace (ADR 0020). In Pro mode, a workspace
// strip without a tab shows three kinds of choices instead of the "New
// task" composer: an agent, an empty terminal, or a text conversation.
// Each choice opens one tab in that workspace. This module decides when
// the launcher shows and which items it lists. The component draws them.

export type LauncherItem =
  | { kind: "agent"; agentKind: string; shortcut: string | null }
  | { kind: "terminal"; shortcut: string }
  | { kind: "text" };

/**
 * The items of the launcher. The agents are the ready agents in the order
 * of the "+" menu: the default agent first, with the Option+Cmd+T chord on
 * macOS. "Terminal" needs shell tabs. "Text conversation" is always last.
 */
export function launcherItems(input: {
  platform: MenuPlatform;
  agents: readonly PlusMenuAgent[];
  defaultAgent: string | null;
  shellTabs: boolean;
}): LauncherItem[] {
  const items: LauncherItem[] = readyAgentsInOrder(input.agents, input.defaultAgent).map(
    (agent) => ({
      kind: "agent",
      agentKind: agent.kind,
      shortcut: agent.kind === input.defaultAgent ? newAgentShortcut(input.platform) : null,
    }),
  );
  if (input.shellTabs) {
    items.push({ kind: "terminal", shortcut: newTerminalShortcut(input.platform) });
  }
  items.push({ kind: "text" });
  return items;
}

/** A stable key for each launcher item. */
export function launcherItemKey(item: LauncherItem) {
  return item.kind === "agent" ? `agent:${item.agentKind}` : item.kind;
}

/**
 * The launcher shows in Pro mode on a workspace strip with no tab, when the
 * user does not compose a new conversation. With Pro mode off, the
 * composer stays (ADR 0020).
 */
export function workspaceLauncherShown(input: {
  proMode: boolean;
  /** The tab keys of the visible strip. Null without a strip. */
  stripKeys: readonly string[] | null;
  composing: boolean;
}) {
  return input.proMode && input.stripKeys !== null && input.stripKeys.length === 0 && !input.composing;
}

/**
 * The focus target of an arrow key in the launcher list. The list wraps,
 * as the "+" menu does. Home and End jump to the ends.
 */
export function launcherFocusIndex(key: string, index: number, count: number): number | null {
  if (count === 0) return null;
  switch (key) {
    case "ArrowDown":
      return index < 0 ? 0 : (index + 1) % count;
    case "ArrowUp":
      return index < 0 ? count - 1 : (index - 1 + count) % count;
    case "Home":
      return 0;
    case "End":
      return count - 1;
    default:
      return null;
  }
}
