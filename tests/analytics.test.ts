import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
// @ts-expect-error Node's test runner needs the source extension.
import { ANALYTICS_EVENTS, analyticsConsentAllowsEvents, analyticsEventsForState, analyticsEventsWithConsent, type AnalyticsState } from "../src/analytics.ts";

const empty: AnalyticsState = {
  accountConnected: false, tokensSaved: 0, signedIn: false,
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
