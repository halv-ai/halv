# ADR 0008: Detached PTY supervisor

Status: Accepted

## Context

Halv owns every agent PTY inside its own process (issue #11).
Each `claude` or `codex` process is a child of the app.
When the app process ends, every agent process ends with it.
Restore is a replay through `--resume`, not a resume.
The in-flight turn and the TUI state are lost on each restart.

## Decision

A separate supervisor process owns every terminal PTY on unix.
The supervisor is a standalone binary: `halv-supervisor`.
The crate lives at `src-tauri/supervisor/` with its own lockfile.
The app talks to it over a unix domain socket with mode 0600.
The protocol is one JSON object per line, with a version handshake.
The socket lives in the app data directory.

The supervisor detaches from the app with `setsid`.
An app exit, clean or not, does not stop the supervisor.
The supervisor keeps a 256 KiB scrollback ring per session.
The ring reproduces the UTF-8 chunk semantics of `terminal.rs`.

`TerminalManager` becomes a client of the supervisor.
It keeps its current Tauri command surface.
On startup, the app adopts the supervisor's live sessions.
`terminal_list` then reports the adopted sessions as running.
The frontend restore path sees a running session and attaches.
The `--resume` replay runs only when no live session exists.

Windows keeps the current in-process PTY path.
The app also keeps the in-process path as a fallback.
The fallback runs when the supervisor is missing or unreachable.
The app shows a visible message when it falls back.

## Restart consequences

Agent CLIs point at the local Headroom proxy port.
The app persists that port in `sidecar-port.json` in the app data directory.
The next launch tries the saved port first, then a random port.
A surviving agent therefore still reaches the proxy after a restart.

`AgentSessionManager` rebuilds each adopted agent pane from its `agent_sessions` row.
It restores the CLI session id, titles, lane, and generation.
It starts the embedded lifecycle thread for the pane.
The registry gains no new rows on a restart.
Rows that stay `running` with no live terminal become `exited`.
A protocol mismatch is the exception (rule 5 in "App updates").
A duplicated adopted pane resolves its credentials again from the default profile.

`terminal_supervisor_status` starts the supervisor when it is not running.
`available: false` therefore means the supervisor is missing or unreachable.
The panel shows a replay notice only in that case.

## Quit behavior

A clean quit keeps the supervisor and its sessions running.
When a downloaded update is ready, the quit installs it first (ADR 0012).
A "Quit and Stop Agents" menu item covers the other intent.
That item asks the supervisor to kill every session, then quits.
The supervisor exits on its own after five idle minutes.
Idle means zero sessions and zero connected clients.

Before a bulk kill, the app marks each running supervised session as stopped by Halv.
Bulk kills are "Quit and Stop Agents" and a supervisor restart that ends sessions.
Some CLIs exit with code 0 on the kill signal.
The mark keeps such an exit from opening a continuation shell (ADR 0007).
The mark does not stop the attach thread, so the exit still updates the session.
A failed kill can stop part way.
The app then clears the mark on each session that the supervisor still lists as running.
During a quit, no call starts a new supervisor.
A late continuation shell then fails and does not outlive the quit.

## Continuation shells

A continuation shell needs no protocol change.
Its `continuedFrom` link travels in the session `meta` with the rest of the terminal session.
An older `meta` without the field reads as no link.
Adoption keeps the link after an app restart.
The prelude of the shell is not in `meta`, so a restart drops it.

## App updates

Amended in October 2026 with ADR 0012.
The supervisor outlives the app, so it also outlives an app update.
The new app adopts the sessions of a supervisor from an older build.
These rules keep that adoption safe.

1. Protocol 1 is the permanent baseline.
   Each hello frame sends `proto: 1`.
   No protocol 1 request or response changes or goes away.
   A test pins `PROTOCOL_VERSION` at 1, and fixture tests keep the protocol 1 lines.
2. The hello frames negotiate a revision.
   Each hello can send `max_proto`, the highest revision that the peer supports.
   A peer without `max_proto` supports revision 1, so Halv 0.5.7 to 0.7.1 still connect.
   Both peers use the lower value.
   A new revision only adds optional fields or new requests.
   The client sends a new request only when the negotiated revision includes it.
   An older supervisor closes the connection on an unknown request.
   The supervisor rejects a request that the hello did not negotiate.
3. The app replaces a stale supervisor only when it runs no session.
   The app must also show no running supervised session.
   The supervisor keeps the last output and the exit status of a session until the app reads them.
   A supervisor is stale when its build fingerprint differs from the bundled one.
   The app checks after the adoption and after each supervised session exits.
   It sends the non-destructive shutdown, then it starts the bundled supervisor.
   The old supervisor refuses the shutdown when a session started in the meantime.
   The app replaces a supervisor at most once in each run.
   During a quit, the app does not replace a supervisor.
4. The terminal panel does not show a stale supervisor notice.
   Settings shows the supervisor build and its state.
   The destructive restart stays in Settings, behind a confirmation.
5. A protocol mismatch never orphans a session.
   A supervisor can answer the hello with another protocol.
   Then the app does not mark the agent rows `exited`, and the panel does not resume the agent panes on its own.
   Each agent pane keeps the manual Resume / Start fresh choice.
   `terminal_supervisor_status` reports `protocolMismatch: true`.
   With rules 1 and 2, only a downgrade or a defect reaches this path.
6. The session metadata keeps its shape.
   A new `TerminalSession` field must be an `Option` or use `#[serde(default)]`.
   A frozen 0.7.1 metadata fixture must decode.

Rule 3 rarely runs for a heavy user, because the supervisor seldom has zero sessions.
A later phase gives each supervisor build its own socket.
Then a new supervisor serves new terminals while the older ones drain.

## Risks

Orphan processes: the idle timeout and the session list bound them.
App updates: the rules in "App updates" apply.
A stale supervisor can run old code until it has no session.
Code signing: the binary ships as a Tauri `externalBin` sidecar.
Tauri signs sidecars with the app identity in CI.
Double ownership: on unix, only the supervisor opens PTYs.
The app reconnects a dropped attach stream with capped exponential backoff.
The client resumes from the next output sequence and keeps the pane running.
The app marks the pane failed only when the session is missing or the protocol differs.

## Out of scope

Ghostty and external terminal modes.
The headroom proxy and the savings runtime.
