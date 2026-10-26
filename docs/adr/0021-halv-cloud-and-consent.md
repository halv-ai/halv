# ADR 0021: Halv Cloud and consent

Date: 2026-10-21
Status: Accepted. Amended by ADR 0022 (decisions 9 and 10, the event list and the facts of each event).

This ADR supersedes the trial wall of ADR 0018. It replaces decisions 1, 2 and 8 of ADR 0018 and the blocking part of decision 3. Decisions 4, 5, 6 and 7 of ADR 0018 stay.

## Context

The desktop app is open source from version 1.0.0. The code lives at `github.com/halv-ai/halv` under the Apache-2.0 license. Anyone can read what the app does and build it.

The app has two kinds of work:

- Local work runs on the machine of the user. It needs no server and no account.
- Cloud work runs on the Halv servers. The JEV router (ADR 0016), the `halv-savings` MCP server, automatic account rotation (ADR 0017), the savings dashboard and sync belong here.

ADR 0018 moved the account ask to after the first result. The ask still blocked the user. A signed-out user met a trial card on the next send. This conflicts with an open source app, because the local half of the app costs Halv nothing to run.

The analytics default was also wrong for an open source app:

- Before this ADR, analytics were on by default. The app never asked.
- The welcome screen showed one checkbox, already checked.
- The "What is sent" list in Settings used old event names. It did not use the 13 names that the app sends.

The marketing site sends users to the download. The site cannot tell which download led to which install. Without a link between the two, Halv cannot measure which page converts.

## Decision

1. **Open source.** The desktop app is open source under Apache-2.0 at `github.com/halv-ai/halv`. The first open source release is version 1.0.0.
2. **Free means local.** Everything that runs on the machine of the user is free. This covers the Headroom proxy and its savings, RTK, crux, terminals, tabs, projects, agent status, the local savings meter, memories, checkpoints, and the Codex, Claude and Gemini panes. No part of it needs an account, ever.
3. **Halv Cloud is the paid plan.** Halv Cloud covers everything that runs on JEV: task routing to the cheapest capable model (ADR 0016), the `halv-savings` MCP server, and automatic account rotation (ADR 0017). It also covers the cloud savings dashboard and sync.
4. **One paywall moment, at the point of use.** A signed-out user sees the Halv Cloud offer only when the user turns task routing on, turns account rotation on, or when a routed task is about to run. The offer is an inline card in that place. The card has two buttons: one starts the trial, and one keeps going without it.
5. **The app never blocks local work.** The offer is never a modal, a banner or a full-screen gate. The offer never blocks the chat or a terminal. A signed-out user can always send a prompt and type in a terminal.
6. **The trial card becomes a pitch card.** The card that ADR 0018 opened after the first result no longer blocks. It appears once after the first result. The user can dismiss it. The dismissal is permanent and lives in `localStorage`.
7. **The welcome screen informs once.** The welcome screen shows two short columns under the prerequisites: "Free, on your machine" and "Halv Cloud". The block has no button. The one call to action stays "Let's try it on my project".
8. **Usage events are opt-in.** `AppPreferences.analyticsConsent` has three values: `unset`, `on` and `off`. The app sends no event while the value is `unset` or `off`. A new install starts as `unset`.
9. **The consent card is plain.** The card has the title "Help improve Halv", one sentence, and a "What is sent" list. The list shows the 13 journey events in four groups. The list also shows the facts that travel with each event, and what Halv never sends. The two answer buttons look the same. ADR 0022 amends this decision: the list grows to 18 events.
10. **The same list lives in Settings.** Settings, Privacy shows the same list as the consent card. The user can change the answer at any time. ADR 0022 amends this decision: Settings shows the list of version 2.
11. **Upgraded installs ask once.** An install that upgraded from the old default (analytics on, never asked) becomes `unset`. It sends no event until the user answers. On the first launch after the upgrade, the app shows one banner at the top of the main column: "Halv now asks before it sends usage events." The banner disappears when the user answers. Later launches do not show the banner again. The answer then stays `unset`, and Settings, Privacy keeps the toggle.
12. **Download attribution uses a deep link.** After the download starts, the site opens `halv://welcome?d=<download_id>`. The app stores the id when it arrives before the first launch or shortly after it. The app sends the id in the `first_open` event as `download_id`. The id is opaque, has at most 64 characters, and uses only `[A-Za-z0-9_-]`. The app drops any other value.
13. **The id is stored without consent, and sent only with consent.** Without consent the app sends nothing. The app still stores the id. If the user consents later, the app sends the stored id with `first_open`.

The 13 journey events are:

| Group | Events |
| --- | --- |
| Setup | `first_open`, `account_connected`, `project_selected`, `setup_failed` |
| First task | `first_task_started`, `first_task_completed`, `first_task_failed`, `context_saved` |
| Halv Cloud | `trial_prompted`, `checkout_opened`, `trial_confirmed`, `payment_confirmed` |
| Activity | `active_day` |

Each event carries these facts: a random install ID, the app version, the operating system, the event time, and the download id when the install came through a site link.

Halv never sends prompts, code, file names, terminal output, account emails or API keys.

## Consequences

- `src/onboarding.ts` exports `shouldAskConsent({ consent, welcomeSeen })`. It returns `true` only when `consent` is `unset`.
- `src/components/AnonymousUsagePreference.tsx` exports `ConsentCard` with `variant="ask"` or `variant="settings"`, `ConsentAskBanner`, and `ConsentDetails`. The welcome screen and the first-run card render `ConsentCard variant="ask"`. Settings keeps `AnonymousUsagePreference` and shows `ConsentDetails` with the install ID.
- App places `ConsentAskBanner` at the top of the main column. `shouldShowConsentBanner` in `src/onboarding.ts` hides the banner while the welcome screen, the first-run card or the prepared first task shows. These surfaces have their own ask, so the app never shows two asks at once.
- The Halv Cloud pitch waits while the consent banner shows. The chat column shows one card at a time. The banner shows on one launch only (`halv.consentBannerShown`), so the pitch can show on a later launch.
- The pitch card shows on one launch only (`halv.cloudPitchShown`). The dismissal stays permanent.
- An install that skips the welcome screen sees the "Free, on your machine" and "Halv Cloud" block once in the first-run card (`halv.cloudInfoSeen`).
- A payment issue never opens a modal. Settings, Account shows the inline payment card with "Fix payment".
- The routing card shows only under the routing toggle while routing is on.
- The task board does not list a route that ran in the parent without Halv Cloud. The board shows the inline Halv Cloud card once instead.
- The `halv-savings` MCP server answers `sign_in_required` when Halv has no valid session (signed out, HTTP 401, or an unreadable token). It answers `halv_cloud_required` only for HTTP 402 and 403. Both answers carry `action: execute_in_parent`.
- A legacy `beta_free` account keeps Halv Cloud in the app. The server rule for Halv Cloud routes must also allow `beta_free`.
- The Headroom proxy never pauses for the plan of the account. Halv starts it with `HEADROOM_BEACON=off`, `DO_NOT_TRACK=1` and `HEADROOM_UPDATE_CHECK=off`, so the proxy sends no telemetry. On macOS only, Halv also sets `MallocAggressiveMadvise=1` and `MallocLargeCache=0` at spawn, because a frozen runtime cannot set them with a re-exec.
- An "off" answer holds for the running session also when the preferences write fails.
- `analyticsEventsWithConsent` in `src/analytics.ts` records `first_open` with the first app state after the answer becomes `on`. The native queue drops each event while the answer is not `on`, so a late yes still sends `first_open` once.
- App no longer calls the send gate of ADR 0018. A local send never waits for an account. The routing and rotation switches take over the paywall moment.
- The funnel counts only the installs that opt in. Absolute counts fall at release. Rates describe the opted-in group, and this group can differ from all installs. The admin funnel report must say this.
- `trial_prompted` fires when the Halv Cloud card shows, not when a wall opens.
- Until an upgraded install answers, it sends no `active_day` or other event.
- A stored download id stays on the device until it is sent. Halv adds no other use for it.
- The new UI strings go through `t()`. The i18n extraction step adds them to the catalogs.
- Revisit the placement of the Halv Cloud card with funnel data after the first full week of opt-in events.
