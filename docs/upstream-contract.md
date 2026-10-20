# Halv Upstream Contract

Reference for M1 (ChatGPT OAuth auth) and M3 (streaming chat engine). All facts read from primary sources: `openai/codex` `codex-rs` (main as of 2026-07-01; cross-checked against installed codex-cli 0.142.0 binary) and installed `headroom-ai` 0.28.0 (the `headroom` package in a local Python 3.12 `site-packages`). Tags: **[verified]** = confirmed by adversarial verification against source; **[unverified]** = read from source during research but not independently re-verified.

---

## 1. OAuth flow (Sign in with ChatGPT — Codex CLI-compatible)

### 1.1 Endpoints
- Issuer: `https://auth.openai.com` (`DEFAULT_ISSUER`, `login/src/server.rs:53`). [verified]
- Authorize: `https://auth.openai.com/oauth/authorize` (built as `{issuer}/oauth/authorize?{qs}`). [verified]
- Token (auth-code exchange AND refresh): `https://auth.openai.com/oauth/token`. [verified]
- Refresh URL overridable via env `CODEX_REFRESH_TOKEN_URL_OVERRIDE`. [verified]

### 1.2 Client ID
- `client_id = app_EMoamEEZ73f0CkXaXp7hrann` (`login/src/auth/manager.rs:1444`; also present in the shipped 0.142.0 binary). [verified]
- Override env: `CODEX_APP_SERVER_LOGIN_CLIENT_ID` (ignored if empty/whitespace-only). [verified]

### 1.3 Callback / redirect URI
- `redirect_uri = http://localhost:{port}/auth/callback`; default port **1455**, fallback **1457**. [verified]
- Socket binds `127.0.0.1:{port}` while the redirect URI hostname is `localhost`. [verified]
- Fallback to 1457 triggers only when port == 1455 and bind fails AddrInUse after 10 retries at 200 ms (ports are on OpenAI's redirect-URI allow-list — do not invent other ports). [verified]

### 1.4 Scopes
- `openid profile email offline_access api.connectors.read api.connectors.invoke` (exact string, `server.rs:515`; identical in the 0.142.0 binary). [verified]

### 1.5 PKCE and authorize-URL query params
- PKCE `S256`: verifier = 64 random bytes → base64url no-pad; challenge = base64url(SHA256(verifier)) no-pad. [verified]
- Query params sent: `response_type=code`, `client_id`, `redirect_uri`, `scope`, `code_challenge`, `code_challenge_method=S256`, `id_token_add_organizations=true`, `codex_cli_simplified_flow=true`, `state`, `originator`, optional `allowed_workspace_id` (comma-joined). [verified]
- `state` is 32 random bytes base64url. [unverified — byte length from source narrative only]

### 1.6 Auth-code exchange
- `POST https://auth.openai.com/oauth/token`, header `Content-Type: application/x-www-form-urlencoded`. [verified]
- Body (values percent-encoded): `grant_type=authorization_code&code=...&redirect_uri=...&client_id=...&code_verifier=...`. [verified]
- Response JSON (all fields required): `{ "id_token", "access_token", "refresh_token" }`. [verified]

### 1.7 Refresh
- `POST https://auth.openai.com/oauth/token`, header `Content-Type: application/json` (note: JSON, not form-encoded, unlike the code exchange). [verified]
- Body: `{ "client_id": "<client_id>", "grant_type": "refresh_token", "refresh_token": "<token>" }` — **no `scope` field**. [verified]
- Response: `{ "id_token"?, "access_token"?, "refresh_token"? }` — all three optional. [verified]
- **Refresh tokens rotate (single-use).** If the response carries a new `refresh_token`, it must overwrite the stored one and be persisted before any further use; if absent, retain the old one. [verified]
- Failure classification by error code (from JSON body `error.code`, `error` as string, or top-level `code`, ASCII-lowercased): `refresh_token_expired`, `refresh_token_reused` (single-use reuse), `refresh_token_invalidated` — all permanent; unknown codes permanent only on HTTP 401, else transient. [verified]
- Proactive refresh trigger: decode `access_token` JWT `exp`; refresh when `exp <= now + 5 minutes`. Fallback when `exp` unavailable: refresh when `last_refresh < now - 8 days`. [verified]

### 1.8 auth.json shape (`~/.codex/auth.json` compatible)
```json
{
  "auth_mode": "chatgpt",
  "OPENAI_API_KEY": null,
  "tokens": {
    "id_token": "<raw JWT string>",
    "access_token": "<JWT>",
    "refresh_token": "<opaque>",
    "account_id": "<uuid or null>"
  },
  "last_refresh": "2026-07-01T00:00:00Z"
}
```
- Serde field names exactly as above; `OPENAI_API_KEY` is a serde rename of `openai_api_key` and is always emitted; every other top-level field is omitted when `None`. Newer optional keys: `agent_identity`, `personal_access_token`, `bedrock_api_key`. [verified]
- `id_token` is serialized **on disk as the raw JWT string** (Codex parses it in memory; a plain `String` field in Halv stays wire-compatible). [verified]
- `last_refresh` is chrono `DateTime<Utc>` → RFC 3339 UTC (e.g. `"2026-07-01T00:00:00Z"`). [verified]

### 1.9 Account-ID derivation
- `account_id` = base64url-decode the **id_token** JWT payload → object under namespaced claim `"https://api.openai.com/auth"` → field `chatgpt_account_id`. [verified]
- Same claim object also carries `chatgpt_plan_type`, `chatgpt_user_id`/`user_id`, `chatgpt_account_is_fedramp`; email from top-level `email` or `"https://api.openai.com/profile"`.`email`. [unverified — narrative]

---

## 2. Headroom proxy contract (headroom-ai 0.28.0, `127.0.0.1:8787`)

### 2.1 Bind / base URL
- Wrap CLI default port 8787; client base URL format is exactly `http://127.0.0.1:{port}/v1`. [verified]
- Server binds `HEADROOM_HOST` (default `127.0.0.1`) / `HEADROOM_PORT` (default `8787`). [unverified — narrative, `proxy/server.py:4061-4062`]

### 2.2 Accepted paths
- `POST /v1/chat/completions`. [verified]
- `POST` + WebSocket on all four of: `/v1/responses`, `/v1/codex/responses`, `/backend-api/responses`, `/backend-api/codex/responses` (all one handler; `/v1/chat/completions` is POST-only, no WS). [verified]
- `GET/POST/DELETE {prefix}/{sub_path}` on the same four prefixes → passthrough (ChatGPT auth → `https://chatgpt.com/backend-api/codex/responses/{sub_path}`). [verified]
- Catch-all `/{path}` for GET/POST/PUT/DELETE: honors `x-headroom-base-url` header first; else, when the request classifies as ChatGPT auth, forwards path+query verbatim to `https://chatgpt.com`. This is what makes arbitrary `/backend-api/*` calls (rate-limit polling, `/backend-api/me`, `/backend-api/accounts/check`) work through the proxy. [verified]
- `GET /v1/models`, `GET /v1/models/{id}` — intercepted under ChatGPT auth (see §2.6). [verified]
- `POST /v1/images/generations|edits` → `https://chatgpt.com/backend-api/codex/images/{generations|edits}` under ChatGPT auth. [unverified — narrative]

### 2.3 ChatGPT-subscription classification and routing
`_resolve_codex_routing_headers` (openai.py:670-685) — a request is routed as ChatGPT-subscription auth **iff**: [verified]
1. Any header key case-insensitively equals `chatgpt-account-id` → ChatGPT auth, headers unchanged; OR
2. The `Authorization: Bearer <token>` JWT (≥2 dots; payload base64url-decoded, no signature check) contains a non-empty string at `payload["https://api.openai.com/auth"]["chatgpt_account_id"]` → proxy **injects** `ChatGPT-Account-ID: <account_id>` and routes as ChatGPT auth. [verified]

Upstreams: ChatGPT auth → `https://chatgpt.com/backend-api/codex/responses` (HTTP) / `wss://chatgpt.com/backend-api/codex/responses` (WS); otherwise `OPENAI_API_URL` (default `https://api.openai.com`) + `/v1/responses`. [verified]

**Halv implication:** sending a valid ChatGPT access token as the bearer is sufficient for subscription routing; sending `ChatGPT-Account-ID` explicitly is also honored. [verified]

### 2.4 Header handling toward chatgpt.com
- HTTP responses path strips: `host`, `content-length`, `accept-encoding`, and all `x-headroom-*` (strip skippable only via env `HEADROOM_STRIP_INTERNAL_HEADERS=disabled`). May overwrite `openai-beta` with a session-sticky merged value. Forwards `Authorization`, `originator`, `user-agent`, `session_id` and other custom headers unchanged. [verified]
- WS handshake additionally drops hop-by-hop headers (`connection`, `upgrade`, `sec-websocket-*`, `transfer-encoding`), strips `x-openai-internal-codex-responses-lite`, and injects `Authorization: Bearer $OPENAI_API_KEY` from env **only** when the client sent no Authorization. [verified]
- Generic passthrough strips `host`, `accept-encoding`, `x-headroom-*`; uses an **HTTP/1.1** httpx client for `chatgpt.com` / `*.chatgpt.com` hosts (host-based, not path-based) because Cloudflare bot management challenges the HTTP/2 fingerprint on `/backend-api/me` and `/backend-api/accounts/check`. [verified]

### 2.5 Token refresh: the proxy does NOT do it
- Headroom never refreshes ChatGPT/OpenAI OAuth tokens; it forwards the client's bearer verbatim on every route (Codex's own refresh traffic is mere passthrough). The only auth injection is the WS `OPENAI_API_KEY` env fallback above. **Halv must keep its access token fresh itself** (§1.7). [verified]

### 2.6 Model registry via proxy
- `GET /v1/models` under ChatGPT auth: proxy fetches `https://chatgpt.com/backend-api/codex/models?client_version=0.130.0` (default; an inbound `client_version` query param overrides it), falling back to a static list on any failure. [verified]
- Static fallback `_CHATGPT_AUTH_CODEX_MODELS = ("gpt-5.5","gpt-5.4","gpt-5.3","gpt-5.2","gpt-5.1","gpt-5")`. [verified]
- Reason: generic `chatgpt.com/backend-api/models` returns **403** to OAuth bearer tokens; only `/backend-api/codex/models` works. [verified]

### 2.7 `GET /stats`
- Route on the proxy; `?cached=1` returns a 5-second-TTL snapshot; `recent_requests`, `request_logs`, `config` appear only for loopback callers. [verified]
- Keys include: `tokens.{input, output, saved, proxy_compression_saved, savings_percent, active_savings_percent, total_before_compression, ...}`, `requests.{total, cached, rate_limited, failed, by_provider, by_model, by_stack}`, `savings.by_layer.{cli_filtering, compression, prefix_cache, output_shaping}`, `cost`, `latency`/`overhead`/`ttfb` (`average_ms`,`min_ms`,`max_ms`), `codex_ws`, `litellm_available`, `summary`, `savings_history`, plus spread-in quota-tracker keys including `codex_rate_limits`. [verified]
- `POST /stats/reset` (loopback only) and `GET /stats-history` (json/csv) exist. [unverified — narrative]
- Dollar `cost` requires LiteLLM tables; `$0` otherwise. [unverified — narrative]

### 2.8 Rate-limit surfacing
- Captured from response headers `x-codex-primary-used-percent`, `x-codex-primary-window-minutes`, `x-codex-primary-reset-at` (+ `x-codex-secondary-*`), `x-codex-credits-has-credits`, `x-codex-credits-unlimited`, `x-codex-credits-balance`, `x-codex-promo-message`, `x-codex-limit-name`. [verified]
- Also from a throttled poll (min 60 s interval) `GET https://chatgpt.com/backend-api/wham/usage` using the **client's own** `Authorization: Bearer` + `ChatGPT-Account-Id` (both required; env override `HEADROOM_CODEX_USAGE_URL`). [verified]
- Note: current Codex reportedly no longer emits `x-codex-*` headers; `/wham/usage` is the live source, headers kept as legacy fallback. [verified — from source docstring]
- `/stats` exposes under `codex_rate_limits`: `{limit_id, limit_name, primary: {used_percent, window_minutes, window_label, resets_at, seconds_until_reset}, secondary, credits: {has_credits, unlimited, balance}, promo_message, captured_at}`. [verified]

### 2.9 What `headroom wrap codex` configures (for parity reference)
- Env: `OPENAI_BASE_URL=http://127.0.0.1:{port}/v1`. Config injected into `$CODEX_HOME/config.toml`: top-level `model_provider = "headroom"` and `openai_base_url = "http://127.0.0.1:{port}/v1"` (the latter is what intercepts subscription-auth traffic), plus `[model_providers.headroom]` with `base_url`, `supports_websockets = true`, `env_http_headers = { "X-Headroom-Project" = "HEADROOM_PROJECT" }`, and `requires_openai_auth = true` when auth.json `auth_mode == "chatgpt"` (case-insensitive; legacy fallback: non-empty `tokens.account_id`). [verified]

---

## 3. Responses API wire contract (ChatGPT-backed Codex endpoint)

### 3.1 Endpoint
- **`POST https://chatgpt.com/backend-api/codex/responses`** with SSE response (base constant `CHATGPT_CODEX_BASE_URL = "https://chatgpt.com/backend-api/codex"` + path `responses`); used for auth modes Chatgpt/ChatgptAuthTokens/AgentIdentity/PersonalAccessToken. API-key mode uses `https://api.openai.com/v1` instead. [verified]
- WebSocket variant: `wss://chatgpt.com/backend-api/codex/responses`. HTTP POST + SSE remains fully supported. [verified]
- Through Headroom, Halv targets `http://127.0.0.1:8787/v1/responses` and the proxy performs this routing (§2.3). [verified]
- Related endpoints on same base: `/responses/compact`, `/memories/trace_summarize`. [unverified — narrative]

### 3.2 Request headers
- `Authorization: Bearer <ChatGPT OAuth access token (JWT)>`. [verified]
- `ChatGPT-Account-ID: <account uuid>` (source literal casing; lowercased on the wire under HTTP/2 — matched case-insensitively upstream and by Headroom). [verified]
- `X-OpenAI-Fedramp: true` only for FedRAMP accounts. [verified]
- `originator: codex_cli_rs` — header name is lowercase `originator`; default value `codex_cli_rs` (override env `CODEX_INTERNAL_ORIGINATOR_OVERRIDE`; other first-party values `codex-tui`, `codex_vscode` [unverified — narrative]). [verified]
- `User-Agent: {originator}/{version} ({os_type} {os_version}; {arch}) {terminal}` — e.g. `codex_cli_rs/0.142.5 (Mac OS 26.1.0; arm64) WezTerm`. [verified]
- Per-request on the responses POST: `Accept: text/event-stream` (unconditional), `Content-Type: application/json`, `session-id: <uuid>`, `thread-id: <uuid>`, `x-client-request-id: <thread uuid>`. [verified]
- **`OpenAI-Beta` is NOT sent on the HTTP POST in the current contract.** It is used only on the WS handshake, value `responses_websockets=2026-02-06` (WS handshake without it → upstream 500). Legacy (v0.50-era) clients sent `OpenAI-Beta: responses=experimental` plus `session_id`/`conversation_id` headers — do not replicate that on new code. [verified]
- Optional headers: `x-openai-subagent`, `x-codex-beta-features`, `x-codex-turn-state` (sticky routing token echoed from the same-named response header), `x-codex-installation-id`, `x-oai-attestation`; optional zstd request compression sets `Content-Encoding: zstd`; built-in provider also sends `version: <CARGO_PKG_VERSION>`. [unverified — narrative]

### 3.3 Request body (`ResponsesApiRequest`)
```json
{
  "model": "gpt-5.5",
  "instructions": "<system prompt>",
  "input": [ /* ResponseItem[] */ ],
  "tools": [ /* flattened tool specs */ ],
  "tool_choice": "auto",
  "parallel_tool_calls": true,
  "reasoning": { "effort": "medium", "summary": "auto" },
  "store": false,
  "stream": true,
  "include": ["reasoning.encrypted_content"],
  "prompt_cache_key": "<conversation/thread uuid>",
  "service_tier": null,
  "text": { "verbosity": "low" },
  "client_metadata": { }
}
```
- Field set exactly as above: `instructions` omitted when empty; `tools`, `service_tier`, `prompt_cache_key`, `text`, `client_metadata` omitted when None; `tool_choice` is `"auto"`; `stream` is `true`. [verified]
- **No `temperature`, `top_p`, or `max_output_tokens` fields exist in the struct** — treat as unsupported on this backend. [verified] `previous_response_id` is absent from the HTTP struct (exists only in the WS `response.create` variant). [unverified — narrative]
- **`store` is explicitly serialized `false`** for chatgpt.com (it equals `is_azure_responses_endpoint()`; Azure is the only `true` case). Never send `store: true` to chatgpt.com. [verified]
- `include: ["reasoning.encrypted_content"]` is sent **whenever `reasoning` is set**; empty array otherwise. [verified]
- `reasoning` serializes only `effort`/`summary` on the standard path (a third optional `context: "all_turns"` key exists only for Responses-Lite). [verified]
- `reasoning.effort` wire values: `none | minimal | low | medium | high | xhigh | max | ultra` (+ custom passthrough); `ultra` is remapped to `max` before sending. [verified]
- `reasoning.summary` wire values: `auto | concise | detailed`; config `none` ⇒ key omitted. [verified]
- `input` items: serde `tag="type"`, snake_case — `{"type":"message","role":"user|assistant|developer","content":[{"type":"input_text","text":...} | {"type":"input_image","image_url":...,"detail"?} | {"type":"output_text","text":...}]}`; plus `reasoning` `{summary:[], encrypted_content}`, `function_call` `{name, arguments:<JSON string>, call_id}`, `function_call_output` `{call_id, output}`, `custom_tool_call`, `custom_tool_call_output`, `local_shell_call`, `web_search_call`. [unverified — narrative; `item` deserializing as `ResponseItem` in SSE is verified]
- `tools[]` are **flattened** (not nested under `"function"`): `{"type":"function","name","description","strict","parameters":{...}}`; `{"type":"custom","name","description","format":{...}}`; `{"type":"web_search",...}`. [unverified — narrative]

### 3.3.1 Stage B shell tool replay shapes
No live LLM/proxy round-trip was performed for Stage B. The shapes below are asserted in
unit tests against local fixtures and request builders, and keep the existing stateless
HTTP path: `store:false`, `include:["reasoning.encrypted_content"]`, no
`previous_response_id`, and upstream response item `id` fields stripped before replay.

Agent-mode requests send the flattened function tool in `tools[]` and keep
`tool_choice:"auto"`:

```json
{
  "type": "function",
  "name": "shell_command",
  "description": "Runs a shell command and returns its output.",
  "strict": false,
  "parameters": {
    "type": "object",
    "properties": {
      "command": {
        "type": "string",
        "description": "The shell command to execute as a shell script."
      },
      "workdir": {
        "type": "string",
        "description": "The working directory for the command. Defaults to the conversation cwd or project root."
      },
      "timeout_ms": {
        "type": "integer",
        "description": "The command timeout in milliseconds.",
        "default": 10000
      },
      "login": {
        "type": "boolean",
        "description": "Run the shell as a login shell.",
        "default": false
      },
      "sandbox_permissions": {
        "type": "string",
        "description": "Requested sandbox permissions for this command.",
        "enum": ["use_default", "with_additional_permissions", "require_escalated"]
      },
      "justification": {
        "type": "string",
        "description": "User-facing justification shown when approval is required."
      },
      "prefix_rule": {
        "type": "array",
        "description": "Reusable approval prefix for this command.",
        "items": { "type": "string" }
      },
      "additional_permissions": {
        "type": "array",
        "description": "Additional sandbox permissions requested by the model.",
        "items": { "type": "string" }
      }
    },
    "required": ["command"]
  }
}
```

Completed SSE tool calls arrive as `response.output_item.done` with an item shaped like:

```json
{
  "type": "function_call",
  "name": "shell_command",
  "call_id": "call_123",
  "arguments": "{\"command\":\"echo hi\"}"
}
```

After execution, the next full-history replay appends:

```json
{
  "type": "function_call_output",
  "call_id": "call_123",
  "output": "Exit code: 0\nWall time: 0.001 seconds\n[Total output lines: 1]\nOutput:\nhi"
}
```

### 3.3.2 Stage C apply_patch custom tool replay shapes
No live LLM/proxy round-trip was performed for Stage C. The shapes below are asserted in
local fixtures and request-builder/parser tests only; no live LLM calls were made.

Write-capable agent-mode requests add the flattened custom tool beside `shell_command`:

```json
{
  "type": "custom",
  "name": "apply_patch",
  "description": "Apply a patch to files. Input must use the Codex apply_patch envelope.",
  "format": {
    "type": "grammar",
    "syntax": "lark",
    "definition": "start: begin_patch hunk+ end_patch ..."
  }
}
```

Completed custom tool calls arrive as `response.output_item.done` with an item shaped like:

```json
{
  "type": "custom_tool_call",
  "name": "apply_patch",
  "call_id": "call_patch_123",
  "input": "*** Begin Patch\n*** Add File: hello.txt\n+hello\n*** End Patch\n"
}
```

After execution, the next full-history replay appends:

```json
{
  "type": "custom_tool_call_output",
  "call_id": "call_patch_123",
  "output": "{\"filesChanged\":1,\"message\":\"Applied patch to 1 file(s).\"}"
}
```

Shell function calls whose `command` is an `apply_patch` heredoc are intercepted locally
and routed through the same handler. Because the upstream item was still a
`function_call`, replay returns a `function_call_output` for that intercepted shell item.

### 3.4 SSE response events (corrected — see §4)
`Content-Type: text/event-stream`; each `data:` is JSON with a `"type"` discriminator. Event kinds matched by `process_responses_event` (codex-api/src/sse/responses.rs:315-449): [verified, uses corrected fact]

| Event `type` | Payload |
|---|---|
| `response.created` | `{response:{...}}` — stream start (requires `response` present) |
| `response.output_item.added` / `response.output_item.done` | `{item:{...}}`, item deserialized as `ResponseItem`; `done` with `item.type=="message"`, `content:[{type:"output_text",text}]` carries final assistant text |
| `response.output_text.delta` | `{delta:"<text chunk>"}` — render streaming text from this |
| `response.reasoning_summary_text.delta` | `{delta, summary_index}` — both required |
| `response.reasoning_text.delta` | `{delta, content_index}` — both required |
| `response.reasoning_summary_part.added` | `{summary_index}` |
| `response.custom_tool_call_input.delta` | `{delta, item_id\|call_id}` |
| `response.completed` | `{response:{id, usage?, end_turn?}}` — terminal success |
| `response.failed` | `{response:{error:{code, message, type?, plan_type?, resets_at?}}}` |
| `response.incomplete` | `{response:{incomplete_details:{reason}}}` |

- All other kinds are ignored by this parser (trace-logged) — **the parser must be tolerant of unknown event types**. [verified]
- `response.metadata` is **not** handled in `process_responses_event`; it is consumed separately in `process_sse_with_treatment` via `turn_state()` / `model_verifications()` / `turn_moderation_metadata()`, reading top-level `headers` (e.g. `x-codex-turn-state`, `openai-model`) and `metadata.openai_verification_recommendation` / `metadata.openai_chatgpt_moderation_metadata`. [verified]
- Failure `error.code` values seen in Codex tests: `rate_limit_exceeded`, `context_length_exceeded`, `insufficient_quota`, `cyber_policy`, `invalid_prompt`. [unverified — narrative]
- Useful response HTTP headers: `x-request-id`, `OpenAI-Model` (actual served model), `X-Reasoning-Included`, `x-codex-turn-state` [unverified — narrative]; rate-limit headers `x-codex-*` as in §2.8 [verified].

### 3.5 Usage / token accounting
- Usage arrives **only** on `response.completed` (never on deltas), at `response.usage`: [verified]
```json
{
  "input_tokens": 0,
  "input_tokens_details": { "cached_tokens": 0 },
  "output_tokens": 0,
  "output_tokens_details": { "reasoning_tokens": 0 },
  "total_tokens": 0
}
```
- All counters `i64`; both `*_tokens_details` objects are optional/nullable — default missing values to 0. [verified]
- Cache-write approximation used by Headroom: `input_tokens - cached_tokens`. [unverified — narrative]

### 3.6 Model slugs under ChatGPT auth
- Bundled Codex catalog (`models-manager/models.json`): `gpt-5.5`, `gpt-5.4`, `gpt-5.4-mini`, `gpt-5.3-codex`, `gpt-5.2` (+ hidden `codex-auto-review`); all with `supported_reasoning_levels: ["low","medium","high","xhigh"]`, `default_reasoning_level: "medium"`, `context_window: 272000`. [verified]
- Headroom static fallback: `gpt-5.5`, `gpt-5.4`, `gpt-5.3`, `gpt-5.2`, `gpt-5.1`, `gpt-5` — note this is **not a superset** of the catalog (omits `gpt-5.4-mini`, `gpt-5.3-codex`). [verified]
- Authoritative live list: `GET https://chatgpt.com/backend-api/codex/models?client_version=<ver>` with the same auth headers, returns `{"models":[{"slug":...},...]}`. [verified endpoint; response shape unverified — narrative]

---

## 4. Implementation warnings

### 4.1 Refuted-then-corrected facts (corrections applied above)
1. **SSE event list** — the original claim that `process_responses_event` handles `response.metadata` was **REFUTED**. Correction (applied in §3.4): `response.metadata` falls into the ignored `_` arm of `process_responses_event`; it is handled separately by `process_sse_with_treatment` (turn state / verification / moderation metadata). The original claim also **omitted `response.reasoning_summary_part.added`**, which IS a real match arm emitting `{summary_index}`. If Halv's SSE parser special-cases `response.metadata`, do it outside the main event loop and read top-level `headers` plus `metadata.*` keys; and do handle `response.reasoning_summary_part.added`. [verified correction]

### 4.2 Unverifiable facts
- None. Every atomic fact submitted to adversarial verification came back `confirmed` or `refuted-with-correction`; no fact received an `unverifiable` verdict. Statements above tagged [unverified] were read from source during research but not independently re-verified — treat them as lower-confidence, not as known-false.

### 4.3 Known hazards (all [verified] unless noted)
- **`store` must be `false`** on chatgpt.com; it is explicitly serialized (not omitted). Azure is the only `store: true` case.
- **Never send `temperature` / `top_p` / `max_output_tokens`** — the fields don't exist in the reference request struct for this backend.
- **Generic `chatgpt.com/backend-api/models` returns 403** to OAuth bearer tokens — use `GET /backend-api/codex/models?client_version=<ver>` (or Headroom's `/v1/models`).
- **Never leak `x-openai-internal-codex-responses-lite` upstream** — OpenAI rejects newer Codex models when it's present.
- **WS handshake requires `OpenAI-Beta: responses_websockets=2026-02-06`** or upstream returns 500; the HTTP POST path must NOT send `OpenAI-Beta`.
- **Cloudflare challenges non-browser HTTP/2 fingerprints** on chatgpt.com account endpoints (`/backend-api/me`, `/backend-api/accounts/check`). If Halv ever calls chatgpt.com directly (bypassing Headroom), configure reqwest for HTTP/1.1 to chatgpt.com hosts; through Headroom this is already handled.
- **Refresh tokens are single-use**: persist a returned rotated `refresh_token` before any retry; reuse yields permanent error `refresh_token_reused`. Design storage writes to be atomic.
- **Headroom will not refresh tokens** — M1's token manager must proactively refresh (`exp <= now + 5 min`) and every request through the proxy must carry a fresh bearer.
- **Reasoning replay**: with reasoning enabled, send `include: ["reasoning.encrypted_content"]` and replay returned `reasoning` items (with `encrypted_content`) in subsequent `input` — nothing is stored server-side (`store: false`).
- **Accept-encoding**: Headroom strips `accept-encoding` toward upstream (brotli from Cloudflare workers caused 502s). If bypassing the proxy, let the HTTP client negotiate encoding it can actually decode. [unverified — narrative rationale; strip behavior verified]
- Headroom's ChatGPT-auth classification requires a non-empty `chatgpt_account_id` string in the JWT claim (or an explicit `chatgpt-account-id` header, any case); an API key (non-JWT) bearer routes to `api.openai.com` instead — do not expect subscription routing with API-key auth.
