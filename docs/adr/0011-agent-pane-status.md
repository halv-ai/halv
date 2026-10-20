# ADR 0011: Agent pane status

Status: Accepted. Amended on 2026-10-16: terminal title fallback.

## Context

Halv showed one amber "waiting" dot for four different situations.
A finished turn, a permission prompt, a new idle pane, and a restored idle pane looked the same.
The webview guessed the state from PTY byte volume and Enter key presses (`src/terminalActivity.ts`).
The guess failed after a permission approval, after an app restart, and when an agent started work itself.
Halv sent no OS notification when a terminal agent needed the user.

Orca shows the same panes with agent hooks.
It shows an orange "?" for a wait and a green check for a finished turn.

Claude Code writes its live state to `<config dir>/sessions/<pid>.json`.
The file holds `status` (`busy`, `idle`, `shell`, or `waiting`).
While the session waits, the file also holds `waitingFor`.
Claude rewrites the file on each status change.

## Decision

### One status for each pane, in Rust

The module `src-tauri/src/agent_status/` owns the status of each agent pane.
The key is the terminal session id.
The statuses are `working`, `done`, `needsInput`, `needsApproval`, `idle`, `exited`, `failed`, and `interrupted`.
Each entry holds `status`, `reason`, `updatedAt`, `unread`, `waitSeq`, `turnSeq`, `source`, and an optional `prompt` detail.
`waitSeq` counts the waits of the pane, and `turnSeq` counts its turns.
`source` names the signal: `presence`, `hook`, `rollout`, `terminal`, `title`, `process`, or `none`.

The store sends each change as the Tauri event `agent-status`.
Rust code receives the same entry through `subscribe()`, a broadcast channel.
The command `agent_status_snapshot` returns every entry.
`PaneStatusMachine` turns the signals into a status.
It has no I/O, so unit tests cover each rule.

### Signals and precedence

1. Claude-family panes (Claude, GLM, and Grok lanes) use the presence file of the pane process for the live state.
   Their hooks give the outcome of a turn and the detail of a wait.
   Without a presence file, the hooks also give the live state.
2. Codex panes use the rollout watcher: `task_started`, `task_complete`, and `turn_aborted`.
   Codex terminal notifications (OSC 9) give its waits.
3. The terminal title of each agent pane is the fallback signal. See "Terminal title fallback".
4. A pane with no signal uses the byte-volume tracker in the webview. A pane of an unknown agent always uses it.
5. The PTY exit always wins.

A thread reads the presence file of each Claude-family pane every 300 ms.
It parses the file only when its modification time or size changes.
It accepts the file only when `pid` is the pane PID.
It also rejects a file whose `startedAt` is more than 5 s before the pane start.
It never lists the folder and never opens the `.key` files.

The presence values map as follows:

- `busy` gives `working`.
- `idle` and `shell` end the turn.
- `waiting` with "permission prompt", "sandbox request", or "worker request" gives `needsApproval`.
- `waiting` with "input needed" or "goal proposal" gives `needsInput`.
- Any other `waiting` value gives `needsInput` only during a turn. A dialog that the user opens at rest is not news.

When the live state comes to rest, the machine picks the outcome of the turn in this order:

1. A `Stop` event of the main agent in this turn gives `done`. A `StopFailure` event gives `failed`.
2. An Esc key press less than 3 s before gives `interrupted`. The key counts when the ordered input queue wrote it to the PTY.
3. A pane without hooks gives `done`.
4. A pane with hooks that rests from a wait gives `interrupted`. The user denied or cancelled the prompt.
5. A pane with hooks waits 1.5 s for a late `Stop` event. Without one, the turn is `interrupted`.

A `Stop` event alone does not end a turn while the presence file shows work.
A user `Stop` hook can continue the turn.
A main-agent tool call or prompt after a `Stop` event clears its outcome, because the turn goes on.
Without a presence file, a `Stop` event waits 1.5 s too. A tool call or a prompt in that time keeps the turn open.
A `UserPromptSubmit` event after a `Stop` event starts a new turn, also when the presence file still shows work.

The launch records the effective `CLAUDE_CONFIG_DIR` of each Claude-family pane.
The record is `<app data>/agent-status/config-dirs.json`, so an adopted pane reads the same folder after a restart.
A pane without a record tries the folder of the active wallet account, then the system folder.

### Terminal title fallback

This section was added on 2026-10-16.

A pane that an older Halv started has no hooks and no recorded config folder.
A pane under another `CLAUDE_CONFIG_DIR` never finds its presence file.
Such panes showed the dashed "not reporting" ring, also while the agent worked.
Orca uses the hooks first and the terminal title as the fallback. Halv now does the same.

The store tracks each agent that Halv launches: Claude, GLM, Grok, Codex, Gemini, Antigravity, and Kimi.
A pane of an unknown agent stays untracked.
No capture shows a Kimi title. So the name `kimi` does not make a title an agent title, and a Kimi pane reports only through the glyph rules.

A scanner reads the live PTY output of each tracked pane.
It finds `ESC ] 0 ; text` and `ESC ] 2 ; text`, with BEL or `ESC \` at the end.
It keeps its state across chunks. It drops a body longer than 4 KiB, so its buffer stays small.
It sends only a change of the title state, because a spinner frame changes the title about once each second.

The rules come from Orca (`shared/agent-title-status.ts`). Halv keeps the rules for its own agents:

- Claude, GLM, and Grok: `✳` as the first glyph is rest. A braille or quarter-circle spinner as the first glyph is work.
  Claude also shows `✳` at a permission prompt, so the title cannot show that wait.
- Codex: `[ . ] Action Required` or `[ ! ] Action Required` is an approval wait. A spinner is work.
  Any other title is rest, because Codex shows the folder name at rest. The words of a thread name do not count, because the user picks them.
  The Orca captures of codex-cli 0.150.1 to 0.162.1 show these titles. Orca itself does not read them, because its Codex hooks write their own titles.
- Gemini: `✋` is an approval wait, `✦` and `⏲` are work, and `◇` is rest.
- Other titles count only with an agent name as a whole word, for example `codex ready` or `gemini - action required`.
  A path such as `~/codex/ready` and a word such as `timestamp ready` do not count.

The title has the lowest precedence of the agent signals:

1. The presence file always wins over the title.
2. A hook or rollout signal wins over the title for 30 minutes after it arrives. Orca uses the same time for its hook rows.
   A replayed journal line keeps its own time. So a journal line older than 30 minutes is stale at once.
   A Codex notification does not hide the title: it never tells the end of its wait, and the title does.
3. Without a fresh signal, the title drives the pane.
   Work continues a turn, and it starts a turn after Enter in the pane. An approval title starts a wait. Rest ends the turn as `done`, or as `interrupted` after a recent Esc press.
4. A title that does not change the status keeps the source of the status. So the banner of a turn or a wait does not come twice.
5. When the presence file goes and no hook or rollout signal is fresh, the last title applies at once.

Work in the title with no Enter before it is not a turn.
Codex shows a spinner in its title for a moment while it starts, before any prompt (Orca capture `codex-fullscreen-startup`).
Without this rule, each new Codex pane showed a finished turn, and a pane in the background sent a "finished" banner.
Such work shows the spinner, but it does not count a turn and it cannot end as a result.
When the title rests, the pane goes back to its state before the work, with the same source.
The title always ends the work that it started, also when another signal came since.
Another signal can make the work a turn: a rollout start, a hook, a presence file, or an approval title.
The store notes a key after the PTY write, so the title of the work can come first.
So Enter up to 500 ms after the work began makes that work a turn. Enter later in the work is for the next work.

A new pane takes the last title in its scrollback as its first state.
This title is the current state, not news: a turn that it shows as finished counts as read.
Orca seeds its restored panes from their last title in the same way.
A spinner in the scrollback of a pane that started 10 s or more before is a turn in progress.
In a younger pane, the spinner can be the start animation of the agent, so it is work with no turn.
So an adopted pane at rest shows the idle dot after an app restart, not the dashed ring.

The webview uses a `title` entry as any other agent signal: a spinner for work, the "?" bubble for a wait, and the check for a finished turn.
The dashed ring stays only for a live agent pane with no signal at all.
A title result gets a banner, as in Orca.

### Claude hooks

Halv adds its hooks to the per-session `--settings` file that it already writes for RTK.
The file now exists without RTK too.
It holds only the `hooks` key: a flag setting overrides the same key of the user and of Orca.
Only Halv PTY panes get the status hooks. The GUI launch and an external Ghostty window do not.

The events are `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `PermissionRequest`, `Notification`, `Stop`, `StopFailure`, `SubagentStart`, `SubagentStop`, and `SessionEnd`.
Claude Code 1.0.23 to 2.1.100 drop the whole settings file when it names an unknown event.
So each event has a version floor, and Halv writes only the events that the Claude version knows.
Halv reads the version from a Claude install path: `claude/versions/<version>`, `Caskroom/claude-code/<version>`, or the Halv install `bin/claude/<version>`.
A version in another path can belong to Node, to a version manager, or to Homebrew, so Halv does not use it.
Else it takes the `claude --version` result of the shared Claude CLI probe.
A launch and the agent list then share one run of the CLI.
An unknown version gets the events up to 1.0.64.
Halv never registers `Elicitation`, because a hook answer can answer the MCP dialog.

Each hook runs `<app data>/agent-status/hooks/bin/halv-agent-hook agent-hook claude`.
Debug builds use `hooks-dev`.
The helper is a stable copy of `halv-supervisor`, so a moved app bundle does not break the hooks of a running pane.
The app replaces the copy at start when its bytes change.
The helper does these steps:

1. It prints `{}`, which is hook output with no decision.
2. It reads the payload: at most 4 MiB, for at most 3 s.
3. It stops when `HALV_AGENT_SESSION_ID` or `HALV_AGENT_STATUS_DIR` is missing, or when `CLAUDE_JOB_DIR` is set.
4. It keeps small fields and 160-character previews: the command, the file path, the first question and its options, and the notification text.
   It keeps no full prompt, tool input, tool output, or assistant message.
5. It appends one line to `<session id>.jsonl` with mode 0600. It moves a journal past 1 MiB to `<session id>.jsonl.1`.
   Claude Code runs the hooks of parallel tool calls at the same time.
   So the helper first locks `<session id>.jsonl.lock`, and only one helper moves the journal.
   After 500 ms without the lock, it writes without the lock.
6. It sends the session id to `wake.sock` as one datagram.
7. It exits with status 0 in every path.

The app reads a journal when a datagram arrives, and on each 300 ms poll before the presence file.
The helper writes the line before Claude changes its presence file, so the outcome comes first.
After an app restart, the app reads each journal from its start.
Each read goes to the end of the journal, and of a moved journal, so the replay ends before the presence file applies.
The old lines rebuild the status, and their results count as read.
The app removes the journal when the pane goes, and journals older than 7 days at start.

Launches of agent panes in Halv PTYs set `HALV_AGENT_SESSION_ID` and `HALV_AGENT_STATUS_DIR`.
Plain shells remove both, so a Halv that runs in a Halv pane does not pass them on.

### Prompt detail

A wait can carry a `prompt`:

- `{kind: "approval", toolName, command?, filePath?, message?}` from `PermissionRequest` and the `PreToolUse` before it,
- `{kind: "question", text, options?}` from `AskUserQuestion` and from MCP input notifications.

With a presence file, the hook detail joins the wait of the presence file. It never starts a new wait.
A tool event after the hook clears the detail, so an old detail never reaches a later wait.
During a wait, a `PostToolUse` event of a parallel tool call does not end the wait.
The `PostToolUse` event of the waiting tool ends the wait, also with a presence file.
So the next prompt is a new wait with its own detail, also when the poll misses the short busy state between two prompts.
Claude sends a `permission_prompt` notification some seconds after each dialog. It joins the open wait and never starts a second one.
A subagent wait ends with a tool event of the same subagent. Events of other subagents do not change the pane.
The banner text uses the detail, for example "Claude wants to run: pnpm test".

### Codex waits

The interactive Codex launch in a Halv pane adds three `-c` flags:
`tui.notifications` with the four kinds, `tui.notification_method="osc9"`, and `tui.notification_condition="always"`.
The app-server launch and an external terminal do not get them.
Codex 0.162.1 accepts these values. A wrong value stops Codex at start, so the values must stay exact.

A scanner reads the live PTY output of each Codex pane for `ESC ] 9 ; text BEL` (or `ESC \`).
It keeps its state across chunks and never reads replayed output.
"Approval requested", "Codex wants to edit", "Question:", and "Plan mode prompt:" start a wait.
The rollout reports the end of the turn.

Codex sends no event when the user answers. Two signals end the wait:

- Enter or Esc in the pane, or y, a, n, or a digit for an approval,
- a tool result in the rollout with a time after the start of the wait.

A letter in a question can be the first letter of a typed answer, so it does not end the wait.

After an app restart, the first read of a rollout finds its last turn.
A turn event that the rollout recorded before the pane was tracked is history: it rebuilds the status, and its result counts as read.

The rollout watcher polls every 500 ms while a Codex pane works or waits, else every 2 s.
Halv writes no Codex `hooks.json`.

### Unread

A turn that ends as `done` or `failed` stays unread until the user views the pane.
A view during a turn does not read the result of that turn.
A turn that ends in the pane that the user looks at is read at once.
The user looks at a pane when the window has focus and the pane is the active pane of the visible terminal panel.
Dot, Settings, and the browser view cover the panes, so the user looks at no pane while one of them is open.
A wait never becomes read. Only the agent ends a wait.
Unread state is in memory only. An app restart clears it.

### Glyphs

`AgentStateGlyph` draws the state of a pane:

- a spinner for `working`,
- an orange "?" bubble for `needsInput` and `needsApproval`,
- a green check for an unread `done`,
- a red mark for an unread `failed`.

A read result, `idle`, and `interrupted` show no glyph.
The glyph shows in the pane header, the terminal tab, the terminal header label, the sidebar rows, and the pinned rail.
The question color is `--agent-question` (orange), not the coral accent of running work.
The bubble shape keeps the two apart.
With reduced motion, the spinner does not turn. A full accent ring with a center dot shows the work.
A conversation row shows its strongest pane in this order: a wait, an unread failure, work, an unread result.

### Notifications

A pane that starts a wait gets one banner for each `waitSeq`, after 1 s.
An unread `done` or `failed` gets one banner after 1.5 s.
The delays drop prompts that close at once.
Only a pane with an agent signal gets a banner, because the byte-volume tracker can report a false finish.
The pane that the user looks at gets no banner.
A focused window that shows another conversation still gets the banner.
The existing preferences apply:

- `permissionNotifications` for `needsApproval`,
- `questionNotifications` for `needsInput`,
- `completionNotifications` for results.

The command `terminal_notify` shows the banner from Rust with notify-rust.
A thread waits for the click, because the notification plugin drops the banner handle.
At most 16 threads wait.
A banner stays in Notification Center after it hides, and its thread waits until the user clears it.
On macOS, a full cap removes the oldest delivered banners of the app until 12 remain.
Their threads end, so the new banner keeps its click action.
On Linux, a banner above the cap shows without a click action.
A click brings the window forward and emits `terminal-notification-clicked`.
The webview then closes Dot and Settings, shows the chat view, opens the conversation, and focuses the pane.
The panel that shows the pane marks it viewed. A pane that no panel can show keeps its unread mark.
The dock badge shows the number of panes that wait for the user.

### Coexistence with Orca

Halv only reads the Claude presence files.
Halv writes no global agent configuration: no `~/.claude/settings.json` and no file in `~/.codex/`.
Halv hooks print `{}` and never decide a permission.
Orca agent hooks report to the Orca pane that the `ORCA_*` variables name.
So Halv removes every `ORCA_*` variable from each PTY that it starts.
The app removes the known names and each `ORCA_*` name in its own environment.
The supervisor also removes each `ORCA_*` name that it inherited.

## Consequences

The presence file is internal Claude Code state.
A format change drops a pane to the byte-volume tracker. It does not give a wrong state.
The webview keeps the raw tracker status, because the guided first task still reads it.
In a dev build, the banner posts as Terminal, so a click opens Terminal and not Halv.

A change to the supervisor sources changes its build fingerprint.
The `agent-hook` command adds no request, so the protocol and the hello negotiation do not change.
ADR 0008 rule 3 replaces the stale supervisor when it runs no session.
The hooks do not need the new supervisor: they run the stable helper copy of the bundled binary.
Until then, the stale supervisor does not remove the `ORCA_*` variables that it inherited.
The app still removes the known names from each new PTY.

A hooked `/compact` turn ends with no `Stop` event, so it shows as stopped and not as done.

The Codex notification texts are English prefixes that Codex can change.
A changed prefix only loses the wait. The rollout still reports the turn.

The title cannot tell a Claude permission prompt from a finished turn.
So a Claude pane without hooks and without a presence file shows a finished turn when it asks for permission. It can also send a "finished" banner.
The work after an answer with Enter starts a new turn. An answer with a number key gives work with no turn, so the end of that work has no banner.
A pane that reports only through its title shows no result for work that did not start with Enter in the pane.
Examples are a prompt from the phone remote, a prompt that Halv gives at launch, and work that the agent starts itself.
The pane shows the spinner for that work, and then goes back to its last state.
Claude keeps a static `✳` title under tmux, screen, or zellij. Such a pane has no title signal.
A Codex pane can leave a spinner title after its turn. Without a rollout, the pane then shows work until the next title.
The title glyphs and words belong to the agents, and the agents can change them. A change drops a pane back to the dashed ring.

Each hook event starts one more process. Machines with Orca also run the no-op Orca hook.

Later work can add:

- an agent chips row under a conversation with several agent panes,
- Codex hooks, after a live test shows how Codex trusts session hooks.
