import {
  createTerminalActivityTracker,
  type TerminalActivityStatus,
} from "./terminalActivity.ts";
import type { TerminalEvent } from "./types";
import type { AgentSessionEvent } from "./agentSessions.ts";
import { createTerminalPromptSubmissionTracker } from "./trialPrompt.ts";
import {
  agentPaneBusy,
  agentPaneViewsEqual,
  agentStatusIsAuthoritative,
  nextHeuristicPane,
  resolveAgentPaneView,
  startHeuristicPane,
  viewHeuristicPane,
  type AgentPaneView,
  type AgentStatusEntry,
  type HeuristicPane,
} from "./agentPaneStatus.ts";

export type TerminalActivitySnapshot = {
  /** The raw tracker status. The guided first task still reads it. */
  statuses: Readonly<Record<string, TerminalActivityStatus>>;
  /** The byte-volume fallback state of each tracked pane. */
  panes: Readonly<Record<string, HeuristicPane>>;
  /** The status that Rust reports for each agent pane. */
  agentStatuses: Readonly<Record<string, AgentStatusEntry>>;
  /** The pane that the user looks at in a focused window. */
  focusedSessionId: string | null;
};

/** Sends the view state of panes to the Rust status store. */
export type AgentStatusBridge = {
  markViewed(sessionId: string): void;
  setFocus(sessionId: string | null): void;
};

type PaneListener = (
  id: string,
  view: AgentPaneView | null,
  previous: AgentPaneView | null,
) => void;

const EMPTY_SNAPSHOT: TerminalActivitySnapshot = {
  statuses: {},
  panes: {},
  agentStatuses: {},
  focusedSessionId: null,
};

/** The display state of one agent pane, or null without any signal. */
export function agentPaneViewFor(
  snapshot: TerminalActivitySnapshot,
  sessionId: string,
): AgentPaneView | null {
  return resolveAgentPaneView(
    snapshot.agentStatuses[sessionId],
    snapshot.panes[sessionId],
  );
}

function without<T>(record: Readonly<Record<string, T>>, id: string) {
  if (!(id in record)) return record;
  const next = { ...record };
  delete next[id];
  return next;
}

// The PTY outlives its mounted pane. Keep activity and acknowledgement at
// session scope so navigation cannot reset either one.
export function createTerminalActivityStore(now: () => number = Date.now) {
  let snapshot: TerminalActivitySnapshot = EMPTY_SNAPSHOT;
  const trackers = new Map<
    string,
    ReturnType<typeof createTerminalActivityTracker>
  >();
  const inputTrackers = new Map<
    string,
    ReturnType<typeof createTerminalPromptSubmissionTracker>
  >();
  const sequences = new Map<string, number>();
  const listeners = new Set<() => void>();
  const statusListeners = new Set<
    (id: string, status: TerminalActivityStatus | null) => void
  >();
  const paneListeners = new Set<PaneListener>();
  let bridge: AgentStatusBridge | null = null;
  let visibleSessionId: string | null = null;
  let windowFocused = false;
  // A full-window surface, such as Dot or Settings, hides the panes.
  let paneCovered = false;

  const commit = (next: TerminalActivitySnapshot, ids: readonly string[]) => {
    const previous = snapshot;
    if (next === previous) return;
    snapshot = next;
    listeners.forEach((listener) => listener());
    for (const id of ids) {
      const before = agentPaneViewFor(previous, id);
      const after = agentPaneViewFor(next, id);
      if (agentPaneViewsEqual(before, after)) continue;
      paneListeners.forEach((listener) => listener(id, after, before));
    }
  };

  const withHeuristic = (
    base: TerminalActivitySnapshot,
    id: string,
    pane: HeuristicPane,
  ): TerminalActivitySnapshot =>
    base.panes[id] === pane
      ? base
      : { ...base, panes: { ...base.panes, [id]: pane } };

  const trackerForSession = (id: string) => {
    let tracker = trackers.get(id);
    if (!tracker) {
      tracker = createTerminalActivityTracker((status) => {
        const current = snapshot;
        let next = current;
        if ((current.statuses[id] ?? null) !== status) {
          const statuses = { ...current.statuses };
          if (status === null) delete statuses[id];
          else statuses[id] = status;
          next = { ...next, statuses };
        }
        const pane = current.panes[id];
        if (pane) {
          next = withHeuristic(
            next,
            id,
            nextHeuristicPane(pane, status, now(), current.focusedSessionId === id),
          );
        }
        if (next === current) return;
        commit(next, [id]);
        if ((current.statuses[id] ?? null) !== status) {
          statusListeners.forEach((listener) => listener(id, status));
        }
      });
      trackers.set(id, tracker);
      // The raw status keeps its old seed for the guided first task. The
      // pane itself starts idle: a new or adopted pane has no news.
      commit(
        {
          ...snapshot,
          statuses: { ...snapshot.statuses, [id]: "waiting_input" },
          panes: { ...snapshot.panes, [id]: startHeuristicPane(now()) },
        },
        [id],
      );
    }
    return tracker;
  };

  const markViewed = (id: string) => {
    trackers.get(id)?.noteViewed();
    let next = snapshot;
    const pane = next.panes[id];
    if (pane) next = withHeuristic(next, id, viewHeuristicPane(pane));
    const entry = next.agentStatuses[id];
    if (entry?.unread) {
      next = {
        ...next,
        agentStatuses: { ...next.agentStatuses, [id]: { ...entry, unread: false } },
      };
      bridge?.markViewed(id);
    }
    commit(next, [id]);
  };

  const updateFocus = () => {
    const focused = windowFocused && !paneCovered ? visibleSessionId : null;
    if (focused === snapshot.focusedSessionId) return;
    commit({ ...snapshot, focusedSessionId: focused }, []);
    bridge?.setFocus(focused);
    // The Rust store reads the focused pane itself.
    if (focused) {
      const pane = snapshot.panes[focused];
      const entry = snapshot.agentStatuses[focused];
      let next = snapshot;
      if (pane) next = withHeuristic(next, focused, viewHeuristicPane(pane));
      if (entry?.unread) {
        next = {
          ...next,
          agentStatuses: {
            ...next.agentStatuses,
            [focused]: { ...entry, unread: false },
          },
        };
      }
      commit(next, [focused]);
    }
  };

  return {
    getSnapshot: () => snapshot,
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => { listeners.delete(listener); };
    },
    subscribeStatus(listener: (id: string, status: TerminalActivityStatus | null) => void) {
      statusListeners.add(listener);
      return () => { statusListeners.delete(listener); };
    },
    /** Calls the listener when the display state of a pane changes. */
    subscribePanes(listener: PaneListener) {
      paneListeners.add(listener);
      return () => { paneListeners.delete(listener); };
    },
    trackerForSession,
    promptSubmissionForSession(id: string) {
      let tracker = inputTrackers.get(id);
      if (!tracker) {
        tracker = createTerminalPromptSubmissionTracker();
        inputTrackers.set(id, tracker);
      }
      return tracker;
    },
    noteEvent(event: TerminalEvent) {
      const tracker = trackers.get(event.sessionId);
      if (!tracker || event.sequence <= (sequences.get(event.sessionId) ?? -1)) {
        return;
      }
      sequences.set(event.sessionId, event.sequence);
      if (event.kind !== "data") tracker.finish();
      else if (!event.replay && event.data) tracker.noteOutput(event.data.length);
    },
    noteAgentEvent(id: string, event: AgentSessionEvent) {
      if (event.type === "embeddedTurnStarted") {
        return trackerForSession(id).noteEmbeddedTurnStarted(event.turnId);
      } else if (event.type === "embeddedTurnFinished") {
        return trackerForSession(id).noteEmbeddedTurnFinished(event.turnId);
      }
      return false;
    },
    /** Stores one `agent-status` entry from Rust. */
    noteAgentStatus(entry: AgentStatusEntry) {
      const id = entry.sessionId;
      // The terminal list owns process exit. Drop the entry of a gone pane.
      const agentStatuses =
        entry.status === "exited"
          ? without(snapshot.agentStatuses, id)
          : { ...snapshot.agentStatuses, [id]: entry };
      if (agentStatuses === snapshot.agentStatuses) return;
      commit({ ...snapshot, agentStatuses }, [id]);
    },
    /** Replaces the Rust entries. Entries in `keep` are newer than the snapshot. */
    hydrateAgentStatuses(
      entries: readonly AgentStatusEntry[],
      keep: ReadonlySet<string> = new Set(),
    ) {
      const agentStatuses: Record<string, AgentStatusEntry> = {};
      for (const id of keep) {
        const current = snapshot.agentStatuses[id];
        if (current) agentStatuses[id] = current;
      }
      for (const entry of entries) {
        if (keep.has(entry.sessionId) || entry.status === "exited") continue;
        agentStatuses[entry.sessionId] = entry;
      }
      const ids = [
        ...new Set([
          ...Object.keys(snapshot.agentStatuses),
          ...Object.keys(agentStatuses),
        ]),
      ];
      commit({ ...snapshot, agentStatuses }, ids);
    },
    setAgentStatusBridge(next: AgentStatusBridge | null) {
      bridge = next;
      bridge?.setFocus(snapshot.focusedSessionId);
    },
    /** Names the active pane of the visible terminal panel, or none. */
    setVisiblePane(id: string | null) {
      visibleSessionId = id;
      updateFocus();
    },
    setWindowFocused(focused: boolean) {
      windowFocused = focused;
      updateFocus();
    },
    /** True while a surface covers the terminal panes. */
    setPaneCovered(covered: boolean) {
      paneCovered = covered;
      updateFocus();
    },
    /** True while the pane works or waits for the user. */
    paneIsBusy(id: string) {
      if (agentStatusIsAuthoritative(snapshot.agentStatuses[id])) {
        return agentPaneBusy(agentPaneViewFor(snapshot, id));
      }
      const activity = snapshot.statuses[id];
      return activity === "running" || activity === "responding";
    },
    markViewed,
    forget(id: string) {
      trackers.get(id)?.finish();
      trackers.get(id)?.dispose();
      trackers.delete(id);
      inputTrackers.delete(id);
      sequences.delete(id);
      const next = {
        ...snapshot,
        panes: without(snapshot.panes, id),
        agentStatuses: without(snapshot.agentStatuses, id),
      };
      if (
        next.panes !== snapshot.panes ||
        next.agentStatuses !== snapshot.agentStatuses
      ) {
        commit(next, [id]);
      }
    },
    dispose() {
      trackers.forEach((tracker) => tracker.dispose());
      trackers.clear();
      inputTrackers.clear();
      sequences.clear();
      listeners.clear();
      statusListeners.clear();
      paneListeners.clear();
      bridge = null;
      visibleSessionId = null;
      windowFocused = false;
      paneCovered = false;
      snapshot = EMPTY_SNAPSHOT;
    },
  };
}

export const terminalActivityStore = createTerminalActivityStore();
