import type {
  WorkspaceTabKey,
  WorkspaceTabRow,
  WorkspaceTabsOp,
  WorkspaceTabsSnapshot,
} from "../types";
import {
  applyBatchToDisk,
  EMPTY_TABS_STATE,
  type TabFacts,
  type TabsState,
} from "../workspaceTabs.ts";

// The fixture copy of the workspace tab tables. It runs the TS disk model,
// which keeps the SQL rules of `storage::workspace_tabs`: a batch commits
// whole or not at all, a duplicate insert is a no-op, a missing owner fails
// the batch, and setActive is a guarded no-op.

// A reload keeps the tables in sessionStorage, as SQLite keeps them across
// app starts. Each new browser context starts empty.
export const FIXTURE_WORKSPACE_TABS_STORAGE_KEY = "halv.harness.workspace-tabs.v1";

type FixtureTabsStorage = Pick<Storage, "getItem" | "setItem">;

function readStoredTabs(storage: FixtureTabsStorage | null) {
  try {
    const parsed = JSON.parse(storage?.getItem(FIXTURE_WORKSPACE_TABS_STORAGE_KEY) ?? "null") as {
      disk?: TabsState;
      seeded?: boolean;
    } | null;
    return parsed?.disk ? { disk: parsed.disk, seeded: parsed.seeded === true } : null;
  } catch {
    return null;
  }
}

export function createFixtureWorkspaceTabs(
  facts: () => TabFacts,
  storage: FixtureTabsStorage | null = null,
) {
  const stored = readStoredTabs(storage);
  let disk: TabsState = stored?.disk ?? EMPTY_TABS_STATE;
  let rev = 0;
  let seeded = stored?.seeded ?? false;
  const save = () => {
    try {
      storage?.setItem(FIXTURE_WORKSPACE_TABS_STORAGE_KEY, JSON.stringify({ disk, seeded }));
    } catch {
      // The fixture keeps working in memory.
    }
  };

  const snapshot = (): WorkspaceTabsSnapshot => ({
    rev,
    rows: Object.values(disk.rows)
      .sort((left, right) => (left.key < right.key ? -1 : left.key > right.key ? 1 : 0))
      .map((row) => ({ ...row })),
    activeByWorkspace: { ...disk.activeByWorkspace },
    seeded,
  });

  return {
    snapshot,
    apply(ops: readonly WorkspaceTabsOp[]) {
      const outcome = applyBatchToDisk(disk, ops, facts());
      if (outcome.error) throw new Error(outcome.error);
      disk = outcome.disk;
      rev += 1;
      save();
      return snapshot();
    },
    seed(
      rows: readonly WorkspaceTabRow[],
      active: Readonly<Record<string, WorkspaceTabKey | null>>,
    ) {
      if (seeded) return snapshot();
      const current = facts();
      if (rows.some((row) => row.conversationId === null)) {
        throw new Error("the seed opens conversation tabs only");
      }
      // Like Rust, the seed skips a conversation that is gone, archived or
      // project-less now.
      const ops: WorkspaceTabsOp[] = [
        ...rows
          .filter((row) => {
            const conversation = current.conversations.get(row.conversationId ?? "");
            return conversation !== undefined && !conversation.archived && conversation.projectId !== null;
          })
          .map((row): WorkspaceTabsOp => ({ type: "insertTab", row })),
        ...Object.entries(active).map(
          ([workspaceId, key]): WorkspaceTabsOp => ({ type: "setActive", workspaceId, key }),
        ),
      ];
      const outcome = applyBatchToDisk(disk, ops, current);
      if (outcome.error) throw new Error(outcome.error);
      disk = outcome.disk;
      seeded = true;
      rev += 1;
      save();
      return snapshot();
    },
  };
}

