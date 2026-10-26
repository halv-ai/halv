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
[github.com/halv-ai/halv](https://github.com/halv-ai/halv/releases/latest).
Releases before 1.0.2 are in [pedr0v/halv-releases](https://github.com/pedr0v/halv-releases/releases).
The app updates itself. It checks this repository first and pedr0v/halv-releases second.

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
The flag clears the onboarding flags at startup.
The flag also treats the account as signed out, so the Halv Cloud pitch shows again after the first result.
The flag does not reset the usage events answer.

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
  - Halv Cloud routing, only when you allow brief sharing. A routing request sends the task brief to halv.ai. See [Task briefs](#task-briefs).
- **Analytics events to halv.ai.** Analytics events are opt-in. Halv sends none until you say yes, and you can change the answer in Settings.
  `src-tauri/src/analytics.rs` lists every event. Each event carries a random install ID, the app version, the operating system and the event time.
  It also carries the download id when the install came through a site link.
  - Version 1 events: setup steps, first task steps, Halv Cloud trial and checkout steps, and one `active_day` event for each day you use Halv.
  - Version 2 events: a daily usage summary (`usage_summarized`), `crash_detected`, `proxy_failed`, `app_updated` and `update_failed`.
    Version 2 also adds `agent_kind` and `auth_method` to `account_connected`, `surface` to `trial_prompted`, and the language of the UI (`locale`) to each batch.
  - Halv sends version 2 data only after a "Yes" to the current consent question. An install that said yes before version 2 stays on version 1 until it answers the version 2 notice.
  - The daily usage summary holds the agents you used and a bucket for the session count. It holds the tokens and dollars saved, rounded to 2 significant digits. It also says whether you used crux and routing, and which plan you have.
  - The Halv server forwards the events to PostHog (EU), a data processor, without an IP address.
  - Analytics events never hold prompts, code, file names, terminal output, account emails, API keys, exact savings or stack traces.
  - The full contract is in [ADR 0022](docs/adr/0022-analytics-v2.md).
- **GitHub**, to check for app updates.
- **The relay at relay.halv.ai**, only when you connect a phone (Halv Cloud). The messages are end-to-end encrypted.

Halv also downloads the newest price table from halv.ai once a day. That request sends no user data.
Halv has no other telemetry.

## Task briefs

Halv Cloud routing picks a model tier for a task before a coding agent gives the task to a worker.
To pick the tier, Halv sends the task brief to the Halv server.

What a brief holds:

- The coding agent writes the brief as free text.
  The agent gives the brief to `route_task` or `route_tasks`, two tools of the local `halv-savings` MCP server.
- The brief holds what the agent puts in it. That can be code, file paths, commands, or secrets.
  Halv does not redact the brief.
- `route_task` accepts a brief, with its investigation evidence, of up to 16,000 characters.
  `route_tasks` accepts up to 4,000 characters for each brief and up to 64,000 characters for each batch.
- The MCP server checks these limits before it sends a brief, in `src-tauri/src/savings_mcp.rs`.
  One exception: for a `route_task` call without a stage, only the Halv server checks the 16,000-character limit.
  The Halv server refuses a longer brief.

When Halv sends a brief:

- From version 1.0.2, Halv sends a brief only after you say yes to brief sharing.
- A session routes tasks only when task routing is on in Settings and your answer is yes.
- A "yes" applies to sessions that start after it. A session that started without routing stays without routing.

How Halv asks:

- Halv asks only when all of these conditions are true:
  - You are signed in.
  - Your plan includes Halv Cloud.
  - Task routing is on in Settings.
  - You did not answer yet.
- When you start a Claude or Codex session, Halv asks before the session starts.
  The question holds the session until your answer saves.
  - If Halv does not know your account status yet, for example just after app start, the session starts without routing.
    A card asks the question later.
  - Halv counts a launch as your action only when you clicked or pressed a key in the 10 seconds before it.
- When a session starts without your action, it starts without routing. A card then asks the same question.
  Examples are a session that Halv restores at app start, a local automation, and a session from the phone remote.
- The buttons are "Allow" and "Don't send".
- If you close the question without an answer, the session starts without routing. Halv sends no brief.
  Halv does not ask again until the next app start.
- You can change the answer in Settings, under JEV, "Task briefs".

The check before each brief:

- Before each brief, the local `halv-savings` MCP server reads local data. This check makes no network call.
- The MCP server sends nothing in these cases:
  - You are not signed in (reason `sign_in_required`).
  - The last known plan of your account has no Halv Cloud (reason `halv_cloud_required`).
  - Your answer in `preferences.json` is not "allowed" (reason `brief_sharing_off`).
- In these cases, the coordinator agent does the task itself.
- The MCP server reads your answer before each brief. A "no" in Settings stops the next brief of each running session.
- When the app does not know your plan yet, the MCP server sends the brief, and the Halv server checks your plan.
- When Halv cannot save a "no" to `preferences.json`, it writes the empty file `brief-sharing-declined` in the same folder.
  The MCP server reads that file as "no".
- If Halv cannot write either file, Halv shows an error. Running sessions can then still send briefs.

Where the brief goes:

1. The MCP server sends the brief to `api.halv.ai/v1/route`.
2. The Halv server sends the brief to OpenRouter. OpenRouter runs the JEV router model of TypeSafe.
3. The Halv server returns the rating of the router to the MCP server. The MCP server uses the rating to pick the model tier.

What the Halv server keeps:

- The Halv server does not store or log the brief.
- It keeps a request counter for each account, for the rate limits. It deletes each counter within about one day.
- It keeps your routing savings in dollars. It keeps the raw rows for up to 90 days. After that, it keeps daily totals.

OpenRouter and TypeSafe process the brief under their own terms.
Read the [OpenRouter privacy policy](https://openrouter.ai/privacy) and the [TypeSafe privacy policy](https://typesafe.ai/legal/privacy-policy).
Neither OpenRouter nor TypeSafe publishes a retention period for prompts.
Each policy gives only a general rule: the company keeps data as long as reasonably necessary.

Other facts:

- Cloud handoff in Automations is off in 1.0.2. The app sends no automation prompt to halv.ai.
  A Cloud handoff automation from an older version stays paused and does not run.
- Brief sharing and analytics are separate.
  Your analytics answer does not change brief sharing, and brief sharing does not turn on analytics.

## License

Halv is licensed under the Apache License, Version 2.0.
See [LICENSE](LICENSE), [NOTICE](NOTICE), and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

"Halv" is a trademark of Pedro Villaca.
The Apache License does not grant permission to use the Halv name or logo.
