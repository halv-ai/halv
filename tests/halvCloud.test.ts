import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import { cloudAccess } from "../src/halvCloud.ts";
import type { AccountAuthStatus, Entitlements } from "../src/types.ts";

const ENTITLEMENTS: Entitlements = {
  beta: false,
  billingRequired: true,
  canUseApp: true,
  plan: "basic",
  message: null,
  checkoutUrl: null,
  billingPortalUrl: null,
  savings: { period: "2026-10", tokensSaved: 0, capTokens: 500_000, exhausted: false },
};

function signedIn(
  entitlements: Partial<Entitlements> | null,
  plan: AccountAuthStatus["plan"] = "basic",
): AccountAuthStatus {
  return {
    signedIn: true,
    email: "user@example.test",
    plan,
    serverUrl: "https://api.halv.ai",
    entitlements: entitlements && { ...ENTITLEMENTS, ...entitlements },
    syncedAt: "2026-10-21T12:00:00Z",
    firstLaunchAt: "2026-10-01T00:00:00Z",
    freeTurnsUsed: 0,
  };
}

test("no status or a signed-out status has no Halv Cloud access", () => {
  assert.deepEqual(cloudAccess(null), { granted: false, reason: "signed_out" });
  assert.deepEqual(
    cloudAccess({ ...signedIn(ENTITLEMENTS), signedIn: false }),
    { granted: false, reason: "signed_out" },
  );
});

test("an account without a plan that includes the app has no access", () => {
  assert.deepEqual(
    cloudAccess(signedIn({ canUseApp: false, plan: null }, null)),
    { granted: false, reason: "no_plan" },
  );
});

test("a legacy beta account keeps Halv Cloud while billing is required", () => {
  assert.deepEqual(
    cloudAccess(signedIn({ canUseApp: false, plan: "beta_free" }, "beta_free")),
    { granted: true, reason: "beta" },
  );
  // A payment issue still wins over the legacy beta plan.
  assert.deepEqual(
    cloudAccess(
      signedIn({ canUseApp: false, plan: "beta_free", paymentIssue: "past_due" }, "beta_free"),
    ),
    { granted: false, reason: "payment_issue" },
  );
});

test("a payment issue without access names the payment issue", () => {
  assert.deepEqual(
    cloudAccess(signedIn({ canUseApp: false, paymentIssue: "past_due" })),
    { granted: false, reason: "payment_issue" },
  );
  // An unpaid subscription has no access, also when the server is late.
  assert.deepEqual(
    cloudAccess(signedIn({ canUseApp: true, paymentIssue: "unpaid" })),
    { granted: false, reason: "payment_issue" },
  );
});

test("a past-due account keeps access while the server allows it", () => {
  assert.deepEqual(
    cloudAccess(signedIn({ paymentIssue: "past_due" })),
    { granted: true, reason: "paid" },
  );
});

test("a running trial has access until the first payment", () => {
  const trialEnd = Date.parse("2026-10-28T00:00:00Z") / 1000;
  assert.deepEqual(
    cloudAccess(signedIn({ trialEnd, trialConfirmed: true })),
    { granted: true, reason: "trial" },
  );
  assert.deepEqual(
    cloudAccess(signedIn({ trialEnd, trialConfirmed: true, paymentConfirmed: true })),
    { granted: true, reason: "paid" },
  );
});

test("each paid plan has access", () => {
  for (const plan of ["basic", "unlimited", "lifetime"] as const) {
    assert.deepEqual(
      cloudAccess(signedIn({ plan, trialEnd: null }, plan)),
      { granted: true, reason: "paid" },
      plan,
    );
  }
});

test("app access without a paid plan is the beta", () => {
  assert.deepEqual(
    cloudAccess(signedIn({ plan: "beta_free", beta: true, billingRequired: false }, "beta_free")),
    { granted: true, reason: "beta" },
  );
});

test("entitlements that did not sync count a cached paid or beta plan", () => {
  assert.deepEqual(cloudAccess(signedIn(null, "unlimited")), {
    granted: true,
    reason: "paid",
  });
  assert.deepEqual(cloudAccess(signedIn(null, "beta_free")), {
    granted: true,
    reason: "beta",
  });
  assert.deepEqual(cloudAccess(signedIn(null, null)), {
    granted: false,
    reason: "no_plan",
  });
});
