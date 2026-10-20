// The per-viewer state of the Projects tree in the sidebar: the collapsed
// rows, the open pills, the groups and the project order. It lives in
// localStorage, so every read and write survives a blocked storage.
//
// The sidebar always shows the Projects tree (ADR 0020). Version 1 also
// stored a Recent view ("view", "recentList"). The reader ignores those
// two fields, and the next write drops them.

export const PROJECT_TREE_STORAGE_KEY = "halv.sidebar.project-tree.v1";

/** A project group of the sidebar (Orca's "project group"). It only
 *  orders the sidebar. Execution stays per project and per workspace. */
export type ProjectGroup = {
  id: string;
  name: string;
  /** "folder-scan": Halv made the group from a folder project that holds
   *  git repositories (ADR 0019). A group without the field is manual. */
  createdFrom?: "manual" | "folder-scan";
  /** The folder project of a folder-scan group. Its row comes first in the
   *  group, as the folder of the group. */
  folderProjectId?: number;
  /** The folder that holds the repositories of a folder-scan group. */
  parentPath?: string;
};

export type ProjectTreeState = {
  version: 1;
  /** The keys of collapsed rows, for example `project:3`. */
  collapsed: string[];
  /** The keys of open "N agents" pills, for example `agents:wt-1`. A pill
   *  starts folded. */
  expanded: string[];
  /** The project groups, in sidebar order. */
  groups: ProjectGroup[];
  /** The group of each grouped project, by project id. */
  projectGroups: Record<string, string>;
  /** The manual order of the projects, by id. A project that is not in the
   *  list follows in the order it was added. */
  projectOrder: number[];
  /** The root paths of the projects that the one-time scan for nested
   *  repositories saw (ADR 0019). Absent: none yet. */
  scannedFolders?: string[];
};

type TreeStorage = Pick<Storage, "getItem" | "setItem">;

export const DEFAULT_PROJECT_TREE_STATE: ProjectTreeState = Object.freeze({
  version: 1,
  collapsed: [],
  expanded: [],
  groups: [],
  projectGroups: {},
  projectOrder: [],
}) as ProjectTreeState;

export function readProjectTreeState(storage: TreeStorage | null): ProjectTreeState {
  if (!storage) return DEFAULT_PROJECT_TREE_STATE;
  try {
    const parsed = JSON.parse(
      storage.getItem(PROJECT_TREE_STORAGE_KEY) ?? "null",
    ) as Partial<ProjectTreeState> | null;
    if (!parsed || typeof parsed !== "object" || parsed.version !== 1) {
      return DEFAULT_PROJECT_TREE_STATE;
    }
    return {
      version: 1,
      collapsed: Array.isArray(parsed.collapsed)
        ? [...new Set(parsed.collapsed.filter((key) => typeof key === "string"))]
        : [],
      expanded: Array.isArray(parsed.expanded)
        ? [...new Set(parsed.expanded.filter((key) => typeof key === "string"))]
        : [],
      ...readGroups(parsed),
      ...readScannedFolders(parsed),
    };
  } catch {
    return DEFAULT_PROJECT_TREE_STATE;
  }
}

// Each group field falls back alone. A project that names a missing group
// is ungrouped, so it never disappears (Orca does the same).
function readGroups(parsed: Partial<ProjectTreeState>) {
  const groups: ProjectGroup[] = [];
  const ids = new Set<string>();
  for (const group of Array.isArray(parsed.groups) ? parsed.groups : []) {
    if (
      !group ||
      typeof group.id !== "string" ||
      typeof group.name !== "string" ||
      !group.id ||
      ids.has(group.id)
    ) {
      continue;
    }
    ids.add(group.id);
    groups.push(readGroupOrigin(group, { id: group.id, name: group.name }));
  }
  const projectGroups: Record<string, string> = {};
  const stored = parsed.projectGroups;
  if (stored && typeof stored === "object" && !Array.isArray(stored)) {
    for (const [projectId, groupId] of Object.entries(stored)) {
      if (typeof groupId === "string" && ids.has(groupId)) {
        projectGroups[projectId] = groupId;
      }
    }
  }
  const projectOrder = Array.isArray(parsed.projectOrder)
    ? [
        ...new Set(
          parsed.projectOrder.filter(
            (id): id is number => typeof id === "number" && Number.isInteger(id),
          ),
        ),
      ]
    : [];
  return { groups, projectGroups, projectOrder };
}

// The folder fields of a group read back only when they are valid, so a
// manual group keeps its two fields.
function readGroupOrigin(stored: Partial<ProjectGroup>, group: ProjectGroup): ProjectGroup {
  if (stored.createdFrom !== "folder-scan") return group;
  const result: ProjectGroup = { ...group, createdFrom: "folder-scan" };
  if (typeof stored.folderProjectId === "number" && Number.isInteger(stored.folderProjectId)) {
    result.folderProjectId = stored.folderProjectId;
  }
  if (typeof stored.parentPath === "string" && stored.parentPath) {
    result.parentPath = stored.parentPath;
  }
  return result;
}

function readScannedFolders(parsed: Partial<ProjectTreeState>) {
  if (!Array.isArray(parsed.scannedFolders)) return {};
  const scannedFolders = [
    ...new Set(
      parsed.scannedFolders.filter(
        (path): path is string => typeof path === "string" && path.length > 0,
      ),
    ),
  ];
  return scannedFolders.length > 0 ? { scannedFolders } : {};
}

export function projectGroupKey(groupId: string) {
  return `group:${groupId}`;
}

/** A new group at the end of the sidebar. "New group from project" moves
 *  that project into it. */
export function withNewGroup(
  state: ProjectTreeState,
  group: ProjectGroup,
  projectId: number | null = null,
): ProjectTreeState {
  const name = group.name.trim();
  if (!name || state.groups.some((item) => item.id === group.id)) return state;
  const next = { ...state, groups: [...state.groups, { id: group.id, name }] };
  return projectId === null ? next : withProjectInGroup(next, projectId, group.id);
}

export function withGroupRenamed(
  state: ProjectTreeState,
  groupId: string,
  name: string,
): ProjectTreeState {
  const trimmed = name.trim();
  const group = state.groups.find((item) => item.id === groupId);
  if (!group || !trimmed || group.name === trimmed) return state;
  return {
    ...state,
    groups: state.groups.map((item) =>
      item.id === groupId ? { ...item, name: trimmed } : item,
    ),
  };
}

/** Deletes a group. Its projects stay in the sidebar, without a group. */
export function withGroupDeleted(state: ProjectTreeState, groupId: string): ProjectTreeState {
  if (!state.groups.some((item) => item.id === groupId)) return state;
  const key = projectGroupKey(groupId);
  return {
    ...state,
    groups: state.groups.filter((item) => item.id !== groupId),
    projectGroups: Object.fromEntries(
      Object.entries(state.projectGroups).filter(([, id]) => id !== groupId),
    ),
    collapsed: state.collapsed.filter((item) => item !== key),
  };
}

/** Puts a project into a group, or takes it out with `null`. */
export function withProjectInGroup(
  state: ProjectTreeState,
  projectId: number,
  groupId: string | null,
): ProjectTreeState {
  const current = state.projectGroups[String(projectId)] ?? null;
  if (current === groupId) return state;
  if (groupId !== null && !state.groups.some((item) => item.id === groupId)) return state;
  const projectGroups = { ...state.projectGroups };
  if (groupId === null) delete projectGroups[String(projectId)];
  else projectGroups[String(projectId)] = groupId;
  return { ...state, projectGroups };
}

/**
 * A drop of a dragged project. It joins the group of the target and moves
 * before the target project, or to the end of the target group. `order` is
 * the project order on screen, so a project without a stored place keeps
 * its place.
 */
export function withProjectMoved(
  state: ProjectTreeState,
  move: {
    projectId: number;
    groupId: string | null;
    beforeProjectId: number | null;
    order: readonly number[];
  },
): ProjectTreeState {
  const { projectId, groupId, beforeProjectId } = move;
  if (beforeProjectId === projectId) return withProjectInGroup(state, projectId, groupId);
  const order = move.order.filter((id) => id !== projectId);
  const index = beforeProjectId === null ? -1 : order.indexOf(beforeProjectId);
  if (index >= 0) {
    order.splice(index, 0, projectId);
  } else if (groupId === null) {
    order.push(projectId);
  } else {
    // The end of the group: after its last project, else the end.
    const inGroup = order.filter((id) => state.projectGroups[String(id)] === groupId);
    const last = inGroup.length > 0 ? order.indexOf(inGroup[inGroup.length - 1]) : -1;
    if (last >= 0) order.splice(last + 1, 0, projectId);
    else order.push(projectId);
  }
  const grouped = withProjectInGroup(state, projectId, groupId);
  const same =
    order.length === state.projectOrder.length &&
    order.every((id, position) => state.projectOrder[position] === id);
  return same ? grouped : { ...grouped, projectOrder: order };
}

/** True when the one-time scan for nested repositories saw this root. */
export function folderWasScanned(state: ProjectTreeState, rootPath: string) {
  return state.scannedFolders?.includes(rootPath) ?? false;
}

/**
 * Records a scan of a folder project for nested git repositories (ADR 0019).
 * Each root is marked as scanned, so the one-time scan never runs again for
 * it, and a project that the user removes does not come back.
 *
 * When the scan found repositories, they go into a "folder-scan" group named
 * after the folder, with the folder project first. A second scan of the same
 * folder reuses its group. A project that is in a group already stays there.
 */
export function withFolderScanGroup(
  state: ProjectTreeState,
  scan: {
    folder: { id: number; name: string; rootPath: string };
    repos: readonly { id: number; rootPath: string }[];
    /** The id of the group, when the scan makes a new one. */
    groupId: string;
  },
): ProjectTreeState {
  const { folder } = scan;
  const scanned = state.scannedFolders ?? [];
  const roots = [folder.rootPath, ...scan.repos.map((repo) => repo.rootPath)].filter(
    (root) => root && !scanned.includes(root),
  );
  let next: ProjectTreeState =
    roots.length > 0 ? { ...state, scannedFolders: [...new Set([...scanned, ...roots])] } : state;

  const repoIds = [
    ...new Set(scan.repos.map((repo) => repo.id).filter((id) => id !== folder.id)),
  ];
  const free = (id: number) => next.projectGroups[String(id)] === undefined;
  let group = next.groups.find(
    (item) => item.createdFrom === "folder-scan" && item.folderProjectId === folder.id,
  );
  if (!group) {
    // A group needs at least one repository that it can hold.
    if (!repoIds.some(free)) return next;
    const name = folder.name.trim() || folderName(folder.rootPath);
    if (!name || next.groups.some((item) => item.id === scan.groupId)) return next;
    group = {
      id: scan.groupId,
      name,
      createdFrom: "folder-scan",
      folderProjectId: folder.id,
      parentPath: folder.rootPath,
    };
    next = { ...next, groups: [...next.groups, group] };
  }
  for (const id of [folder.id, ...repoIds]) {
    if (free(id)) next = withProjectInGroup(next, id, group.id);
  }
  return next;
}

function folderName(rootPath: string) {
  return rootPath.split(/[\\/]+/).filter(Boolean).pop() ?? "";
}

/** Collapses or expands many rows in one change. */
export function withRowsCollapsed(
  state: ProjectTreeState,
  keys: readonly string[],
  collapsed: boolean,
): ProjectTreeState {
  return keys.reduce((current, key) => withRowCollapsed(current, key, collapsed), state);
}

export function writeProjectTreeState(
  state: ProjectTreeState,
  storage: TreeStorage | null,
) {
  if (!storage) return false;
  try {
    storage.setItem(PROJECT_TREE_STORAGE_KEY, JSON.stringify(state));
    return true;
  } catch {
    return false;
  }
}

export function withRowExpanded(
  state: ProjectTreeState,
  key: string,
  expanded: boolean,
): ProjectTreeState {
  const present = state.expanded.includes(key);
  if (present === expanded) return state;
  return {
    ...state,
    expanded: expanded
      ? [...state.expanded, key]
      : state.expanded.filter((item) => item !== key),
  };
}

export function withRowCollapsed(
  state: ProjectTreeState,
  key: string,
  collapsed: boolean,
): ProjectTreeState {
  const present = state.collapsed.includes(key);
  if (present === collapsed) return state;
  return {
    ...state,
    collapsed: collapsed
      ? [...state.collapsed, key]
      : state.collapsed.filter((item) => item !== key),
  };
}

function treeStorage(): TreeStorage | null {
  try {
    return typeof window === "undefined" ? null : window.localStorage;
  } catch {
    return null;
  }
}

// One state for every reader of the tree state.
export function createProjectTreeStateStore(storage: () => TreeStorage | null = treeStorage) {
  const listeners = new Set<() => void>();
  let snapshot: ProjectTreeState | null = null;
  const getSnapshot = () => {
    snapshot ??= readProjectTreeState(storage());
    return snapshot;
  };
  return {
    getSnapshot,
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    update(change: (current: ProjectTreeState) => ProjectTreeState) {
      const current = getSnapshot();
      const next = change(current);
      if (next === current) return;
      writeProjectTreeState(next, storage());
      snapshot = next;
      for (const listener of listeners) listener();
    },
  };
}

export const projectTreeStateStore = createProjectTreeStateStore();
