# ADR 0020: Project launcher and tabs-first sidebar

Status: Accepted

## Context

Pedro asked for these changes for release 0.8.0 (2026-10-16):

- In Pro mode, the open of a project must ask at once what to open.
- The choices are an agent (Claude Code, Codex and others), an empty terminal, or a text conversation.
- A text conversation keeps the old flow: the user picks the agent and the model in the composer.
- The terminals of the user must go to projects. The "Recent" view goes away. A terminal without a project goes to the chats.
- Each terminal that is open in the same project becomes a tab of that project.
- The Run button is out of context in the window header.

Pedro decided one more rule:

- A terminal can be in a git repository that is no project yet. Then Halv adds that repository as a project, and the terminal becomes a tab there.
- A terminal outside every git repository, for example in the home folder, goes to the Chats area.

Before this change, an empty workspace strip showed the "New task" composer (ADR 0010).
A click on a project row only collapsed the row.
The sidebar had a "Projects | Recent" toggle for legacy viewers.
Terminals without a tab waited under "Other terminals".
The Run button sat in the header, also with no action.

Orca is the reference.
Its Landing page and its quick launch menu give the visual language of the launcher.
Orca puts its Quick Commands split button at the right end of the tab row (`TabGroupPanel`).

## Decision

### The launcher of an empty workspace

In Pro mode, a workspace strip with no tab shows a launcher instead of the "New task" composer.
`src/workspaceLauncher.ts` decides when it shows and what it lists.
`src/components/WorkspaceLauncher.tsx` draws it.

- The launcher lists the ready agents in the order of the "+" menu. The default agent is first.
- Then it lists "Terminal", when shell tabs are on.
- "Text conversation" is always last.
- Each choice opens one tab in that workspace. "Terminal" does the same as Cmd+T. An agent does the same as the "+" menu.
- The rows show the Option+Cmd+T hint for the default agent on macOS and the Cmd+T hint for the terminal.
- A workspace click moves the focus to the first row. The arrow keys, Home and End move between rows. Enter opens the row.
- The launcher shows only while nothing composes. With Pro mode off, the composer stays, as before.
- The close of the last tab of a strip shows the launcher in Pro mode. The close of an empty draft also goes back to the launcher.

### Text conversation

"Text conversation" opens the composer of the workspace for a chat conversation, also in Pro mode.
The draft tab reads "Text conversation".
The composer lists the chat models, so the user picks the agent and the model as before Pro mode.
`modeForComposerDraft` in `src/conversationMode.ts` gives the mode.
"Text conversation" replaces "New task…" in the "+" menu of the strip. It has no chord.
Cmd+N and "New chat" still open the composer, and the Pro toggle still decides the mode there.

### One click opens a project

A click on a project row selects the primary workspace of the project and expands the project.
An empty strip then shows the launcher.
The chevron of the row still collapses and expands the project. The arrow keys do too.
Orca toggles a repository header on a click. Halv differs on purpose: Pedro asked for a one-click open.
The chevron keeps the width of the old chevron, so the row does not move.

### The sidebar always shows the Projects tree

The Recent view is removed: the "Projects | Recent" toggle, the legacy viewer decision, and the header workspace switcher for the Recent view.
The search of the sidebar still finds every conversation.
The tree state store reads the old `view` and `recentList` fields without errors. The next write drops them.
The flat conversation list stays only while the `projectTree` flag is off. Stage S7 of ADR 0010 removes that flag.

### Terminals go to projects (adoption)

`src/terminalAdoption.ts` decides. The App runs the steps.

| Live item without a tab | Result |
|---|---|
| A terminal without an owner whose folder is in a project workspace | A shell tab of the deepest workspace that holds the folder. The project of the terminal wins first. |
| A terminal in a git repository that is no project yet | Halv adds the repository root as a project, then the terminal becomes a shell tab of its primary workspace. |
| Any other terminal, for example in the home folder | A terminal row in the Chats area of the sidebar. Its menu offers "Open in tab". |
| A live conversation of a project | A tab of its home workspace. |

- A terminal without an owner is a running plain shell, or a running agent terminal without a conversation. Rust `check_shell_adoption` now accepts both in a shell tab.
- A terminal with a shell tab id never moves again, also when its tab is gone. It shows in the Chats area.
- The Rust command `project_adoption_root` gives the git root of a folder only when Halv may add it. It refuses the filesystem root, the home folder and every folder above it, the app data folder, and the temporary folders. For a linked worktree, it gives the root of the main repository.
- An existing project with the same root gets the terminal. Halv adds no second project.
- A new project does not change the selected project.

Adoption never steals focus:

- A new tab never becomes active. The visible workspace and the visible tab stay.
- Adoption waits for the visible workspace while its strip has no tab, because a first tab would replace the launcher or the composer. It runs when the user opens a tab there or leaves.
- Adoption pauses while the bottom terminal panel or the Agent sessions panel shows. Those panels can show the terminals that adoption moves.

Adoption runs at most once per terminal and per conversation for each viewer:

- localStorage `halv.tab-adoption.v1` keeps the handled ids, at most 500 of each kind.
- The App marks an id before the step runs. A failed step does not run again.
- The close of a conversation tab marks its conversation. A closed tab does not come back by itself.
- A terminal whose folder has no adoptable root stays in the Chats area until the next start.
- The App keeps each root that Rust gave until the next start. A known root needs no second git call.
- "Open in tab" also marks its terminal. When an adoption step of that terminal runs, "Open in tab" waits for it and shows its tab. No second move starts.

### The Run button

The Run button shows only when the workspace has a local environment action.
It sits at the right end of the tab strip, as Orca puts its Quick Commands right of the tabs.
It is no longer in the header controls.
The "•••" menu keeps the actions.
Without an action, the "•••" menu offers "Set up local environment…", so the settings stay one click away.

### What this ADR supersedes

- ADR 0007, section "Pro mode": a new conversation in a workspace follows the launcher choice. An agent choice opens an agent tab. "Text conversation" opens a chat conversation, also in Pro mode. The Pro toggle keeps its meaning for the composer (Cmd+N, "New chat") and for the switch of an open conversation.
- ADR 0010, section "Sidebar": the Recent legacy view is removed. The sentence "Plain shells are not agent rows" changes. A plain shell is still not a workspace row. But a terminal without a tab now shows in the Chats area, and adoption moves it into a tab when it can. The "Other terminals" list is gone. A dimmed live conversation without a tab becomes a tab by adoption.
- ADR 0010, invariant T4 ("Only explicit intents and the one-time seed create tabs"): adoption is a third source of tabs. It is not a render pass: one serial runner issues it, at most once per id. T6 ("A closed tab stays closed") still holds.
- ADR 0010, the draft tab: in Pro mode, an empty strip shows the launcher, not the draft tab.
- The Run button of ADR 0010 stage S5b moves from the header to the right end of the strip. It hides without an action.

## Consequences

- The first 0.8.0 start moves the old terminals of a viewer into tabs. A strip can get several new shell tabs at once.
- A terminal in a linked worktree that another tool made, for example Orca, goes to the project of the main repository. Its tab opens in the primary workspace of that project, because Halv shows only the worktrees that Halv made.
- A shell that the user opened in the bottom panel moves into a tab after the panel closes.
- A shell tab can now hold a standalone agent. The close of that tab stops the agent. It has no conversation to park into.
- The flat list of the sidebar stays behind the `projectTree` flag until stage S7.
