# ADR 0017: Automatic rotation of Claude accounts

Status: Accepted

## Context

ADR 0013 added the assisted switch.
At 90 % of the binding usage window, a card offers the account with the most headroom.
The switch starts only when the user clicks "Switch account".
During long work, the user does not always see the card, and the CLI stops at the limit.

The maintainer asked on 2026-10-15 for an opt-in setting: "rotate when usage is between 92–99% (default, configurable), for Claude."
Orca has no automatic rotation, so Halv has no Orca code to adapt.

## Decision

### The setting

- Settings > AI connections shows "Rotate Claude accounts automatically" below the toggle of the assisted card.
- The setting is off by default.
- It has two values: the start percent (default 92) and the latest percent (default 99). The two number fields show only while the setting is on.
- The start percent stays in [50, 100]. The latest percent stays in [start, 100]. The setting clamps a value outside these ranges. A value that is not a number keeps the current value.
- The setting lives in the localStorage of the viewer as one JSON value (`halv.account-auto-rotation.v1`). A storage that throws keeps the rotation off.
- The rotation applies only to Claude. Codex keeps the assisted card.

The code is `src/accountAutoRotation.ts` (the pure rules) and `src/useAccountAutoRotation.ts` (the hook in the status bar card).

### The reading

The rotation uses the reading of the assisted card: the binding window of the active Claude account.
The usage counts only when it belongs to the active account, so its wallet version must be the current wallet version.
Halv never rotates away from System default.
Halv does not rotate before the wallet loads.

### When Halv switches

| Used percent of the binding window | Action |
| --- | --- |
| Below the start percent | No switch. |
| From the start percent | Switch when no Claude pane is busy. |
| At or above the latest percent | Switch at once, also during a turn. |

A Claude pane is a live agent session in Halv's terminals on the Claude subscription lane.
The API key lanes do not use the wallet, so they do not count.
A pane is busy while a turn runs, or while an approval or a question waits (`terminalActivityStore.paneIsBusy`).
A pane without an activity signal rests.
Halv evaluates the rotation again when the activity of a pane changes. So a waiting switch starts when the turn ends.

### The target

The target uses the rules of the assisted switch (ADR 0013), with three more rules:

- A target with known usage must have its highest used percent below the start percent.
- A target with unknown usage still counts, after the targets with known usage. Only Orca knows the usage of the accounts that are not active, so without this rule the rotation does not work in Halv's own wallet.
- Halv remembers each account that it left automatically. That account is not an automatic target until its windows reset. Halv uses the last reset of the windows at or above the start percent, which is usually the reset of the binding window. Without a known reset, the account stays out for 5 hours. This rule stops a ping-pong between two accounts.

The memory of left accounts lives in the app session only.

Without a target, Halv does not switch.
The assisted card then shows as the fallback, when its setting is on.
The card uses the start percent as its threshold and its own target rules, so the user can still pick an account.
When no account qualifies for the card, no card shows.
Halv looks for a target again after one minute.

### One switch per crossing

After a switch, Halv does not switch again for the same account until another account is active or the use drops below the start percent.
A failed switch shows its error in a card.
Halv tries again at most once per minute while the use stays at or above the start percent.
When the user closes the failure card, later failures of the same crossing stay quiet.

### The switch

The switch uses the normal select path: `walletStore.select("claude", accountId)`.
In Orca mode, this path is one call to Orca's RPC.
Orca stays the single writer: Halv never writes Orca's files, its database, its keychain items, or its account folders.

Halv then applies the stale panes of the result, as ADR 0013 describes.
The user opted in to automatic switches, so Halv also queues the restart of the stale Claude panes (phase "queued").
Each pane restarts only between turns, and it resumes the same CLI session on the new account.

### The feedback

A card above the status bar, at the place of the assisted card, shows the result:

- Title: "Switched Claude to {account}".
- Body: "{old account} reached {percent}% of its limit."
- "Dismiss" closes the card. Escape also closes it while the focus is in the card.

The card never takes the focus. Halv sends no OS notification.

With the rotation on, the assisted Claude card does not show at 90 %. It shows only as the fallback above.

## Consequences

- The default does not change. Halv switches without a click only after the user turns on the rotation.
- A switch at the latest percent can come during a turn. A pane with its own account folder finishes that turn on the old account, and then it restarts on the new account.
- After an app restart, the memory of left accounts is empty. Halv can then rotate back once to a left account with unknown usage.
- In Halv's own wallet, Halv does not know the usage of the target. The target can also be near its limit. The next crossing then switches again, and the memory stops a loop.
- Only Halv's terminal panes delay a switch. A chat session or an external terminal does not.
- The setting lives in the localStorage of the viewer. It does not sync to other computers.
- Codex has no automatic rotation.
