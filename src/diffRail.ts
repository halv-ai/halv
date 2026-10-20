import type { AgentKind } from "./agentSessions";
import type { ProjectReviewSnapshot, ReviewFile } from "./types";

export const DIFF_RAIL_STORAGE_KEY = "halv.diffRail.v1";
export const DEFAULT_DIFF_RAIL_WIDTH = 520;
export const MIN_DIFF_RAIL_WIDTH = 320;
export const MAX_DIFF_RAIL_WIDTH = 960;
export const DIFF_RAIL_LARGE_FILE_CHANGED_LINES = 800;
export const DIFF_RAIL_DEFAULT_EXPANDED_FILES = 3;
export const DIFF_RAIL_REFRESH_WINDOW_MS = 1_000;
export const DIFF_RAIL_REFRESH_WINDOW_LIMIT = 5;
export const DIFF_RAIL_REFRESH_BROKEN_WINDOWS = 3;

const PENDING_SNAPSHOT_PREFIX = "pane:";

export type DiffRailViewMode = "unified" | "split";
export type DiffRailFetchState = "idle" | "loading" | "ready" | "error";

export type DiffRailPaneDescriptor = {
  sessionId: string;
  cwd: string;
  gitRoot: string | null;
  gitRoots: string[];
  projectId: number | null;
  title: string;
  agentKind: AgentKind | null;
  sessionEnded?: boolean;
};

export type DiffRailSnapshot = {
  gitRoot: string | null;
  snapshot: ProjectReviewSnapshot | null;
  files: ReviewFile[];
  fetchState: DiffRailFetchState;
  stale: boolean;
  error: string | null;
  requestId: number;
};
export type DiffRailChipGroup = {
  key: string;
  gitRoot: string | null;
  panes: DiffRailPaneDescriptor[];
  snapshot: DiffRailSnapshot | null;
};

export type DiffRailSelection = {
  activeSessionId: string | null;
  activeGitRoot: string | null;
  pinned: boolean;
};

export type DiffRailUiState = {
  open: boolean;
  width: number;
  viewMode: DiffRailViewMode;
  collapsedFileIds: Record<string, string[]>;
  collapsedRoots: string[];
};

export type DiffRailState = {
  panes: DiffRailPaneDescriptor[];
  snapshots: Record<string, DiffRailSnapshot>;
  selection: DiffRailSelection;
  ui: DiffRailUiState;
};

export type DiffRailPersistedPreferences = {
  version: 1;
  open: boolean;
  width: number;
  viewMode: DiffRailViewMode;
};

export type DiffRailStorage = Pick<Storage, "getItem" | "setItem">;

export type DiffRailRefreshGuard = {
  count: number;
  startedAt: number;
  suppressedWindows: number;
  broken: boolean;
};

export type DiffRailRefreshDecision = "start" | "suppress" | "broken";

// Decides if an automatic refresh for this key can start now.
// The guard admits DIFF_RAIL_REFRESH_WINDOW_LIMIT starts per window.
// After DIFF_RAIL_REFRESH_BROKEN_WINDOWS suppression windows in a row,
// the guard latches the key broken. A manual refresh clears the latch.
export function admitDiffRailRefresh(
  guards: Map<string, DiffRailRefreshGuard>,
  key: string,
  now: number,
): DiffRailRefreshDecision {
  const guard = guards.get(key);
  if (guard?.broken) return "broken";
  if (!guard || now - guard.startedAt >= DIFF_RAIL_REFRESH_WINDOW_MS) {
    const suppressedWindows =
      guard && guard.count > DIFF_RAIL_REFRESH_WINDOW_LIMIT
        ? guard.suppressedWindows + 1
        : 0;
    if (suppressedWindows >= DIFF_RAIL_REFRESH_BROKEN_WINDOWS) {
      guards.set(key, {
        count: 0,
        startedAt: now,
        suppressedWindows,
        broken: true,
      });
      return "broken";
    }
    guards.set(key, {
      count: 1,
      startedAt: now,
      suppressedWindows,
      broken: false,
    });
    return "start";
  }
  guard.count += 1;
  return guard.count > DIFF_RAIL_REFRESH_WINDOW_LIMIT ? "suppress" : "start";
}

export function clearDiffRailRefreshGuard(
  guards: Map<string, DiffRailRefreshGuard>,
  key: string,
) {
  guards.delete(key);
}

// Restores the loaded-query map after a refresh did not start.
// A later fetch result wins: the map only rolls back while it still
// holds the query this attempt wrote.
export function rollbackDiffRailLoadedQuery(
  queries: Map<string, string>,
  key: string,
  query: string,
  previousQuery: string | undefined,
) {
  if (queries.get(key) !== query) return;
  if (previousQuery === undefined) {
    queries.delete(key);
  } else {
    queries.set(key, previousQuery);
  }
}

export function shouldRefreshSnapshot(
  snapshot: DiffRailSnapshot | null | undefined,
  previousQuery: string | undefined,
  query: string,
  hasTimer: boolean,
) {
  if (snapshot?.fetchState === "loading") return false;
  if (
    snapshot?.fetchState === "error" &&
    previousQuery === query &&
    !snapshot.stale
  ) {
    return false;
  }
  if (previousQuery !== query || !snapshot || snapshot.fetchState === "idle") {
    return true;
  }
  return snapshot.stale && !hasTimer;
}

export function buildDiffRailChipGroups(
  panes: readonly DiffRailPaneDescriptor[],
  snapshots: Readonly<Record<string, DiffRailSnapshot>>,
  activeSessionId: string | null,
): DiffRailChipGroup[] {
  const groups = new Map<string, DiffRailChipGroup>();
  for (const pane of panes) {
    for (const key of snapshotKeysForPane(pane)) {
      const current = groups.get(key);
      if (current) {
        current.panes.push(pane);
        continue;
      }
      const gitRoot = pane.gitRoots.includes(key) ? key : null;
      groups.set(key, {
        key,
        gitRoot,
        panes: [pane],
        snapshot: snapshots[key] ?? null,
      });
    }
  }
  return [...groups.values()].filter((group) => {
    if (
      group.snapshot?.fetchState === "ready" &&
      !group.snapshot.stale &&
      group.snapshot.files.length === 0 &&
      group.gitRoot !== null &&
      group.panes.every((pane) => pane.gitRoot !== group.gitRoot)
    ) {
      return false;
    }
    if (group.panes.some((pane) => pane.sessionId === activeSessionId)) {
      return true;
    }
    if (!group.snapshot) return true;
    return (
      group.snapshot.files.length > 0 ||
      group.snapshot.fetchState !== "ready" ||
      group.snapshot.stale
    );
  });
}

export function chipLabelForRoot(
  root: string | null | undefined,
  paneTitle: string,
  allRoots: readonly (string | null | undefined)[],
) {
  const normalizedRoot = normalizeRootForLabel(root);
  if (!normalizedRoot) return paneTitle;
  const parts = rootLabelParts(normalizedRoot);
  const basename = parts.at(-1);
  if (!basename) return paneTitle;

  const matchingRoots = new Set(
    allRoots
      .map(normalizeRootForLabel)
      .filter((candidate): candidate is string => {
        if (!candidate) return false;
        return rootLabelParts(candidate).at(-1) === basename;
      }),
  );
  if (matchingRoots.size < 2) return basename;

  const parent = parts.at(-2);
  return parent ? `${basename} (${parent})` : basename;
}

export function createDiffRailState(
  preferences: Partial<DiffRailPersistedPreferences> = {},
): DiffRailState {
  return {
    panes: [],
    snapshots: {},
    selection: {
      activeSessionId: null,
      activeGitRoot: null,
      pinned: false,
    },
    ui: {
      open: typeof preferences.open === "boolean" ? preferences.open : false,
      width: validWidth(preferences.width)
        ? clampWidth(preferences.width)
        : DEFAULT_DIFF_RAIL_WIDTH,
      viewMode: isViewMode(preferences.viewMode)
        ? preferences.viewMode
        : "unified",
      collapsedFileIds: {},
      collapsedRoots: [],
    },
  };
}

export function readDiffRailPreferences(
  storage: Pick<DiffRailStorage, "getItem"> | null = browserStorage(),
): DiffRailPersistedPreferences {
  const defaults = persistedPreferences(createDiffRailState().ui);
  if (!storage) return defaults;
  try {
    const parsed = JSON.parse(
      storage.getItem(DIFF_RAIL_STORAGE_KEY) ?? "null",
    ) as Partial<DiffRailPersistedPreferences> | null;
    if (!parsed || parsed.version !== 1) return defaults;
    return {
      version: 1,
      open: typeof parsed.open === "boolean" ? parsed.open : defaults.open,
      width: validWidth(parsed.width)
        ? clampWidth(parsed.width)
        : defaults.width,
      viewMode: isViewMode(parsed.viewMode)
        ? parsed.viewMode
        : defaults.viewMode,
    };
  } catch {
    return defaults;
  }
}

export function writeDiffRailPreferences(
  ui: DiffRailUiState,
  storage: Pick<DiffRailStorage, "setItem"> | null = browserStorage(),
) {
  if (!storage) return;
  try {
    storage.setItem(
      DIFF_RAIL_STORAGE_KEY,
      JSON.stringify(persistedPreferences(ui)),
    );
  } catch {
    // Restricted webviews can reject storage. The in-memory choice still wins.
  }
}

export function publishDiffRailPanes(
  state: DiffRailState,
  descriptors: readonly DiffRailPaneDescriptor[],
): DiffRailState {
  const previousBySession = new Map(
    state.panes.map((pane) => [pane.sessionId, pane]),
  );
  const published = dedupePanes(descriptors).map((pane) => {
    const previous = previousBySession.get(pane.sessionId);
    if (
      pane.gitRoot === null &&
      pane.gitRoots.length === 0 &&
      previous &&
      previous.cwd === pane.cwd &&
      (previous.gitRoot !== null || previous.gitRoots.length > 0)
    ) {
      return {
        ...pane,
        gitRoot: previous.gitRoot,
        gitRoots: previous.gitRoots,
        projectId: previous.projectId,
      };
    }
    return pane;
  });
  const publishedIds = new Set(published.map((pane) => pane.sessionId));
  const retained = state.panes
    .filter(
      (pane) =>
        !publishedIds.has(pane.sessionId) &&
        snapshotsForPane(state, pane).some(
          (snapshot) => snapshot.files.length > 0,
        ),
    )
    .map((pane) => ({ ...pane, sessionEnded: true }));
  const panes = [...published, ...retained];
  let snapshots = { ...state.snapshots };
  let collapsedFileIds = { ...state.ui.collapsedFileIds };

  for (const pane of published) {
    const roots = pane.gitRoots;
    if (roots.length === 0) continue;
    const pendingKey = pendingSnapshotKey(pane.sessionId);
    const pending = snapshots[pendingKey];
    if (pending) {
      const primaryRoot = pane.gitRoot ?? roots[0];
      snapshots[primaryRoot] = snapshots[primaryRoot] ?? {
        ...pending,
        gitRoot: primaryRoot,
      };
      delete snapshots[pendingKey];
      if (!collapsedFileIds[primaryRoot] && collapsedFileIds[pendingKey]) {
        collapsedFileIds[primaryRoot] = collapsedFileIds[pendingKey];
      }
      delete collapsedFileIds[pendingKey];
    }
    for (const root of roots) snapshots[root] ??= emptySnapshot(root);
  }

  snapshots = pruneSnapshots(snapshots, panes);
  collapsedFileIds = pruneCollapsedFileIds(collapsedFileIds, snapshots);
  const activeSessionId = publishedIds.has(
    state.selection.activeSessionId ?? "",
  )
    ? state.selection.activeSessionId
    : null;
  const selection = reconcileSelection(
    { ...state.selection, activeSessionId },
    panes,
    snapshots,
  );
  return {
    ...state,
    panes,
    snapshots,
    selection,
    ui: {
      ...state.ui,
      collapsedFileIds,
      collapsedRoots: pruneCollapsedRoots(state.ui.collapsedRoots, panes),
    },
  };
}

export function setDiffRailActivePane(
  state: DiffRailState,
  sessionId: string | null,
): DiffRailState {
  const pane = sessionId
    ? state.panes.find(
        (candidate) =>
          candidate.sessionId === sessionId && !candidate.sessionEnded,
      ) ?? null
    : null;
  const activeSessionId = pane?.sessionId ?? null;
  return {
    ...state,
    selection: {
      ...state.selection,
      activeSessionId,
      activeGitRoot: state.selection.pinned
        ? state.selection.activeGitRoot
        : primaryRootForPane(pane),
    },
  };
}

export function selectDiffRailRoot(
  state: DiffRailState,
  gitRoot: string | null,
): DiffRailState {
  const root = normalizeGitRoot(gitRoot);
  if (
    root &&
    !state.snapshots[root] &&
    !state.panes.some((pane) => pane.gitRoots.includes(root))
  ) {
    return state;
  }
  if (state.selection.activeGitRoot === root) return state;
  return {
    ...state,
    selection: { ...state.selection, activeGitRoot: root },
  };
}

export function setDiffRailPinned(
  state: DiffRailState,
  pinned: boolean,
): DiffRailState {
  const canPin = pinned && state.selection.activeGitRoot !== null;
  if (canPin === state.selection.pinned) return state;
  const activePane = state.panes.find(
    (pane) =>
      pane.sessionId === state.selection.activeSessionId && !pane.sessionEnded,
  );
  return {
    ...state,
    selection: {
      ...state.selection,
      pinned: canPin,
      activeGitRoot: canPin
        ? state.selection.activeGitRoot
        : primaryRootForPane(activePane),
    },
  };
}

export function setDiffRailOpen(
  state: DiffRailState,
  open: boolean,
): DiffRailState {
  return state.ui.open === open
    ? state
    : { ...state, ui: { ...state.ui, open } };
}

export function setDiffRailWidth(
  state: DiffRailState,
  width: number,
): DiffRailState {
  if (!validWidth(width)) return state;
  const nextWidth = clampWidth(width);
  return state.ui.width === nextWidth
    ? state
    : { ...state, ui: { ...state.ui, width: nextWidth } };
}

export function setDiffRailViewMode(
  state: DiffRailState,
  viewMode: DiffRailViewMode,
): DiffRailState {
  return state.ui.viewMode === viewMode
    ? state
    : { ...state, ui: { ...state.ui, viewMode } };
}

export function changedLineCount(file: ReviewFile) {
  return file.additions + file.deletions;
}

export function defaultCollapsedFileIds(
  files: readonly ReviewFile[],
): string[] {
  return files
    .filter(
      (file, index) =>
        index >= DIFF_RAIL_DEFAULT_EXPANDED_FILES ||
        changedLineCount(file) > DIFF_RAIL_LARGE_FILE_CHANGED_LINES,
    )
    .map((file) => file.id);
}

export function isDiffRailFileCollapsed(
  state: DiffRailState,
  fileId: string,
  gitRoot: string | null = state.selection.activeGitRoot,
) {
  const root = normalizeGitRoot(gitRoot);
  return root ? (state.ui.collapsedFileIds[root] ?? []).includes(fileId) : false;
}

export function toggleDiffRailFileCollapsed(
  state: DiffRailState,
  fileId: string,
  gitRoot: string | null = state.selection.activeGitRoot,
): DiffRailState {
  const root = normalizeGitRoot(gitRoot);
  if (!root || !state.snapshots[root]?.files.some((file) => file.id === fileId)) {
    return state;
  }
  const current = state.ui.collapsedFileIds[root] ?? [];
  const collapsedFileIds = current.includes(fileId)
    ? current.filter((id) => id !== fileId)
    : [...current, fileId];
  return {
    ...state,
    ui: {
      ...state.ui,
      collapsedFileIds: {
        ...state.ui.collapsedFileIds,
        [root]: collapsedFileIds,
      },
    },
  };
}

export function areAllFilesCollapsed(
  state: DiffRailState,
  groups: readonly DiffRailChipGroup[],
) {
  return groups.every((group) => {
    const root = normalizeGitRoot(group.gitRoot);
    if (!root) return true;
    const collapsed = new Set(state.ui.collapsedFileIds[root] ?? []);
    return (group.snapshot?.files ?? []).every((file) => collapsed.has(file.id));
  });
}

export function setAllFilesCollapsed(
  state: DiffRailState,
  collapsed: boolean,
  groups: readonly DiffRailChipGroup[],
): DiffRailState {
  const collapsedFileIds = { ...state.ui.collapsedFileIds };
  let changed = false;
  for (const group of groups) {
    const root = normalizeGitRoot(group.gitRoot);
    if (!root) continue;
    const next = collapsed
      ? (group.snapshot?.files ?? []).map((file) => file.id)
      : [];
    const current = collapsedFileIds[root] ?? [];
    if (
      current.length === next.length &&
      current.every((fileId, index) => fileId === next[index])
    ) {
      continue;
    }
    collapsedFileIds[root] = next;
    changed = true;
  }
  return changed
    ? { ...state, ui: { ...state.ui, collapsedFileIds } }
    : state;
}

export function isDiffRailRootCollapsed(
  state: DiffRailState,
  gitRoot: string,
) {
  const root = normalizeGitRoot(gitRoot);
  return root ? state.ui.collapsedRoots.includes(root) : false;
}

export function toggleDiffRailRootCollapsed(
  state: DiffRailState,
  gitRoot: string,
): DiffRailState {
  const root = normalizeGitRoot(gitRoot);
  if (!root || !state.snapshots[root]) return state;
  const collapsedRoots = state.ui.collapsedRoots.includes(root)
    ? state.ui.collapsedRoots.filter((candidate) => candidate !== root)
    : [...state.ui.collapsedRoots, root];
  return {
    ...state,
    ui: { ...state.ui, collapsedRoots },
  };
}

export function markDiffRailStaleBySession(
  state: DiffRailState,
  sessionId: string,
): DiffRailState {
  const pane = state.panes.find((candidate) => candidate.sessionId === sessionId);
  if (!pane) return state;
  const keys = snapshotKeysForPane(pane);
  const staleKeys = keys.filter((key) => {
    const snapshot = state.snapshots[key];
    return snapshot && !snapshot.stale;
  });
  if (staleKeys.length === 0) return state;
  const snapshots = { ...state.snapshots };
  for (const key of staleKeys) {
    snapshots[key] = { ...snapshots[key], stale: true };
  }
  return {
    ...state,
    snapshots,
  };
}

export function beginDiffRailSnapshot(
  state: DiffRailState,
  paneOrKey: string,
  requestId = nextRequestId(state),
): DiffRailState {
  const key = resolveSnapshotKey(state, paneOrKey);
  const current = state.snapshots[key] ?? emptySnapshot(rootFromKey(key));
  return {
    ...state,
    snapshots: {
      ...state.snapshots,
      [key]: {
        ...current,
        fetchState: "loading",
        stale: false,
        error: null,
        requestId,
      },
    },
  };
}

export function installDiffRailSnapshot(
  state: DiffRailState,
  paneOrKey: string,
  snapshot: ProjectReviewSnapshot,
  files: readonly ReviewFile[],
  requestId?: number,
): DiffRailState {
  const sourceKey = resolveSnapshotKey(state, paneOrKey);
  const source = state.snapshots[sourceKey];
  if (requestId !== undefined && source?.requestId !== requestId) return state;
  const gitRoot = normalizeGitRoot(snapshot.cwd) ?? rootFromKey(sourceKey);
  if (!gitRoot) return state;
  const target = state.snapshots[gitRoot];
  const previous = target ?? source ?? emptySnapshot(gitRoot);
  const previousFiles = target?.files ?? source?.files ?? [];
  const nextFiles = reuseUnchangedFiles(previousFiles, files);
  const snapshots = { ...state.snapshots };
  snapshots[gitRoot] = {
    ...previous,
    gitRoot,
    snapshot,
    files: nextFiles,
    fetchState: "ready",
    stale: false,
    error: null,
    requestId: source?.requestId ?? previous.requestId,
  };
  if (sourceKey !== gitRoot) delete snapshots[sourceKey];

  const resolvedPendingPane = sourceKey.startsWith(PENDING_SNAPSHOT_PREFIX);
  let panes = state.panes.map((pane) => {
    const resolvesPane =
      resolvedPendingPane &&
      (pane.sessionId === paneOrKey ||
        pendingSnapshotKey(pane.sessionId) === sourceKey);
    if (resolvesPane) {
      return {
        ...pane,
        gitRoot,
        gitRoots: dedupeGitRoots([gitRoot, ...pane.gitRoots]),
        projectId: snapshot.projectId,
      };
    }
    if (!pane.gitRoots.includes(gitRoot)) return pane;
    return { ...pane, projectId: snapshot.projectId };
  });
  if (nextFiles.length === 0) {
    panes = panes.filter(
      (pane) =>
        !(
          pane.sessionEnded &&
          pane.gitRoots.includes(gitRoot) &&
          !snapshotsForPane({ ...state, snapshots }, pane).some(
            (paneSnapshot) => paneSnapshot.files.length > 0,
          )
        ),
    );
  }

  const previousCollapsed =
    state.ui.collapsedFileIds[gitRoot] ??
    state.ui.collapsedFileIds[sourceKey] ??
    [];
  const collapsedFileIds = { ...state.ui.collapsedFileIds };
  collapsedFileIds[gitRoot] = reconcileCollapsedFiles(
    previousFiles,
    previousCollapsed,
    nextFiles,
  );
  if (sourceKey !== gitRoot) delete collapsedFileIds[sourceKey];

  const requestedSession = state.panes.find(
    (pane) =>
      pane.sessionId === paneOrKey || snapshotKeysForPane(pane).includes(sourceKey),
  )?.sessionId;
  let activeGitRoot = state.selection.activeGitRoot;
  const sourceRoot = rootFromKey(sourceKey);
  if (
    (sourceRoot !== null && activeGitRoot === sourceRoot) ||
    (!state.selection.pinned &&
      requestedSession === state.selection.activeSessionId)
  ) {
    activeGitRoot = gitRoot;
  }
  const nextSnapshots = pruneSnapshots(snapshots, panes);
  const selection = reconcileSelection(
    { ...state.selection, activeGitRoot },
    panes,
    nextSnapshots,
  );
  return {
    ...state,
    panes,
    snapshots: nextSnapshots,
    selection,
    ui: {
      ...state.ui,
      collapsedFileIds: pruneCollapsedFileIds(
        collapsedFileIds,
        nextSnapshots,
      ),
    },
  };
}

export function failDiffRailSnapshot(
  state: DiffRailState,
  paneOrKey: string,
  error: unknown,
  requestId?: number,
): DiffRailState {
  const key = resolveSnapshotKey(state, paneOrKey);
  const current = state.snapshots[key] ?? emptySnapshot(rootFromKey(key));
  if (requestId !== undefined && current.requestId !== requestId) return state;
  return {
    ...state,
    snapshots: {
      ...state.snapshots,
      [key]: {
        ...current,
        fetchState: "error",
        stale: false,
        error: errorMessage(error),
      },
    },
  };
}

export function snapshotForPane(
  state: DiffRailState,
  paneOrSessionId: DiffRailPaneDescriptor | string,
) {
  const pane =
    typeof paneOrSessionId === "string"
      ? state.panes.find((candidate) => candidate.sessionId === paneOrSessionId)
      : paneOrSessionId;
  return pane ? state.snapshots[snapshotKeyForPane(pane)] ?? null : null;
}

export function snapshotsForPane(
  state: DiffRailState,
  paneOrSessionId: DiffRailPaneDescriptor | string,
) {
  const pane =
    typeof paneOrSessionId === "string"
      ? state.panes.find((candidate) => candidate.sessionId === paneOrSessionId)
      : paneOrSessionId;
  if (!pane) return [];
  return snapshotKeysForPane(pane)
    .map((key) => state.snapshots[key])
    .filter((snapshot): snapshot is DiffRailSnapshot => Boolean(snapshot));
}

export function panesForDiffRailRoot(
  state: DiffRailState,
  gitRoot: string,
) {
  const root = normalizeGitRoot(gitRoot);
  return state.panes.filter((pane) => Boolean(root && pane.gitRoots.includes(root)));
}

export function snapshotKeysForPane(pane: DiffRailPaneDescriptor) {
  return pane.gitRoots.length > 0
    ? pane.gitRoots
    : [pendingSnapshotKey(pane.sessionId)];
}

export function snapshotKeyForPane(
  pane: DiffRailPaneDescriptor,
  gitRoot: string | null = primaryRootForPane(pane),
) {
  const root = normalizeGitRoot(gitRoot);
  return root && pane.gitRoots.includes(root)
    ? root
    : snapshotKeysForPane(pane)[0];
}

export class DiffRailStore {
  private state: DiffRailState;
  private listeners = new Set<() => void>();
  private requestSequence = 0;
  private readonly storage: DiffRailStorage | null;

  constructor(storage: DiffRailStorage | null = browserStorage()) {
    this.storage = storage;
    this.state = createDiffRailState(readDiffRailPreferences(storage));
  }

  getSnapshot = () => this.state;

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  publishPanes(descriptors: readonly DiffRailPaneDescriptor[]) {
    this.commit(publishDiffRailPanes(this.state, descriptors));
  }

  setActivePane(sessionId: string | null) {
    this.commit(setDiffRailActivePane(this.state, sessionId));
  }

  selectRoot(gitRoot: string | null) {
    this.commit(selectDiffRailRoot(this.state, gitRoot));
  }

  setPinned(pinned: boolean) {
    this.commit(setDiffRailPinned(this.state, pinned));
  }

  togglePin() {
    this.setPinned(!this.state.selection.pinned);
  }

  setOpen(open: boolean) {
    this.commit(setDiffRailOpen(this.state, open));
  }

  toggleOpen() {
    this.setOpen(!this.state.ui.open);
  }

  setWidth(width: number) {
    this.commit(setDiffRailWidth(this.state, width));
  }

  setViewMode(viewMode: DiffRailViewMode) {
    this.commit(setDiffRailViewMode(this.state, viewMode));
  }

  toggleFileCollapsed(
    fileId: string,
    gitRoot: string | null = this.state.selection.activeGitRoot,
  ) {
    this.commit(toggleDiffRailFileCollapsed(this.state, fileId, gitRoot));
  }

  setAllFilesCollapsed(
    collapsed: boolean,
    groups: readonly DiffRailChipGroup[],
  ) {
    this.commit(setAllFilesCollapsed(this.state, collapsed, groups));
  }

  toggleRootCollapsed(gitRoot: string) {
    this.commit(toggleDiffRailRootCollapsed(this.state, gitRoot));
  }

  markStaleBySession(sessionId: string) {
    this.commit(markDiffRailStaleBySession(this.state, sessionId));
  }

  // Async callers keep only this request id across their await, never state.
  beginSnapshot(paneOrKey: string) {
    const requestId = ++this.requestSequence;
    this.commit(beginDiffRailSnapshot(this.state, paneOrKey, requestId));
    return requestId;
  }

  installSnapshot(
    paneOrKey: string,
    snapshot: ProjectReviewSnapshot,
    files: readonly ReviewFile[],
    requestId?: number,
  ) {
    this.commit(
      installDiffRailSnapshot(
        this.state,
        paneOrKey,
        snapshot,
        files,
        requestId,
      ),
    );
  }

  failSnapshot(paneOrKey: string, error: unknown, requestId?: number) {
    this.commit(
      failDiffRailSnapshot(this.state, paneOrKey, error, requestId),
    );
  }

  private commit(state: DiffRailState) {
    if (state === this.state) return;
    const shouldPersist =
      state.ui.open !== this.state.ui.open ||
      state.ui.width !== this.state.ui.width ||
      state.ui.viewMode !== this.state.ui.viewMode;
    this.state = state;
    if (shouldPersist) writeDiffRailPreferences(state.ui, this.storage);
    for (const listener of this.listeners) listener();
  }
}

export const diffRailStore = new DiffRailStore();

function emptySnapshot(gitRoot: string | null): DiffRailSnapshot {
  return {
    gitRoot,
    snapshot: null,
    files: [],
    fetchState: "idle",
    stale: false,
    error: null,
    requestId: 0,
  };
}

function dedupePanes(
  descriptors: readonly DiffRailPaneDescriptor[],
): DiffRailPaneDescriptor[] {
  const bySessionId = new Map<string, DiffRailPaneDescriptor>();
  for (const descriptor of descriptors) {
    const sessionId = descriptor.sessionId.trim();
    if (!sessionId) continue;
    bySessionId.set(sessionId, {
      ...descriptor,
      sessionId,
      cwd: descriptor.cwd.trim(),
      gitRoot: normalizeGitRoot(descriptor.gitRoot),
      gitRoots: dedupeGitRoots([
        descriptor.gitRoot,
        ...descriptor.gitRoots,
      ]),
      title: descriptor.title.trim() || sessionId,
      sessionEnded: descriptor.sessionEnded || undefined,
    });
  }
  return [...bySessionId.values()];
}

function pendingSnapshotKey(sessionId: string) {
  return `${PENDING_SNAPSHOT_PREFIX}${sessionId}`;
}

function rootFromKey(key: string) {
  return key.startsWith(PENDING_SNAPSHOT_PREFIX)
    ? null
    : normalizeGitRoot(key);
}

function resolveSnapshotKey(state: DiffRailState, paneOrKey: string) {
  const pane = state.panes.find(
    (candidate) => candidate.sessionId === paneOrKey,
  );
  return pane ? snapshotKeyForPane(pane) : rootFromKey(paneOrKey) ?? paneOrKey;
}

function primaryRootForPane(
  pane: DiffRailPaneDescriptor | null | undefined,
) {
  return pane?.gitRoot ?? pane?.gitRoots[0] ?? null;
}

function dedupeGitRoots(values: readonly (string | null | undefined)[]) {
  return [
    ...new Set(
      values
        .map(normalizeGitRoot)
        .filter((root): root is string => Boolean(root)),
    ),
  ];
}

function normalizeGitRoot(value: string | null | undefined) {
  const trimmed = value?.trim();
  if (!trimmed) return null;
  if (trimmed === "/") return trimmed;
  return trimmed.replace(/\/+$/, "");
}

function normalizeRootForLabel(value: string | null | undefined) {
  const trimmed = value?.trim().replace(/[\\/]+$/, "");
  return trimmed || null;
}

function rootLabelParts(root: string) {
  return root.split(/[\\/]/).filter(Boolean);
}

function nextRequestId(state: DiffRailState) {
  return (
    Math.max(0, ...Object.values(state.snapshots).map((row) => row.requestId)) +
    1
  );
}

function reconcileCollapsedFiles(
  previousFiles: readonly ReviewFile[],
  previousCollapsed: readonly string[],
  files: readonly ReviewFile[],
) {
  const previousIds = new Set(previousFiles.map((file) => file.id));
  const collapsed = new Set(previousCollapsed);
  const defaults = new Set(defaultCollapsedFileIds(files));
  return files
    .filter((file) =>
      previousIds.has(file.id) ? collapsed.has(file.id) : defaults.has(file.id),
    )
    .map((file) => file.id);
}

function reuseUnchangedFiles(
  previousFiles: readonly ReviewFile[],
  files: readonly ReviewFile[],
) {
  const previousById = new Map(previousFiles.map((file) => [file.id, file]));
  return files.map((file) => {
    const previous = previousById.get(file.id);
    return previous?.raw === file.raw ? previous : file;
  });
}

function reconcileSelection(
  selection: DiffRailSelection,
  panes: readonly DiffRailPaneDescriptor[],
  snapshots: Record<string, DiffRailSnapshot>,
): DiffRailSelection {
  const activePane = panes.find(
    (pane) =>
      pane.sessionId === selection.activeSessionId && !pane.sessionEnded,
  );
  const selectedRootExists =
    selection.activeGitRoot !== null &&
    (Boolean(snapshots[selection.activeGitRoot]) ||
      panes.some((pane) => pane.gitRoots.includes(selection.activeGitRoot!)));
  if (selection.pinned && selectedRootExists) return selection;
  if (activePane) {
    return {
      activeSessionId: activePane.sessionId,
      activeGitRoot: primaryRootForPane(activePane),
      pinned: false,
    };
  }
  return {
    activeSessionId: null,
    activeGitRoot:
      primaryRootForPane(panes.find((pane) => !pane.sessionEnded)),
    pinned: false,
  };
}

function pruneSnapshots(
  snapshots: Record<string, DiffRailSnapshot>,
  panes: readonly DiffRailPaneDescriptor[],
) {
  const paneKeys = new Set(panes.flatMap(snapshotKeysForPane));
  return Object.fromEntries(
    Object.entries(snapshots).filter(
      ([key, snapshot]) =>
        paneKeys.has(key) ||
        snapshot.files.length > 0 ||
        snapshot.fetchState === "loading",
    ),
  );
}

function pruneCollapsedRoots(
  collapsedRoots: readonly string[],
  panes: readonly DiffRailPaneDescriptor[],
) {
  const roots = new Set(panes.flatMap((pane) => pane.gitRoots));
  return collapsedRoots.filter((root) => roots.has(root));
}

function pruneCollapsedFileIds(
  collapsed: Record<string, string[]>,
  snapshots: Record<string, DiffRailSnapshot>,
) {
  return Object.fromEntries(
    Object.entries(collapsed).filter(([key]) => Boolean(snapshots[key])),
  );
}

function persistedPreferences(
  ui: Pick<DiffRailUiState, "open" | "width" | "viewMode">,
): DiffRailPersistedPreferences {
  return {
    version: 1,
    open: ui.open,
    width: clampWidth(ui.width),
    viewMode: ui.viewMode,
  };
}

function validWidth(width: unknown): width is number {
  return typeof width === "number" && Number.isFinite(width) && width > 0;
}

function clampWidth(width: number) {
  return Math.min(MAX_DIFF_RAIL_WIDTH, Math.max(MIN_DIFF_RAIL_WIDTH, width));
}

function isViewMode(value: unknown): value is DiffRailViewMode {
  return value === "unified" || value === "split";
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function browserStorage(): Storage | null {
  try {
    return typeof window === "undefined" ? null : window.localStorage;
  } catch {
    return null;
  }
}
