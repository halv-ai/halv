import assert from "node:assert/strict";
import test from "node:test";

import { gateDecision, paywallDismissable } from "../src/gate.ts";
import type { AccountAuthStatus, Entitlements } from "../src/types.ts";

const NOW_MS = Date.parse("2026-08-13T12:00:00Z");
const FIRST_LAUNCH_AT = "2026-08-01T00:00:00Z";

const ENTITLED: Entitlements = {
  beta: false,
  billingRequired: true,
  canUseApp: true,
  offlineGraceHours: 72,
  plan: "basic",
  message: null,
  checkoutUrl: null,
  billingPortalUrl: null,
  savings: {
    period: "2026-08",
    tokensSaved: 0,
    capTokens: 500_000,
    exhausted: false,
  },
};

function signedOutStatus(): AccountAuthStatus {
  return {
    signedIn: false,
    email: null,
    plan: null,
    serverUrl: "https://api.halv.ai",
    entitlements: null,
    syncedAt: null,
    firstLaunchAt: FIRST_LAUNCH_AT,
    freeTurnsUsed: 5,
    stale: false,
  };
}

function signedInStatus(
  entitlements: Entitlements | null = ENTITLED,
  syncedAt: string | null = "2026-08-13T11:00:00Z",
): AccountAuthStatus {
  return {
    signedIn: true,
    email: "user@example.com",
    plan: "basic",
    serverUrl: "https://api.halv.ai",
    entitlements,
    syncedAt,
    firstLaunchAt: FIRST_LAUNCH_AT,
    freeTurnsUsed: 5,
    stale: false,
  };
}

test("signed-out users always reach the app, also after earlier conversations", () => {
  assert.equal(gateDecision(signedOutStatus(), NOW_MS), "app");
});

test("signed-in users with active trial entitlements can use the app", () => {
  const trialEntitlements: Entitlements = { ...ENTITLED, plan: "trial" };

  assert.equal(gateDecision(signedInStatus(trialEntitlements), NOW_MS), "app");
});

test("signed-in users with expired trial entitlements reach the paywall", () => {
  const expiredTrialEntitlements: Entitlements = {
    ...ENTITLED,
    canUseApp: false,
    plan: "trial",
  };

  assert.equal(
    gateDecision(signedInStatus(expiredTrialEntitlements), NOW_MS),
    "paywall",
  );
});

test("signed-in users without current entitlements reach verification", () => {
  assert.equal(gateDecision(signedInStatus(null, null), NOW_MS), "verify");
  assert.equal(
    gateDecision(signedInStatus(ENTITLED, "2026-08-01T00:00:00Z"), NOW_MS),
    "verify",
  );
});

test("only an account without payment history can leave the paywall", () => {
  assert.equal(paywallDismissable({ ...ENTITLED, canUseApp: false }), true);
  assert.equal(
    paywallDismissable({ ...ENTITLED, canUseApp: false, trialConfirmed: true }),
    false,
  );
  assert.equal(
    paywallDismissable({ ...ENTITLED, canUseApp: false, paymentConfirmed: true }),
    false,
  );
  assert.equal(
    paywallDismissable({
      ...ENTITLED,
      canUseApp: false,
      paymentIssue: "past_due",
    }),
    false,
  );
  assert.equal(paywallDismissable(null), false);
});
