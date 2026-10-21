import { ipc } from "./ipc.ts";
import type { AnalyticsAttribution, AnalyticsConsent } from "./types.ts";

/** The app sends this event when a site link gives a new download id. */
export const ANALYTICS_ATTRIBUTION_EVENT = "halv://attribution";

// New names preserve the meaning of historical observations: the old
// `trial_started` meant checkout opened and `paid` also included trials.
export const ANALYTICS_EVENTS = [
  "first_open", "account_connected", "project_selected", "setup_failed",
  "first_task_started", "first_task_completed", "first_task_failed",
  "context_saved", "trial_prompted", "checkout_opened",
  "trial_confirmed", "payment_confirmed", "active_day",
] as const;

export type AnalyticsEvent = (typeof ANALYTICS_EVENTS)[number];
export type AnalyticsFailure = "connection" | "engine" | "task" | "cancelled";
export type AnalyticsState = {
  accountConnected: boolean;
  tokensSaved: number;
  signedIn: boolean;
  trialConfirmed: boolean;
  paymentConfirmed: boolean;
};

export function analyticsEventsForState(
  previous: AnalyticsState | null,
  next: AnalyticsState,
): AnalyticsEvent[] {
  const events: AnalyticsEvent[] = [];
  if (!previous?.accountConnected && next.accountConnected) events.push("account_connected");
  if ((previous?.tokensSaved ?? 0) <= 0 && next.tokensSaved > 0) events.push("context_saved");
  if (!(previous?.signedIn && previous.trialConfirmed) && next.signedIn && next.trialConfirmed) {
    events.push("trial_confirmed");
  }
  if (!(previous?.signedIn && previous.paymentConfirmed) && next.signedIn && next.paymentConfirmed) {
    events.push("payment_confirmed");
  }
  return events;
}

// Native storage owns deduplication and the saved privacy preference. Caching
// a skipped/failed invocation here would prevent retry after sharing is enabled.
export function recordAnalyticsEvent(
  event: AnalyticsEvent,
  category?: AnalyticsFailure,
): Promise<void> {
  return ipc.analytics.record({ event, category }).catch(() => undefined);
}

export function flushAnalytics(): Promise<void> {
  return ipc.analytics.flush().catch(() => undefined);
}

export function analyticsInstallId(): Promise<string> {
  return ipc.analytics.installId();
}

/** Halv sends usage events only after the user said yes (ADR 0021). */
export function analyticsConsentAllowsEvents(
  consent: AnalyticsConsent | null | undefined,
): boolean {
  return consent === "on";
}

/**
 * The funnel events for one change of the app state (ADR 0021).
 * - Without consent, Halv records no event and forgets the state. The native
 *   queue also drops each event while the answer is not "on".
 * - The first state with consent records `first_open` first. An install that
 *   answers "on" after its first launch thus still sends `first_open` once.
 * Native storage drops repeats, so a later launch can ask again.
 */
export function analyticsEventsWithConsent(
  consent: AnalyticsConsent | null | undefined,
  previous: AnalyticsState | null,
  next: AnalyticsState,
): { events: AnalyticsEvent[]; state: AnalyticsState | null } {
  if (!analyticsConsentAllowsEvents(consent)) {
    return { events: [], state: null };
  }
  const events = analyticsEventsForState(previous, next);
  return {
    events: previous === null ? ["first_open", ...events] : events,
    state: next,
  };
}

/**
 * The stored download id of the site link. It is `null` when no link came,
 * and after `first_open` reached the server.
 */
export function analyticsAttribution(): Promise<AnalyticsAttribution> {
  return ipc.analytics.attributionGet();
}

export function listenForAnalyticsAttribution(
  handler: (attribution: AnalyticsAttribution) => void,
): Promise<() => void> {
  return ipc.events.listen<AnalyticsAttribution>(
    ANALYTICS_ATTRIBUTION_EVENT,
    handler,
  );
}
