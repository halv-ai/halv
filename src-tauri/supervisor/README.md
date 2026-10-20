# Halv PTY supervisor

The `halv-supervisor` process owns PTY child processes for Halv. It uses one JSON object on each line of a Unix socket. Every connection sends a `hello` request first. Protocol version 1 supports `open`, `write`, `resize`, `close`, `remove`, `list`, `update_meta`, `shutdown`, and `attach`. The server sends one response for each control request. An attach connection receives one snapshot, zero or more data events, and one exit event. Data fields use standard padded base64. A data event contains the UTF-8 bytes that produced its sequence number.

Protocol 1 is the permanent baseline. Both hello frames always send `proto: 1`, and no protocol 1 request or response changes. The hello request and the hello response can also send `max_proto`, the highest revision that the peer supports. A peer that sends no `max_proto` supports revision 1. Both peers use the lower of the two values. A new revision only adds optional fields or new requests. The client sends a new request only when the negotiated revision includes it, because an older supervisor closes the connection on an unknown request. The supervisor rejects a request that the hello did not negotiate. Tests in `src/protocol.rs` keep the protocol 1 lines that Halv 0.5.7 to 0.7.1 send.

Run `halv-supervisor --socket <path> [--log <path>] [--idle-timeout-secs <n>]`. The default idle timeout is 300 seconds. The process creates the socket with mode `0600`. It starts a new session, ignores `SIGHUP` and `SIGPIPE`, and writes standard output and standard error to the log. It uses `/dev/null` when no log path is set.

The supervisor is a single instance for one socket. A new process connects to an existing socket and completes the protocol hello. It exits with status 0 and does not print text when another instance answers. It removes a stale socket only when the socket rejects a connection.

The idle timer runs only when the supervisor has no sessions and no clients. A running session, an exited session, a control client, or an attach client resets the timer. The supervisor removes its socket when the full idle time passes.

A client disconnect does not stop a PTY child. The reader thread continues to add decoded output to the 256 KiB ring. A later client can list the session and attach to the retained output. The `close` request sends `SIGHUP` through the portable PTY child killer. The session stays in the list after exit until a client sends `remove`.

The `shutdown` request controls quit behavior. A request with `kill_sessions: false` fails when a child is running. A request with `kill_sessions: true` closes every running child before the supervisor exits. Exited sessions do not block either shutdown form. The server sends and flushes the shutdown response before it stops.

## Agent hook helper

`halv-supervisor agent-hook <agent>` is the Claude Code hook helper of Halv agent panes. It does not start a server. It prints `{}`, reads the hook payload from standard input, and appends one short JSON line to `$HALV_AGENT_STATUS_DIR/<id>.jsonl`, where `<id>` is `$HALV_AGENT_SESSION_ID`. Helpers that run at the same time take turns on `<id>.jsonl.lock`, so only one moves a full journal to `<id>.jsonl.1`. Then it sends the id as one datagram to `$HALV_AGENT_STATUS_DIR/wake.sock`. It exits with status 0 in every path. ADR 0011 describes the journal.

The supervisor removes each inherited `ORCA_*` variable from the environment of a new session.

## Packaging

The packaging script writes `src-tauri/binaries/halv-supervisor-<target-triple>[.exe]`. Tauri installs it beside the Halv executable. Set `HALV_SUPERVISOR_BINARY` to use a different binary.
