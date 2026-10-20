# ADR 0018: The account gate opens after the first result

Date: 2026-10-16
Status: Accepted

## Context

An internal funnel investigation of 2026-10-15 found that the app asked for a Halv account before it proved its value:

- The trial card opened after one conversation, or on the first send to a second conversation. No measured saving was required.
- On the next launch, any install with a conversation met a full-screen account wall with no way back.
- The guided first task was a read-only tour that read few files. None of the completed guided tasks produced a measured saving.
- The payoff line could never be positive. `lastTurnSavingsPercent` returned a negative percent, and the copy treated it as "no saving".
- "Skip the guide" wrote a permanent dismissal. The funnel then lost `project_selected`, `first_task_started` and `first_task_completed` for that install, because those events fired only inside the guided card.
- Trials ended in silence. The app did not know the trial end, and the server sent no email before it.

The design playbook that guided the rework states the same rule in product terms: value first, ask later; perceived loss retains more than promised benefit; a trial with an active goal builds commitment before billing.

## Decision

1. **Value first.** The trial card opens only after the install delivered value: a completed task on any lane (chat or terminal), a completed guided task, or a measured saving. `src/activation.ts` holds the rule (`valueDelivered`). The first result persists in `localStorage` (`halv.firstResult`), so a relaunch keeps the same rule.
2. **No launch wall.** A signed-out user always reaches the app. `gateDecision` no longer returns `account`. The next send after the first result opens the trial card. A signed-in account without app access sees the paywall; an account with no payment history can go back and read its chats (`paywallDismissable`). The next send opens the paywall again.
3. **The card tells the truth.** The trial card shows the measured saving when one exists, then what the device keeps: the number of conversations and the dollars or tokens saved. The price and currency come from `/v1/billing/prices`.
4. **The first task proves the promise.** The three first-task prompts read the README, the entry points, the larger modules and the test setup before they answer, so the reply has context for Halv to remove. The success state celebrates the saving of the day with the amount and the sparkline of recent replies.
5. **Snooze, not dismiss.** "Skip the guide" and "I'll start with my own question" hide the guide for the current launch only. The guide returns while the app is still fresh, so the user still declares a first task.
6. **True funnel events.** `project_selected` fires when any project becomes selected. `first_task_started` fires on any send. `first_task_completed` fires on the first completed chat turn or terminal turn on any lane. `account_connected` counts ChatGPT OAuth and pasted API keys.
7. **A trial with a goal.** The server stores `trial_end` from the Stripe subscription and returns it in `/v1/me`. The app shows the trial goal: a task on five of the seven days, with the day count and the active days. On `customer.subscription.trial_will_end`, the server sends one email with the user's own savings, never a generic notice.
8. **Prerequisites before the ask.** The welcome screen states what the user needs: a ChatGPT or Claude plan or an API key, a Halv account after the first result, and a card for the trial.

## Consequences

- `trialGateForSend` takes `{ signedIn, valueDelivered }`. The per-conversation allowance (`allowedConversationId`) is gone.
- `gateDecision` takes no anonymous facts. The visual state `account-gate` is now `returning-signed-out`.
- An install that already had conversations before 0.8.0, but no stored first result or guided completion, gets one more free result before the card. This is the intended generosity.
- The journey event vocabulary did not change. The admin funnel keeps its order; the organic sources only make the existing steps true.
- `OnboardingPreference` still reads a stored `dismissed` from older installs, but the app no longer writes it.
- Revisit the card with the sample-gated rule of the funnel report: at 20 prompts of the new kind, or at day 60.
