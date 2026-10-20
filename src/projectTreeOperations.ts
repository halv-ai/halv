// The worktree operations that the Projects tree shows inline (ADR 0010,
// stage S7), as Orca does. A new worktree shows a pending row under its
// project ("Creating worktree…"), and a failed creation keeps the row
// with the error. A removal dims its workspace row with "Removing…", and
// a failed removal leaves a red row with the error under the workspace.
// The store lives outside React, so an operation keeps its row when the
// sidebar closes and opens again.

export type WorktreeCreation = {
  id: string;
  projectId: number;
  /** The branch name, or null for a detached worktree. */
  branch: string | null;
  status: "creating" | "failed";
  error: string | null;
};

export type WorktreeRemoval = {
  workspaceId: string;
  status: "removing" | "failed";
  error: string | null;
};

export type TreeOperationsState = {
  /** In start order. */
  creations: readonly WorktreeCreation[];
  /** By workspace id. */
  removals: Readonly<Record<string, WorktreeRemoval>>;
};

const EMPTY: TreeOperationsState = { creations: [], removals: {} };

export function createTreeOperationsStore() {
  let state = EMPTY;
  let nextCreation = 0;
  const listeners = new Set<() => void>();
  const commit = (next: TreeOperationsState) => {
    if (next === state) return;
    state = next;
    for (const listener of listeners) listener();
  };
  const setRemoval = (workspaceId: string, removal: WorktreeRemoval | null) => {
    const removals = { ...state.removals };
    if (removal) removals[workspaceId] = removal;
    else if (workspaceId in removals) delete removals[workspaceId];
    else return;
    commit({ ...state, removals });
  };
  return {
    getSnapshot: () => state,
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    startCreation(projectId: number, branch: string | null) {
      nextCreation += 1;
      const id = `creation:${nextCreation}`;
      commit({
        ...state,
        creations: [
          ...state.creations,
          { id, projectId, branch, status: "creating", error: null },
        ],
      });
      return id;
    },
    finishCreation(id: string) {
      if (!state.creations.some((item) => item.id === id)) return;
      commit({ ...state, creations: state.creations.filter((item) => item.id !== id) });
    },
    failCreation(id: string, error: string) {
      if (!state.creations.some((item) => item.id === id)) return;
      commit({
        ...state,
        creations: state.creations.map((item) =>
          item.id === id ? { ...item, status: "failed", error } : item,
        ),
      });
    },
    /** Orca's "Cancel" on a failed pending row. A running creation stays. */
    dismissCreation(id: string) {
      if (!state.creations.some((item) => item.id === id && item.status === "failed")) return;
      commit({ ...state, creations: state.creations.filter((item) => item.id !== id) });
    },
    /** False when a removal of this workspace runs already. */
    startRemoval(workspaceId: string) {
      if (state.removals[workspaceId]?.status === "removing") return false;
      setRemoval(workspaceId, { workspaceId, status: "removing", error: null });
      return true;
    },
    finishRemoval(workspaceId: string) {
      setRemoval(workspaceId, null);
    },
    failRemoval(workspaceId: string, error: string) {
      setRemoval(workspaceId, { workspaceId, status: "failed", error });
    },
    dismissRemoval(workspaceId: string) {
      if (state.removals[workspaceId]?.status === "failed") setRemoval(workspaceId, null);
    },
  };
}

export type TreeOperationsStore = ReturnType<typeof createTreeOperationsStore>;

export const treeOperationsStore = createTreeOperationsStore();
