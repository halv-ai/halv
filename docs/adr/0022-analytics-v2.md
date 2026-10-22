# ADR 0022: Analytics v2

Date: 2026-10-22
Status: Accepted

This ADR extends ADR 0021. It adds five usage events and three facts to the "What is sent" list of ADR 0021. It changes decision 9 and decision 10 of ADR 0021. It also changes the event table and the sentence about event facts. All other decisions of ADR 0021 stay.

The owner approved this change on 2026-10-22.

## Context

The 13 events of ADR 0021 cover the funnel up to the first result. `active_day` covers retention. These events cannot answer five questions:

- Which agents do users run?
- How many tokens and dollars do users save each week?
- Does the Headroom proxy fail?
- Does the app close in a bad way?
- Does the app update work?

The answers need a few new events. The volume must stay low. Each install sends at most one summary event each day. No event holds content.

A new event changes the "What is sent" list. A user who said "yes" to the old list did not say "yes" to the new list. So the new data needs a new answer.

## Decision

1. **Consent has a version.** `analyticsConsentVersion` is `1` or `2`. The value is the list version that the "on" answer covers. An install that answered "on" before this ADR has version 1. A new "on" answer has version 2.
2. **Version 1 keeps the version 1 list.** An install with version 1 sends the 13 events of ADR 0021 and nothing else. It sends no version 2 event, no version 2 field and no `locale`.
3. **One notice asks for version 2.** `analyticsConsentAsked` stores the newest list version that the app asked about. When consent is `on` and `analyticsConsentAsked` is less than 2, the app shows a banner: "Halv now also sends a daily usage summary". "Yes" sets version 2. "No" keeps version 1. Each answer sets `analyticsConsentAsked` to 2. Without an answer, the banner shows again on the next launch. The banner never blocks work.
4. **Five events are new.** See the table "New events".
5. **Three facts are new.** `account_connected` gets `agent_kind` and `auth_method`. `trial_prompted` gets `surface`. Under version 2, each batch carries `locale`, the language of the UI.
6. **The app rounds and buckets the values.** The exact value never goes into a usage event. See "Rounding".
7. **The summary starts at consent.** `usage_summarized` covers only the UTC days on or after the day that the install moved to version 2. The app never summarizes a day before consent.
8. **The server checks every field.** The server rejects an unknown field, a value outside its enum or range, a nested object, and an unrounded number. It answers HTTP 400.
9. **PostHog (EU) is a data processor.** The Halv server forwards the events to PostHog at `eu.i.posthog.com`. The app never calls PostHog. ADR 0021 did not name this processor. The forwarder sends no IP address. It sets `$ip` to the placeholder `0.0.0.0` and sets `$geoip_disable` to `true`. It does not forward the request headers of the app, for example the user agent or X-Forwarded-For. An allowlist of events and properties is the last filter before the send. The forwarder drops and counts all other data.
10. **No stack trace and no session replay.** `crash_detected` holds a version and two buckets. It holds no stack trace, no file name and no memory content. Halv does not record sessions. A crash report with a stack trace needs a new ADR, a separate toggle and a new approval from Pedro.

## Changes to ADR 0021

- Decision 9: the list for version 2 shows 18 events (the 13 events of ADR 0021 and the 5 new events). It also shows the new facts. The list for version 1 stays at 13 events.
- Decision 10: Settings, Privacy shows the version 2 list. It also shows the list version that the user agreed to. For version 1, a note says that the install sends only the 13 events of the first list, without the new details.
- The event table and the sentence about event facts: the Contract section of this ADR replaces them for version 2. They stay true for version 1.
- The "never sends" sentence adds three items: a stack trace, an IP address in the PostHog event, and the exact savings in a usage event. Usage events never hold the exact savings. A savings report of a signed-in account still sends exact token counts and dollar amounts to the Halv server. It is not a usage event.
- Decision 11 stays. The notice of decision 3 uses the same place and style as the banner of decision 11. Unlike that banner, the notice shows on each launch until the user answers.

## Contract

The app sends `POST /v1/analytics/journey` with a JSON batch. The server keeps `deny_unknown_fields`.

Batch fields:

| Field | Type | Rule |
| --- | --- | --- |
| `install_id`, `app_version`, `os`, `events` | as in ADR 0021 | Unchanged. |
| `consent_version` | integer, optional | `1` or `2`. A missing value means `1`. |
| `locale` | string, optional | Only with `consent_version` 2. One of `en`, `pt-BR`, `es`, `fr`, `de`, `it`, `nl`, `ja`, `zh-CN`, `zh-TW`, `ko`. |

An event is a flat JSON object. The version 1 fields stay: `event`, `at`, `category`, `app_version`, `download_id`. A version 2 event or field in a batch without `consent_version` 2 gets HTTP 400 "invalid consent version". A field that the event does not list also gets HTTP 400.

New events:

| Event | Field | Values | Required |
| --- | --- | --- | --- |
| `usage_summarized` | `day` | Integer UTC day number (unix seconds / 86400). Server rule: today - 7 <= `day` <= today - 1. | yes |
| `usage_summarized` | `agents_used` | Array of 1 to 8 unique `AGENT` values, sorted ascending. | yes |
| `usage_summarized` | `sessions_bucket` | `"1"`, `"2-5"`, `"6-20"`, `"21+"` | yes |
| `usage_summarized` | `tokens_saved` | Integer 0 to 100000000000, rounded to 2 significant digits. | yes |
| `usage_summarized` | `usd_saved` | Number 0 to 1000000, rounded to 2 significant digits. | yes |
| `usage_summarized` | `crux_used` | Boolean | yes |
| `usage_summarized` | `routing_used` | Boolean | yes |
| `usage_summarized` | `plan` | `"none"`, `"trial"`, `"beta_free"`, `"basic"`, `"unlimited"`, `"lifetime"` | yes |
| `crash_detected` | `prev_app_version` | Version string, same rule as `app_version`. | yes |
| `crash_detected` | `uptime_bucket` | `"lt_1m"`, `"1m_10m"`, `"10m_1h"`, `"1h_6h"`, `"6h_plus"` | yes |
| `crash_detected` | `rss_bucket` | `"lt_500mb"`, `"500mb_1gb"`, `"1gb_2gb"`, `"2gb_4gb"`, `"4gb_plus"`, `"unknown"` | yes |
| `proxy_failed` | `category` | `"start"`, `"exit"`, `"restart_exhausted"` | yes |
| `proxy_failed` | `recovered` | Boolean | yes |
| `app_updated` | `from_version` | Version string | yes |
| `update_failed` | `category` | `"download"`, `"signature"`, `"install"` | yes |

New fields on version 1 events:

| Event | Field | Values | Required |
| --- | --- | --- | --- |
| `account_connected` | `agent_kind` | `AGENT` value | no, version 2 only |
| `account_connected` | `auth_method` | `"oauth"`, `"api_key"`, `"cli"` | no, version 2 only |
| `trial_prompted` | `surface` | `"pitch"`, `"routing"`, `"rotation"`, `"task_board"` | no, version 2 only |

The `AGENT` values are `claude_code`, `codex`, `gemini`, `kimi`, `glm`, `grok`, `antigravity` and `other`. The app maps an agent it does not know to `other`. The version 1 `category` rule stays: `first_task_failed` and `setup_failed` take `connection`, `engine`, `task` or `cancelled`.

When the app fires each event:

| Event | When |
| --- | --- |
| `usage_summarized` | Once for each UTC day with usage. The app sends it after the day ends, up to 7 days later. |
| `crash_detected` | At start, when the previous run ended unclean. At most one for each start. |
| `proxy_failed` | The Headroom proxy does not start, exits unexpectedly, or uses all its restarts. |
| `app_updated` | The first start on a new version. A first install records nothing. |
| `update_failed` | The updater fails to download, to check the signature, or to install. |

The app looks for days to summarize at start and every 6 hours. A day needs at least one agent session or one savings event. The app fills the fields from these sources:

- `agents_used` and `sessions_bucket`: the agent sessions active on that day.
- `tokens_saved` and `usd_saved`: the local savings ledger for that UTC day. These are the sources of the savings meter. Dollars follow ADR 0015.
- `crux_used`: true when the crux savings of that day are above 0.
- `routing_used`: true when at least one routed task has a receipt on that day.
- `plan`: the current entitlements. Signed out gives `none`. Trialing gives `trial`.

Dedup keys:

| Event | App queue key | Server `day` column |
| --- | --- | --- |
| Version 1 milestones | event name | 0 (unchanged) |
| `active_day` | `active_day:<at/86400>` | `at/86400` (unchanged) |
| `usage_summarized` | `usage_summarized:<day>` | `day` |
| `crash_detected`, `proxy_failed`, `update_failed` | `<event>:<at/86400>` | `at/86400` |
| `app_updated` | `app_updated:<new app_version>` | `at/86400` |

The first row of a key wins on the server. The primary key and `INSERT OR IGNORE` stay. The app keeps the queue keys of version 2 events for at most 40 days.

## Consent state

- `analyticsConsentAsked` is `0`, `1` or `2`. An install that answered before this field existed gets `1`. An install that did not answer gets `0`.
- An existing `on` without `analyticsConsentVersion` gets version `1`.
- The command `preferences_set_analytics_consent` takes an optional `consent_version`. An `on` answer without a version gives version `2`.
- An `off` answer keeps the version, sets `analyticsConsentAsked` to `2` and clears the queue.
- Any answer from the consent card or from the notice sets `analyticsConsentAsked` to `2`.

## Native rules

- `analytics_record` takes an optional `props` object. Its keys are flat and in snake case. The only keys are `agent_kind`, `auth_method` and `surface`. The app checks them with the rules of the server.
- `analytics_set_locale` takes a language code. The UI calls it at start and on each language change. The app keeps the code in memory and in the analytics state file. It checks the code against the 11 codes of `locale`. It attaches the code to batches only under version 2.
- Both commands have entries in the app command ACL. Without the entries, the main window cannot call them.

The app maps `AgentKind` to `AGENT` with this table. Any other kind gives `other`.

| `AgentKind` | `AGENT` |
| --- | --- |
| `Claude` | `claude_code` |
| `Codex` | `codex` |
| `Gemini` | `gemini` |
| `Kimi` | `kimi` |
| `Zai` | `glm` |
| `Grok` | `grok` |
| `Antigravity` | `antigravity` |

## Rounding

`round2(x)` returns 0 for 0. Otherwise it keeps the 2 most significant decimal digits and rounds half up. Examples: 1234 gives 1200, 1250 gives 1300, 99 gives 99, 0.0456 gives 0.046, 12.5 gives 13.

- `tokens_saved` is the `round2` of the exact integer.
- `usd_saved` is the `round2` of the exact dollars, as a JSON number with no trailing noise.
- The server accepts `usd_saved` v when |v - `round2`(v)| <= 1e-9 x max(1, v).

`round2` changes a value by at most 5 percent. The sum of the daily `usd_saved` values matches the local meter within this error.

## Consequences

- When the version is below 2, the native app drops version 2 events and strips version 2 fields. It does this at record time and at flush time. The server also rejects them. Two checks guard the same rule.
- With consent `unset` or `off`, the app sends nothing, as in ADR 0021. The queue clears on "off".
- The server stores the version 2 fields of an event, with `locale` and `consent_version`, as canonical JSON in the new `journey_events.props` column. Version 1 rows keep `NULL`. The 90-day retention covers the column. The PostHog forwarder reads `props` in the live path and in the backfill.
- The forwarder allows exactly the fields of the Contract tables, plus `locale` and `consent_version` on every event. `agents_used` is the only array. The forwarder sets the PostHog `timestamp` of `usage_summarized` to the start of its `day` plus 12 hours (UTC). PostHog then charts the event on the usage day.
- An old server answers HTTP 400 to a batch with version 2 data. The app then sends the batch again one time without version 2 data. The version 2 events stay in the queue for a later try.
- A version 2 event adds no funnel step. The admin funnel and cohort code ignore the version 2 events, so no count changes.
- Three places must show the version 2 events and facts:
  - The "What is sent" list in the app.
  - The README of `github.com/halv-ai/halv`.
  - The privacy page of the site.
- The release QA checklist in `RELEASING.md` compares the list with the real traffic. The capture runs across a UTC midnight, so that it holds a `usage_summarized` event.
- The usage numbers and the version 2 facts count only the installs on version 2. The funnel counts all installs that opt in, as in ADR 0021. Rates describe the group that opts in.
- The Halv Cloud pitch waits while the notice shows, as it waits while the banner of ADR 0021 shows. The notice hides where the banner hides: on the welcome screen, in the first-run card and in the prepared first task. The banner shows on one launch only, so the pitch can follow on a later launch. The notice shows on each launch until the user answers. An install that never answers does not see the pitch card. The offers at the point of use (ADR 0021 decision 4) do not wait for the notice.
- The mobile app is out of scope.
