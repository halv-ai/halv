import assert from "node:assert/strict";
import test from "node:test";
import { analyticsEventsForState, type AnalyticsState } from "../src/analytics.ts";

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
