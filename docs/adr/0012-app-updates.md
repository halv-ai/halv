# ADR 0012: App updates

Status: Accepted

## Context

Before 0.7.2, the webview owned the update flow.
`App.tsx` ran the update checks with `@tauri-apps/plugin-updater`.
A top banner, Settings, and the Connect gate each ran an install state machine.
One click downloaded, installed, and relaunched Halv in one step.
A dismissed banner stopped all reminders until the next launch.
The 177 MB macOS archive stayed in webview memory during the download.
Halv had no install on quit, no busy guard, and no exit watchdog.

The supervisor from ADR 0008 already keeps agent sessions alive across a relaunch on unix.
One supervisor process served Halv 0.6.3 to 0.7.1 through four update relaunches.
Orca shows its update in a small card in the bottom-right corner.
Orca also restarts its app; its daemon keeps the terminals alive.

## Decision

### One owner

`AppUpdateManager` in `src-tauri/src/app_update/` owns the update state.
It sends one `AppUpdateSnapshot` on `app_update://snapshot` after each change.
Every window shows the same snapshot.
The webview never holds the plugin `Update` object.

### Checks

The first check runs 10 seconds after the start.
Later checks run every 6 hours.
A main window focus also starts a check when the last successful check is older than 5 minutes.
Checks continue after a download is ready.
A newer release replaces the staged release.
When the feed no longer offers the staged release, the manager removes it.

An automatic check does not change the status until it ends.
An automatic check that fails changes nothing.
A user check shows "checking" and reports an error.
A development build checks only on request and cannot download or install.

### Install modes

At the start, the manager finds the install mode from the bundle type and the access to the install location.
The plugin replaces the macOS app bundle or the AppImage with a rename, so Halv must be able to write the bundle and its folder.
`access(2)` with `W_OK` tests both on macOS, and the folder of `$APPIMAGE` on Linux.

| Mode | When | Download | "Restart now" | Install on quit |
|---|---|---|---|---|
| `replace` | Halv can write the app bundle or the AppImage. | Background | Yes | Yes |
| `elevated` | A Linux deb or rpm, or a macOS bundle that only an administrator can change. | Background | Yes, with the administrator prompt | No |
| `installer` | Windows MSI or NSIS. | Background | Yes | No |
| `readOnly` | A disk image, App Translocation, a read-only volume, or an AppImage in a folder that Halv cannot write. | No | No | No |
| `unsupported` | A development build, or no bundle. | No | No | No |

A `readOnly` copy still checks on the schedule.
Its card asks the user to move Halv to the Applications folder.
Without this rule, Halv would download 177 MB at each start and fail the install at each quit.

### Background download

The download runs in the background by default.
The `appUpdateAutoDownload` preference turns it off.
Then the card offers "Download".

The plugin checks the minisign signature of the archive.
The manager writes the archive to `<app data>/updates/halv-<version>.archive` with mode 0600.
The manager keeps the SHA-256 digest in memory, and the install compares it.
A staged file is valid only for the process that wrote it.
Each start removes the staged files of earlier runs.

A download fails when no data arrives for 60 seconds.
An automatic check starts a failed download again after 6 hours.
After 3 failed automatic downloads of one release, only the user starts it again.
So a short network problem does not stop the update, and a permanent failure does not repeat a 177 MB download for days.
A newer release starts a new count.
Progress events arrive at most 4 times per second.

When the user turns off the preference, an automatic download stops at once.
The status changes to `available`, and the card offers "Download".
A download that the user started continues.

### Restart

The update has two steps.
First the download runs in the background.
Then the card offers "Restart now" and "Later".

"Later" hides the card for that version until the next start or a newer version.
The status bar segment stays visible while the update downloads, is ready, or failed.
A user check shows the card again.

These items block a restart:

- Chat replies in progress.
- GUI agent sessions with a turn in progress.
- Running terminals inside the app process: the fallback terminals and all Windows terminals.

"Restart now" without `force` returns the blockers and changes nothing.
"Restart when done" waits until the chat replies and the agent turns end.
Terminals do not end on their own, so the card only names them.

The restart runs in this sequence:

1. The status changes to `installing`.
2. The manager sends `app_update://before-restart` with a request ID.
3. The frontend saves its state and calls `app_update_restart_ack`. The manager waits at most 1 second.
4. A worker thread reads the staged file, compares the digest, and installs it.
5. The status changes to `restarting`.
6. The relaunch helper starts the new version, and a 20 second watchdog ends a stuck exit.

A quit during the restart install waits for the install.
Then Halv quits and does not start again, also when the install failed.

The install never runs on the main thread.
The macOS admin path of the plugin waits for the main thread.
The install runs only right before a relaunch or a quit.
An early install makes the old app start new processes from the new bundle.

### Install on quit

Every quit inside Halv calls `AppHandle::exit`.
This includes the tray, "Quit and Stop Agents", the frontend quit, and the app menu.
The predefined macOS Quit item ends Halv without `RunEvent::ExitRequested`.
So the app menu uses a custom "Quit Halv" item with the same Cmd+Q shortcut.

On `ExitRequested`, Halv installs a staged update when the install replaces the app in place.
This is true only in the `replace` mode: a macOS app bundle or an AppImage that Halv can write.
Halv prevents the exit, hides its windows, and installs on a worker thread.
Then Halv exits again, without a relaunch.
A failed install does not stop the quit.
After an install fails, the quits in the same run do not try it again.

The Dock Quit item, logout, and shutdown skip `ExitRequested`.
The update then stays pending, and the next start checks again.
A Linux package needs administrator rights, and the Windows installer ends the process.
So these install only through "Restart now".

The Windows installer ends the process with `std::process::exit`.
The plugin first calls `on_before_exit`, and the manager sets it to Halv's exit cleanup.

### After the restart

The boot record shows the version of the previous run.
When that version is older, the snapshot carries `postUpdate`.
`reconnectedSessions` counts the running supervised agent terminals that the new app adopted.
Plain shells do not count, because the card names agent sessions.
The card shows "Updated to Halv X" with the number of reconnected agent sessions.

### The card

`AppUpdateCard` shows the snapshot in the bottom-right corner of every screen, also over the Connect gate.
`main.tsx` mounts it beside `App`, inside its own error boundary.
`appUpdateStore` keeps the last snapshot and only two view facts: the card that the user hid, and the card that the user opened.
`appUpdateCard.ts` builds the card from the snapshot as pure data, so unit tests cover each state.

The card is 360 pixels wide, 16 pixels from the right edge, and 12 pixels above the status bar.
It moves above the chat composer when it would cover it, so the send button stays usable.
It hides while a composer menu (`data-update-card-yield`) opens under it, so a click on the menu never lands on the card.
The Connect gate has no status bar, so it keeps `--app-update-card-space` below its controls and scrolls them clear of the card.
Escape hides the card.
The card does not take focus.
When its actions change while it has focus, focus moves to its first action.
When it hides, focus goes back to where it came from.
The status bar segment then shows the download, the ready update, the install, or the failure, and a click opens the card again.
A background check and a failed background download show no card.
Motion stops when the system asks for reduced motion.

The ready card says that the agent sessions keep running.
When `sessionsSurviveRestart` is false, it says that open terminals and agents close.
This is true when the supervisor is not available, when it answered with another protocol, or when a terminal runs inside Halv.

Settings > System shows the same snapshot, the download switch, and the terminal supervisor.
The terminal panel no longer shows a notice for a supervisor from an older build.
Settings keeps the destructive supervisor restart behind the existing confirmation.

The top banner, the Connect gate banner, `useAppUpdateInstall`, and the webview scheduler are gone.
The webview does not call the updater plugin, so the capability no longer grants `updater:default`.
The UX harness takes `?update=<state>` and sends the same snapshot events as Rust.

### Supervisor

ADR 0008, section "App updates", has the supervisor rules.
In short: protocol 1 is a permanent baseline, the hello negotiates new revisions, and a stale supervisor is replaced only when it runs no session.

### Running agents keep their tools

An agent keeps the `halv --mcp-savings` server that it started before an update.
That server runs from the old bundle until the agent restarts.
When the agent starts the server again, the new bundle receives the arguments of the old app.
So the argument parser must accept older argument sets.
A test keeps the argument sets of Halv 0.6 and 0.7.1.
Database migrations must stay additive, because old servers read the migrated database.

## Contract

Commands:

| Command | Effect |
|---|---|
| `app_update_snapshot` | Returns the snapshot. |
| `app_update_check { userInitiated }` | Starts a check and returns at once. |
| `app_update_download` | Starts the download of the last checked release. |
| `app_update_restart_blockers` | Returns the restart blockers. |
| `app_update_restart { force }` | Returns `null` when the restart started, or the blockers. |
| `app_update_restart_when_idle` | Restarts when the chat replies and agent turns end. |
| `app_update_cancel_restart` | Cancels "Restart when done" and clears the blockers. |
| `app_update_restart_ack { requestId }` | Ends the wait for the frontend save. |
| `app_update_defer { version }` | Hides the card for this version in this run. |
| `app_update_set_auto_download { enabled }` | Stores the preference. Turning it off stops an automatic download. |
| `app_update_dismiss_post_update` | Clears `postUpdate`. |

`app_update_relaunch` stays for the JavaScript installer of earlier builds.

The snapshot:

```ts
type AppUpdateStatus =
  | { state: "idle" }
  | { state: "checking"; userInitiated: boolean }
  | { state: "upToDate"; userInitiated: boolean }
  | { state: "available"; version: string; notes: string | null; releaseUrl: string }
  | { state: "downloading"; version: string; downloadedBytes: number; totalBytes: number | null }
  | { state: "ready"; version: string; notes: string | null; releaseUrl: string }
  | { state: "installing"; version: string; trigger: "restart" | "quit" }
  | { state: "restarting"; version: string }
  | { state: "error"; operation: "check" | "download" | "install" | "relaunch";
      message: string; version?: string; retryable: boolean; userInitiated: boolean };

type AppUpdateRestartBlockers = {
  chatReplies: number;
  guiAgentSessions: number;
  inProcessTerminals: number;
};

type AppUpdateSnapshot = {
  status: AppUpdateStatus;
  currentVersion: string;
  autoDownload: boolean;
  installMode: "replace" | "installer" | "elevated" | "readOnly" | "unsupported";
  deferredVersion: string | null;
  restartWhenIdle: boolean;
  blockers: AppUpdateRestartBlockers | null;
  postUpdate: { fromVersion: string; toVersion: string; reconnectedSessions: number;
                releaseUrl: string } | null;
  lastCheckedAt: string | null;
  sessionsSurviveRestart: boolean;
};
```

`notes` is the Markdown of `latest.json`.
The card shows a short excerpt and a link to `releaseUrl`.
The error `message` is the raw plugin text, and the frontend maps it to a sentence.

## Platform matrix

| Platform | Install | Relaunch | Agent sessions survive | Install on quit |
|---|---|---|---|---|
| macOS app bundle | Plugin replaces the bundle | `open -n` helper | Yes | Yes |
| macOS bundle that only an administrator can change | Plugin, with the administrator prompt | `open -n` helper | Yes | No |
| macOS disk image or App Translocation | None: the card asks the user to move Halv | — | — | No |
| Linux AppImage | Plugin replaces the file | `request_restart` | Yes, with a SIGBUS risk | Yes |
| Linux deb, rpm | Plugin, with administrator rights | `request_restart` | Yes | No |
| Windows | MSI or NSIS installer ends Halv | The installer | No | No |

## Consequences

The plugin still keeps the whole archive in memory during the download.
The staged file shortens that time, not the peak.
A streaming download is a later option.

The Headroom proxy stops during the relaunch.
Surviving agents can see a short API gap.

A surviving agent that first opens a protected macOS folder during the bundle swap can be denied.
Linux AppImage can unmount the supervisor binary after the app exits.
Both need QA on release builds.

A heavy user rarely reaches zero sessions.
So a stale supervisor can run old code for weeks.
Supervisor generations (one socket for each build) fix this in a later phase.
