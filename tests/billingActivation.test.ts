import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  billingActivationComplete,
  pollForBillingActivation,
} from "../src/billingActivation.ts";
import type { BillingPlan } from "../src/billing.ts";
import type { AccountAuthStatus } from "../src/types.ts";

function accountStatus(
  plan: BillingPlan | "beta_free",
  canUseApp: boolean,
): AccountAuthStatus {
  return {
    signedIn: true,
    email: "checkout@example.com",
    plan,
    serverUrl: "https://api.halv.ai",
    entitlements: {
      beta: false,
      billingRequired: true,
      canUseApp,
      plan,
      message: canUseApp ? null : "Subscription required",
      checkoutUrl: null,
      billingPortalUrl: null,
      savings: {
        period: "2026-08",
        tokensSaved: 0,
        capTokens: null,
        exhausted: false,
      },
    },
    syncedAt: "2026-08-10T12:00:00Z",
    firstLaunchAt: "2026-08-01T12:00:00Z",
    freeTurnsUsed: 0,
    stale: false,
  };
}

test("activation requires app access and the checkout target plan", () => {
  assert.equal(billingActivationComplete(accountStatus("basic", false), "basic"), false);
  assert.equal(billingActivationComplete(accountStatus("basic", true), "basic"), true);
  assert.equal(
    billingActivationComplete(accountStatus("basic", true), "unlimited"),
    false,
  );
  assert.equal(
    billingActivationComplete(accountStatus("unlimited", true), "unlimited"),
    true,
  );
});

test("lifetime activation waits for the lifetime entitlement", () => {
  assert.equal(billingActivationComplete(accountStatus("unlimited", true), "lifetime"), false);
  assert.equal(billingActivationComplete(accountStatus("lifetime", false), "lifetime"), false);
  assert.equal(billingActivationComplete(accountStatus("lifetime", true), "lifetime"), true);
});

test("activation polling refreshes on the configured interval until the plan arrives", async () => {
  let clockMs = 0;
  let refreshCount = 0;
  const observed: AccountAuthStatus[] = [];

  const result = await pollForBillingActivation({
    expectedPlan: "unlimited",
    intervalMs: 3_000,
    timeoutMs: 90_000,
    now: () => clockMs,
    sleep: async (durationMs) => {
      clockMs += durationMs;
    },
    refresh: async () => {
      refreshCount += 1;
      return refreshCount < 3
        ? accountStatus("basic", true)
        : accountStatus("unlimited", true);
    },
    onStatus: (status) => observed.push(status),
  });

  assert.equal(result.outcome, "activated");
  assert.equal(refreshCount, 3);
  assert.equal(observed.length, 3);
  assert.equal(clockMs, 6_000);
});

test("activation polling times out after the bounded window", async () => {
  let clockMs = 0;
  let refreshCount = 0;

  const result = await pollForBillingActivation({
    expectedPlan: "basic",
    intervalMs: 3_000,
    timeoutMs: 9_000,
    now: () => clockMs,
    sleep: async (durationMs) => {
      clockMs += durationMs;
    },
    refresh: async () => {
      refreshCount += 1;
      return accountStatus("beta_free", false);
    },
    onStatus: () => undefined,
  });

  assert.equal(result.outcome, "timedOut");
  assert.equal(refreshCount, 3);
  assert.equal(clockMs, 9_000);
});

test("activation polling stops without updating status after cancellation", async () => {
  let cancelled = false;
  let statusUpdates = 0;

  const result = await pollForBillingActivation({
    refresh: async () => {
      cancelled = true;
      return accountStatus("basic", true);
    },
    onStatus: () => {
      statusUpdates += 1;
    },
    cancelled: () => cancelled,
  });

  assert.equal(result.outcome, "cancelled");
  assert.equal(statusUpdates, 0);
});

test("browser checkout starts the shared activation flow after opening", () => {
  const source = readFileSync(
    new URL("../src/useBillingCheckout.ts", import.meta.url),
    "utf8",
  );
  const browserCheckout = source.match(
    /async function openInBrowser[\s\S]*?async function contactSupport/,
  )?.[0];

  assert.ok(browserCheckout, "expected the browser checkout handler");
  assert.match(
    browserCheckout,
    /await billingCheckoutInBrowser\(plan, interval\);\s*if \(mountedRef\.current\) \{\s*beginActivation\(\);/,
  );
  assert.doesNotMatch(browserCheckout, /setPhase\(\(current\)/);
});

test("subscription sales offer only Unlimited monthly", () => {
  const source = readFileSync(
    new URL("../src/components/SubscriptionPlans.tsx", import.meta.url),
    "utf8",
  );

  assert.match(
    source,
    /const billingInterval: BillingInterval = "month"/,
  );
  assert.match(source, /const availablePlans: BillingPlan\[\] = \["unlimited"\]/);
  assert.doesNotMatch(source, /setBillingInterval/);
});
