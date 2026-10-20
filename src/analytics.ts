import { ipc } from "./ipc.ts";

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
