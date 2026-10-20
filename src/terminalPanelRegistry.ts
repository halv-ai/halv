// The mount registry of terminal panels (ADR 0010, rule P4). A panel scope is
// the TerminalPanel storage key, for example
// `halv.terminal-layout.v1:conversation:<id>`.
//
// Rules:
// - At most one mounted panel holds a scope. Only the holder restores,
//   refreshes, persists and resumes the panes of the scope.
// - A panel claims its scope when it mounts and releases it when it
//   unmounts. A panel that cannot claim waits. It gets the scope when the
//   holder releases it.
// - Other code reaches the holder through `request(scope)`: close every
//   pane, focus the active pane, open a shell, or read the panes for the
//   close prompt.

import type { ClosePaneFact } from "./workspaceTabs.ts";

/** One pane of a panel, for the close prompt of its tab. The session id
 *  lets the prompt ask the busy probe about a shell. */
export type TerminalPanelPaneFact = ClosePaneFact & { sessionId: string };

export type TerminalPanelHandle = {
  /** Stops every process of the panel. With `park`, each agent pane with a
   *  CLI session id stays saved, so the conversation can resume it later. */
  closeAll(options: { park: boolean }): Promise<void>;
  /** Focuses the active pane. False when the panel has no live pane. */
  focusActivePane(): boolean;
  openShell(): void;
  /** The panes of the panel, for the one close prompt of a tab. */
  panes(): TerminalPanelPaneFact[];
  /** Closes one pane with its own prompt for a running process: the pane
   *  of `sessionId`, else the active pane. The Cmd+W chain uses it. */
  closePane(sessionId: string | null): Promise<void>;
  /** Splits the active pane with a copy of its terminal, as Cmd+D does.
   *  False when the panel has no active pane yet. The tab menu uses it. */
  splitActivePane(direction: "vertical" | "horizontal"): boolean;
};

type Holder = { token: symbol; handle: TerminalPanelHandle | null };

export function createTerminalPanelRegistry() {
  const holders = new Map<string, Holder>();
  const listeners = new Set<() => void>();

  const notify = () => {
    for (const listener of [...listeners]) listener();
  };

  return {
    /** True when `token` holds `scope` after the call. */
    claim(scope: string, token: symbol) {
      const holder = holders.get(scope);
      if (holder && holder.token !== token) return false;
      if (!holder) {
        holders.set(scope, { token, handle: null });
        notify();
      }
      return true;
    },
    release(scope: string, token: symbol) {
      if (holders.get(scope)?.token !== token) return;
      holders.delete(scope);
      notify();
    },
    holds(scope: string, token: symbol) {
      return holders.get(scope)?.token === token;
    },
    /** The holder publishes its handle. Other tokens cannot. */
    setHandle(scope: string, token: symbol, handle: TerminalPanelHandle | null) {
      const holder = holders.get(scope);
      if (holder?.token === token) holder.handle = handle;
    },
    /** The handle of the panel that holds the scope, or null when no panel
     *  is mounted for it. */
    request(scope: string): TerminalPanelHandle | null {
      return holders.get(scope)?.handle ?? null;
    },
    mounted(scope: string) {
      return holders.has(scope);
    },
    /** Runs on every claim and release. A waiting panel tries again. */
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}

export type TerminalPanelRegistry = ReturnType<typeof createTerminalPanelRegistry>;

export const terminalPanelRegistry = createTerminalPanelRegistry();
