# Halv

Halv (halv.ai) is a desktop app for Claude Code, Codex, and other coding agents.
Halv runs a local Headroom proxy between your agents and their model providers.
The proxy cuts the context tokens that each request sends.
Halv shows the dollars that it saved for you, at API prices.
Halv also gives you terminals, tabs for each project, and a live status for each agent.
Halv runs on macOS, Windows, and Linux.

## Halv Cloud

Halv Cloud is an optional paid plan. Its features run on Halv servers: JEV model routing, automatic account rotation, and the phone relay.
The app works without Halv Cloud. All local features are free.
For more information, go to [halv.ai](https://halv.ai).

## Install

Download the latest release from
[github.com/pedr0v/halv-releases](https://github.com/pedr0v/halv-releases/releases/latest).
The app updates itself from the same repository.

## Building from source

You need:

- The Rust stable toolchain
- Node.js 22 and pnpm

Run these commands from the repository root:

```sh
pnpm install
pnpm fetch:rtk
pnpm fetch:crux
pnpm build:supervisor
pnpm tauri dev
```

The release installers include a frozen Headroom runtime.
Halv builds that runtime from a private fork of Headroom.
A build from this repository does not include the frozen runtime.
Instead, the app uses one of these Headroom executables:

- The file that `HALV_HEADROOM_BINARY` points to.
- A `headroom` executable on `PATH`.

A release build needs the frozen runtime that `scripts/headroom-runtime.lock.json` locks.
To build that runtime from a Headroom checkout, run:

```sh
pnpm build:headroom-runtime -- --source /path/to/headroom
pnpm verify:headroom-runtime
python scripts/smoke-headroom-runtime.py src-tauri/resources/headroom-runtime
```

RELEASING.md gives the full release procedure.

To replay onboarding, set `VITE_HALV_RESET_ONBOARDING=1`.
Example: `VITE_HALV_RESET_ONBOARDING=1 pnpm tauri dev`.
The flag clears the onboarding flags at startup and skips the launch trial gate.

## Tests

```sh
pnpm test
cd src-tauri && cargo test -- --skip live_
pnpm test:visual
```

- `pnpm test` runs the frontend unit tests and the packaging contract tests.
- `cargo test -- --skip live_` runs the Rust tests. The command skips the `live_` tests.
- `pnpm test:visual` runs the Playwright visual regression suite.

The visual suite needs Chromium. Install it once with `pnpm exec playwright install chromium`.
After an approved UI change, run `pnpm test:visual:update` to update the baselines.
`pnpm shots` exports all registered states as 2x PNG files into `shots/`.

## Project layout

- `src/` — React UI shell for chat, settings, and savings views.
- `src-tauri/` — Tauri v2 Rust shell.
- `src-tauri/src/auth/` — authentication boundary.
- `src-tauri/src/sidecar/` — Headroom lifecycle boundary.
- `src-tauri/src/chat/` — chat engine boundary.
- `src-tauri/src/storage/` — persistence boundary.
- `src-tauri/tauri.conf.json` — Halv app metadata, identifier, window config, and frontend build hooks.
- `crates/halv-remote/` — protocol crate for the phone remote control.
- `docs/` — architecture decision records (`docs/adr/`) and protocol documents.

## Data the app sends

Your agents send their requests to their own model providers through the local proxy.
Halv sends data to these places only:

- **The account service at halv.ai**, when you sign in:
  - Sign-in and billing.
  - Savings reports. A report holds token counts and dollar amounts. It holds no prompts, code, or file contents.
  - Halv Cloud routing. A routing request sends the task brief to halv.ai.
- **Analytics events to halv.ai.** Halv sends only the events that `src-tauri/src/analytics.rs` lists, with a random install ID and the app version.
  You can turn off these events in Settings.
- **GitHub**, to check for app updates.
- **The relay at relay.halv.ai**, only when you connect a phone (Halv Cloud). The messages are end-to-end encrypted.

Halv also downloads the newest price table from halv.ai once a day. That request sends no user data.
Halv has no other telemetry.

## License

Halv is licensed under the Apache License, Version 2.0.
See [LICENSE](LICENSE), [NOTICE](NOTICE), and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

"Halv" is a trademark of Pedro Villaca.
The Apache License does not grant permission to use the Halv name or logo.
