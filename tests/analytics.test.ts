import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
// @ts-expect-error Node's test runner needs the source extension.
import { ANALYTICS_EVENTS, analyticsConsentAllowsEvents, analyticsEventsForState, analyticsEventsWithConsent, type AnalyticsState } from "../src/analytics.ts";

const empty: AnalyticsState = {
  accountConnected: false, signIns: [], tokensSaved: 0, signedIn: false,
  trialConfirmed: false, paymentConfirmed: false,
};

test("connection milestone covers any connected provider", () => {
  assert.deepEqual(analyticsEventsForState(empty, { ...empty, accountConnected: true }), ["account_connected"]);
  assert.deepEqual(analyticsEventsForState({ ...empty, accountConnected: true }, { ...empty, accountConnected: true }), []);
});
test("zero savings does not invent a saving or interfere with task completion", () => {
  assert.deepEqual(analyticsEventsForState(null, empty), []);
  assert.deepEqual(analyticsEventsForState(empty, { ...empty, tokensSaved: 1 }), ["context_saved"]);
  assert.deepEqual(analyticsEventsForState({ ...empty, tokensSaved: 1 }, { ...empty, tokensSaved: 2 }), []);
});
test("a signed-in account or checkout alone does not confirm a trial or payment", () => {
  assert.deepEqual(analyticsEventsForState(empty, { ...empty, signedIn: true }), []);
  assert.deepEqual(analyticsEventsForState(empty, { ...empty, trialConfirmed: true, paymentConfirmed: true }), []);
});
test("confirmed trial and positive payment are distinct server facts", () => {
  const trial = { ...empty, signedIn: true, trialConfirmed: true };
  assert.deepEqual(analyticsEventsForState(empty, trial), ["trial_confirmed"]);
  assert.deepEqual(analyticsEventsForState(trial, { ...trial, paymentConfirmed: true }), ["payment_confirmed"]);
  assert.deepEqual(analyticsEventsForState({ ...trial, paymentConfirmed: true }, { ...trial, paymentConfirmed: true }), []);
});
test("restored confirmed billing facts are recorded without assuming a checkout", () => {
  assert.deepEqual(analyticsEventsForState(null, { ...empty, signedIn: true, trialConfirmed: true, paymentConfirmed: true }), ["trial_confirmed", "payment_confirmed"]);
});
test("first_open waits for consent on and goes first after a late yes", () => {
  const connected = { ...empty, accountConnected: true };
  // Unset, off and a preference that has not loaded record nothing and keep no state.
  for (const consent of ["unset", "off", null, undefined] as const) {
    assert.deepEqual(analyticsEventsWithConsent(consent, null, connected), { events: [], state: null });
    assert.deepEqual(analyticsEventsWithConsent(consent, connected, connected), { events: [], state: null });
  }
  // The first state after a yes records first_open, then the state events.
  const afterYes = analyticsEventsWithConsent("on", null, connected);
  assert.deepEqual(afterYes, { events: ["first_open", "account_connected"], state: connected });
  // Later changes record only the new milestones.
  assert.deepEqual(analyticsEventsWithConsent("on", afterYes.state, connected), { events: [], state: connected });
  const saved = { ...connected, tokensSaved: 10 };
  assert.deepEqual(analyticsEventsWithConsent("on", connected, saved), { events: ["context_saved"], state: saved });
  // A no forgets the state, so the next yes asks for first_open again (native dedupes it).
  assert.equal(analyticsEventsWithConsent("off", saved, saved).state, null);
  assert.deepEqual(analyticsEventsWithConsent("on", null, saved).events, ["first_open", "account_connected", "context_saved"]);
});


test("a failed record can be retried after privacy or storage recovery", async () => {
  const { ipc } = await import("../src/ipc.ts");
  const { recordAnalyticsEvent } = await import("../src/analytics.ts");
  const original = ipc.analytics.record;
  let calls = 0;
  ipc.analytics.record = async () => { if (++calls === 1) throw new Error("temporary failure"); };
  try {
    await recordAnalyticsEvent("first_open");
    await recordAnalyticsEvent("first_open");
    assert.equal(calls, 2);
  } finally { ipc.analytics.record = original; }
});

test("the webview and the native allowlist name the same 13 journey events", async () => {
  const source = await readFile(new URL("../src-tauri/src/analytics.rs", import.meta.url), "utf8");
  const list = source.match(/const JOURNEY_EVENTS: \[&str; 13\] = \[([\s\S]*?)\];/);
  assert.ok(list, "analytics.rs must define JOURNEY_EVENTS");
  const native = [...list[1].matchAll(/"([a-z_]+)"/g)].map((match) => match[1]);
  assert.equal(ANALYTICS_EVENTS.length, 13);
  assert.deepEqual([...ANALYTICS_EVENTS].sort(), native.sort());
});

test("only a yes allows usage events", () => {
  assert.equal(analyticsConsentAllowsEvents("on"), true);
  for (const consent of ["unset", "off", null, undefined] as const) {
    assert.equal(analyticsConsentAllowsEvents(consent), false, String(consent));
  }
});

test("the attribution helpers use the attribution command and event", async () => {
  const { setIpcTransport, tauriTransport } = await import("../src/ipc.ts");
  const { analyticsAttribution, listenForAnalyticsAttribution, ANALYTICS_ATTRIBUTION_EVENT } = await import("../src/analytics.ts");
  const calls: string[] = [];
  let listener: ((payload: unknown) => void) | null = null;
  setIpcTransport({
    invoke: async (name: string) => {
      calls.push(name);
      return { downloadId: "site_link-1" } as never;
    },
    listen: async (event: string, handler: (payload: unknown) => void) => {
      calls.push(event);
      listener = handler;
      return () => undefined;
    },
  });
  try {
    assert.deepEqual(await analyticsAttribution(), { downloadId: "site_link-1" });
    const received: unknown[] = [];
    await listenForAnalyticsAttribution((attribution) => received.push(attribution));
    listener?.({ downloadId: "site_link-2" });
    assert.deepEqual(received, [{ downloadId: "site_link-2" }]);
    assert.deepEqual(calls, ["analytics_attribution_get", ANALYTICS_ATTRIBUTION_EVENT]);
    assert.equal(ANALYTICS_ATTRIBUTION_EVENT, "halv://attribution");
  } finally {
    setIpcTransport(tauriTransport);
  }
});

const noConnections = {
  anthropic: false, claude: false, codex: false, grok: false,
  kimi: false, moonshot: false, openai: false, zai: false,
};

test("each sign-in maps to the agent and the sign-in type of the contract", async () => {
  const { analyticsSignIns } = await import("../src/analytics.ts");
  const signIns = (connections: Partial<typeof noConnections>, auth: unknown = null, anthropic: unknown = null) =>
    analyticsSignIns({ connections: { ...noConnections, ...connections }, auth: auth as never, anthropic: anthropic as never });
  assert.deepEqual(signIns({}), []);
  assert.deepEqual(signIns({}, { signedOut: {} }), []);
  assert.deepEqual(signIns({ claude: true }), [{ agentKind: "claude_code", authMethod: "cli" }]);
  assert.deepEqual(signIns({ anthropic: true }), [{ agentKind: "claude_code", authMethod: "api_key" }]);
  assert.deepEqual(signIns({}, null, { configured: true }), [{ agentKind: "claude_code", authMethod: "api_key" }]);
  assert.deepEqual(signIns({ codex: true }), [{ agentKind: "codex", authMethod: "cli" }]);
  assert.deepEqual(signIns({ openai: true }), [{ agentKind: "codex", authMethod: "api_key" }]);
  assert.deepEqual(signIns({ kimi: true }), [{ agentKind: "kimi", authMethod: "cli" }]);
  assert.deepEqual(signIns({ moonshot: true }), [{ agentKind: "kimi", authMethod: "api_key" }]);
  assert.deepEqual(signIns({ zai: true }), [{ agentKind: "glm", authMethod: "api_key" }]);
  assert.deepEqual(signIns({ grok: true }), [{ agentKind: "grok", authMethod: "api_key" }]);
  const chatgpt = (source: string) => ({ chatgpt: { source, accountId: "a", accessTokenValid: true } });
  // The ChatGPT sign-in inside Halv is OAuth. A token from the Codex CLI is a CLI sign-in.
  assert.deepEqual(signIns({}, chatgpt("app")), [{ agentKind: "codex", authMethod: "oauth" }]);
  assert.deepEqual(signIns({}, chatgpt("codex-cli")), [{ agentKind: "codex", authMethod: "cli" }]);
  assert.deepEqual(signIns({}, { apiKey: { masked: "sk-...1" } }), [{ agentKind: "codex", authMethod: "api_key" }]);
  assert.deepEqual(signIns({}, { anthropicApiKey: { source: "api-key", masked: "sk-...1" } }), [{ agentKind: "claude_code", authMethod: "api_key" }]);
});

test("account_connected sends only the facts that all sign-ins agree on", async () => {
  const { accountConnectedProps } = await import("../src/analytics.ts");
  assert.deepEqual(accountConnectedProps([]), {});
  assert.deepEqual(accountConnectedProps([{ agentKind: "glm", authMethod: "api_key" }]), { agent_kind: "glm", auth_method: "api_key" });
  // One agent with two sign-in types: the agent is known, the type is not.
  assert.deepEqual(
    accountConnectedProps([{ agentKind: "codex", authMethod: "oauth" }, { agentKind: "codex", authMethod: "cli" }]),
    { agent_kind: "codex" },
  );
  // Two agents: the app cannot tell which one the user connected.
  assert.deepEqual(
    accountConnectedProps([{ agentKind: "claude_code", authMethod: "cli" }, { agentKind: "codex", authMethod: "cli" }]),
    { auth_method: "cli" },
  );
  assert.deepEqual(
    accountConnectedProps([{ agentKind: "claude_code", authMethod: "cli" }, { agentKind: "kimi", authMethod: "api_key" }]),
    {},
  );
  // An unknown sign-in type is never sent.
  assert.deepEqual(accountConnectedProps([{ agentKind: "codex", authMethod: null }]), { agent_kind: "codex" });
});

test("only account_connected takes the sign-in facts of the state", async () => {
  const { analyticsEventProps } = await import("../src/analytics.ts");
  const state = { ...empty, accountConnected: true, signIns: [{ agentKind: "glm", authMethod: "api_key" }] } as AnalyticsState;
  assert.deepEqual(analyticsEventProps("account_connected", state), { agent_kind: "glm", auth_method: "api_key" });
  assert.equal(analyticsEventProps("first_open", state), undefined);
  assert.equal(analyticsEventProps("context_saved", state), undefined);
});

test("record arguments keep only the facts that the event lists", async () => {
  const { analyticsRecordArgs } = await import("../src/analytics.ts");
  assert.deepEqual(analyticsRecordArgs("first_open"), { event: "first_open" });
  assert.deepEqual(analyticsRecordArgs("setup_failed", "engine"), { event: "setup_failed", category: "engine" });
  assert.deepEqual(
    analyticsRecordArgs("account_connected", undefined, { agent_kind: "glm", auth_method: "api_key" }),
    { event: "account_connected", props: { agent_kind: "glm", auth_method: "api_key" } },
  );
  assert.deepEqual(
    analyticsRecordArgs("trial_prompted", undefined, { surface: "routing" }),
    { event: "trial_prompted", props: { surface: "routing" } },
  );
  // The server rejects `surface` on account_connected and any fact on other events.
  assert.deepEqual(analyticsRecordArgs("account_connected", undefined, { surface: "pitch" }), { event: "account_connected" });
  assert.deepEqual(analyticsRecordArgs("trial_prompted", undefined, { agent_kind: "codex" }), { event: "trial_prompted" });
  assert.deepEqual(analyticsRecordArgs("first_open", undefined, { agent_kind: "codex", surface: "pitch" }), { event: "first_open" });
  // An empty or unknown fact leaves no `props` key.
  assert.deepEqual(analyticsRecordArgs("account_connected", undefined, {}), { event: "account_connected" });
  assert.deepEqual(analyticsRecordArgs("trial_prompted", undefined, { surface: undefined }), { event: "trial_prompted" });
});

test("record, trial surface and locale calls send the contract arguments", async () => {
  const { setIpcTransport, tauriTransport } = await import("../src/ipc.ts");
  const { recordAnalyticsEvent, recordTrialPrompted, setAnalyticsLocale } = await import("../src/analytics.ts");
  const calls: [string, unknown][] = [];
  setIpcTransport({
    invoke: async (name: string, args: unknown) => {
      calls.push([name, args]);
      return undefined as never;
    },
    listen: async () => () => undefined,
  });
  try {
    await recordAnalyticsEvent("account_connected", undefined, { agent_kind: "glm", auth_method: "api_key" });
    await recordTrialPrompted("routing");
    await recordTrialPrompted("task_board");
    await recordTrialPrompted(undefined);
    await recordAnalyticsEvent("first_task_failed", "cancelled");
    await setAnalyticsLocale("pt-BR");
    assert.deepEqual(calls, [
      ["analytics_record", { event: "account_connected", props: { agent_kind: "glm", auth_method: "api_key" } }],
      ["analytics_record", { event: "trial_prompted", props: { surface: "routing" } }],
      ["analytics_record", { event: "trial_prompted", props: { surface: "task_board" } }],
      ["analytics_record", { event: "trial_prompted" }],
      ["analytics_record", { event: "first_task_failed", category: "cancelled" }],
      ["analytics_set_locale", { locale: "pt-BR" }],
    ]);
  } finally {
    setIpcTransport(tauriTransport);
  }
});

test("a failed locale call does not throw", async () => {
  const { setIpcTransport, tauriTransport } = await import("../src/ipc.ts");
  const { setAnalyticsLocale } = await import("../src/analytics.ts");
  setIpcTransport({
    invoke: async () => { throw new Error("unknown command"); },
    listen: async () => () => undefined,
  });
  try {
    await setAnalyticsLocale("en");
  } finally {
    setIpcTransport(tauriTransport);
  }
});

test("saved answers from before list version 2 read as version 1", async () => {
  const { analyticsConsentAsked, analyticsConsentVersion } = await import("../src/analytics.ts");
  // Migration of ADR 0022: an old "on" covers version 1 and was asked about version 1.
  assert.equal(analyticsConsentVersion({ analyticsConsent: "on" }), 1);
  assert.equal(analyticsConsentAsked({ analyticsConsent: "on" }), 1);
  assert.equal(analyticsConsentAsked({ analyticsConsent: "off" }), 1);
  assert.equal(analyticsConsentAsked({ analyticsConsent: "unset" }), 0);
  // Saved values win.
  assert.equal(analyticsConsentVersion({ analyticsConsent: "on", analyticsConsentVersion: 2 }), 2);
  assert.equal(analyticsConsentVersion({ analyticsConsent: "on", analyticsConsentVersion: 1 }), 1);
  assert.equal(analyticsConsentAsked({ analyticsConsent: "on", analyticsConsentAsked: 2 }), 2);
  // Only "on" covers a list.
  assert.equal(analyticsConsentVersion({ analyticsConsent: "off", analyticsConsentVersion: 2 }), null);
  assert.equal(analyticsConsentVersion({ analyticsConsent: "unset", analyticsConsentVersion: 2 }), null);
});

test("the v2 events are the five native events of the contract", async () => {
  const { ANALYTICS_V2_EVENTS } = await import("../src/analytics.ts");
  assert.deepEqual([...ANALYTICS_V2_EVENTS], ["usage_summarized", "crash_detected", "proxy_failed", "app_updated", "update_failed"]);
  for (const event of ANALYTICS_V2_EVENTS) {
    assert.ok(!(ANALYTICS_EVENTS as readonly string[]).includes(event), `${event} is not a webview event`);
  }
});
