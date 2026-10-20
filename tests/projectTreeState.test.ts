import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  createProjectTreeStateStore,
  DEFAULT_PROJECT_TREE_STATE,
  PROJECT_TREE_STORAGE_KEY,
  readProjectTreeState,
  withGroupDeleted,
  withGroupRenamed,
  withNewGroup,
  withProjectInGroup,
  withProjectMoved,
  withRowCollapsed,
  withRowsCollapsed,
  withRowExpanded,
  writeProjectTreeState,
} from "../src/projectTreeState.ts";

function memoryStorage(initial: Record<string, string> = {}) {
  const values = new Map(Object.entries(initial));
  return {
    values,
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => {
      values.set(key, value);
    },
  };
}

test("the default state survives corrupt storage", () => {
  assert.deepEqual(readProjectTreeState(memoryStorage()), DEFAULT_PROJECT_TREE_STATE);
  for (const value of ["{", "[]", "{\"version\":2}", "7"]) {
    assert.deepEqual(
      readProjectTreeState(memoryStorage({ [PROJECT_TREE_STORAGE_KEY]: value })),
      DEFAULT_PROJECT_TREE_STATE,
      value,
    );
  }
  const throwing = {
    getItem(): string | null {
      throw new Error("blocked");
    },
    setItem() {
      throw new Error("blocked");
    },
  };
  assert.deepEqual(readProjectTreeState(throwing), DEFAULT_PROJECT_TREE_STATE);
  assert.equal(writeProjectTreeState(DEFAULT_PROJECT_TREE_STATE, throwing), false);
  assert.deepEqual(readProjectTreeState(null), DEFAULT_PROJECT_TREE_STATE);
});

test("bad fields fall back one by one", () => {
  const storage = memoryStorage({
    [PROJECT_TREE_STORAGE_KEY]: JSON.stringify({
      version: 1,
      view: "grid",
      collapsed: ["project:1", 4, "project:1", "workspace:w"],
      expanded: "agents:w",
    }),
  });
  assert.deepEqual(readProjectTreeState(storage), {
    version: 1,
    collapsed: ["project:1", "workspace:w"],
    expanded: [],
    groups: [],
    projectGroups: {},
    projectOrder: [],
  });
});

test("a stored Recent view of an older build reads without errors, and the next write drops it", () => {
  // Version 1 stored the Recent view. The sidebar always shows the Projects
  // tree now (ADR 0020).
  const storage = memoryStorage({
    [PROJECT_TREE_STORAGE_KEY]: JSON.stringify({
      version: 1,
      view: "recent",
      recentList: "legacy",
      collapsed: ["project:2"],
      expanded: [],
    }),
  });
  const state = readProjectTreeState(storage);
  assert.deepEqual(state, {
    version: 1,
    collapsed: ["project:2"],
    expanded: [],
    groups: [],
    projectGroups: {},
    projectOrder: [],
  });
  assert.equal("view" in state, false);
  assert.equal("recentList" in state, false);
  assert.equal(writeProjectTreeState(withRowCollapsed(state, "project:3", true), storage), true);
  const written = JSON.parse(storage.values.get(PROJECT_TREE_STORAGE_KEY) ?? "null");
  assert.equal("view" in written, false);
  assert.equal("recentList" in written, false);
});

test("collapse changes round-trip", () => {
  const storage = memoryStorage();
  const state = withRowCollapsed(DEFAULT_PROJECT_TREE_STATE, "project:2", true);
  assert.equal(withRowCollapsed(state, "project:2", true), state);
  assert.equal(writeProjectTreeState(state, storage), true);
  assert.deepEqual(readProjectTreeState(storage), {
    version: 1,
    collapsed: ["project:2"],
    expanded: [],
    groups: [],
    projectGroups: {},
    projectOrder: [],
  });
  assert.deepEqual(withRowCollapsed(state, "project:2", false).collapsed, []);
});

test("an agents pill starts folded and keeps its open state", () => {
  let state = withRowExpanded(DEFAULT_PROJECT_TREE_STATE, "agents:wt-1", true);
  assert.deepEqual(state.expanded, ["agents:wt-1"]);
  assert.equal(withRowExpanded(state, "agents:wt-1", true), state);
  state = withRowExpanded(state, "agents:wt-1", false);
  assert.deepEqual(state.expanded, []);
});

test("one store serves every reader, and writes through", () => {
  const storage = memoryStorage();
  const store = createProjectTreeStateStore(() => storage);
  let calls = 0;
  const unsubscribe = store.subscribe(() => {
    calls += 1;
  });
  store.update((state) => withRowCollapsed(state, "project:4", true));
  assert.deepEqual(store.getSnapshot().collapsed, ["project:4"]);
  assert.equal(calls, 1);
  store.update((state) => state);
  assert.equal(calls, 1, "no change, no notice");
  unsubscribe();
  assert.deepEqual(readProjectTreeState(storage).collapsed, ["project:4"]);
});

test("groups read back field by field, and a project of a missing group is ungrouped", () => {
  const storage = memoryStorage({
    [PROJECT_TREE_STORAGE_KEY]: JSON.stringify({
      version: 1,
      view: "projects",
      collapsed: [],
      expanded: [],
      groups: [{ id: "g1", name: "halv" }, { id: "g1", name: "copy" }, { id: 7 }, null],
      projectGroups: { "1": "g1", "2": "gone", "3": 4 },
      projectOrder: [2, 1, 2, "x", 1.5],
    }),
  });
  const state = readProjectTreeState(storage);
  assert.deepEqual(state.groups, [{ id: "g1", name: "halv" }]);
  assert.deepEqual(state.projectGroups, { "1": "g1" });
  assert.deepEqual(state.projectOrder, [2, 1]);
});

test("a group from a project, a rename, a move, and a delete that ungroups", () => {
  let state = withNewGroup(DEFAULT_PROJECT_TREE_STATE, { id: "g1", name: "  halv  " }, 1);
  assert.deepEqual(state.groups, [{ id: "g1", name: "halv" }]);
  assert.deepEqual(state.projectGroups, { "1": "g1" });
  assert.equal(withNewGroup(state, { id: "g2", name: "  " }), state, "a group needs a name");
  assert.equal(withNewGroup(state, { id: "g1", name: "again" }), state, "ids are unique");

  state = withGroupRenamed(state, "g1", "Halv apps");
  assert.equal(state.groups[0].name, "Halv apps");
  assert.equal(withGroupRenamed(state, "g1", ""), state);

  state = withNewGroup(state, { id: "g2", name: "clients" });
  state = withProjectInGroup(state, 2, "g2");
  assert.equal(withProjectInGroup(state, 3, "missing"), state, "no move into a missing group");
  state = withRowCollapsed(state, "group:g1", true);
  state = withGroupDeleted(state, "g1");
  assert.deepEqual(state.groups, [{ id: "g2", name: "clients" }]);
  assert.deepEqual(state.projectGroups, { "2": "g2" }, "project 1 is ungrouped, not gone");
  assert.deepEqual(state.collapsed, [], "the collapse mark of the group goes too");
  assert.deepEqual(withProjectInGroup(state, 2, null).projectGroups, {});
});

test("a dragged project joins the group of its target and takes its place", () => {
  let state = withNewGroup(DEFAULT_PROJECT_TREE_STATE, { id: "g1", name: "halv" }, 1);
  state = withProjectInGroup(state, 2, "g1");
  const order = [1, 2, 3, 4];
  // Project 4 drops before project 2: it joins the group, between 1 and 2.
  let moved = withProjectMoved(state, { projectId: 4, groupId: "g1", beforeProjectId: 2, order });
  assert.deepEqual(moved.projectOrder, [1, 4, 2, 3]);
  assert.equal(moved.projectGroups["4"], "g1");
  // A drop on the group header goes to the end of the group.
  moved = withProjectMoved(state, { projectId: 3, groupId: "g1", beforeProjectId: null, order });
  assert.deepEqual(moved.projectOrder, [1, 2, 3, 4]);
  assert.equal(moved.projectGroups["3"], "g1");
  // A drop among the ungrouped projects leaves the group.
  moved = withProjectMoved(state, { projectId: 1, groupId: null, beforeProjectId: 4, order });
  assert.deepEqual(moved.projectOrder, [2, 3, 1, 4]);
  assert.equal(moved.projectGroups["1"], undefined);
  // A drop on itself changes nothing.
  assert.equal(
    withProjectMoved(state, { projectId: 2, groupId: "g1", beforeProjectId: 2, order }),
    state,
  );
});

test("many rows collapse in one change", () => {
  const state = withRowsCollapsed(DEFAULT_PROJECT_TREE_STATE, ["workspace:a", "workspace:b"], true);
  assert.deepEqual(state.collapsed, ["workspace:a", "workspace:b"]);
  assert.deepEqual(withRowsCollapsed(state, ["workspace:a"], false).collapsed, ["workspace:b"]);
});
