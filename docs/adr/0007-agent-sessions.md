# ADR 0007: Agent sessions

Status: Accepted

Superseded in part by ADR 0020: in a workspace, a new conversation follows the launcher choice, not the Pro toggle. See the section "Pro mode".

## Context

Halv needs resumable sessions with official agent command-line tools.
Each tool uses a different stream protocol and authentication method.
The frontend needs one stable event model for all supported tools.

## Session model

The Rust backend owns the `agent_sessions` registry.
The registry limits the app to eight active sessions.
Claude and Codex sessions own long-lived child processes.
Kimi sessions start one child process for each turn.
The registry stores session state and completed turn usage in SQLite.

The backend emits normalized events on `agent-session-event`.
Each payload contains the Halv session ID and one tagged event.
The model covers initialization, turns, text, tools, permissions, usage, exits, and protocol errors.
Malformed lines produce protocol errors without stopping the output reader.

## Adapters

The Claude adapter starts Claude Code with stream JSON input and output.
It maps Claude records to the normalized event model.
It stores the Claude session ID for later resume operations.

The Codex adapter starts `codex app-server` in the session working directory.
It uses newline-delimited JSON-RPC messages without the `jsonrpc` field.
The adapter initializes the server, then starts or resumes a thread.
It stores the Codex thread ID in `cliSessionId`.

The Codex adapter maps turn, text, command, patch, and tool notifications.
Codex 0.147 reports turn usage in `thread/tokenUsage/updated`.
The adapter retains the latest usage until `turn/completed` arrives.

Codex approval requests produce `permissionRequest` events.
The current bridge accepts command and patch requests after it emits the event.
A later change must connect frontend decisions to these requests.

The Kimi adapter starts Kimi Code with `--prompt` and `--output-format stream-json`.
Current Kimi Code has no stream JSON input mode.
Halv starts one process for each turn and resumes with `--resume <session-id>`.
The adapter maps OpenAI-shaped assistant, tool, and resume records.

## Account and credential sourcing

`agent_session_lane_options` returns lanes for Claude, Codex, Kimi, GLM, and Grok.
Subscription lanes include a login status when Halv can determine it safely.
Halv checks CLI status or credential path presence.
Halv never reads an authentication file.

API lanes return eligible credential profiles.
Claude uses Anthropic profiles.
Kimi uses Moonshot profiles.
GLM uses Z.ai credential profiles for Coding Plan API keys.
Grok uses xAI credential profiles for API keys.
The frontend sends a profile ID, not a secret.
The backend resolves the keychain secret when it starts the child.
Legacy callers can still provide a raw key.

`agent_login_launch` starts the supported CLI login flow in the built-in terminal.
The command returns the terminal session ID.
The frontend opens that terminal so the user can finish the login flow.
Kimi uses its `login` subcommand when help lists it.
Otherwise, Halv starts the interactive Kimi CLI.

## Claude lanes

The `claudeSubscription` lane uses the CLI subscription login.
Halv does not add authentication variables for this lane.
Halv starts its managed Headroom proxy for nested Codex commands.
Claude traffic stays on the Claude request path.
Halv gives the child `HEADROOM_PROJECT=<session-id>`.
Halv also gives the child an isolated `CODEX_HOME`.

The `claudeApiKey` lane selects an Anthropic credential profile.
The backend sets `ANTHROPIC_API_KEY` only in the child environment.
The lane can also set `ANTHROPIC_BASE_URL` for a caller-supplied proxy.

The `custom` lane reserves an environment list for advanced callers.

## GLM Coding Plan lane

The `zaiGlm` lane uses the Claude adapter and a selected GLM Coding Plan profile.
Halv sets `ANTHROPIC_AUTH_TOKEN` and `API_TIMEOUT_MS=3000000`.
Halv removes any inherited `ANTHROPIC_API_KEY` from the child.

The lane maps Sonnet and Opus to `glm-5.2` by default.
It maps Haiku to `glm-4.7` by default.
Callers can override each mapping.

The Claude child uses `http://127.0.0.1:<port>/p/<session-id>` as `ANTHROPIC_BASE_URL`.
Halv sends the Z.ai upstream URL through `ANTHROPIC_CUSTOM_HEADERS`.
The pinned Headroom runtime consumes this header on `/v1/messages`.
It removes the header before upstream forwarding.
The project route gives each session an exact Headroom savings rollup.

## Grok API-key lane

The `grokApiKey` lane uses the Claude adapter against xAI's
Anthropic-compatible Messages endpoint. Halv sets `ANTHROPIC_AUTH_TOKEN`,
removes any inherited `ANTHROPIC_API_KEY`, and routes the Claude child through
the managed Headroom project endpoint. `X-Headroom-Base-Url` carries
`https://api.x.ai` as the true upstream.

The lane maps Sonnet and Opus to `grok-4.6` by default. It maps Haiku to
`grok-build-0.1`. Callers can override each mapping.

xAI marks the Anthropic compatibility endpoint as deprecated. A native Grok
Build adapter is the fallback if xAI removes that endpoint; it is not part of
this lane.

## Engine upstream allowlist

Halv starts the engine with `HEADROOM_ALLOWED_BASE_URLS`.
This list puts the SSRF guard of Headroom in allow-only mode.
The list holds these hosts:

- The loopback hosts `localhost`, `127.0.0.1` and `[::1]`.
- The upstream host of each lane: Anthropic, Z.ai and xAI.
- The host of `ANTHROPIC_BASE_URL` in the Halv environment.
- The host of each registered session base URL.

The engine gives HTTP 400 to an `X-Headroom-Base-Url` host that is not on the list.
A `claudeApiKey` session with a new base URL host goes direct until the next engine start.
Halv does not restart a running engine for a new host.
A restart stops the requests of all other sessions.

Halv adds only a host that Python `urllib.parse` can read.
A bracketed host must be an IPv6 address without a zone ID.
Each other host holds only ASCII letters, digits, `.`, `-` and `_`.
A bad base URL sends only its own session direct.
The other lanes keep the engine.

Limit: the session row does not hold the session base URL.
Thus a restored `claudeApiKey` pane with a custom base URL host gets HTTP 400 after an app restart.
A new session with the same base URL registers the host again.
The host then applies at the next engine start.

## Codex lane

The `codexChatGpt` lane keeps ChatGPT OAuth inside the Codex CLI.
The `proxied` field controls Headroom routing.
The direct lane does not change the Codex configuration.

The proxied lane starts the managed Headroom sidecar first.
Halv reads the selected port from the running sidecar status.
Halv passes private `-c` overrides to each Codex invocation.
Halv does not edit the user configuration file.

`chatgpt_base_url` uses the `/backend-api/codex` route.
`openai_base_url` uses the `/p/<session-id>/v1` project route.
Halv also sets `HEADROOM_PROJECT=<session-id>`.
The bundled runtime forwards HTTP, WebSocket, and subpath traffic to ChatGPT.
The project route or header gives each session an exact savings rollup.

Non-Codex sessions receive a temporary `CODEX_HOME`.
Halv copies the user config and changes routing only in the temporary copy.
The temporary provider uses the selected proxy port.
Its `env_http_headers` maps `X-Headroom-Project` to `HEADROOM_PROJECT`.
Halv links authentication, instruction, skill, plugin, rule, and memory entries.
It copies regular files when a link fails.
Nested session files stay in the temporary home.
This design excludes usage from unrelated Codex processes.
Halv removes the temporary home when the agent session exits.

## Kimi lanes

The `kimiApiKey` lane selects a Moonshot credential profile.
Halv sets the Kimi model, key, provider, and base URL variables in memory.
The base URL uses `/p/<session-id>/v1` on an isolated Headroom proxy.

The isolated proxy uses stateless mode.
It sets `OPENAI_TARGET_API_URL=https://api.moonshot.ai/v1`.
Its in-memory project counters provide measured session savings.
Halv stops the proxy when the session exits.

The `kimiSubscription` lane does not set Kimi authentication or routing variables.
Kimi Code uses the existing CLI login.
Halv starts Headroom for nested Codex commands.
Kimi traffic stays on the Kimi request path.

Current Kimi stream JSON does not include usage in documented records.
The parser accepts usage if a later CLI adds it.
Otherwise, the completion event contains zero usage.

## Savings semantics

| Lane | Route | Savings semantics |
| --- | --- | --- |
| `claudeSubscription` | Claude direct; nested Codex through Headroom | Measured nested Codex delta |
| `claudeApiKey` | Claude direct or caller proxy; nested Codex through Headroom | Measured nested Codex delta |
| `codexChatGpt`, `proxied=false` | Direct | Estimated from reported cache-read usage |
| `codexChatGpt`, `proxied=true` | Shared Headroom project route | Measured Headroom turn delta |
| `zaiGlm` | Shared Headroom project route | Measured Headroom turn delta |
| `grokApiKey` | Shared Headroom project route | Measured Headroom turn delta |
| `kimiApiKey` | Isolated Headroom project route | Measured Headroom turn delta |
| `kimiSubscription` | Kimi direct; nested Codex through Headroom | Measured nested Codex delta |
| `custom` with Claude | Caller parent; nested Codex through Headroom | Measured nested Codex delta |
| `custom` with Codex | Caller-defined | Estimated from reported cache-read usage |

Since 1.0.1 the Halv Engine (Headroom) is optional and off by default. While it is off, each lane goes direct to its upstream URL. The nested Codex home stays without the Headroom provider, so nested Codex usage stays measured. Native Chat mode still starts the engine on demand at the first send.

## Usage semantics

Claude result cost is cumulative for the CLI session. Halv stores the delta from the previous result as the turn cost.

Claude result usage omits Task agent usage. Halv reads usage fields from the local main transcript and its Task agent transcripts.

Halv subtracts the transcript snapshot taken before the turn. Duplicate message identifiers count once.

Halv reads cumulative usage from each nested Codex rollout in the temporary home. It adds the turn delta to parent usage.

Headroom project counters remain the authoritative savings source. Nested Codex rollouts provide input, output, and cache usage.

## Pro mode

ADR 0020 changes this section in part. In Pro mode, an empty workspace shows a launcher. An agent choice opens an agent tab, and "Text conversation" opens a chat conversation. The toggle keeps its meaning for the composer and for an open conversation.

The frontend shows one Pro mode toggle.
Normal mode uses the GUI chat adapter.
Pro mode uses Ghostty when its probe is ready.
Otherwise, Pro mode uses the built-in terminal.
The frontend stores the toggle in local storage.
A caption identifies the terminal that Pro mode will open.

Both terminal paths run each official CLI in interactive mode.
The launch specification contains the program, arguments, environment, and removed environment names.
GUI and terminal adapters share the same lane builders.

The built-in terminal injects the launch specification into the existing PTY manager.
The terminal session and agent session use the same identifier.

Ghostty mode requires macOS Ghostty 1.3 or newer.
The probe verifies the required AppleScript properties.
Halv sends the Ghostty script through `osascript` standard input.
This method keeps API keys out of logged process arguments.

Ghostty receives lane variables through its environment property.
These variables apply only to the new window process.
Halv disables Ghostty mode when the property is unavailable.

Terminal modes poll Headroom savings every 15 seconds.
Each successful poll stores one measured sample with zero usage tokens.
Ghostty polling stops when Ghostty exits or after four hours.
Halv cannot kill a Ghostty window through the agent session registry.

Composer conversations change surfaces at a resumable turn boundary.
The first GUI turn captures the CLI session ID from the stream protocol.
When Pro mode becomes active, Halv stops the GUI process after that turn.
Halv then starts an interactive terminal with `--resume <cliSessionId>`.
The terminal replaces the GUI transcript before the next turn.
When Pro mode becomes inactive, Halv stops the terminal process.
Halv then starts a GUI process with the same resume ID before the next turn.
This decision rule applies to each resumable agent kind.
Halv cannot close a Ghostty window, so this switch leaves that window open.
The user must close the old Ghostty window before using the resumed GUI session.

Built-in terminal tabs can contain a binary split tree.
Each split records the user-selected direction and the first-pane ratio.
Halv never infers a direction when it restores a saved tree.
The renderer stores localStorage layout version 1 per panel scope.
Closing a leaf collapses its parent to the remaining sibling.

## Exit to shell

The built-in terminal still runs each agent CLI as the direct PTY child.
When the CLI exits, its terminal session ends and the agent session finishes as before.
A clean self-exit then continues the pane in a plain login shell.
A clean self-exit has exit code 0, no signal, and no Halv stop.
Halv stops include a pane close, an agent kill, a bulk kill, and the app quit.
The lifecycle thread opens the shell before it finishes the agent session.

The shell is a new terminal session with its own identifier.
Its `continuedFrom` field names the ended agent session.
The panel binds the same pane to the shell, so the identity rule above stays true.
The pane shows the last agent screen above the shell prompt.
This prelude lives in memory only.

The shell gets the plain-shell environment of the active wallet account.
No agent key, session route, ledger, or Custom-lane variable crosses into the shell.
Every other ended agent pane keeps its exit card with an "Open shell here" action.
Closing the shell pane forgets the ended agent terminal and its agent row.
A saved shell pane restores as a shell, not as a resumed agent.

A plain shell can also open next to any pane: the split "Terminal" choice, the header "+" button, or the `/shell` command.
The shell starts in the source pane folder and keeps its project and conversation.
Plain shells do not pass through the trial gate.
They never set the conversation status or title.

## Probe contract

The probe returns `claude`, `codex`, `kimi`, and `ghostty` entries.
The GLM and Grok presentations use the Claude probe because both use the
Claude adapter.
Agent entries report the executable path, version, and probe error.
Each agent probe runs its binary with `--version`.
Kimi discovery checks `kimi` before `kimi-code`.

## Claude Code models in chat

The composer adds Opus, Sonnet, and Haiku after the Claude login probe succeeds.
The wire IDs use the stable Claude CLI aliases `opus`, `sonnet`, and `haiku`.
The conversation stores `claude_session` in `agent_backend` and the alias in `agent_session_model`.
Each conversation reuses one GUI Claude session while that process remains active.
Halv resumes the CLI session after the user kills its process in the sessions panel.

Claude session events feed the normal chat delta stream.
Halv stores the final assistant text as an ordinary conversation message.
Tool starts become compact transcript notes in the assistant stream.
Turn usage feeds the compact session savings meter above the composer.

The first regenerate version resends the last user message in the same Claude session.
Image attachments are not supported for Claude session conversations in this version.
The native route remains unchanged when `agent_backend` is null.

The Ghostty entry reports presence, version, and AppleScript readiness.
Readiness includes the command, working directory, and environment properties.

## Security

The profile resolver reads keychain secrets only when it starts a keyed lane.
Halv does not send profile secrets to the frontend.
Presence checks inspect only authentication paths.
The temporary Codex home links or copies existing Codex files without logging them.
Halv keeps the temporary Codex home, the Claude settings file, and the Kimi agent files in `<app data>/agent-sessions/<session-id>`.
That directory has mode 0700, and no sandbox write root contains it.
A sandboxed command therefore cannot change a file that an agent CLI loads later.
The Claude adapter scans local transcripts for usage fields and message identifiers.
It does not log or persist transcript content.
Halv does not put OAuth data or API keys in logs, events, snapshots, or SQLite.
API keys exist only in session memory and child environments.
