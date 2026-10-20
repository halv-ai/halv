# ADR 0015: Dollars saved as the savings metric

Status: Accepted

## Context

Halv showed "tokens saved" as its headline. JEV routes work to models that can use more tokens but cost less money, so a token count can go up while the bill goes down. The headline must show the money that Halv saves.

Halv already stored a dollar value per saving, but the value was wrong in three ways:

- Each saved token got the list input price of the model. In agent sessions most prompt tokens are cache reads. On Opus 5.5 usage, the average prompt token costs about $0.31 per million, not $4.
- Headroom counts the removed tokens again on every request. Most of the repeats sit in the cached prefix, where they would have been cache reads.
- Terminal panes had no model, so most saved tokens used a flat $1.25 per million.

The price table had only input prices and some stale entries (Claude Haiku 5.5 was 10 times too high; `gpt-6.1-sol`, `gpt-6.2-*` and Gemini had no entry).

An internal design study supports this decision. Pedro decided the open questions on 2026-10-15 (section 12 of the study).

## Decision

### The metric

Dollars saved for a period is the sum, over the savings events of the period, of the counterfactual cost minus the actual cost, in US dollars at API list prices. Each event stores its method and the price version that priced it. A later price change does not rewrite an old event.

Every user sees "saved at API prices". A subscription user pays a flat plan, so the dollars are the API value of the avoided work, not money back. The session meter adds one line for plan lanes ("You pay a flat plan...").

### Price cards

`src-tauri/src/pricing/prices.json` holds one card per model: input, cache write 5m, cache write 1h, cache read and output, in dollars per million tokens, plus an optional long-context tier. Each card names its source page and the date of the check. A card that Halv could not check on the provider page has a "NOT VERIFIED" note.

- The file has a `price_version` (a date). Each saving stores it.
- The server serves the same file at `GET /v1/prices`. The app asks once per day, keeps a valid newer table, and caches it in the app data folder. The embedded file is the fallback. `server/src/prices.json` is a copy, and an app test checks that the two files are equal.
- An unknown model gets the card of the agent kind's default model, with the `fallback` source. The flat $1.25 rate stays only in the frozen legacy table (`pricing/legacy.rs`), which old rows and old migrations use.

### Model and usage capture

A usage meter per agent session reads the session's own transcript after each poll (`agent_sessions/usage_meter.rs`):

- Claude, GLM and Grok sessions read the Claude Code transcript, its subagent files and its workflow agent files. A response counts once, by `message.id`.
- Codex sessions read the rollout (`turn_context` model and `token_count` totals).

The meter keeps the model and the token classes, and prices each request with its own card. The session stores the observed model and its billing (`api`, `plan` or `unknown`).

### Methods

`usd_method` names the method of each new row. NULL in an old row means `legacy_input`.

| Method | Tokens | Price |
| --- | --- | --- |
| `effective_rate` | Headroom tokens | The effective prompt price of the transcript window: the cost of all prompt classes divided by the prompt tokens. An empty window uses the session's running mix. |
| `default_mix` | Headroom tokens, no usage data | The default prompt mix of the price table on the card of the model. |
| `input_once` | RTK and crux tokens | The input price, once. crux tokens stay an estimate. |
| `mixed` | One record with both | The sum of both parts. |

Headroom's own dollar figure prices every token at the input rate, so Halv no longer uses it. A launch hint of another provider (a Codex id in a Claude pane) falls back to the kind default.

### History

The dollar metric starts at the upgrade date (migration 0057). Old rows keep their tokens and their legacy dollars. The headline sums only rows with a method. "All time" says "since {date}" and shows the tokens saved before that date on a separate line.

### Server and queue

- The client sends batches of 200 events (was 500). On HTTP 400, 413 or 422 it splits the batch. A single refused event becomes a dead letter, so one bad event cannot block the queue.
- New events add the optional fields `kind`, `net_usd`, `method` and `price_version`. `savings_usd` stays >= 0, because old servers refuse a negative value. Old servers ignore unknown fields.
- The server clamps the new fields and never refuses a batch because of them. Routing events have 0 tokens, so they never count toward a token cap.
- The savings summary adds `all_time_v2_usd`, `period_v2_usd`, `v2_since` and `all_time_routing_net_usd`. The admin view leads with dollars, keeps the legacy series, and marks the method change.

### Routing

JEV routing savings come from the JEV receipts and the JEV estimator (study sections 7 and 10, stages 6 to 8). ADR 0016 describes the estimator and its bench calibration. Until a routed chain with a price settles, a surface shows "Routing savings not estimated yet". `RoutingSavings` in `src/savingsMethod.ts` and the `kind = 'routing'` queue events are the seams. A total that includes a routing estimate shows "≈".

### Basic plan

The Basic plan no longer exists. The app does not show the Basic plan cap. The client still reads `capTokens` and `exhausted`, because the server still sends them.

## Consequences

- Displayed savings drop by about 10 times for cache-heavy agents. The new figure is the honest one.
- Wrong or stale prices bias every figure. The price version, the check date per card, the server endpoint and the "Prices as of {date}" line make a fix fast and visible.
- Halv now depends on the transcript formats of Claude Code and Codex. A format change sends sessions to `default_mix`, not to a wrong price.
- The native tray menu formats dollars in US English. The tray panel and the app use the app locale.

## Alternatives

- Restate old rows with a default mix: rejected (Pedro, Q1). It is an estimate on an estimate, and the server cannot change reported events.
- Price Headroom tokens at the cache read price only: rejected. Fresh compression sits in the cache-write part of the request, so the effective rate is closer.
- Ship price changes only with app releases: rejected (Pedro, Q7). Prices change faster than releases.
