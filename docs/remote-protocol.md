# Halv remote protocol, version 1

Status: normative contract for the relay, `crates/halv-remote`, the desktop gateway, and the phone app.
ADR 0009 records the decisions. This document gives the exact formats.

The words MUST, MUST NOT, SHOULD, and MAY have their RFC 2119 meanings.

## 1. Terms

| Term | Meaning |
| --- | --- |
| Host | The Halv desktop app with the remote gateway turned on. |
| Device | A phone that runs the Halv mobile app. |
| Relay | The Cloudflare Worker and the `HostRelay` Durable Object class. |
| Host object | The one Durable Object instance for one host id. |
| Host socket | The WebSocket from the host to its host object. |
| Device socket | A WebSocket from a device to a host object. |
| Connection | One device socket, identified by a connection id. |
| Invite | A single-use admission for pairing. It comes from a QR code. |
| Session | A Noise channel between a paired device and the host. |
| Pane | A terminal session of the host. Its id is the terminal session id. |

## 2. Topology

```text
 Phone (Rust core)                Cloudflare                        Desktop (Halv app)
 ┌───────────────┐   WSS    ┌──────────────────────┐   WSS    ┌──────────────────────┐
 │ client engine ├─────────►│ Worker ─► HostRelay  │◄─────────┤ host engine          │
 │ Noise init.   │  device  │ (one object per host)│   host   │ Noise responder      │
 └───────────────┘  socket  └──────────────────────┘  socket  │ TerminalManager tap  │
                                                              └──────────────────────┘
```

- Both peers dial out. The host keeps one host socket open.
- Each device opens one device socket for each connection.
- The host object forwards Noise messages between the two sockets.
  The relay never sees plaintext.

## 3. Encodings

- `b64u`: base64url without padding (RFC 4648, section 5).
- Binary integers are big-endian.
- Times are Unix epoch milliseconds in a `u64`, unless the text says otherwise.
- Relay control messages are UTF-8 JSON text frames.
- Application messages are CBOR (RFC 8949) maps with text keys.
  Byte strings use CBOR major type 2.
- Every receiver MUST ignore unknown fields in maps and objects.

## 4. Identities and keys

### 4.1 Host identity

| Key | Algorithm | Use |
| --- | --- | --- |
| `host_noise` | X25519 | Noise static key of the responder |
| `host_relay` | Ed25519 | Signature for the relay host authentication |

The desktop stores both secrets in one keychain item.
The service is `Halv`.
The account is `remote-identity`, or `remote-identity-dev` in debug builds.
A keychain read error is not the same as a missing item.
After a read error, the gateway stays off and reports the error.
It MUST NOT make a new identity in that case, because a new identity cuts off every paired device.
The value is this JSON text:

```json
{"v":1,"noise":"<b64u 32-byte X25519 secret>","relay":"<b64u 32-byte Ed25519 seed>","createdAt":1760460000000}
```

### 4.2 Host id

```text
host_id = b64u( SHA-256( "halv-remote/host-id/v1" || 0x00 || host_relay_public ) [0..16] )
```

- The result has 22 characters and matches `^[A-Za-z0-9_-]{22}$`.
- The host id is the name of the host object (`idFromName(host_id)`).

### 4.3 Device keys

For each paired host, the device makes:

- `device_noise`: a new X25519 key pair,
- `relay_token`: 32 random bytes.

The device keeps both values in the iOS Keychain or the Android Keystore.
These items MUST be "this device only": they do not sync to other devices or backups.
`token_hash = b64u(SHA-256(relay_token))`.

### 4.4 Other ids

| Id | Format | Made by |
| --- | --- | --- |
| Device id | `b64u` of 16 random bytes (22 characters) | Host, at pairing |
| Invite id | `b64u` of 16 random bytes (22 characters) | Host, for each pairing offer |
| Connection id | `u32`, not zero, unique among the open device sockets of one host object | Host object |
| Request id | `u32`, not zero, unique among the open requests of one session | Device |

## 5. Pairing offer

### 5.1 URI

```text
halv://pair?p=<b64u(JSON)>
```

The JSON object has these fields:

| Field | Type | Rule |
| --- | --- | --- |
| `v` | integer | 1 |
| `r` | string | Relay base URL. `wss://` is required. `ws://` is allowed only for the hosts `127.0.0.1`, `::1`, and `localhost`. No trailing slash. |
| `h` | string | Host id (section 4.2) |
| `k` | string | `b64u` of the 32-byte `host_noise` public key |
| `s` | string | `b64u` of the 32-byte pairing secret |
| `e` | integer | Expiry time in epoch milliseconds |
| `n` | string | Host display name, at most 64 characters. Optional. |

The desktop shows the URI as a QR code and as text that the user can copy.
An offer lasts 5 minutes: `e` is at most 300,000 ms after the creation time.

The device accepts an offer only from its in-app QR scanner or from a code that the user pastes into the app.
In version 1, the device MUST NOT accept pairing offers from deep links, because any app can claim the `halv://` scheme.

The device MUST reject an offer that fails a rule above.
The device MUST reject an offer with `v` greater than 1 and ask the user to update the app.
The device MUST reject an offer when `e` is more than 60 seconds in the past.

### 5.2 Derived values

```text
invite_token = HKDF-SHA256(ikm = secret, salt = "halv-remote/pairing/v1", info = "relay-invite-token", L = 32)
pairing_psk  = HKDF-SHA256(ikm = secret, salt = "halv-remote/pairing/v1", info = "noise-psk",          L = 32)
```

- The device sends `invite_token` to the relay.
- `pairing_psk` never leaves the two peers.
- The host keeps `pairing_psk` in memory only, until the offer ends.

## 6. Relay protocol

### 6.1 Endpoints

| Method and path | Use |
| --- | --- |
| `GET /v1/hosts/{hostId}/host` | WebSocket upgrade for the host |
| `GET /v1/hosts/{hostId}/device` | WebSocket upgrade for a device |
| `GET /health` | Returns `200` with the body `ok` |

- The Worker MUST check `{hostId}` against the regex in section 4.2 before it routes to a host object.
- A request without `Upgrade: websocket` on an upgrade path gets `426`.
- An unknown path gets `404`.

### 6.2 Host authentication

The host sends these headers with the upgrade request:

| Header | Value |
| --- | --- |
| `X-Halv-Host-Key` | `b64u(host_relay_public)` |
| `X-Halv-Timestamp` | The current time in epoch milliseconds, as decimal text |
| `X-Halv-Signature` | `b64u` of the Ed25519 signature over the message below |
| `X-Halv-Client` | Informational, for example `halv-desktop/0.8.0` |

```text
message = "halv-relay/host-auth/v1" || "\n" || host_id || "\n" || timestamp
```

The host object MUST do these checks in this order:

1. All headers are present and well formed. Otherwise `400`.
2. The host id derived from `X-Halv-Host-Key` equals `{hostId}`. Otherwise `401`.
3. A stored host key, if present, equals `X-Halv-Host-Key`. Otherwise `401`.
4. The timestamp is within 300,000 ms of the relay clock. Otherwise `401` with the code `clock_skew`.
5. The timestamp is greater than the stored `last_auth_ts`. Otherwise `401` with the code `replay`.
6. The signature is valid. Otherwise `401`.

Then the host object:

1. stores the host key (first time only) and the new `last_auth_ts`,
2. closes an older host socket with `4409`,
3. closes every device socket with `4408`,
4. accepts the WebSocket with the tag `host`,
5. sends `{"t":"ready","v":1}`.

### 6.3 Device admission

The device sends `Authorization: Bearer <b64u token>`.
The token is a `relay_token` or an `invite_token`.

The host object MUST do these steps in this order:

1. Reject a missing or malformed token with `401`.
2. Reject the request with `503`, the code `host_offline`, and `Retry-After: 5` when no host socket is open.
3. Reject with `429` when 8 device sockets are already open.
4. Look up `SHA-256(token)`:
   - In `devices`: the connection kind is `device`.
   - In `invites`, not expired, and `tries` greater than 0: decrease `tries` by 1. The connection kind is `invite`.
   - An expired invite: delete it and reject with `401` and the code `invite_expired`.
   - No match: reject with `401`.
5. Choose a connection id, accept the WebSocket with the tags `device` and `c:<connection id>`, and store the attachment `{role, conn, kind, device | invite}`.
6. Send `conn_open` to the host socket.

A rejected request MUST NOT create storage in a host object.
The Worker checks the header format, the host id derivation, and the token format before it routes a request.
The host object creates its tables only after a host authenticates or after it reads a stored row.

### 6.4 Host socket frames

Binary frames carry data:

```text
offset 0   u8   type = 0x01 (DATA)
offset 1   u32  connection id
offset 5   ...  one Noise message (1 to 65,535 bytes)
```

- From the host object to the host: data that the device sent on that connection.
- From the host to the host object: data for that connection.
  The host object sends the Noise message to the device socket unchanged.
  If the connection does not exist, the host object answers with `conn_close`.
- The host object MUST answer any other type with `err` and drop the frame.

Text frames carry control messages.
Each message is a JSON object with a `t` field.
`rid` is a request id that the host chooses. The host object echoes it in `ok` or `err`.

Host to host object:

| `t` | Fields | Effect |
| --- | --- | --- |
| `invite_put` | `rid`, `invite`, `hash`, `exp`, `tries` | Add or replace an invite. At most 4 invites. `tries` is at most 5. |
| `invite_del` | `rid`, `invite` | Delete an invite. |
| `device_put` | `rid`, `device`, `hash` | Add or replace a device. At most 32 devices. |
| `device_del` | `rid`, `device` | Delete a device. Close its device sockets with `4401`. |
| `devices_sync` | `rid`, `devices: [{device, hash}]` | Replace all devices. Close the sockets of removed devices with `4401`. |
| `conn_close` | `conn`, `code`, `reason` (optional) | Close one device socket. `code` is in 4000–4999. |

Host object to host:

| `t` | Fields | Meaning |
| --- | --- | --- |
| `ready` | `v` | The host socket is open. |
| `ok` | `rid` | A control request succeeded. |
| `err` | `rid` (optional), `code`, `msg` | A control request or a frame failed. |
| `conn_open` | `conn`, `kind`, `device` or `invite` | A device socket opened. |
| `conn_close` | `conn`, `code` | A device socket closed. |

The host sends `devices_sync` after every `ready`.

### 6.5 Device socket frames

- A binary frame holds one Noise message of at most 65,535 bytes.
  The host object sends it to the host in a DATA frame.
- The text frame `ping` gets the auto-response `pong` without a wake.
  This is a fallback for clients that cannot send protocol pings.
- The host object closes the socket with `4400` for any other text frame.

### 6.6 Close codes and HTTP errors

| Code | Meaning | Device action |
| --- | --- | --- |
| `4400` | Bad frame or protocol error | Reconnect with backoff |
| `4401` | Unauthorized, or the device was revoked | Stop automatic retries for 5 minutes. Keep the keys. Show "Access revoked" only after the encrypted event `device.revoked` (section 8.5). Otherwise show "Not authorized". |
| `4408` | The host socket closed, or the host ended the connection | Reconnect with backoff |
| `4409` | A newer host socket replaced this one (host sockets only) | Host: stop this socket |
| `4410` | Pairing finished | Connect as a paired device |
| `4413` | Frame too large | Report a bug. Reconnect. |
| `4426` | Protocol versions do not overlap | Stop. Ask the user to update. |
| `4429` | Rate limited | Reconnect with a longer backoff |
| `4500` | Internal relay error | Reconnect with backoff |

Close codes come from the untrusted relay.
A device treats `4426` as "update needed" only when its own handshake showed that the version windows do not overlap.
Otherwise it treats `4426` like `4400`.
A host that gets `4409` waits 5 minutes before it connects again, and it stops after a second `4409` until the user turns the gateway on again.

Upgrade errors use HTTP status codes and the JSON body `{"code": "<code>"}`:
`400`, `401` (`unauthorized`, `clock_skew`, `replay`, `invite_expired`), `404`, `426`, `429`, and `503` (`host_offline`).

### 6.7 Limits

| Limit | Value |
| --- | --- |
| Open device sockets per host object | 8 |
| Paired devices per host object | 32 |
| Active invites per host object | 4 |
| Invite lifetime | 5 minutes at most |
| Invite tries | 5 at most |
| Device frame | 65,535 bytes |
| Host DATA frame | 65,540 bytes |
| Host control frame | 65,536 bytes |
| Worker upgrade rate | 30 for each client IP and path kind in 60 s. An IPv6 client counts by its /64 prefix. |
| Frames per socket | 400 per second on average, bursts up to 2,000 |
| Bytes per socket | 8 MiB per second on average, bursts up to 32 MiB |

A frame above its limit closes the socket with `4413`.
A socket above its frame or byte budget closes with `4429`.
The budgets live in memory only, so a wake resets them.

### 6.8 Keepalive

- Clients MUST send a WebSocket ping frame at least every 20 seconds.
- A client MUST close the socket and connect again after 60 seconds with no received frame and no pong.
- The host object sends no pings.
  The runtime answers pings without a wake.

### 6.9 Host object storage

```sql
CREATE TABLE IF NOT EXISTS meta    (k TEXT PRIMARY KEY, v TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS devices (device TEXT PRIMARY KEY, hash TEXT NOT NULL UNIQUE, created INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS invites (invite TEXT PRIMARY KEY, hash TEXT NOT NULL UNIQUE, exp INTEGER NOT NULL, tries INTEGER NOT NULL);
```

`meta` holds `host_key` and `last_auth_ts`.
The host object MUST NOT store frames.

### 6.10 Hibernation rules

The relay code MUST follow these rules:

- Accept sockets only with `ctx.acceptWebSocket`. Never call `ws.accept()`.
- Forward each frame with no `await` between the receive and the send.
  An `await` lets a later frame pass an earlier one, and the Noise layer then fails.
- Use no `setTimeout`, no `setInterval`, and no outgoing connections in the host object.
- Keep per-socket state in `serializeAttachment` (16 KiB at most).
- Read state again after a wake, because the constructor runs again.
- Turn off invocation logs for the Worker.
- Catch every exception in a handler. An uncaught exception resets the object.

## 7. Noise layer

### 7.1 Protocol names

| Use | Noise protocol name |
| --- | --- |
| Pairing | `Noise_IKpsk1_25519_ChaChaPoly_BLAKE2s` |
| Session | `Noise_IK_25519_ChaChaPoly_BLAKE2s` |

The device is the initiator. The host is the responder.
The device knows the host static key (`k` in the offer) before the handshake.

### 7.2 Prologue

```text
prologue = "halv-remote/noise/v1" || 0x00 || mode || 0x00 || host_id
mode     = "pair" | "session"
```

### 7.3 Pairing flow

1. The device opens a device socket with `invite_token`.
2. The host object sends `conn_open` with `kind: "invite"`.
3. The device sends handshake message 1 (`e, es, s, ss, psk`) with payload P1.
4. The host takes the PSK of the invite and reads message 1.
   On failure, the host sends `conn_close` with `4401`.
5. The host sends handshake message 2 (`e, ee, se`) with payload P2.
6. Both peers switch to transport mode.
7. The device sends the request `pair.request` (section 8.3).
8. Both peers compute the confirmation code of section 7.3.1.
   The device shows the code while it waits.
   The host shows the code, the device name, and the platform in its approval dialog.
   The user checks that the two codes match.
9. The host asks the user. On "Allow", the host:
   1. stores the device,
   2. sends `device_put` and waits for `ok`,
   3. sends `invite_del`,
   4. answers `pair.request`.
10. The device closes the socket.
    The host sends `conn_close` with `4410` if the socket is still open after 10 seconds.

### 7.3.1 Confirmation code

```text
code = decimal( u32_be( SHA-256( "halv-remote/sas/v1" || 0x00 || handshake_hash )[0..4] ) mod 1,000,000 )
```

- `handshake_hash` is the Noise handshake hash after message 2 (`get_handshake_hash()` in `snow`).
- The code is shown with 6 digits and leading zeros, for example `028 809`.
- The code protects against a person who photographs the QR code and tries to pair first.

### 7.4 Session flow

1. The device opens a device socket with `relay_token`.
2. The host object sends `conn_open` with `kind: "device"` and the device id.
3. The device sends handshake message 1 (`e, es, s, ss`) with payload P1.
4. The host reads message 1 and gets the remote static key.
   The key MUST belong to a paired device that is not revoked.
   That device id MUST equal the device id in `conn_open`.
   Otherwise the host sends `conn_close` with `4401`.
5. The host sends handshake message 2 (`e, ee, se`) with payload P2.
6. Both peers switch to transport mode.
   The device sends `hello` first.
7. The host marks the device as connected, and updates its "last seen" time, only after the first valid transport message.
   A replayed message 1 therefore changes no state.
   The host closes a session with `4400` when `hello` does not arrive within 15 seconds.
8. The host keeps at most 8 connections and at most 4 open handshakes. It closes extra connections with `4429`.

### 7.5 Handshake payloads

P1 and P2 are CBOR maps:

```text
{ "v": 1, "min": 1, "app": "halv-mobile/0.1.0" }    P1, from the device
{ "v": 1, "min": 1, "app": "halv-desktop/0.8.0" }   P2, from the host
```

- `v` is the highest protocol version of the sender. `min` is its lowest.
- The windows overlap when `peer.v >= self.min` and `self.v >= peer.min`.
- If the windows do not overlap, the host still sends message 2.
  Then it closes the connection with `4426`.
- P1 has no forward secrecy and an attacker can replay it.
  P1 MUST NOT contain secrets or commands.

### 7.6 Transport framing

Each Noise transport plaintext (at most 65,519 bytes) starts with a flag byte:

| Flag | Meaning |
| --- | --- |
| `0x00` | The rest is one complete application message. |
| `0x01` | The rest is a chunk. More chunks follow. |
| `0x02` | The rest is the last chunk. |

- The receiver joins chunks in order.
- A message of more than 8 MiB is a protocol error.
- Each Noise message travels in its own WebSocket frame.

### 7.7 Failures

- A decryption failure, an unexpected message, or a framing error ends the connection.
  The host sends `conn_close` with `4400` or `4401`. The device closes its socket.
- There is no retry inside a connection.
  The device connects again with a new handshake.
- Version 1 has no rekey.
  A connection MUST close after 24 hours.

## 8. Application protocol

### 8.1 Envelope

```text
request   { "t": "q", "id": u32, "m": text, "p": map }
response  { "t": "r", "id": u32, "ok": true,  "v": any }
          { "t": "r", "id": u32, "ok": false, "err": { "code": text, "msg": text } }
event     { "t": "e", "m": text, "p": map }
```

- In version 1, only the device sends requests.
- Both peers send events.
- The device waits 15 seconds for a response, and 130 seconds for `pair.request`.

### 8.2 Versions and capabilities

- The protocol window comes from the handshake payloads.
- `hello` exchanges capability lists. Each side ignores unknown capabilities.
- Version 1 host capabilities: `panes.v1`, `terminal.v1`, `terminal.paste.v1`, `device.unpair.v1`.
  The host adds `agent-status.v1` when `Pane.status` is available.
- An unknown method gets the error `unknown_method`.
- A receiver ignores an unknown event.

### 8.3 Pairing session

Only `pair.request` is allowed in a pairing session.

```text
pair.request  p: { "name": text (1–64), "platform": "ios" | "android" | text (1–32), "model": text? (at most 64),
                   "token": bytes(32), "app": text }
              v: { "device": text (device id), "host": text (host name) }
```

Errors: `denied`, `timeout` (no answer in 120 s), `busy`, `expired`, `bad_request`.

### 8.4 Session methods

| Method | Params | Result |
| --- | --- | --- |
| `hello` | `app`, `platform`, `caps: [text]` | `host: {name, app, os}`, `device`, `caps: [text]`, `time` |
| `panes.list` | — | `panes: [Pane]`, `rev: u64` |
| `panes.subscribe` | — | `panes: [Pane]`, `rev: u64`. Then `panes.changed` events. |
| `panes.unsubscribe` | — | `{}` |
| `terminal.subscribe` | `session`, `fromSeq: u64?` | `sub: u32`, `cols`, `rows`, `seq: u64`. Then terminal events. |
| `terminal.unsubscribe` | `sub` | `{}` |
| `terminal.input` | `session`, `data: bytes` (at most 4 KiB) | `{}` |
| `terminal.paste` | `session`, `text` (at most 64 KiB), `submit: bool` | `{}` |
| `device.unpair` | — | `{}`. Then the host revokes the device and closes with `4401`. |

A session has at most 8 terminal subscriptions.

### 8.5 Events

Host to device:

| Event | Payload |
| --- | --- |
| `panes.changed` | `panes: [Pane]`, `rev: u64` |
| `terminal.snapshot` | `sub`, `seq`, `cols`, `rows`, `data: bytes` |
| `terminal.data` | `sub`, `seq`, `data: bytes` |
| `terminal.resized` | `sub`, `cols`, `rows` |
| `terminal.exit` | `sub`, `code: i32?`, `signal: text?` |
| `terminal.closed` | `sub`, `reason: text` |
| `device.revoked` | — (the host closes the session right after this event) |

Device to host:

| Event | Payload |
| --- | --- |
| `terminal.ack` | `sub`, `seq` |

### 8.6 Types

```text
Pane {
  session:      text            terminal session id
  title:        text
  kind:         text            "claude" | "codex" | "kimi" | "glm" | "grok" | "gemini" | "shell" | other
  project:      text?
  cwd:          text?
  cols:         u16
  rows:         u16
  state:        "running" | "exited"
  exitCode:     i32?
  lastOutputAt: u64?
  status:       AgentStatus?    present with the capability agent-status.v1
}

AgentStatus {                   provisional, follows the desktop agent_status module
  state:     "working" | "done" | "needsInput" | "needsApproval" | "idle" | "exited" | "failed" | "interrupted" | text
             a receiver shows an unknown value as a neutral state
  reason:    text?
  updatedAt: u64
  unread:    bool
  prompt:    { kind: "approval", toolName: text, command: text?, filePath: text?, message: text? }
           | { kind: "question", text: text, options: [text]? }
           | null
}
```

### 8.7 Error codes

`bad_request`, `unknown_method`, `not_found`, `denied`, `timeout`, `busy`, `expired`, `unavailable`, `incompatible`, `too_large`, `internal`.

### 8.8 Terminal streaming rules

Subscribe:

- With `fromSeq`, when the ring still holds every chunk after `fromSeq`, the host sends those chunks as `terminal.data` events.
- Otherwise the host sends one `terminal.snapshot` with the full ring.
- The device resets its terminal before it writes a snapshot.

Live output:

- The host collects output for each subscription.
  It sends a `terminal.data` event 50 ms after the first collected byte, or at 32 KiB, whichever comes first.
- `seq` in `terminal.data` is the sequence number of the last chunk in the event.

Flow control:

- The device sends `terminal.ack` with the last applied `seq`.
  It sends at most one ack every 250 ms, and one ack after each snapshot.
- The host counts the bytes that it sent after the last acknowledged `seq`.
- Above 512 KiB, the host stops sending data for that subscription.
- When an ack brings the count below 128 KiB, the host sends a new `terminal.snapshot` and continues.

Other rules:

- The host sends `terminal.resized` when the PTY size changes.
- On a resume with `fromSeq`, the device applies `cols` and `rows` of the subscribe result before it writes the replayed data.
  When the pane exited while the device was away, the host sends `terminal.exit` after the replay.
- The host sends `terminal.exit` when the process exits. The subscription stays open.
- The host sends `terminal.closed` when the pane goes away. The subscription ends.
- The device terminal MUST NOT send terminal query replies to the host.

Paste:

- The host tracks `ESC [ ? 2004 h` and `ESC [ ? 2004 l` in the output of each pane.
  The scanner keeps its state across chunks.
- When bracketed paste is on, the host writes `ESC [ 200 ~`, the text without any `ESC [ 201 ~`, and then `ESC [ 201 ~`.
- When it is off, the host writes the text with each `\n` changed to `\r`.
- When the host does not know the mode, it uses bracketed paste for text with a line break.
  A visible marker is better than a partial prompt that runs too early.
- The host splits the bytes of one paste into writes of at most 32 KiB.
- When `submit` is true, the host then writes `\r`.

## 9. Host behavior

- The gateway is off until the user turns it on (`remoteEnabled` preference).
- The relay URL defaults to `wss://relay.halv.ai` in release builds and `ws://127.0.0.1:8791` in debug builds.
  The `remoteRelayUrl` preference (release) or `remoteRelayUrlDev` (debug), and the `HALV_RELAY_URL` variable, can change it.
  Each build profile keeps its own value, so a debug loopback URL never reaches a release build.
- Reconnect backoff: 1 s to 60 s with full jitter. The backoff resets after 60 s of connection.
- At most one pairing offer is active.
  An offer lasts 5 minutes. A cancel sends `invite_del`.
- The approval dialog shows the device name, the platform, and the confirmation code (section 7.3.1).
  No answer in 120 seconds means "deny".
- The device registry is the SQLite table `remote_devices`:
  host id, device id, name, platform, model, Noise public key (unique for each host id), token hash, created at, last seen at, revoked at.
  Every query filters by the host id of the current identity, so debug and release identities never see each other's devices.
- "Last seen" updates when a session starts and when a device disconnects.
- A revoke sets `revoked_at`, sends the event `device.revoked` on each open session of that device, sends `device_del`, and then closes those connections.
- The host registry decides revocation. A revoked key fails at message 1 even when the relay still admits the token.
- A reset of the identity revokes all devices and makes a new identity.
- Pairing, revoke, and reset commands accept only calls from the `main` webview.

## 10. Device behavior

- For each paired host, the device stores the host id, the relay URL, the host Noise public key, the host name, the device id, `device_noise`, and `relay_token`.
  The secrets go to the iOS Keychain or the Android Keystore.
- The device connects while the app is in the foreground.
- Reconnect backoff: 0.5 s to 30 s with full jitter.
- With `503 host_offline`, the device shows "Desktop offline" and tries again every 10 to 30 seconds.
- With `4401` or HTTP `401`, the device pauses automatic retries for 5 minutes and shows "Not authorized".
- After the encrypted event `device.revoked`, the device shows "Access revoked" and offers pairing again.
- The device never deletes stored keys because of a relay close code or an HTTP error.
  Only the user removes a host, or the host sends `device.revoked`.
- Pairing input comes only from the in-app QR scanner or a pasted code (section 5.1).
- After a reconnect, the device subscribes again with `fromSeq` set to the last applied `seq`.
- The terminal view is read-only: xterm.js with `disableStdin` and no `onData` forwarding.
  Input comes only from the input bar and the key row.

## 11. Security notes

Threat model:

- The relay operator and the network can drop, delay, reorder, replay, and inject frames.
  They can also read metadata.
- The QR code is visible only on the desktop screen.
- A lost phone can be revoked from the desktop.
- A compromise of the desktop is out of scope.

Properties:

- Confidentiality and integrity: Noise with ChaChaPoly. The relay has no key.
- Host authentication: the device pins the host Noise key from the QR code.
- Device authentication: the PSK at pairing, then the paired device key list.
- Forward secrecy: each session uses new ephemeral keys.
- Replay: a replayed message 1 gets a message 2, but the attacker cannot finish the handshake.
  P1 carries nothing that acts.

Metadata that the relay sees: host ids, device ids, connection times, frame sizes and counts, IP addresses, and the device name only if the user puts it into a pane.

Never log: tokens, secrets, keys, pairing URIs, or frame contents.

## 12. Reserved

This section is reserved. The section numbers stay the same, because the tests of `crates/halv-remote` refer to them.

## 13. Conformance tests

Each implementation MUST have tests for these cases:

1. Host id derivation and HKDF derivation with fixed test vectors.
2. Pairing URI round trip, and rejection of each invalid field.
3. Pairing and session handshakes between the host engine and the client engine.
4. A wrong PSK, an unknown device key, and a revoked device key each fail.
5. A replayed, reordered, or changed transport frame ends the connection.
6. Chunking: messages of 0 bytes, 65,518 bytes, 65,519 bytes, and 1 MiB.
7. Relay: host signature checks (bad key, skew, replay), invite tries and expiry, device admission, `devices_sync` closes removed devices, frame size limits.
8. Terminal streaming: snapshot, resume with `fromSeq`, flow control pause and new snapshot, bracketed paste.
9. End to end through a local relay: pair, connect, list panes, stream, type, revoke.

### 13.1 Shared test vectors

An independent Python implementation (`hashlib`, `hmac`) made these values.
The Rust crate and the relay MUST both reproduce them.

| Id | Input | Expected output |
| --- | --- | --- |
| V1 | `host_relay_public` = bytes `00 01 02 … 1f` | `host_id` = `nYqEorAQnfAioCLxARz8mA` |
| V2 | pairing secret = 32 bytes of `0x42` | `invite_token` = `70c315d483322856d11594f882598c861c3900dd734726428bb806d57c86c247` (hex) |
| V2 | same | `pairing_psk` = `08ae975f36336b98f5d6f415b0706f143f283d5e5d52d8d0219c8aa9e417a0e2` (hex) |
| V3 | token = bytes `20 21 22 … 3f` | `token_hash` = `ctu3M2x2eAAj-D2kw1Xy7uqFczsT00d2l5F3kMEikIQ` |
| V4 | host id of V1, timestamp `1760460000000` | signed message = `halv-relay/host-auth/v1\nnYqEorAQnfAioCLxARz8mA\n1760460000000` |
| V5 | handshake hash = 32 bytes of `0x11` | confirmation code = `280809` |
