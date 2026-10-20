# Halv — Architecture

Desktop chat client (macOS / Windows / Linux) that routes every LLM request through a
local **Headroom** proxy, which compresses prompts and responses (60–90% token
savings). Users sign in with their **ChatGPT account** (Codex OAuth flow) to spend
their plan limits; an OpenAI **API key** is an optional fallback. The app surfaces
real-time token/cost savings.

The proxy and the agents run on the device. An optional account service at
halv.ai handles sign-in, billing, savings reports, and Halv Cloud routing.
README.md lists the data that the app sends.

## Local diagnostics and support reports

Halv keeps a local activation record (first/last activation and launch count) in
its SQLite app data and can run local storage, settings, and bundled-Headroom
health checks from Settings. Halv does not upload diagnostics. A support report
is created only after the user chooses **Export support report**, is saved to a
user-selected local JSON file, and can be reviewed before sharing.

Reports contain app/platform metadata, activation timestamps, aggregate record
counts, and component health. They never contain prompts or message contents,
secrets or credentials, account identifiers, project names or raw project paths,
hostnames or device identifiers, or file contents.

## Stack

| Layer | Choice |
|---|---|
| Shell | Tauri v2 (Rust, stable toolchain) |
| Frontend | React 18 + TypeScript + Vite + Tailwind CSS |
| Package manager | pnpm |
| Conversation storage | SQLite via `rusqlite` (bundled) |
| Secret storage | OS keychain via `keyring` crate |
| Compression backend | Headroom proxy (external CLI; automatic install pinned to 0.28.0) |

## Components (Rust, `src-tauri/src/`)

1. **`auth/`** — "Sign in with ChatGPT" via the same OAuth 2.0 PKCE flow Codex CLI
   uses (auth.openai.com, localhost callback on port 1455). Reuse `~/.codex/auth.json`
   tokens when present. Refresh handling. Tokens stored in OS keychain, never in
   plaintext config. Optional API-key auth as a second provider.
2. **`sidecar/`** — Headroom proxy lifecycle: spawn and retain Halv's own
   `headroom proxy --host 127.0.0.1 --port <p>` child, preferring port 8787 and
   selecting an unused loopback port when it is occupied. Halv never reuses or
   sends credentials to a pre-existing listener; readiness and `/stats` polling
   require the managed child to remain alive. If the `headroom` binary is missing,
   the guided uv/pipx installer pins the architecture-verified `headroom-ai==0.28.0`;
   discovery of already-installed user binaries remains unchanged.
3. **`chat/`** — Streaming chat engine (SSE). One local endpoint for both modes:
   send Responses-API requests to the managed proxy's dynamic
   `http://127.0.0.1:<port>/v1/responses` endpoint; the Headroom
   proxy picks the upstream per request (verified in headroom 0.28.0 source,
   `providers/proxy_routes.py` + `proxy/handlers/openai.py::_resolve_codex_routing_headers`):
   - **Subscription mode**: include `Authorization: Bearer <oauth access_token>` +
     `chatgpt-account-id: <account_id>` (or rely on the JWT's
     `https://api.openai.com/auth.chatgpt_account_id` claim) → proxy routes to
     `https://chatgpt.com/backend-api` (Codex responses endpoint). Proxy also serves
     the model list from `backend-api/codex/models` and forces HTTP/1.1 to
     chatgpt.com (Cloudflare rejects its HTTP/2 fingerprint).
   - **API-key mode**: plain `Authorization: Bearer <api key>` → proxy routes to
     `api.openai.com`.
   Model picker includes gpt-5.5 family with reasoning effort (low→xhigh).
4. **`storage/`** — SQLite: conversations, messages, per-message token/savings
   snapshots.
5. **Frontend (`src/`)** — thread sidebar, streaming markdown chat view with code
   blocks, model/effort picker, savings HUD (per-message + lifetime, sourced from
   proxy `/stats`), settings (auth, proxy port/mode, API key).

## Data flow

```
UI (React) ──IPC──▶ Tauri (Rust chat engine)
                        │  HTTP/SSE
                        ▼
          Halv-managed Headroom proxy (127.0.0.1:<port>)
                         ← compresses input & output
                        │
        ┌───────────────┴────────────────┐
        ▼                                ▼
chatgpt.com/backend-api (OAuth)   api.openai.com (API key)
```

## Milestones

- **M0 Scaffold** — Tauri v2 + React/TS/Vite/Tailwind project builds and runs on
  macOS; Rust modules stubbed with wired IPC commands; CI-friendly layout.
- **M1 Auth** — ChatGPT OAuth PKCE + `~/.codex/auth.json` reuse + keychain + API key.
- **M2 Sidecar** — Headroom detect/spawn/health/stats.
- **M3 Chat** — streaming conversations (both upstream modes) persisted to SQLite.
- **M4 UI** — savings dashboard, settings, polish.
- **M5 Packaging** — dmg / msi / AppImage; Headroom install story for end users.

## Open questions

- Distribution of Headroom itself (Python CLI) inside a desktop bundle — MVP detects
  a system install and guides the user; bundling comes at M5.

## Verified facts (2026-07-01, local machine)

- Headroom 0.28.0: loopback proxy with health/savings via `GET /stats` and
  per-request upstream routing as described in `chat/` above. Halv owns the child
  and may choose a non-default port when 8787 is unavailable.
- `~/.codex/auth.json` shape: `{auth_mode, OPENAI_API_KEY, tokens: {id_token,
  access_token, refresh_token, account_id}, last_refresh}`.
