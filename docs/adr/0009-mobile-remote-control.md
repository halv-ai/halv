# ADR 0009: Mobile remote control through an end-to-end encrypted relay

Status: Accepted

## Context

Halv runs coding agents in terminal panes on the desktop.
The user wants to watch and control these agents from a phone, as Orca Mobile does.
The phone must work from any network, not only from the local network.

The owner set two constraints on 2026-10-14:

- End-to-end encryption is mandatory.
  No server can read terminal output, input, or keys.
- The phone reaches the desktop through a relay in the cloud.
  The relay runs on Cloudflare.

Facts from the current code:

- The PTY supervisor owns every agent PTY on unix (ADR 0008).
  Several clients can attach to one session.
  An attach can resume from a sequence number.
- `TerminalManager` sends all terminal output to the webview through `terminal_event_sink` (`src-tauri/src/terminal.rs`).
  The app keeps a scrollback ring with the same sequence numbers as the supervisor.
- A closed window only hides the app.
  A clean quit stops the app but keeps the supervisor and its agents.
- For terminal (TUI) panes, the agent status is a timing heuristic in the webview (`src/terminalActivity.ts`).
  The backend cannot tell "needs approval" from "finished".
  A separate work item adds a backend agent status from Claude and Codex hooks.
- The webview CSP allows only `ipc:` in `connect-src`.
  Network clients must run in Rust.

Orca solves the same problem with a hosted relay fleet on Google Cloud.
That design has a director, relay cells, Postgres, and a push gateway.
Halv needs a smaller design.

## Decision

### Components

1. **Relay.** A Cloudflare Worker and one Durable Object for each desktop host.
   The object uses SQLite storage and the WebSocket Hibernation API.
   The relay source is not in this repository.
   The desktop and the phone both dial out to the relay over WSS.
   The relay forwards opaque frames and never decrypts them.
2. **Shared crate.** `crates/halv-remote` contains the protocol code without a Tauri dependency:
   keys, the pairing offer, the Noise handshakes, framing, and the application messages.
   The `host` feature adds the host engine.
   The `client` feature adds the client engine.
   The desktop and the phone run the same protocol code.
3. **Desktop gateway.** `src-tauri/src/remote/` runs inside the app process.
   It connects the host engine to `TerminalManager`, storage, the keychain, and preferences.
   The gateway is off by default.
   The user turns it on in Settings > Mobile.
4. **Phone app.** A separate Tauri 2 mobile project, with the identifier `ai.halv.mobile`.
   The phone app source is not in this repository.
   The UI uses React and xterm.js, like the desktop.
   The Rust core uses the client engine of the shared crate.

`docs/remote-protocol.md` is the normative wire contract for all four components.

### Identity and keys

- The desktop host has two long-term keys:
  - an X25519 key for Noise,
  - an Ed25519 key for relay authentication.
- The keychain stores both keys in one item: service `Halv`, account `remote-identity`.
  Debug builds use the account `remote-identity-dev`.
- The host id is a hash of the Ed25519 public key.
  The host id names the Durable Object.
- The phone makes a new X25519 key and a new 32-byte relay token for each desktop that it pairs with.
  The phone keeps them in the iOS Keychain or the Android Keystore.
- A reset of the host identity revokes every paired phone.
- A keychain read error never makes a new identity.
  The gateway stays off and reports the error.
- The phone keeps its secrets in "this device only" items.
- The phone deletes its keys only when the user removes the desktop, or after the encrypted `device.revoked` event.
  A relay error never deletes keys.

### Pairing

1. The desktop shows a QR code with a `halv://pair?...` URI.
   The URI holds the relay URL, the host id, the host Noise public key, a 32-byte pairing secret, an expiry, and the host name.
2. HKDF derives two values from the pairing secret:
   an invite token for the relay and a pre-shared key (PSK) for Noise.
   The relay sees only the invite token.
3. The desktop registers the SHA-256 hash of the invite token in the Durable Object.
   The invite expires after 5 minutes and allows 5 attempts.
4. The phone connects with the invite token.
   It runs the `Noise_IKpsk1_25519_ChaChaPoly_BLAKE2s` handshake.
   The phone pins the host key from the QR code.
   The desktop checks the PSK and learns the phone public key.
5. The phone sends a pair request with its name, its platform, and its new relay token.
6. Both sides show a 6-digit confirmation code from the handshake hash.
   The desktop asks the user to allow the phone and to compare the codes.
   On "Allow", the desktop stores the device, registers the hash of the relay token, and deletes the invite.
7. The phone closes the pairing connection and connects again as a paired device.

A pairing secret is good for one phone only.
The relay operator cannot pair a phone, because the relay never sees the PSK.
The phone accepts an offer only from its in-app scanner or a pasted code.
It ignores `halv://` deep links, because any app can claim that scheme.

### Sessions

- A paired phone connects with its relay token.
- The phone and the desktop run `Noise_IK_25519_ChaChaPoly_BLAKE2s`.
  The phone pins the host key.
  The desktop accepts only the public key of a paired device that is not revoked.
- Each connection makes a new handshake.
  The ephemeral keys give forward secrecy.
- One Noise message goes into one WebSocket binary frame.
  Larger application messages are split into chunks.
- A lost, reordered, or replayed frame makes decryption fail.
  The connection then closes, and the phone connects again.
  The terminal stream resumes from the last sequence number.

### Relay protocol

- The desktop opens `/v1/hosts/{hostId}/host`.
  It signs a timestamp with its Ed25519 key.
  The Durable Object stores the public key on the first connection and checks the signature on every connection.
- A phone opens `/v1/hosts/{hostId}/device` with a bearer token.
  The token is an invite token or a device relay token.
  The Durable Object compares the token hash with its stored hashes.
- The desktop socket carries the frames of all phones.
  Each binary frame starts with a connection id.
  JSON text frames carry control messages: invites, devices, and connection events.
- The desktop is the source of truth for paired devices.
  It sends the full device list to the Durable Object after each connection.
- Hibernation rules:
  - The Durable Object uses no timers and no outgoing sockets.
  - It does not store frames.
  - Clients send WebSocket protocol pings for keepalive.

### Remote API

The application messages travel inside the Noise channel and use CBOR.
Version 1 contains:

- `hello`, with a protocol version window and capability lists,
- `panes.list` and `panes.subscribe`,
- `terminal.subscribe`, `terminal.unsubscribe`, `terminal.ack`, `terminal.input`, and `terminal.paste`,
- `device.unpair`.

Each side ignores unknown fields.
Each side rejects a peer whose version is outside its window.

### Terminal streaming

- The gateway taps the output in `TerminalManager`.
  It uses the same sequence numbers as the supervisor ring.
- Version 1 has a watch mode only.
  The phone shows the pane at the desktop size, and the PTY keeps its size.
- The phone terminal never answers terminal queries.
  The desktop pane already answers them.
  The phone sends input only from its input bar and its key row.
- The gateway collects output for 50 ms before it sends a frame.
- Each subscription has a window of 512 KiB of unacknowledged output.
  When a phone falls behind, the gateway stops the deltas.
  It sends a new snapshot after the phone catches up.
- The gateway follows the bracketed paste mode of each pane from the output.
  `terminal.paste` uses bracketed paste only when the pane turned it on.

### Desktop lifetime

The gateway runs while the app runs, also with a hidden window.
A quit stops the gateway.
The agents continue in the supervisor, and the phone shows "desktop offline".

### Cost controls

- The relay runs on the Workers Paid plan.
  The Free plan fails at about 54 active users.
- The Durable Object hibernates between messages.
- Invocation logs are off for the relay Worker.
- The desktop collects output into larger frames, because the relay bills each incoming message.
- A rate limit at the Worker protects the upgrade and pairing paths.

## Consequences

- The phone works from any network, with no account and no port forwarding.
- The relay sees metadata: host ids, connection times, frame sizes and counts, and IP addresses.
  It does not see content.
- Halv now runs a cloud service.
  A relay deploy drops every WebSocket, so both clients must reconnect and resume.
- Issue #32 holds the next hardening steps: a device PSK in the session handshake against recorded traffic, replay timestamps in message 1, rekey, padding, and stale command rejection.
- The phone app needs App Store and Play Store review.
  Guideline 4.2.7(a) is a risk for a remote client that uses a cloud relay.
  Orca Mobile and Happy passed review with this model.

## Risks

- A snapshot can start inside an escape sequence.
  The phone resets its terminal before it writes a snapshot.
- iOS stops sockets in the background.
  The phone reconnects on resume.
  Push notifications fill this gap later.
- The relay is a single path.
  There is no LAN fallback in version 1.
- The phone and the desktop can type into the same pane at the same time.
  The input bar sends one complete write, which keeps collisions rare.
- A stolen phone keeps access until the user revokes it on the desktop.

## Out of scope for version 1

- Push notifications.
  They need the backend agent status, APNs and FCM keys, and a native plugin.
- The attention inbox and one-tap approvals.
  They need the backend agent status.
- A drive mode that resizes the PTY to the phone.
- A direct LAN or Tailscale path.
- Desktop-served phone UI bundles.
- Relay access tied to a Halv account or plan.

## Related records

- ADR 0007 says that the Codex bridge accepts approvals automatically (lines 39-41).
  Commit 7680569 changed this, so that text is out of date.
  Remote approvals for GUI sessions can use `agent_session_respond_permission`.
- ADR 0008 and issue #11 excluded network listeners.
  This decision keeps that rule.
  The gateway dials out and opens no listener.
