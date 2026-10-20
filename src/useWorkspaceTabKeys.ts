import { useEffect, useRef } from "react";
import {
  shortcutDigitForEvent,
  shortcutForAction,
  shortcutMatchesEvent,
  type KeyboardShortcutOverrides,
} from "./shortcuts.ts";

// The tab chords of the workspace strip (ADR 0010). One window listener in
// the capture phase owns them, so xterm never receives them as PTY input.
// The listener ignores keys while a dialog, a menu, a rename input or an
// IME composition is active. The pinned cycle uses the same pattern.
//
// The macOS app menu has no Cmd+W item (ADR 0010), so the page gets Cmd+W.
// On macOS the listener owns it in every state: with nothing to close it
// hides the window, as the menu item did before. Elsewhere the close chord
// works with shell tabs only.

export type WorkspaceTabKeyActions = {
  selectByNumber: (digit: number) => void;
  cycle: (offset: -1 | 1) => void;
  newAgentTab: () => void;
  newShellTab: () => void;
  closePaneOrTab: () => void;
  reopenClosedTab: () => void;
  renameTab: () => void;
  cycleWorkspace: (direction: "up" | "down") => void;
};

export function workspaceTabKeyTarget(target: EventTarget | null) {
  if (typeof Element === "undefined" || !(target instanceof Element)) return true;
  return !target.closest(
    '[role="dialog"], [role="menu"], [data-tab-rename-input="true"], .workspace-tab-menu',
  );
}

export type WorkspaceTabKeyAction =
  | { type: "select"; digit: number }
  | { type: "cycle"; offset: -1 | 1 }
  | { type: "new-agent" }
  | { type: "new-shell" }
  | { type: "close" }
  | { type: "reopen" }
  | { type: "rename" }
  | { type: "workspace"; direction: "up" | "down" };

/** The action of one key event, or null. Pure for tests. */
export function workspaceTabKeyAction(
  event: Pick<KeyboardEvent, "key" | "code" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">,
  overrides: KeyboardShortcutOverrides,
  platform?: string,
): WorkspaceTabKeyAction | null {
  const digit = shortcutDigitForEvent(
    event,
    shortcutForAction("workspace.select_tab_by_number", overrides, platform),
    platform,
  );
  if (digit !== null) return { type: "select", digit };
  const matches = (id: string) =>
    shortcutMatchesEvent(event, shortcutForAction(id, overrides, platform), platform);
  if (matches("workspace.previous_terminal_tab")) return { type: "cycle", offset: -1 };
  if (matches("workspace.next_terminal_tab")) return { type: "cycle", offset: 1 };
  if (matches("workspace.new_agent_tab")) return { type: "new-agent" };
  if (matches("workspace.new_shell_tab")) return { type: "new-shell" };
  if (matches("workspace.close_pane_or_tab")) return { type: "close" };
  if (matches("workspace.reopen_closed_tab")) return { type: "reopen" };
  if (matches("workspace.rename_tab")) return { type: "rename" };
  if (matches("workspace.previous_workspace")) return { type: "workspace", direction: "up" };
  if (matches("workspace.next_workspace")) return { type: "workspace", direction: "down" };
  return null;
}

/** Where the Cmd+W chain starts: the focused pane, or no pane. */
export type CloseChainFocus =
  | { kind: "tab-pane"; tabKey: string; sessionId: string; paneCount: number }
  | { kind: "bottom-pane"; scope: string; sessionId: string; paneCount: number }
  | { kind: "elsewhere" };

export type CloseChainStep =
  | { type: "close-pane"; scope: "tab"; tabKey: string; sessionId: string | null }
  | { type: "close-pane"; scope: "bottom"; panelScope: string; sessionId: string; last: boolean }
  | { type: "close-tab"; tabKey: string }
  | { type: "leave-draft" }
  | { type: "hide-window" };

/**
 * One Cmd+W press (ADR 0010, section 9.4). The caller drops the key while a
 * dialog, a menu, the rename input or an IME composition is active.
 * - Without shell tabs, Cmd+W hides the window, as before.
 * - A focused pane with siblings closes. The only pane of a tab closes the
 *   tab. A pane of the bottom panel closes; the last one hides the panel.
 * - Without a focused pane, a terminal tab with several panes closes its
 *   active pane, and any other tab closes. The draft tab leaves composing.
 * - With no pane and no tab to close, the window hides.
 */
export function closeChainStep(input: {
  shellTabs: boolean;
  focus: CloseChainFocus;
  /** The visible tab: null for the draft tab, the loose view and an empty
   *  strip. `paneCount` counts the panes of its mounted terminal panel. */
  visibleTab: { key: string; paneCount: number } | null;
  /** The draft tab shows next to other tabs that it can return to. */
  draftWithTabs: boolean;
}): CloseChainStep {
  if (!input.shellTabs) return { type: "hide-window" };
  const { focus } = input;
  if (focus.kind === "tab-pane") {
    return focus.paneCount > 1
      ? { type: "close-pane", scope: "tab", tabKey: focus.tabKey, sessionId: focus.sessionId }
      : { type: "close-tab", tabKey: focus.tabKey };
  }
  if (focus.kind === "bottom-pane") {
    return {
      type: "close-pane",
      scope: "bottom",
      panelScope: focus.scope,
      sessionId: focus.sessionId,
      last: focus.paneCount <= 1,
    };
  }
  if (input.visibleTab) {
    return input.visibleTab.paneCount > 1
      ? { type: "close-pane", scope: "tab", tabKey: input.visibleTab.key, sessionId: null }
      : { type: "close-tab", tabKey: input.visibleTab.key };
  }
  if (input.draftWithTabs) return { type: "leave-draft" };
  return { type: "hide-window" };
}

export function useWorkspaceTabKeys(options: {
  /** The strip shows: the tab chords route to it. */
  enabled: boolean;
  /** Shell tabs are on: Cmd+T opens a shell tab. */
  shellTabs: boolean;
  /** The close chord runs the Cmd+W chain. */
  closeChain: boolean;
  /** Workspace tabs are on: the workspace chords work also in the loose
   *  view, where no strip shows. */
  workspaces: boolean;
  blocked: boolean;
  overrides: KeyboardShortcutOverrides;
  actions: WorkspaceTabKeyActions;
}) {
  const actionsRef = useRef(options.actions);
  actionsRef.current = options.actions;
  const { enabled, shellTabs, closeChain, workspaces, blocked, overrides } = options;
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.defaultPrevented || event.isComposing || blocked) return;
      if (!workspaceTabKeyTarget(event.target)) return;
      const action = workspaceTabKeyAction(event, overrides);
      if (!action) return;
      // The close chord and the workspace chords work without the strip.
      // The other chords need it.
      if (
        action.type === "close"
          ? !closeChain
          : action.type === "workspace"
            ? !workspaces
            : !enabled
      ) {
        return;
      }
      if (action.type === "new-shell" && !shellTabs) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      if (action.type === "select") actionsRef.current.selectByNumber(action.digit);
      else if (action.type === "cycle") actionsRef.current.cycle(action.offset);
      else if (action.type === "new-agent") actionsRef.current.newAgentTab();
      else if (action.type === "new-shell") actionsRef.current.newShellTab();
      else if (action.type === "reopen") actionsRef.current.reopenClosedTab();
      else if (action.type === "rename") actionsRef.current.renameTab();
      else if (action.type === "workspace") actionsRef.current.cycleWorkspace(action.direction);
      else actionsRef.current.closePaneOrTab();
    }
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [blocked, closeChain, enabled, overrides, shellTabs, workspaces]);
}
