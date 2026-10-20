# ADR 0019: Folder projects with nested git repositories

Status: Accepted

## Context

A user often keeps the repositories of one product in one folder.
For example, `~/code/acme` is not a git repository. It holds the repositories `app/` and `web/`.
Before this ADR, Halv added such a folder as one plain project.
The plain project has no worktrees, and its repositories do not show in the tree.

The maintainer asked on 2026-10-16 for 0.8.0: "os projetos tem q abrir todos git dentro dele, tipo o halv abre o app e web, e etc.., igual no orca."
The maintainer decided that Halv groups the repositories automatically, with no dialog.

Orca scans a folder for nested repositories (`main/project-groups/nested-repo-discovery.ts`, `nested-repo-scan-rules.ts`).
Orca then imports the repositories as a project group (`nested-repo-import.ts`).
Project groups exist in Halv since ADR 0010, stage S7 (`src/projectTreeState.ts`).

## Decision

### The scan

The Rust command `project_scan_nested_repos` scans one folder (`src-tauri/src/projects/nested_repos.rs`).
The scan uses the rules of Orca:

- When the folder is a git repository or is inside one, the scan stops. The result has `gitRepo: true` and no repositories.
- The walk goes breadth first. It reads the children of each folder in name order. Case counts only to break a tie.
- A repository is a directory with a `.git` directory or `.git` file, or a bare repository (`HEAD` file, `objects` and `refs` directories).
- The walk does not look inside a repository that it found. Thus a submodule or a nested checkout is not a project.
- The scan skips a linked worktree: a `.git` file that points into the `worktrees` folder of another repository. A linked worktree belongs to its main repository, so it is not a project of its own. The walk does not look inside it. Orca accepts a linked worktree as a repository; Halv does not.
- The walk does not follow a symbolic link.
- The deepest repository is at level 3. A child of the folder has level 1. Orca reads the folders at level 3 and checks their children, so its deepest repository is at level 4. Halv stops one level higher on purpose: a reference checkout deep in a work folder does not become a project.
- The scan returns at most 100 repositories. The result then has `truncated: true`.
- The walk skips `node_modules`, `.next`, `dist`, `build`, `.cache`, `vendor`, `__pycache__`, `.turbo` and `.parcel-cache`.
- The walk skips the metadata directories of version control systems: `.git`, `.svn`, `.hg`, `.jj`, `.sl`, `.repo` and `CVS`.
- The walk skips a hidden directory below level 1. A hidden directory at level 1 is a candidate, as in Orca.
- The walk applies the `.gitignore` file of each folder that it reads, to the folders below it. Halv supports the Orca subset: `*`, `?`, `**`, a leading `/`, a trailing `/`, `!` and `#`.

Halv adds these limits that Orca does not have:

- The walk stops after 50,000 directory entries, and the result then has `truncated: true`.
- The scan does not read a `.gitignore` file that is larger than 256 KiB.
- The home folder and each folder above it give no repositories. A scan of the home folder finds the tool repositories in hidden folders, for example `~/.nvm` and `~/.oh-my-zsh`.
- When Halv cannot read the folder itself, the command returns an error. For example, macOS can refuse the folder before the user gives permission. The one-time scan then tries again at the next start.

The review scan (`project_git_repos_under_path`) uses the same walk with its own rules.
It still skips every hidden directory and `node_modules`, reads no `.gitignore` file, and accepts no bare repository.

### The add flow

When the user adds a local folder, Halv adds the folder as a project first, as before.
Then Halv scans the folder (`importNestedRepos` in `src/folderProjects.ts`):

- When the folder is a git repository, the folder stays one project. Halv ignores nested repositories.
- When the scan finds no repository, the folder stays one plain project.
- When the scan finds repositories, Halv adds each repository that is not a project yet. Then Halv makes a project group, named after the folder.

A repository that fails to add stays out. The other repositories still go in.
A failed scan keeps the folder project and shows no error.
The add flow and the one-time scan run only when the Projects tree flag is on.

### The group

The group is a normal project group with three more fields:

- `createdFrom: "folder-scan"`. A group without the field is a manual group.
- `folderProjectId`: the id of the folder project.
- `parentPath`: the root path of the folder project.

The group holds the folder project and each repository project that has no group yet.
A project in another group stays in that group.
When no repository can go into the group, Halv makes no group.
A second scan of the same folder uses the same group, so it adds nothing twice.

The user can rename, delete and reorder the group as any other group.
The group only orders the sidebar. Execution stays per project and per workspace (ADR 0010).

Halv does not make sub-groups.
Orca makes a sub-group for a folder between the parent and its repositories when that folder holds two or more repositories.
Halv puts all repositories of the folder into one flat group.

### The folder project

The folder project stays a project. It is the folder workspace of the group, as in Orca.
Its terminals and agents start in the folder, so one agent can work across all repositories.
It has no "+" button, because a plain folder has no worktrees.

In the tree, the folder project is the first row under the group header (Orca's order: the folder workspace, then the repositories).
Its row shows a folder icon instead of the avatar, and a "folder" badge.
When the user moves the folder project out of its group, it is a plain project again.

### The one-time scan

Projects from before this ADR get the same scan once per viewer.
After the first project list loads, Halv scans each project whose root is not marked as scanned.
The deepest root goes first, so a repository goes to the nearest folder project that holds it.
Terminal adoption (ADR 0020) waits until the one-time scan ends. Thus a terminal in a repository of a folder goes to the repository project, not to the folder project.

The tree state keeps the scanned roots in `scannedFolders` (localStorage, `halv.sidebar.project-tree.v1`).
Each scan marks the folder root and the root of each repository that it found.
Thus a later start scans only new roots.
A project that the user removes after the scan does not come back.
The one-time scan does not add a found repository whose root is marked as scanned, because such a root is a project that the user removed.
An add flow that the user starts adds such a repository again, because the user asked for the folder.
When the user removes a project, Halv also takes it out of its group. SQLite can give the id of the removed project to the next project, and that project must not join the old group.
A scan that fails, for example on a removed disk, does not mark its root, so the next start tries again.

Halv reads tree state from before this ADR without change.
A group without the new fields is a manual group, and a state without `scannedFolders` has no scanned roots.

## What Halv does not do

- Halv does not watch a folder. A repository that the user adds to the folder later does not become a project. The user adds the folder again to scan it again.
- Halv does not add a submodule or a nested checkout as a project, because the walk does not look inside a repository.
- Halv does not ask before it adds the repositories. Orca shows a dialog that offers a group or separate projects.
- Halv does not make sub-groups (see "The group").
- Halv does not scan a folder again at each start. The one-time scan runs once per root.
- This ADR does not change the Run button or the quick commands of the folder project. Orca hides them in its folder workspace.

## Consequences

- A folder with many repositories adds up to 100 projects at once. A broad folder, for example `~/code`, can add many projects. The user can remove the projects or delete the group.
- A linked worktree that another tool made, for example Orca, does not show in the tree. Halv shows only the worktrees that Halv made.
- The scan runs in a blocking task of the Rust runtime. A large folder can make the add flow slow, but the window stays responsive.
- The scanned roots are per viewer, like the other tree state. A second viewer of the same projects runs the one-time scan again. The scan finds the repositories that are already projects, and it can add again a repository that the first viewer removed.
