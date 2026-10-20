// Folder projects with nested git repositories (ADR 0019). When a folder
// project is not a git repository and holds repositories, Halv adds each
// repository as a project and groups them under the folder, as Orca does.
// The add flow runs the scan for each added folder. The one-time scan runs
// it once for each project from before this change. No dialog asks first.

import {
  folderWasScanned,
  withFolderScanGroup,
  type ProjectTreeState,
} from "./projectTreeState.ts";
import type { NestedRepoScan, Project } from "./types";

export type FolderScanDeps = {
  scan: (path: string) => Promise<NestedRepoScan>;
  addLocal: (path: string) => Promise<Project>;
  updateState: (change: (state: ProjectTreeState) => ProjectTreeState) => void;
  newGroupId: () => string;
};

export type FolderScanOutcome = {
  /** The folder is a git repository or is inside one. */
  gitRepo: boolean;
  /** The repository projects that the scan added. */
  added: Project[];
  /** Every repository project of the folder, new or known, in scan order. */
  repoProjectIds: number[];
};

/**
 * Scans a folder project, adds each repository that is not a project yet,
 * and records the result in the tree state. `known` is the current project
 * list. A repository that fails to add stays out; the others still go in.
 * `skipRoot` keeps a root out that is not a project now, for example a
 * repository that the user removed.
 */
export async function importNestedRepos(
  folder: Project,
  known: readonly Project[],
  deps: FolderScanDeps,
  skipRoot: (rootPath: string) => boolean = () => false,
): Promise<FolderScanOutcome> {
  const scan = await deps.scan(folder.rootPath);
  const byRoot = new Map(known.map((project) => [project.rootPath, project]));
  const repos: Project[] = [];
  const added: Project[] = [];
  const addRepo = async (path: string) => {
    try {
      const project = await deps.addLocal(path);
      byRoot.set(project.rootPath, project);
      const isNew = ![...known, ...added].some((item) => item.id === project.id);
      if (isNew) added.push(project);
      return project;
    } catch {
      return null;
    }
  };
  if (!scan.gitRepo) {
    for (const path of scan.repos) {
      if (path === folder.rootPath) continue;
      const project = byRoot.get(path) ?? (skipRoot(path) ? null : await addRepo(path));
      if (project && !repos.some((item) => item.id === project.id)) repos.push(project);
    }
  }
  const groupId = deps.newGroupId();
  deps.updateState((state) =>
    withFolderScanGroup(state, {
      folder,
      repos: repos.map(({ id, rootPath }) => ({ id, rootPath })),
      groupId,
    }),
  );
  return {
    gitRepo: scan.gitRepo,
    added,
    repoProjectIds: repos.map((project) => project.id),
  };
}

/** The projects that the one-time scan has not seen yet, in list order. */
export function projectsAwaitingFolderScan(
  projects: readonly Project[],
  state: ProjectTreeState,
): Project[] {
  return projects.filter((project) => !folderWasScanned(state, project.rootPath));
}

/**
 * The one-time scan of the projects from before ADR 0019. It scans each
 * project that the tree state has not marked, one after the other. The
 * deepest folder goes first, so a repository goes to the nearest folder
 * project that holds it. A scan that fails leaves its project unmarked, so
 * the next start tries again. Returns the projects that the scans added.
 */
export async function scanFolderProjectsOnce(
  projects: readonly Project[],
  readState: () => ProjectTreeState,
  deps: FolderScanDeps,
): Promise<Project[]> {
  const known = [...projects];
  const added: Project[] = [];
  const awaiting = projectsAwaitingFolderScan(projects, readState()).sort(
    (left, right) => right.rootPath.length - left.rootPath.length,
  );
  for (const project of awaiting) {
    // An earlier scan of this run can mark this root, for example a
    // repository that it added.
    if (folderWasScanned(readState(), project.rootPath)) continue;
    try {
      // A scanned root that is not a project now is a project that the
      // user removed. The scan does not bring it back.
      const outcome = await importNestedRepos(project, known, deps, (root) =>
        folderWasScanned(readState(), root),
      );
      known.push(...outcome.added);
      added.push(...outcome.added);
    } catch {
      // The folder can be unavailable now, for example on a removed disk.
    }
  }
  return added;
}
