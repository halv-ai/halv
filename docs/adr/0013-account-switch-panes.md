# ADR 0013: Agent panes after an account switch, and the assisted switch

Status: Accepted

## Context

The wallet keeps several Claude and Codex logins, and one active account per provider.
When Orca is on the computer, Orca owns the accounts, and Halv uses Orca's store (`src-tauri/src/wallet/orca.rs`).
A CLI reads its account folder (`CLAUDE_CONFIG_DIR` or `CODEX_HOME`) when it starts.
The PTY supervisor (ADR 0008) keeps panes alive across app restarts.
So a running pane keeps the account that it started with.

Before this ADR, a switch showed one notice: "N open sessions keep the previous account. Restart them."
The notice counted every open subscription session.
It had no restart action.
For Claude it was also wrong in most cases:

- Orca 1.4.x copies the selected Claude login into the default Claude folder.
  Every Claude pane in that folder follows a switch, so no restart is necessary.
- Orca `main` gives each Claude account its own folder and writes the selected folder into a pointer file.
  There, a Claude pane does not follow a switch, as with Codex.

Orca marks its live Codex panes after a switch.
Each pane shows "Restart" and "Keep old account".
The status bar menu shows "N Codex sessions are still on the old account" and "Restart N Sessions".
A restart resumes the Codex conversation on the new account.

Neither Orca nor Halv switched accounts near a usage limit.
The user saw the limit in the status bar, opened the switcher, and picked another account by hand.

## Decision

### Orca stays the single writer

Halv never writes Orca's files, its SQLite database, its keychain items, or its account folders.
A switch in Orca mode is one call to Orca's own RPC on its local socket: `accounts.selectClaude` or `accounts.selectCodex`.
The assisted switch and the restarts below use the same select path as the status bar and Settings.
The records in this ADR live in Halv's own wallet folder.

### The launch records the account folder of each pane

`wallet::record_launch` runs for each agent pane that Halv starts in its terminals.
It reads the account variable from the launch environment: `CLAUDE_CONFIG_DIR` for Claude, `CODEX_HOME` for Codex.
No variable means the user's own folder ("System default").
It also records the active account id, for the labels.
Only the subscription lanes use the wallet, so only they get a record.

The records live in `<app data>/accounts/launches.json`.
So a pane that outlives an app restart keeps its record.
A record expires after 30 days. The file keeps at most 512 records.

### A pane is stale when its folder differs from the folder of a new launch

`wallet_stale_sessions` compares the folder of each live pane with the folder that a new launch gets now.
The folder decides, not the account id:

- Orca 1.4.x routes every Claude account to the default folder. Claude panes never become stale there.
- Orca's per-account router gives each Claude account its own folder. Claude panes become stale like Codex panes.
- Halv's own wallet gives each account its own folder. Panes of both providers become stale.
- A switch back to the launch folder makes the pane current again.

When Halv cannot launch with the active account (a missing folder, or an Orca folder that is not set up), no pane is stale.
A restart would fail then.
Only panes in Halv's terminals count. Chat sessions and external terminals cannot restart in place.
A pane without a record is never stale.

`wallet_select` returns the stale panes of both providers.
The webview also asks again after each wallet change, also after a switch in Orca, and on window focus.

### Each stale pane asks; Halv restarts it only between turns

Each stale pane shows a chip in its top right corner: "Account switched", "{provider} is still signed in as {account}", "Keep old account", and "Restart".
The chip never blocks input, unlike Orca's card.
The status bar popover of the provider shows "N sessions are still on the old account" and "Restart N sessions".

"Restart" queues the pane.
The terminal panel that shows the pane restarts it when the pane rests: no turn runs, and no approval or question waits.
The restart does these steps:

1. It stops the old process.
2. It waits until the terminal process is gone.
3. It resumes the same CLI session in the same pane (`createResumedSession`). The new launch gets the active account.
4. It forgets the rows of the old session.

A pane without a CLI session id does not restart, because the conversation would be lost.
When the resume fails, the pane keeps its saved state, and "Resume" tries again.

"Keep old account" keeps the pane on its account until the active folder changes again.
The record stores this answer, so it survives an app restart.

### The assisted switch near a limit

When the binding usage window of the active account reaches `ACCOUNT_SWAP_NOTICE_PERCENT` (90 %), a card above the status bar offers a switch.
The binding window is the window with the highest use.
The card shows the use, the reset, and the target account.
"Switch account" calls the normal select path. The card never switches without that click (ADR 0017 adds an opt-in rotation for Claude).

The target is the account with the most headroom:

- The account with the lowest highest used percent wins.
- A tie goes to the soonest reset.
- An account that needs a sign-in, an account without its folder, and an account at 100 % of its weekly window drop out.
- An account with more use than the active account drops out.
- Only Orca knows the use of the other accounts. Accounts with unknown use come after the known ones.
- System default is never a target.
- Without a target, no card shows.

The card shows once per crossing of the threshold.
A drop below the threshold, or another active account, arms it again.
Usage that Halv read before a switch does not count for the new account.
A setting in Settings > AI connections turns the card off. The setting lives in the localStorage of the viewer.

## Consequences

- Codex panes follow Orca's restart flow, and Claude panes follow it under Orca's per-account router.
- Claude panes in Orca's shared default folder show no prompt, which matches Orca.
- A pane in a long turn keeps the old account until the turn ends.
- A Codex pane on an Orca account folder can have no CLI session id, because Halv looks for Codex sessions only in `~/.codex`. The restart of such a pane fails with a message, and the user can keep the old account.
- A `claude` or `codex` that the user types in an old shell keeps the environment of that shell. Halv does not track shells.
- By default, Halv does not rotate accounts on its own, and it adds no providers. ADR 0017 adds an opt-in automatic rotation of Claude accounts.
