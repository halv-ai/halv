import type { ConnectGateConnections } from "./connectGate.ts";
import type { Locale } from "./i18n/config.ts";
import { ipc, type AnalyticsRecordArgs } from "./ipc.ts";
import type {
  AnalyticsAttribution,
  AnalyticsConsent,
  AnalyticsConsentAsked,
  AnalyticsConsentVersion,
  AnthropicCredentialStatus,
  AppPreferences,
  AuthStatus,
} from "./types.ts";

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

// List version 2 (ADR 0022). Only native code records these events. The
// webview names them for the "What is sent" list.
export const ANALYTICS_V2_EVENTS = [
  "usage_summarized", "crash_detected", "proxy_failed", "app_updated",
  "update_failed",
] as const;

export type AnalyticsV2Event = (typeof ANALYTICS_V2_EVENTS)[number];

/** The `AGENT` values of the analytics contract (ADR 0022). */
export type AnalyticsAgent =
  | "claude_code" | "codex" | "gemini" | "kimi" | "glm" | "grok"
  | "antigravity" | "other";
export type AnalyticsAuthMethod = "oauth" | "api_key" | "cli";
/** The place that showed the Halv Cloud trial offer. */
export type AnalyticsTrialSurface = "pitch" | "routing" | "rotation" | "task_board";

/**
 * The optional facts of list version 2, with the snake_case keys of the
 * wire format. Native code drops them while the answer covers version 1.
 */
export type AnalyticsProps = {
  agent_kind?: AnalyticsAgent;
  auth_method?: AnalyticsAuthMethod;
  surface?: AnalyticsTrialSurface;
};

// The server rejects a fact on an event that does not list it.
const ANALYTICS_EVENT_PROPS: Partial<
  Record<AnalyticsEvent, readonly (keyof AnalyticsProps)[]>
> = {
  account_connected: ["agent_kind", "auth_method"],
  trial_prompted: ["surface"],
};

/**
 * One connected sign-in. `authMethod` is `null` when the app cannot tell
 * how the user signed in.
 */
export type AnalyticsSignIn = {
  agentKind: AnalyticsAgent;
  authMethod: AnalyticsAuthMethod | null;
};

export type AnalyticsState = {
  accountConnected: boolean;
  /** The sign-ins behind `accountConnected`, for `account_connected`. */
  signIns: readonly AnalyticsSignIn[];
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

/**
 * The sign-ins that make `accountConnected` true, in the words of the
 * analytics contract.
 * - A CLI login (Claude Code, Codex, Kimi Code) is `cli`.
 * - The ChatGPT sign-in inside Halv is `oauth`. A ChatGPT token that Halv
 *   reads from the Codex CLI is `cli`.
 * - A saved provider key is `api_key`.
 */
export function analyticsSignIns({
  connections,
  auth,
  anthropic,
}: {
  connections: ConnectGateConnections;
  auth: AuthStatus | null;
  anthropic: AnthropicCredentialStatus | null;
}): AnalyticsSignIn[] {
  const signIns: AnalyticsSignIn[] = [];
  const add = (agentKind: AnalyticsAgent, authMethod: AnalyticsAuthMethod) =>
    signIns.push({ agentKind, authMethod });
  if (connections.claude) add("claude_code", "cli");
  if (connections.anthropic || anthropic?.configured) add("claude_code", "api_key");
  if (connections.codex) add("codex", "cli");
  if (connections.openai) add("codex", "api_key");
  if (connections.kimi) add("kimi", "cli");
  if (connections.moonshot) add("kimi", "api_key");
  if (connections.zai) add("glm", "api_key");
  if (connections.grok) add("grok", "api_key");
  if (auth && "chatgpt" in auth) {
    add("codex", auth.chatgpt.source === "app" ? "oauth" : "cli");
  }
  if (auth && "apiKey" in auth) add("codex", "api_key");
  if (auth && "anthropicApiKey" in auth) add("claude_code", "api_key");
  return signIns;
}

/**
 * The facts of `account_connected`. A fact goes out only when all sign-ins
 * agree on it. With two different agents, the app cannot tell which one the
 * user connected, so it sends no `agent_kind`.
 */
export function accountConnectedProps(
  signIns: readonly AnalyticsSignIn[],
): AnalyticsProps {
  const props: AnalyticsProps = {};
  const agents = new Set(signIns.map((signIn) => signIn.agentKind));
  const methods = new Set(signIns.map((signIn) => signIn.authMethod));
  if (agents.size === 1) props.agent_kind = [...agents][0];
  const [method] = methods;
  if (methods.size === 1 && method) props.auth_method = method;
  return props;
}

/** The facts that one state event carries. */
export function analyticsEventProps(
  event: AnalyticsEvent,
  state: AnalyticsState,
): AnalyticsProps | undefined {
  return event === "account_connected"
    ? accountConnectedProps(state.signIns)
    : undefined;
}

/**
 * The arguments of `analytics_record`. The call keeps only the facts that
 * the event lists and drops `props` when no fact is left.
 */
export function analyticsRecordArgs(
  event: AnalyticsEvent,
  category?: AnalyticsFailure,
  props?: AnalyticsProps,
): AnalyticsRecordArgs {
  const allowed = ANALYTICS_EVENT_PROPS[event] ?? [];
  const kept: Record<string, string> = {};
  for (const key of allowed) {
    const value = props?.[key];
    if (value !== undefined) kept[key] = value;
  }
  return {
    event,
    ...(category === undefined ? {} : { category }),
    ...(Object.keys(kept).length === 0 ? {} : { props: kept as AnalyticsProps }),
  };
}

// Native storage owns deduplication and the saved privacy preference. Caching
// a skipped/failed invocation here would prevent retry after sharing is enabled.
export function recordAnalyticsEvent(
  event: AnalyticsEvent,
  category?: AnalyticsFailure,
  props?: AnalyticsProps,
): Promise<void> {
  return ipc.analytics
    .record(analyticsRecordArgs(event, category, props))
    .catch(() => undefined);
}

/** `trial_prompted` with the place that showed the offer. */
export function recordTrialPrompted(
  surface: AnalyticsTrialSurface | undefined,
): Promise<void> {
  return recordAnalyticsEvent(
    "trial_prompted",
    undefined,
    surface === undefined ? undefined : { surface },
  );
}

/**
 * Tells native code the language of the UI. Native code sends it only
 * while the answer covers list version 2.
 */
export function setAnalyticsLocale(locale: Locale): Promise<void> {
  return ipc.analytics.setLocale({ locale }).catch(() => undefined);
}

export function flushAnalytics(): Promise<void> {
  return ipc.analytics.flush().catch(() => undefined);
}

export function analyticsInstallId(): Promise<string> {
  return ipc.analytics.installId();
}

/**
 * The list version that the saved "on" answer covers (ADR 0022). An answer
 * from before version 2 has no saved version and covers version 1.
 */
export function analyticsConsentVersion(
  preferences: Pick<AppPreferences, "analyticsConsent"> &
    Partial<Pick<AppPreferences, "analyticsConsentVersion">>,
): AnalyticsConsentVersion | null {
  if (preferences.analyticsConsent !== "on") return null;
  return preferences.analyticsConsentVersion === 2 ? 2 : 1;
}

/**
 * The newest list version that Halv asked about. An answer from before
 * version 2 has no saved value and counts as asked about version 1.
 */
export function analyticsConsentAsked(
  preferences: Pick<AppPreferences, "analyticsConsent"> &
    Partial<Pick<AppPreferences, "analyticsConsentAsked">>,
): AnalyticsConsentAsked {
  const asked = preferences.analyticsConsentAsked;
  if (asked === 0 || asked === 1 || asked === 2) return asked;
  return preferences.analyticsConsent === "unset" ? 0 : 1;
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
