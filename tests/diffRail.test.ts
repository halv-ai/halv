import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  DEFAULT_DIFF_RAIL_WIDTH,
  DIFF_RAIL_REFRESH_BROKEN_WINDOWS,
  DIFF_RAIL_REFRESH_WINDOW_LIMIT,
  DIFF_RAIL_REFRESH_WINDOW_MS,
  DIFF_RAIL_STORAGE_KEY,
  DiffRailStore,
  admitDiffRailRefresh,
  areAllFilesCollapsed,
  beginDiffRailSnapshot,
  buildDiffRailChipGroups,
  chipLabelForRoot,
  clearDiffRailRefreshGuard,
  createDiffRailState,
  defaultCollapsedFileIds,
  failDiffRailSnapshot,
  installDiffRailSnapshot,
  isDiffRailFileCollapsed,
  isDiffRailRootCollapsed,
  markDiffRailStaleBySession,
  panesForDiffRailRoot,
  publishDiffRailPanes,
  readDiffRailPreferences,
  rollbackDiffRailLoadedQuery,
  setAllFilesCollapsed,
  setDiffRailActivePane,
  setDiffRailPinned,
  shouldRefreshSnapshot,
  snapshotForPane,
  snapshotKeysForPane,
  toggleDiffRailFileCollapsed,
  toggleDiffRailRootCollapsed,
  writeDiffRailPreferences,
  type DiffRailPaneDescriptor,
  type DiffRailRefreshGuard,
  type DiffRailStorage,
} from "../src/diffRail.ts";
import type { ProjectReviewSnapshot, ReviewFile } from "../src/types.ts";

test("labels diff chips by repository root", () => {
  const sameRoot = ["/work/repo/app", "/work/repo/app"];
  assert.equal(
    chipLabelForRoot("/work/repo/app", "Release audit", sameRoot),
    "app",
  );
  assert.equal(chipLabelForRoot(null, "Release audit", sameRoot), "Release audit");
  assert.equal(chipLabelForRoot("", "Release audit", sameRoot), "Release audit");

  const duplicateBasenames = ["/work/repoA/app", "/work/repoB/app"];
  assert.equal(
    chipLabelForRoot("/work/repoA/app", "Release audit", duplicateBasenames),
    "app (repoA)",
  );
  assert.equal(
    chipLabelForRoot("/work/repoB/app", "Release audit", duplicateBasenames),
    "app (repoB)",
  );
});

test("hides ready clean discovered roots for the active pane", () => {
  let state = publishDiffRailPanes(createDiffRailState(), [
    {
      ...pane("plain", "/workspace", null),
      gitRoots: ["/repo/clean", "/repo/dirty"],
    },
  ]);
  state = installDiffRailSnapshot(
    state,
    "/repo/clean",
    reviewSnapshot("/repo/clean"),
    [],
  );
  state = installDiffRailSnapshot(
    state,
    "/repo/dirty",
    reviewSnapshot("/repo/dirty"),
    [file("changed.ts")],
  );

  assert.deepEqual(
    buildDiffRailChipGroups(
      state.panes,
      state.snapshots,
      "plain",
    ).map((group) => group.gitRoot),
    ["/repo/dirty"],
  );
});

test("keeps the active primary root and stale discovered roots visible", () => {
  let state = publishDiffRailPanes(createDiffRailState(), [
    {
      ...pane("one", "/repo/primary", "/repo/primary"),
      gitRoots: ["/repo/primary", "/repo/discovered"],
    },
  ]);
  state = installDiffRailSnapshot(
    state,
    "/repo/primary",
    reviewSnapshot("/repo/primary"),
    [],
  );
  state = installDiffRailSnapshot(
    state,
    "/repo/discovered",
    reviewSnapshot("/repo/discovered"),
    [],
  );

  assert.deepEqual(
    buildDiffRailChipGroups(
      state.panes,
      state.snapshots,
      "one",
    ).map((group) => group.gitRoot),
    ["/repo/primary"],
  );

  const staleState = {
    ...state,
    snapshots: {
      ...state.snapshots,
      "/repo/discovered": {
        ...state.snapshots["/repo/discovered"],
        stale: true,
      },
    },
  };
  assert.deepEqual(
    buildDiffRailChipGroups(
      staleState.panes,
      staleState.snapshots,
      "one",
    ).map((group) => group.gitRoot),
    ["/repo/primary", "/repo/discovered"],
  );
});

test("publishes panes and deduplicates snapshots by normalized git root", () => {
  const state = publishDiffRailPanes(createDiffRailState(), [
    pane("one", "/repo/one", "/repo"),
    pane("two", "/repo/two", "/repo/"),
    pane("three", "/other", null),
    pane("two", "/repo/two", "/repo"),
  ]);

  assert.deepEqual(
    state.panes.map((item) => item.sessionId),
    ["one", "two", "three"],
  );
  assert.deepEqual(Object.keys(state.snapshots), ["/repo"]);
  assert.equal(panesForDiffRailRoot(state, "/repo/").length, 2);
  assert.equal(snapshotForPane(state, "one"), snapshotForPane(state, "two"));
  assert.equal(snapshotForPane(state, "three"), null);
});

test("publishes one snapshot key for each repository a pane touched", () => {
  const descriptor = {
    ...pane("one", "/tmp", null),
    gitRoots: ["/repo/one/", "/repo/two", "/repo/one"],
  };
  const state = publishDiffRailPanes(createDiffRailState(), [descriptor]);

  assert.deepEqual(state.panes[0].gitRoots, ["/repo/one", "/repo/two"]);
  assert.deepEqual(snapshotKeysForPane(state.panes[0]), [
    "/repo/one",
    "/repo/two",
  ]);
  assert.deepEqual(Object.keys(state.snapshots), ["/repo/one", "/repo/two"]);
  assert.equal(panesForDiffRailRoot(state, "/repo/two").length, 1);
});

test("resolves null-root panes from the snapshot cwd and deduplicates later fetches", () => {
  const store = new DiffRailStore(null);
  store.publishPanes([
    pane("one", "/repo/packages/one", null),
    pane("two", "/repo/packages/two", null),
  ]);
  store.setActivePane("one");

  const firstRequest = store.beginSnapshot("one");
  assert.equal(snapshotForPane(store.getSnapshot(), "one")?.fetchState, "loading");
  store.installSnapshot(
    "one",
    reviewSnapshot("/repo", "snapshot-one"),
    [file("one.ts")],
    firstRequest,
  );

  assert.equal(store.getSnapshot().panes[0].gitRoot, "/repo");
  assert.equal(store.getSnapshot().selection.activeGitRoot, "/repo");
  assert.deepEqual(Object.keys(store.getSnapshot().snapshots), ["/repo"]);

  const secondRequest = store.beginSnapshot("two");
  store.installSnapshot(
    "two",
    reviewSnapshot("/repo/", "snapshot-two"),
    [file("two.ts")],
    secondRequest,
  );

  assert.deepEqual(Object.keys(store.getSnapshot().snapshots), ["/repo"]);
  assert.equal(snapshotForPane(store.getSnapshot(), "one")?.snapshot?.id, "snapshot-two");
  assert.equal(snapshotForPane(store.getSnapshot(), "two")?.snapshot?.id, "snapshot-two");
});

test("keeps backend root resolution across null-root pane publications", () => {
  const store = new DiffRailStore(null);
  store.publishPanes([pane("one", "/repo/packages/one", null, 3)]);
  const requestId = store.beginSnapshot("one");
  store.installSnapshot(
    "one",
    reviewSnapshot("/repo", "resolved", 9),
    [file("one.ts")],
    requestId,
  );

  store.publishPanes([
    {
      ...pane("one", "/repo/packages/one", null, 3),
      title: "Updated title",
    },
  ]);

  assert.equal(store.getSnapshot().panes[0].gitRoot, "/repo");
  assert.equal(store.getSnapshot().panes[0].projectId, 9);
  assert.equal(store.getSnapshot().panes[0].title, "Updated title");

  store.publishPanes([pane("one", "/different/repo", null, 3)]);
  assert.equal(store.getSnapshot().panes[0].gitRoot, null);
  assert.equal(store.getSnapshot().panes[0].projectId, 3);
});

test("follows the active pane unless the selected root is pinned", () => {
  let state = publishDiffRailPanes(createDiffRailState(), [
    pane("one", "/one", "/one"),
    pane("two", "/two", "/two"),
  ]);
  state = setDiffRailActivePane(state, "one");
  assert.equal(state.selection.activeGitRoot, "/one");

  state = setDiffRailPinned(state, true);
  state = setDiffRailActivePane(state, "two");
  assert.deepEqual(state.selection, {
    activeSessionId: "two",
    activeGitRoot: "/one",
    pinned: true,
  });

  state = setDiffRailPinned(state, false);
  assert.deepEqual(state.selection, {
    activeSessionId: "two",
    activeGitRoot: "/two",
    pinned: false,
  });
});

test("marks the one shared root stale for any pane in that worktree", () => {
  let state = publishDiffRailPanes(createDiffRailState(), [
    pane("one", "/repo/one", "/repo"),
    pane("two", "/repo/two", "/repo"),
  ]);
  state = installDiffRailSnapshot(
    state,
    "/repo",
    reviewSnapshot("/repo"),
    [file("one.ts")],
  );
  const marked = markDiffRailStaleBySession(state, "two");

  assert.equal(marked.snapshots["/repo"].stale, true);
  assert.equal(snapshotForPane(marked, "one"), snapshotForPane(marked, "two"));
  assert.equal(markDiffRailStaleBySession(marked, "two"), marked);
});

test("clears stale when a snapshot begins, fails, or installs", () => {
  let state = publishDiffRailPanes(createDiffRailState(), [
    pane("one", "/repo", "/repo"),
  ]);
  state = installDiffRailSnapshot(
    state,
    "/repo",
    reviewSnapshot("/repo"),
    [file("one.ts")],
  );
  state = markDiffRailStaleBySession(state, "one");

  state = beginDiffRailSnapshot(state, "/repo", 1);
  assert.equal(state.snapshots["/repo"].stale, false);

  state = markDiffRailStaleBySession(state, "one");
  state = failDiffRailSnapshot(state, "/repo", new Error("failed"), 1);
  assert.equal(state.snapshots["/repo"].stale, false);

  state = markDiffRailStaleBySession(state, "one");
  state = installDiffRailSnapshot(
    state,
    "/repo",
    reviewSnapshot("/repo", "refreshed"),
    [file("one.ts")],
    1,
  );
  assert.equal(state.snapshots["/repo"].stale, false);
});

test("stops automatic refresh after a stale snapshot fails", () => {
  const query = "working_tree::conversation";
  let state = publishDiffRailPanes(createDiffRailState(), [
    pane("one", "/repo", "/repo"),
  ]);
  state = installDiffRailSnapshot(
    state,
    "/repo",
    reviewSnapshot("/repo"),
    [file("one.ts")],
  );
  state = markDiffRailStaleBySession(state, "one");
  assert.equal(
    shouldRefreshSnapshot(state.snapshots["/repo"], query, query, false),
    true,
  );

  state = beginDiffRailSnapshot(state, "/repo", 1);
  assert.equal(
    shouldRefreshSnapshot(
      state.snapshots["/repo"],
      query,
      "staged::conversation",
      false,
    ),
    false,
  );

  state = failDiffRailSnapshot(state, "/repo", new Error("failed"), 1);
  assert.equal(
    shouldRefreshSnapshot(state.snapshots["/repo"], query, query, false),
    false,
  );

  state = markDiffRailStaleBySession(state, "one");
  assert.equal(
    shouldRefreshSnapshot(state.snapshots["/repo"], query, query, true),
    false,
  );
  assert.equal(
    shouldRefreshSnapshot(state.snapshots["/repo"], query, query, false),
    true,
  );
});

test("collapses large files and files after the first three by default", () => {
  const files = [
    file("large.ts", 801),
    file("second.ts", 2),
    file("third.ts", 3),
    file("fourth.ts", 4),
    file("fifth.ts", 5),
  ];
  assert.deepEqual(defaultCollapsedFileIds(files), [
    "large.ts",
    "fourth.ts",
    "fifth.ts",
  ]);

  let state = publishDiffRailPanes(createDiffRailState(), [
    pane("one", "/repo", "/repo"),
  ]);
  state = installDiffRailSnapshot(
    state,
    "/repo",
    reviewSnapshot("/repo"),
    files,
  );
  assert.equal(isDiffRailFileCollapsed(state, "large.ts", "/repo"), true);
  assert.equal(isDiffRailFileCollapsed(state, "second.ts", "/repo"), false);
  assert.equal(isDiffRailFileCollapsed(state, "fourth.ts", "/repo"), true);

  state = toggleDiffRailFileCollapsed(state, "large.ts", "/repo");
  state = toggleDiffRailFileCollapsed(state, "second.ts", "/repo");
  state = installDiffRailSnapshot(
    state,
    "/repo",
    reviewSnapshot("/repo", "refreshed"),
    files,
  );
  assert.equal(isDiffRailFileCollapsed(state, "large.ts", "/repo"), false);
  assert.equal(isDiffRailFileCollapsed(state, "second.ts", "/repo"), true);
});

test("toggles repository sections independently", () => {
  let state = publishDiffRailPanes(createDiffRailState(), [
    {
      ...pane("one", "/tmp", null),
      gitRoots: ["/repo/one", "/repo/two"],
    },
  ]);
  state = toggleDiffRailRootCollapsed(state, "/repo/two");

  assert.equal(isDiffRailRootCollapsed(state, "/repo/one"), false);
  assert.equal(isDiffRailRootCollapsed(state, "/repo/two"), true);
  state = toggleDiffRailRootCollapsed(state, "/repo/two");
  assert.equal(isDiffRailRootCollapsed(state, "/repo/two"), false);
});

test("collapses and expands every file in the supplied groups", () => {
  let state = publishDiffRailPanes(createDiffRailState(), [
    {
      ...pane("one", "/tmp", null),
      gitRoots: ["/repo/one", "/repo/two", "/repo/other"],
    },
  ]);
  for (const root of ["/repo/one", "/repo/two", "/repo/other"]) {
    state = installDiffRailSnapshot(
      state,
      root,
      reviewSnapshot(root),
      [file(`${root}-first.ts`), file(`${root}-second.ts`)],
    );
  }
  state = toggleDiffRailFileCollapsed(
    state,
    "/repo/other-first.ts",
    "/repo/other",
  );
  const visibleGroups = buildDiffRailChipGroups(
    state.panes,
    state.snapshots,
    state.selection.activeSessionId,
  ).filter((group) => group.gitRoot !== "/repo/other");

  const collapsed = setAllFilesCollapsed(state, true, visibleGroups);
  assert.notEqual(collapsed, state);
  assert.deepEqual(collapsed.ui.collapsedFileIds["/repo/one"], [
    "/repo/one-first.ts",
    "/repo/one-second.ts",
  ]);
  assert.deepEqual(collapsed.ui.collapsedFileIds["/repo/two"], [
    "/repo/two-first.ts",
    "/repo/two-second.ts",
  ]);
  assert.deepEqual(collapsed.ui.collapsedFileIds["/repo/other"], [
    "/repo/other-first.ts",
  ]);
  assert.deepEqual(state.ui.collapsedFileIds["/repo/one"], []);

  const expanded = setAllFilesCollapsed(collapsed, false, visibleGroups);
  assert.deepEqual(expanded.ui.collapsedFileIds["/repo/one"], []);
  assert.deepEqual(expanded.ui.collapsedFileIds["/repo/two"], []);
  assert.deepEqual(expanded.ui.collapsedFileIds["/repo/other"], [
    "/repo/other-first.ts",
  ]);
});

test("reports whether all files in the supplied groups are collapsed", () => {
  let state = publishDiffRailPanes(createDiffRailState(), [
    pane("one", "/repo", "/repo"),
  ]);
  state = installDiffRailSnapshot(
    state,
    "/repo",
    reviewSnapshot("/repo"),
    [file("one.ts"), file("two.ts")],
  );
  const groups = buildDiffRailChipGroups(
    state.panes,
    state.snapshots,
    state.selection.activeSessionId,
  );

  assert.equal(areAllFilesCollapsed(state, groups), false);
  state = setAllFilesCollapsed(state, true, groups);
  assert.equal(areAllFilesCollapsed(state, groups), true);
  state = toggleDiffRailFileCollapsed(state, "one.ts", "/repo");
  assert.equal(areAllFilesCollapsed(state, groups), false);
  assert.equal(areAllFilesCollapsed(state, []), true);
});

test("reuses parsed file objects when their raw diff is unchanged", () => {
  let state = publishDiffRailPanes(createDiffRailState(), [
    pane("one", "/repo", "/repo"),
  ]);
  const original = { ...file("one.ts"), raw: "same raw diff" };
  state = installDiffRailSnapshot(
    state,
    "/repo",
    reviewSnapshot("/repo"),
    [original],
  );
  const reparsed = { ...file("one.ts", 99), raw: "same raw diff" };
  const refreshed = installDiffRailSnapshot(
    state,
    "/repo",
    reviewSnapshot("/repo", "refreshed"),
    [reparsed],
  );

  assert.equal(refreshed.snapshots["/repo"].files[0], original);
});

test("retains an exited pane only while its snapshot still has changes", () => {
  let state = publishDiffRailPanes(createDiffRailState(), [
    pane("one", "/repo", "/repo"),
  ]);
  state = installDiffRailSnapshot(
    state,
    "/repo",
    reviewSnapshot("/repo"),
    [file("one.ts")],
  );
  state = publishDiffRailPanes(state, []);

  assert.equal(state.panes.length, 1);
  assert.equal(state.panes[0].sessionEnded, true);

  state = installDiffRailSnapshot(
    state,
    "/repo",
    reviewSnapshot("/repo", "clean"),
    [],
  );
  assert.deepEqual(state.panes, []);
  assert.deepEqual(state.snapshots, {});
});

test("commits each store mutation once and ignores stale request completions", () => {
  const store = new DiffRailStore(null);
  let notifications = 0;
  const unsubscribe = store.subscribe(() => {
    notifications += 1;
  });

  store.publishPanes([pane("one", "/repo", null)]);
  const oldRequest = store.beginSnapshot("one");
  const currentRequest = store.beginSnapshot("one");
  const beforeLateCompletion = store.getSnapshot();
  store.failSnapshot("one", new Error("late failure"), oldRequest);

  assert.equal(store.getSnapshot(), beforeLateCompletion);
  assert.equal(notifications, 3);

  store.installSnapshot(
    "one",
    reviewSnapshot("/repo"),
    [file("one.ts")],
    currentRequest,
  );
  assert.equal(notifications, 4);
  assert.equal(store.getSnapshot().snapshots["/repo"].fetchState, "ready");
  unsubscribe();
});

test("persists only safe UI preferences and tolerates unavailable storage", () => {
  const values = new Map<string, string>();
  const storage: DiffRailStorage = {
    getItem(key) {
      return values.get(key) ?? null;
    },
    setItem(key, value) {
      values.set(key, value);
    },
  };
  const store = new DiffRailStore(storage);
  store.setOpen(true);
  store.setWidth(700);
  store.setViewMode("split");

  assert.deepEqual(JSON.parse(values.get(DIFF_RAIL_STORAGE_KEY) ?? "null"), {
    version: 1,
    open: true,
    width: 700,
    viewMode: "split",
  });
  assert.deepEqual(readDiffRailPreferences(storage), {
    version: 1,
    open: true,
    width: 700,
    viewMode: "split",
  });
  store.setOpen(false);
  assert.equal(readDiffRailPreferences(storage).open, false);

  const unavailable: DiffRailStorage = {
    getItem() {
      throw new Error("storage disabled");
    },
    setItem() {
      throw new Error("storage disabled");
    },
  };
  const fallback = new DiffRailStore(unavailable);
  assert.equal(fallback.getSnapshot().ui.width, DEFAULT_DIFF_RAIL_WIDTH);
  assert.doesNotThrow(() => fallback.setOpen(true));
});

test("persists a closed diff rail preference without closing the store", () => {
  const values = new Map<string, string>();
  const storage: DiffRailStorage = {
    getItem(key) {
      return values.get(key) ?? null;
    },
    setItem(key, value) {
      values.set(key, value);
    },
  };
  const store = new DiffRailStore(storage);
  store.setOpen(true);

  writeDiffRailPreferences(
    { ...store.getSnapshot().ui, open: false },
    storage,
  );

  assert.equal(store.getSnapshot().ui.open, true);
  assert.equal(readDiffRailPreferences(storage).open, false);

  writeDiffRailPreferences(store.getSnapshot().ui, storage);
  assert.equal(readDiffRailPreferences(storage).open, true);
});

test("rolls back the loaded query when a refresh does not start", () => {
  const key = "/repo";
  const previousQuery = "working_tree::";
  const query = "staged::";
  const queries = new Map([[key, previousQuery]]);

  // The effect records the query first. Then the fetch does not start.
  queries.set(key, query);
  rollbackDiffRailLoadedQuery(queries, key, query, previousQuery);
  assert.equal(queries.get(key), previousQuery);

  // The restored query asks for a refresh again.
  const readySnapshot = {
    gitRoot: key,
    snapshot: null,
    files: [],
    fetchState: "ready" as const,
    stale: false,
    error: null,
    requestId: 1,
  };
  assert.equal(
    shouldRefreshSnapshot(readySnapshot, queries.get(key), query, false),
    true,
  );

  // A first load rolls back to an empty map entry.
  queries.delete(key);
  queries.set(key, query);
  rollbackDiffRailLoadedQuery(queries, key, query, undefined);
  assert.equal(queries.has(key), false);

  // A newer recorded query survives a late rollback.
  queries.set(key, "commit:HEAD:");
  rollbackDiffRailLoadedQuery(queries, key, query, previousQuery);
  assert.equal(queries.get(key), "commit:HEAD:");
});

test("latches a broken refresh key after repeated suppression windows", () => {
  const guards = new Map<string, DiffRailRefreshGuard>();
  const key = "/repo";
  let now = 0;
  const loopWindow = () => {
    const decisions: string[] = [];
    for (let call = 0; call <= DIFF_RAIL_REFRESH_WINDOW_LIMIT; call += 1) {
      decisions.push(admitDiffRailRefresh(guards, key, now));
      now += 10;
    }
    now += DIFF_RAIL_REFRESH_WINDOW_MS;
    return decisions;
  };

  for (let window = 0; window < DIFF_RAIL_REFRESH_BROKEN_WINDOWS; window += 1) {
    const decisions = loopWindow();
    assert.equal(decisions[0], "start");
    assert.equal(decisions[DIFF_RAIL_REFRESH_WINDOW_LIMIT - 1], "start");
    assert.equal(decisions[DIFF_RAIL_REFRESH_WINDOW_LIMIT], "suppress");
  }

  assert.equal(admitDiffRailRefresh(guards, key, now), "broken");
  assert.equal(
    admitDiffRailRefresh(guards, key, now + 10 * DIFF_RAIL_REFRESH_WINDOW_MS),
    "broken",
  );

  // Other keys stay unaffected.
  assert.equal(admitDiffRailRefresh(guards, "/other", now), "start");

  // A manual refresh clears the latch.
  clearDiffRailRefreshGuard(guards, key);
  assert.equal(admitDiffRailRefresh(guards, key, now), "start");
});

test("a calm window resets the suppression streak", () => {
  const guards = new Map<string, DiffRailRefreshGuard>();
  const key = "/repo";
  let now = 0;
  const suppressWindow = () => {
    for (let call = 0; call <= DIFF_RAIL_REFRESH_WINDOW_LIMIT; call += 1) {
      admitDiffRailRefresh(guards, key, now);
      now += 10;
    }
    now += DIFF_RAIL_REFRESH_WINDOW_MS;
  };

  suppressWindow();
  suppressWindow();
  // One calm window: a single admitted call, then silence.
  assert.equal(admitDiffRailRefresh(guards, key, now), "start");
  now += DIFF_RAIL_REFRESH_WINDOW_MS;
  suppressWindow();
  suppressWindow();
  assert.equal(admitDiffRailRefresh(guards, key, now), "start");
});

function pane(
  sessionId: string,
  cwd: string,
  gitRoot: string | null,
  projectId = 1,
): DiffRailPaneDescriptor {
  return {
    sessionId,
    cwd,
    gitRoot,
    gitRoots: gitRoot ? [gitRoot] : [],
    projectId,
    title: `Pane ${sessionId}`,
    agentKind: "codex",
  };
}

function reviewSnapshot(
  cwd: string,
  id = "snapshot",
  projectId = 1,
): ProjectReviewSnapshot {
  return {
    id,
    projectId,
    generatedAt: "2026-08-21T00:00:00.000Z",
    cwd,
    diff: "",
    truncated: false,
  };
}

function file(id: string, additions = 1, deletions = 0): ReviewFile {
  return {
    id,
    path: id,
    status: "modified",
    additions,
    deletions,
    binary: false,
    hunks: [],
    raw: "",
  };
}
