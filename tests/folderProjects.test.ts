import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import { importNestedRepos, scanFolderProjectsOnce } from "../src/folderProjects.ts";
// @ts-expect-error Node's test runner needs the source extension.
import { buildProjectTree } from "../src/projectTree.ts";
// @ts-expect-error Node's test runner needs the source extension.
import {
  DEFAULT_PROJECT_TREE_STATE,
  PROJECT_TREE_STORAGE_KEY,
  folderWasScanned,
  readProjectTreeState,
  withFolderScanGroup,
  withNewGroup,
  withProjectInGroup,
  writeProjectTreeState,
  type ProjectTreeState,
} from "../src/projectTreeState.ts";
// @ts-expect-error Node's test runner needs the source extension.
import type { NestedRepoScan, Project } from "../src/types.ts";

// Folder projects with nested git repositories (ADR 0019): the automatic
// group, the one-time scan, and the folder row first in its group.

const CREATED = "2026-10-16T12:00:00.000Z";

function project(id: number, rootPath: string): Project {
  return {
    id,
    name: rootPath.split("/").pop() ?? rootPath,
    rootPath,
    createdAt: new Date(Date.parse(CREATED) + id * 1_000).toISOString(),
    lastOpenedAt: CREATED,
  };
}

/** A fake backend: a project list, the scan results, and a tree state. */
function backend(options: {
  projects: Project[];
  folders: Record<string, string[]>;
  failingPaths?: string[];
  failingScans?: string[];
  state?: ProjectTreeState;
}) {
  const projects = [...options.projects];
  let nextId = Math.max(0, ...projects.map((item) => item.id)) + 1;
  let state = options.state ?? DEFAULT_PROJECT_TREE_STATE;
  let groupCount = 0;
  const scans: string[] = [];
  const deps = {
    scan: async (path: string): Promise<NestedRepoScan> => {
      scans.push(path);
      if (options.failingScans?.includes(path)) throw new Error("folder is gone");
      const repos = options.folders[path];
      return { path, gitRepo: repos === undefined, repos: repos ?? [], truncated: false };
    },
    addLocal: async (path: string) => {
      if (options.failingPaths?.includes(path)) throw new Error("cannot add");
      const existing = projects.find((item) => item.rootPath === path);
      if (existing) return existing;
      const added = project(nextId++, path);
      projects.push(added);
      return added;
    },
    updateState: (change: (current: ProjectTreeState) => ProjectTreeState) => {
      state = change(state);
    },
    newGroupId: () => `g${++groupCount}`,
  };
  return {
    deps,
    projects,
    scans,
    state: () => state,
    remove(id: number) {
      projects.splice(
        projects.findIndex((item) => item.id === id),
        1,
      );
    },
  };
}

const HALV = "/u/programming/halv";
const folders = { [HALV]: [`${HALV}/app`, `${HALV}/web`] };

test("a folder with repositories adds each one and groups them under the folder", async () => {
  const folder = project(1, HALV);
  const fake = backend({ projects: [folder], folders });

  const outcome = await importNestedRepos(folder, [folder], fake.deps);

  assert.equal(outcome.gitRepo, false);
  assert.deepEqual(
    outcome.added.map((item) => item.rootPath),
    [`${HALV}/app`, `${HALV}/web`],
  );
  assert.deepEqual(outcome.repoProjectIds, [2, 3]);
  const state = fake.state();
  assert.deepEqual(state.groups, [
    { id: "g1", name: "halv", createdFrom: "folder-scan", folderProjectId: 1, parentPath: HALV },
  ]);
  assert.deepEqual(state.projectGroups, { "1": "g1", "2": "g1", "3": "g1" });
  assert.deepEqual(state.scannedFolders, [HALV, `${HALV}/app`, `${HALV}/web`]);
});

test("a second scan of the same folder adds nothing twice", async () => {
  const folder = project(1, HALV);
  const fake = backend({ projects: [folder], folders });
  await importNestedRepos(folder, [folder], fake.deps);
  const first = fake.state();

  const again = await importNestedRepos(folder, [...fake.projects], fake.deps);

  assert.deepEqual(again.added, []);
  assert.deepEqual(again.repoProjectIds, [2, 3]);
  assert.equal(fake.projects.length, 3);
  assert.equal(fake.state(), first, "the state does not change");
});

test("a git repository and a folder without repositories stay single projects", async () => {
  const repo = project(1, "/u/programming/solo");
  const plain = project(2, "/u/notes");
  const fake = backend({ projects: [repo, plain], folders: { "/u/notes": [] } });

  const repoOutcome = await importNestedRepos(repo, fake.projects, fake.deps);
  const plainOutcome = await importNestedRepos(plain, fake.projects, fake.deps);

  assert.equal(repoOutcome.gitRepo, true);
  assert.equal(plainOutcome.gitRepo, false);
  assert.deepEqual(fake.state().groups, []);
  assert.deepEqual(fake.state().projectGroups, {});
  assert.ok(folderWasScanned(fake.state(), repo.rootPath));
  assert.ok(folderWasScanned(fake.state(), plain.rootPath));
});

test("known repositories join the group only when they have no group", async () => {
  const folder = project(1, HALV);
  const app = project(2, `${HALV}/app`);
  const web = project(3, `${HALV}/web`);
  let state = withNewGroup(DEFAULT_PROJECT_TREE_STATE, { id: "work", name: "Work" }, 3);
  const fake = backend({ projects: [folder, app, web], folders, state });

  const outcome = await importNestedRepos(folder, fake.projects, fake.deps);

  assert.deepEqual(outcome.added, [], "no repository is added twice");
  state = fake.state();
  assert.deepEqual(
    state.groups.map((group) => group.id),
    ["work", "g1"],
  );
  assert.deepEqual(state.projectGroups, { "3": "work", "1": "g1", "2": "g1" });
});

test("no group when every repository already has a group", async () => {
  const folder = project(1, HALV);
  const app = project(2, `${HALV}/app`);
  let state = withNewGroup(DEFAULT_PROJECT_TREE_STATE, { id: "work", name: "Work" }, 2);
  const fake = backend({
    projects: [folder, app],
    folders: { [HALV]: [`${HALV}/app`] },
    state,
  });

  await importNestedRepos(folder, fake.projects, fake.deps);

  state = fake.state();
  assert.deepEqual(state.groups.map((group) => group.id), ["work"]);
  assert.equal(state.projectGroups["1"], undefined, "the folder stays ungrouped");
  assert.ok(folderWasScanned(state, HALV));
});

test("a repository that fails to add stays out, and the others go in", async () => {
  const folder = project(1, HALV);
  const fake = backend({ projects: [folder], folders, failingPaths: [`${HALV}/app`] });

  const outcome = await importNestedRepos(folder, [folder], fake.deps);

  assert.deepEqual(outcome.added.map((item) => item.rootPath), [`${HALV}/web`]);
  assert.deepEqual(fake.state().projectGroups, { "1": "g1", "2": "g1" });
});

test("the one-time scan runs once per folder and respects removed projects", async () => {
  const folder = project(1, HALV);
  const solo = project(2, "/u/programming/solo");
  const fake = backend({ projects: [folder, solo], folders });

  const added = await scanFolderProjectsOnce(fake.projects, fake.state, fake.deps);
  assert.deepEqual(added.map((item) => item.rootPath), [`${HALV}/app`, `${HALV}/web`]);
  assert.deepEqual(fake.scans, [HALV, "/u/programming/solo"]);

  // The user removes the web project. A later start does not bring it back,
  // and it scans nothing again.
  const web = fake.projects.find((item) => item.rootPath === `${HALV}/web`)!;
  fake.remove(web.id);
  const later = await scanFolderProjectsOnce([...fake.projects], fake.state, fake.deps);
  assert.deepEqual(later, []);
  assert.deepEqual(fake.scans, [HALV, "/u/programming/solo"]);
  assert.equal(fake.projects.some((item) => item.rootPath === `${HALV}/web`), false);
});

test("the one-time scan does not bring back a removed repository of a folder", async () => {
  const folder = project(1, HALV);
  const web = project(2, `${HALV}/web`);
  // The folder is not available at the first start, so only the web
  // repository gets its scan mark.
  const fake = backend({ projects: [folder, web], folders, failingScans: [HALV] });
  await scanFolderProjectsOnce(fake.projects, fake.state, fake.deps);
  assert.ok(folderWasScanned(fake.state(), `${HALV}/web`));
  assert.equal(folderWasScanned(fake.state(), HALV), false);

  // The user removes the web project. The folder scan then works.
  fake.remove(web.id);
  const ready = backend({ projects: [...fake.projects], folders, state: fake.state() });
  const added = await scanFolderProjectsOnce(ready.projects, ready.state, ready.deps);

  assert.deepEqual(added.map((item) => item.rootPath), [`${HALV}/app`]);
  assert.equal(ready.projects.some((item) => item.rootPath === `${HALV}/web`), false);
  assert.ok(folderWasScanned(ready.state(), HALV));
});

test("an added folder brings back a removed repository, because the user asked", async () => {
  const folder = project(1, HALV);
  const state = { ...DEFAULT_PROJECT_TREE_STATE, scannedFolders: [`${HALV}/web`] };
  const fake = backend({ projects: [folder], folders, state });

  const outcome = await importNestedRepos(folder, [folder], fake.deps);

  assert.deepEqual(
    outcome.added.map((item) => item.rootPath),
    [`${HALV}/app`, `${HALV}/web`],
  );
});

test("the one-time scan gives a repository to the nearest folder", async () => {
  const outer = project(1, "/u/programming");
  const inner = project(2, HALV);
  const fake = backend({
    projects: [outer, inner],
    folders: {
      "/u/programming": [`${HALV}/app`, `${HALV}/web`, "/u/programming/tools"],
      ...folders,
    },
  });

  await scanFolderProjectsOnce(fake.projects, fake.state, fake.deps);

  const state = fake.state();
  assert.deepEqual(
    state.groups.map((group) => [group.id, group.name, group.folderProjectId]),
    [
      ["g1", "halv", 2],
      ["g2", "programming", 1],
    ],
  );
  const groupOf = (path: string) =>
    state.projectGroups[String(fake.projects.find((item) => item.rootPath === path)!.id)];
  assert.equal(groupOf(`${HALV}/app`), "g1");
  assert.equal(groupOf(`${HALV}/web`), "g1");
  assert.equal(groupOf("/u/programming/tools"), "g2");
  assert.equal(groupOf(HALV), "g1");
});

test("a failed scan leaves its folder for the next start", async () => {
  const folder = project(1, HALV);
  const fake = backend({ projects: [folder], folders, failingScans: [HALV] });

  assert.deepEqual(await scanFolderProjectsOnce(fake.projects, fake.state, fake.deps), []);
  assert.equal(folderWasScanned(fake.state(), HALV), false);
});

test("folder groups and scanned roots read back, and old state reads as before", () => {
  const values = new Map<string, string>();
  const storage = {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => {
      values.set(key, value);
    },
  };
  const state = withFolderScanGroup(DEFAULT_PROJECT_TREE_STATE, {
    folder: { id: 1, name: "halv", rootPath: HALV },
    repos: [{ id: 2, rootPath: `${HALV}/app` }],
    groupId: "g1",
  });
  writeProjectTreeState(state, storage);
  assert.deepEqual(readProjectTreeState(storage), state);

  values.set(
    PROJECT_TREE_STORAGE_KEY,
    JSON.stringify({
      version: 1,
      groups: [
        { id: "a", name: "manual" },
        { id: "b", name: "bad", createdFrom: "folder-scan", folderProjectId: "1", parentPath: 4 },
        { id: "c", name: "odd", createdFrom: "elsewhere", folderProjectId: 3 },
      ],
      scannedFolders: [HALV, 7, "", HALV],
    }),
  );
  const read = readProjectTreeState(storage);
  assert.deepEqual(read.groups, [
    { id: "a", name: "manual" },
    { id: "b", name: "bad", createdFrom: "folder-scan" },
    { id: "c", name: "odd" },
  ]);
  assert.deepEqual(read.scannedFolders, [HALV]);
  // State from before ADR 0019 has no scanned roots.
  values.set(PROJECT_TREE_STORAGE_KEY, JSON.stringify({ version: 1, groups: [] }));
  assert.equal("scannedFolders" in readProjectTreeState(storage), false);
});

test("the folder project comes first in its group with a folder mark", () => {
  // The folder project is the newest, so only the folder rule puts it first.
  const projects = [project(2, `${HALV}/app`), project(3, `${HALV}/web`), project(9, HALV)];
  const state = withFolderScanGroup(DEFAULT_PROJECT_TREE_STATE, {
    folder: { id: 9, name: "halv", rootPath: HALV },
    repos: [
      { id: 2, rootPath: `${HALV}/app` },
      { id: 3, rootPath: `${HALV}/web` },
    ],
    groupId: "g1",
  });
  const build = (organization: ProjectTreeState) =>
    buildProjectTree({
      projects,
      workspaces: [],
      conversations: [],
      terminals: [],
      runningConversationIds: new Set<string>(),
      unviewedConversationIds: new Set<string>(),
      maxRowsPerWorkspace: 5,
      organization,
    });

  const tree = build(state);
  assert.deepEqual(
    tree.groups[0].projects.map((node) => [node.project.id, node.groupFolder ?? false]),
    [
      [9, true],
      [2, false],
      [3, false],
    ],
  );
  // Out of its group, the folder is a plain project again.
  const moved = build(withProjectInGroup(state, 9, null));
  assert.deepEqual(moved.groups[0].projects.map((node) => node.project.id), [2, 3]);
  assert.equal(moved.ungrouped[0].groupFolder, undefined);
});
