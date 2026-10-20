# ADR 0010: Workspace tabs

Status: Accepted

Superseded in part by ADR 0020: the Recent view, the "Other terminals" list and the draft tab of an empty strip in Pro mode are gone. Terminal adoption is a new source of tabs (T4). The Run button moved to the strip.

## Context

Halv opens work through one flat conversation list and one header switcher for the workspaces of one project.
In Pro mode, an agent conversation shows a terminal panel with its own split panes.
Open work has no visible home: a user cannot see which conversations run in which worktree.
To open a conversation is slow and unstable: a half-open terminal shows first, and the layout jumps when the messages load.

Orca opens work in a different shape.
A sidebar tree lists projects, their worktrees, and the agents of each worktree.
Each worktree has one tab strip, and each tab is one terminal with its own split panes.

In August 2026, Halv had terminal tabs inside each panel.
Render passes, refreshes and session events added and removed tabs, so tabs doubled and came back after a close (`e584219`).
Halv removed the strip and kept one split tree per panel.

Pedro chose Orca's shape for Halv.
The work spec is `workspace-spec` (2026-10-14). It lands in eleven stages, each behind a flag when it is visible.

## Decision

### Terms

| Term | Meaning |
|---|---|
| workspace | The primary checkout (`primary-<projectId>`) or a Halv-managed worktree. Orca calls it a worktree. |
| strip | The ordered tabs of one workspace. A strip is a view. Halv does not store it. |
| tab | One open owner in one strip. |
| owner | The record that owns a set of terminal sessions: a conversation, or a shell tab row. |
| tab key | `conversation:<conversationId>` or `shell:<uuid>`. The tab key is also the panel scope. |
| home | The workspace whose strip shows a tab. |
| park | Keep the resume point of each agent pane of a closed tab, with no scrollback. |

### A tab is an open owner

- A conversation tab has the key `conversation:<conversationId>`. Its body is the conversation in chat mode or in agent mode.
- A shell tab has the key `shell:<uuid>`. It owns plain shells only. Cmd+T opens one (stage S5b).
- The draft tab is the "New task" entry while the user writes a new task. Halv never stores it.
- Chat-mode conversations are tabs too. The conversation mode selects the tab body. It never decides membership, so the Pro toggle never removes the visible tab.
- An agent session id or a terminal id is never a tab key. Both change on resume, split and continuation, and an identity change caused the August duplicates.
- Panes are never tabs. A panel keeps one split tree.

A conversation can have a tab when it is not archived, it has a project, and its home workspace exists.
A conversation without a project, and an archived conversation, open in the loose view without a strip.

### The home is derived

The home of a conversation tab is `conversations.workspace_id`, else `primary-<projectId>`.
`effectiveWorkspaceId` in `src/conversationWorkspace.ts` is the only implementation of this rule.
The project tree places a conversation row with the same function, so the tree and the strip cannot disagree.
Halv never stores the home of a conversation tab, so a tab cannot be in two strips.
A worktree handoff moves the tab with no tab write.

### Where state lives

| State | Home | Writer |
|---|---|---|
| Open tabs, order, activation time, shell tab titles | SQLite `workspace_tabs` | Rust `workspace_tabs_apply`, fed by one serial frontend queue |
| Active tab of each workspace | SQLite `workspace_tab_state` | the same batch as the intent that changes it |
| Seed marker | SQLite `workspace_tab_seed` | Rust `workspace_tabs_seed` |
| Home of a conversation tab | derived from `conversations.workspace_id` | none |
| Panel layout (split tree, active pane) | localStorage `halv.terminal-layout.v1:<scope>` and `terminal-state-v1.json` | `TerminalPanel`, format unchanged |
| Session owner | `TerminalSession.conversation_id` and a new `TerminalSession.shell_tab_id` | Rust at spawn |
| Navigation | localStorage `halv.navigation.v1` | App `navigate()` |
| Current conversation | derived from navigation and the tab store | none |
| Pane status | Rust agent-status service (ADR 0011) | Rust |
| Tree view, collapse and expansion | localStorage `halv.sidebar.project-tree.v1` | the sidebar |
| Feature flags | localStorage `halv.feature-flags.v1` | the Labs commands of the command palette |

SQLite holds the tabs because its constraints make a duplicate tab impossible at rest.
A primary key, a UNIQUE conversation column and a CHECK hold the tab rows.
Foreign keys remove the tabs of deleted conversations and workspaces, also in 0.7.1.
One transaction holds one open or one close.
Rust can read the tabs for the seed, the cleanup at start, and the remote gateway of ADR 0009.
The tab tables come with migration `0054_workspace_tabs` (stage S2).
The migration runner checks the version row of each migration, not a highest number.
So a migration with a lower number that lands later (`0053_remote_devices`) still applies on a database that has 0054.

### Writes

- Only explicit intents write tab rows: open, activate, close, move, rename, and the one-time seed.
- Render, refresh, session events, loading and reconcile passes never write a tab row.
- Nothing removes a tab because its panes or sessions are absent.
- One serial queue carries all tab IPC, reads and writes. A failed batch drops alone.
- The insert uses `INSERT … ON CONFLICT DO NOTHING` with no target. `INSERT OR IGNORE` is forbidden, because it also drops a CHECK violation in silence.
- `setActive` is a guarded no-op when its workspace or its tab is gone, so a close never fails on a stale successor.
- At start, Rust deletes the tab rows of archived and project-less conversations. The predicate never joins `workspaces`.

### Invariants

| # | Invariant |
|---|---|
| T1 | A tab key exists at most once. |
| T2 | A conversation has at most one tab. |
| T3 | A tab shows in exactly one strip. |
| T4 | Only explicit intents and the one-time seed create tabs. |
| T5 | No tab disappears because panes or sessions are absent. |
| T6 | A closed tab stays closed. |
| T7 | The active pointer is valid: the stored tab, else the most recent tab, else the first tab. |
| T8 | A close of the active tab activates the most recent survivor, else the right neighbor, else the left neighbor. |
| T9 | A tab leaves the strip only after its processes stopped and its park state is written. |
| T10 | One session shows in at most one panel. |
| T11 | One scope has at most one mounted panel. |
| T12 | Panes are never tabs. |
| T13 | The current conversation equals the conversation of the visible tab, or of the loose view. |
| T14 | Keys never reach a hidden surface. |
| T15 | Memory and disk converge. |
| T16 | The seed runs once per install. |

### Close and park

A tab close follows Orca, with one Halv addition:

1. The draft tab only leaves the compose state.
2. Halv asks once for the whole tab, only when a pane is busy. An agent is busy while it works, needs input or needs approval. A shell is busy while a foreground job runs.
3. An idle agent closes with no prompt, because the close parks it.
4. The panel enters a closing guard: no refresh, no persistence and no auto-resume.
5. Halv builds the park payload. Each agent pane with a CLI session id stays as a saved pane with its agent kind, lane, credential profile, CLI session id, cwd and titles. The payload carries no scrollback. Storage keeps the last scrollback that it saved for the session, so a parked pane shows its last screen until it resumes. Shell panes drop.
6. Halv stops every session of the tab, then writes the park payload as the last write.
7. On success, the tab row goes and the successor shows. On failure, the tab stays and shows the error.

A later open of the conversation resumes the parked agents once with `--resume`.
A chat-mode tab close stops its owned shells. It never cancels a chat stream.

### Mounting

The visible tab is mounted. A tab whose owner has a running session stays mounted.
Hidden bodies keep `visibility:hidden` and get `inert`, so no key reaches them.
A switch between mounted bodies remounts nothing.
A tab body has no panel header. Its pane title bars are its only header, as in Orca (S7).
The savings meter of the visible agent tab shows in the app header, in a slot with a fixed width.
A `display:none` body with a mount cap comes later, only after a measurement.

One mounted panel holds one scope. A scope is the panel storage key, for example `halv.terminal-layout.v1:conversation:<id>`. The tab bodies and the bottom panel claim their scope in a registry. Only the holder restores, refreshes, persists and resumes. A second panel waits.
The Agent sessions panel uses the same key for an agent of a conversation, because the key takes the conversation of the agent. So that panel shows "Open in tab" for such an agent and mounts no second terminal.

### Keyboard

| Action | macOS | Windows and Linux | Stage |
|---|---|---|---|
| Select tab 1 to 8; tab 9 selects the last tab | ⌘1–⌘9 | Ctrl+1–Ctrl+9 | S4a |
| Previous tab, next tab | ⇧⌘[, ⇧⌘] | Ctrl+Shift+[, Ctrl+Shift+] | S4a |
| New agent tab | ⌥⌘T | none | S4a |
| New shell tab | ⌘T | Ctrl+Shift+T | S5b |
| Close the pane, else the tab | ⌘W | Ctrl+Shift+W | S5b |
| Reopen the closed tab | ⇧⌘T | none | S7 |
| Rename the tab | ⌘R | none | S7 |
| Recent tab switcher (hold Ctrl) | ⌃Tab, ⌃⇧Tab | Ctrl+Tab, Ctrl+Shift+Tab | S7 |
| Previous workspace, next workspace | ⌥⌘↑, ⌥⌘↓ | Ctrl+Shift+↑, Ctrl+Shift+↓ | S7 |

One window capture listener owns the tab chords before xterm.
It ignores events during IME composition, in dialogs, in menus and in the rename input.
Ctrl+T, Ctrl+W and Ctrl+R stay with the CLIs on Windows and Linux.
The native "Close Window" item moves to ⇧⌘W, so ⌘W reaches the page.
With nothing to close, ⌘W hides the window as today.

### Sidebar

ADR 0020 removes the Recent legacy view below and the "Other terminals" list. A terminal without a tab shows in the Chats area, and adoption moves it into a tab when it can.

The default sidebar view becomes a Projects tree: projects, workspaces, and agent rows.
The time-bucketed Recent list becomes a legacy view (Pedro, 2026-10-15):
- The first conversation list load decides once if the viewer is a legacy viewer.
- A viewer with conversations at that load gets a "Projects | Recent" toggle. Projects is the default.
- A new viewer gets no toggle and never sees the Recent list. Search finds every conversation.
- A viewer with no project sees an empty state with "Add folder..." and "Clone from GitHub...".
A workspace row shows its branch, a "primary" badge for the project root, and the strongest state of its rows.
An agent row shows the state glyph of ADR 0011, the agent icon, the title and a short age ("now", "18m", "3h", "5d").
The tree, the strip and the chips use Orca's state vocabulary (2026-10-15):
- A yellow ring turns while the agent works. It turns once per second in 12 steps, and every ring turns in phase.
- An orange question bubble shows while the agent waits for the user.
- A green check shows for a finished turn. It stays after a view, until the next turn starts. Only an unread turn makes the title bold.
- An amber dashed ring shows for a live agent pane that sends no signal: no hook, no presence file, no rollout, no output yet.
- A red dot shows a failure. A muted dot shows a stop or an idle pane.
- A workspace or project lane summarizes its rows: a finished turn there is a green dot, as on Orca's worktree card.
- Two or more rows of a workspace fold into Orca's summary chips: one chip per state in rank order, at most three, each with the agent icons and "+N".
The pinned rail, the Recent list and the panel header keep the news glyph: they show only news.
Plain shells are not agent rows.
Before the strip exists, the rows are the conversations of the workspace.
After the strip exists, the rows are the open tabs of the workspace, then one dimmed row for each live conversation without a tab.

`workspace_overview` gives the tree all workspaces of all projects.
It runs one `git rev-parse --abbrev-ref HEAD` per workspace, in parallel, with a timeout.
It runs no `git status`.

### Flags

`src/featureFlags.ts` holds three flags: `projectTree`, `workspaceTabs` and `shellTabs`.
A flag is off by default until its stage ships. `projectTree` is on by default since 2026-10-15.
`workspaceTabs` and `shellTabs` are on by default from release 0.8.0. Their flags stay as an off switch.
Three Labs commands of the command palette turn them on or off.
The store keeps a flag that a viewer turned off, also when its default is on.
The UX audit harness starts with every flag off unless `?flags=` names them, so the baselines stay stable.
A flag goes on by default after one week in Pedro's daily build with no bug of its class.
Stage S7 removes the flags that are on by default.

## Consequences

- Every saved panel layout uses the scope `conversation:<id>` already, so it becomes the layout of its tab with no migration.
- A 0.7.1 build ignores the new tables. Every new foreign key is child-side `CASCADE` or `SET NULL`, so no 0.7.1 statement can fail.
- Tab growth is a risk: every Recent click opens a tab. Middle-click and "Close others" come in S7.
- A park cannot always resume, because a CLI can lose its session file. The resume failure path then offers "Start fresh".
- New logic goes into new modules. Hot files such as `src/App.tsx` and `src/components/TerminalPanel.tsx` get short insertions only.
- A fresh Pro agent now starts in the root of the conversation workspace, not in the project root (`agentLaunchCwd`).
