// The workspace tab store (ADR 0010). One module singleton holds the last
// snapshot from Rust, one serial queue for all tab IPC, and the visible
// state for useSyncExternalStore.
//
// Rules:
// - One serial queue carries every tab request, reads too. The store sends
//   one request, waits for it, then sends the next one.
// - The visible state is the snapshot plus every queued batch (optimistic
//   overlay, `rebase`).
// - A failed batch drops alone. Later batches that name its row become
//   no-ops, as they do in Rust.
// - The store ignores a snapshot with a lower revision.
// - Only consecutive unsent activation batches merge.
// - The store exports intents only. It has no raw operation API.

import { ipc } from "./ipc.ts";
import type { WorkspaceTabsSnapshot } from "./types";
import {
  activateTab,
  closeTab,
  cycleTab,
  EMPTY_TABS_STATE,
  INCOMPLETE_TAB_FACTS,
  isActivationBatch,
  mergeActivationBatches,
  moveTab,
  openTab,
  rebase,
  renameShellTab,
  seedTabs,
  selectTabByNumber,
  tabsStateFromSnapshot,
  type IntentResult,
  type OpenTabIntent,
  type SeedInput,
  type TabFacts,
  type TabKey,
  type TabRow,
  type TabsOp,
  type TabsState,
} from "./workspaceTabs.ts";

export type WorkspaceTabsBackend = {
  snapshot(): Promise<WorkspaceTabsSnapshot>;
  apply(ops: TabsOp[]): Promise<WorkspaceTabsSnapshot>;
  seed(
    rows: TabRow[],
    active: Record<string, TabKey | null>,
  ): Promise<WorkspaceTabsSnapshot>;
};

export type WorkspaceTabsStoreState = {
  /** True after the first snapshot arrived. */
  loaded: boolean;
  rev: number;
  seeded: boolean;
  /** The snapshot plus the queued batches. */
  tabs: TabsState;
  facts: TabFacts;
  /** The error of the last failed request, for diagnostics. */
  lastError: string | null;
};

/** What an intent did. `queued` is false for a no-op or a rejection. */
export type IntentOutcome = { queued: boolean; rejected?: string };

type QueueItem =
  | { kind: "read"; sent: boolean }
  | { kind: "batch"; ops: TabsOp[]; sent: boolean }
  | {
      kind: "seed";
      rows: TabRow[];
      active: Record<string, TabKey | null>;
      /** The overlay of the seed while it waits. */
      ops: TabsOp[];
      sent: boolean;
      settle: (seeded: boolean) => void;
    };

function errorText(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

export function createWorkspaceTabsStore(options: {
  backend: WorkspaceTabsBackend;
  now?: () => number;
}) {
  const { backend } = options;
  const clock = options.now ?? Date.now;
  // Activation times only grow, so two activations in one millisecond
  // still give a clear most recent tab.
  let lastNow = 0;
  const now = () => {
    lastNow = Math.max(clock(), lastNow + 1);
    return lastNow;
  };
  const listeners = new Set<() => void>();
  const idleWaiters = new Set<() => void>();
  const queue: QueueItem[] = [];
  let disk: TabsState = EMPTY_TABS_STATE;
  let pumping = false;
  let state: WorkspaceTabsStoreState = {
    loaded: false,
    rev: -1,
    seeded: false,
    tabs: EMPTY_TABS_STATE,
    facts: INCOMPLETE_TAB_FACTS,
    lastError: null,
  };

  function queuedBatches() {
    return queue.flatMap((item) =>
      item.kind === "batch" || item.kind === "seed" ? [item.ops] : [],
    );
  }

  function publish(change: Partial<WorkspaceTabsStoreState> = {}) {
    const next = { ...state, ...change };
    state = { ...next, tabs: rebase(disk, queuedBatches(), next.facts) };
    for (const listener of listeners) listener();
  }

  function acceptSnapshot(snapshot: WorkspaceTabsSnapshot) {
    if (snapshot.rev < state.rev) return false;
    disk = tabsStateFromSnapshot(snapshot);
    state = { ...state, loaded: true, rev: snapshot.rev, seeded: snapshot.seeded };
    return true;
  }

  function enqueueRead() {
    if (queue.some((item) => item.kind === "read" && !item.sent)) return;
    queue.push({ kind: "read", sent: false });
  }

  async function send(item: QueueItem) {
    switch (item.kind) {
      case "read":
        acceptSnapshot(await backend.snapshot());
        return;
      case "batch":
        acceptSnapshot(await backend.apply(item.ops));
        return;
      case "seed": {
        // Rust raises the revision only when this call wrote the marker.
        const before = state.rev;
        const snapshot = await backend.seed(item.rows, item.active);
        acceptSnapshot(snapshot);
        item.settle(snapshot.seeded && snapshot.rev > before);
        return;
      }
    }
  }

  async function pump() {
    if (pumping) return;
    pumping = true;
    try {
      while (queue.length > 0) {
        const item = queue[0];
        item.sent = true;
        let failure: string | null = null;
        try {
          await send(item);
        } catch (error) {
          failure = errorText(error);
          if (item.kind === "seed") item.settle(false);
        }
        queue.shift();
        if (failure !== null) {
          // The failed batch drops alone. A read then picks up any change
          // that another writer made, such as a foreign key cascade.
          if (item.kind !== "read") enqueueRead();
          publish({ lastError: failure });
        } else {
          publish();
        }
      }
    } finally {
      pumping = false;
      for (const resolve of idleWaiters) resolve();
      idleWaiters.clear();
    }
  }

  function enqueueBatch(ops: TabsOp[]) {
    const last = queue[queue.length - 1];
    if (
      last?.kind === "batch" &&
      !last.sent &&
      isActivationBatch(last.ops) &&
      isActivationBatch(ops)
    ) {
      last.ops = mergeActivationBatches(last.ops, ops);
    } else {
      queue.push({ kind: "batch", ops, sent: false });
    }
  }

  function ready() {
    return state.loaded && state.facts.complete;
  }

  function run(intent: (tabs: TabsState, facts: TabFacts) => IntentResult): IntentOutcome {
    if (!ready()) return { queued: false, rejected: "the tabs are not loaded yet" };
    const result = intent(state.tabs, state.facts);
    if (result.rejected) return { queued: false, rejected: result.rejected };
    if (result.ops.length === 0) return { queued: false };
    enqueueBatch(result.ops);
    publish();
    void pump();
    return { queued: true };
  }

  return {
    getSnapshot(): WorkspaceTabsStoreState {
      return state;
    },
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    /** The App lists changed. Facts are inputs, not tab state. */
    setFacts(facts: TabFacts) {
      if (facts === state.facts) return;
      publish({ facts });
    },
    /** Reads the snapshot through the queue. */
    refresh() {
      enqueueRead();
      void pump();
    },
    /** Resolves when the queue is empty. */
    whenIdle(): Promise<void> {
      if (!pumping && queue.length === 0) return Promise.resolve();
      return new Promise((resolve) => idleWaiters.add(resolve));
    },

    // Intents. Only explicit user actions call these.
    open(intent: Omit<OpenTabIntent, "now">) {
      return run((tabs, facts) => openTab(tabs, facts, { ...intent, now: now() }));
    },
    activate(key: TabKey) {
      return run((tabs, facts) => activateTab(tabs, facts, key, now()));
    },
    /** Call only after the processes of the tab stopped (ADR 0010, T9). */
    close(key: TabKey) {
      return run((tabs, facts) => closeTab(tabs, facts, key));
    },
    move(key: TabKey, toIndex: number) {
      return run((tabs, facts) => moveTab(tabs, facts, key, toIndex));
    },
    cycle(workspaceId: string, offset: -1 | 1) {
      return run((tabs, facts) => cycleTab(tabs, facts, workspaceId, offset, now()));
    },
    selectByNumber(workspaceId: string, digit: number) {
      return run((tabs, facts) => selectTabByNumber(tabs, facts, workspaceId, digit, now()));
    },
    renameShell(key: TabKey, title: string | null) {
      return run((tabs) => renameShellTab(tabs, key, title));
    },
    /** The one-time seed. It resolves to true when this call seeded. */
    seedOnce(input: Omit<SeedInput, "facts" | "now">): Promise<boolean> {
      if (!ready() || state.seeded || queue.some((item) => item.kind === "seed")) {
        return Promise.resolve(false);
      }
      const seed = seedTabs({ ...input, facts: state.facts, now: now() });
      const ops: TabsOp[] = [
        ...seed.rows.map((row): TabsOp => ({ type: "insertTab", row })),
        ...Object.entries(seed.activeByWorkspace).map(
          ([workspaceId, key]): TabsOp => ({ type: "setActive", workspaceId, key }),
        ),
      ];
      return new Promise((settle) => {
        queue.push({
          kind: "seed",
          rows: seed.rows,
          active: seed.activeByWorkspace,
          ops,
          sent: false,
          settle,
        });
        publish();
        void pump();
      });
    },
  };
}

export type WorkspaceTabsStore = ReturnType<typeof createWorkspaceTabsStore>;

export const workspaceTabsStore = createWorkspaceTabsStore({
  backend: {
    snapshot: () => ipc.workspace.tabsSnapshot(),
    apply: (ops) => ipc.workspace.tabsApply({ ops }),
    seed: (rows, active) => ipc.workspace.tabsSeed({ rows, active }),
  },
});
