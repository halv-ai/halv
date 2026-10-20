import test from "node:test";
import assert from "node:assert/strict";

import {
  canDismissLifetimeOffer,
  canLoadLifetimeOffer,
  LIFETIME_OFFER_DISMISSED_STORAGE_KEY,
  readLifetimeOfferDismissed,
  remainingOfferSeconds,
  subscribeLifetimeOfferDismissed,
  writeLifetimeOfferDismissed,
} from "../src/lifetimeOffer.ts";
import type { AccountAuthStatus, HalvPlan } from "../src/types.ts";

const offer = {
  active: true,
  owned: false,
  currency: "usd" as const,
  amount: 9000,
  serverNow: 100,
  startsAt: 50,
  endsAt: 130,
  capacity: 100,
  sold: 0,
  available: 100,
};

function account(
  plan: HalvPlan | null,
  entitlementPlan: HalvPlan | null = plan,
): AccountAuthStatus {
  return {
    signedIn: true,
    email: "subscriber@example.com",
    plan,
    serverUrl: "https://example.com",
    entitlements: entitlementPlan
      ? {
          beta: false,
          billingRequired: true,
          canUseApp: true,
          plan: entitlementPlan,
          message: null,
          checkoutUrl: null,
          billingPortalUrl: null,
          savings: {
            period: "2026-09",
            tokensSaved: 0,
            capTokens: null,
            exhausted: false,
          },
        }
      : null,
    syncedAt: null,
    firstLaunchAt: "2026-09-30T00:00:00Z",
    freeTurnsUsed: 0,
  };
}

test("lifetime countdown expires against server time without resetting", () => {
  assert.equal(remainingOfferSeconds(offer, 0), 30);
  assert.equal(remainingOfferSeconds(offer, 10_000), 20);
  assert.equal(remainingOfferSeconds(offer, 30_000), 0);
  assert.equal(remainingOfferSeconds({ ...offer, endsAt: 999 }, 66_000), 0);
});

test("inactive and owned offers never display a countdown", () => {
  assert.equal(remainingOfferSeconds({ ...offer, active: false }, 0), 0);
  assert.equal(remainingOfferSeconds({ ...offer, owned: true }, 0), 0);
});

test("offer waits for entitlements before loading", () => {
  assert.equal(canLoadLifetimeOffer(account("basic", null), false), false);
  assert.equal(canLoadLifetimeOffer(account("basic"), false), true);
});

test("lifetime access in either account field blocks the offer", () => {
  assert.equal(canLoadLifetimeOffer(account("lifetime", "basic"), false), false);
  assert.equal(canLoadLifetimeOffer(account("basic", "lifetime"), false), false);
});

test("dismissal blocks every later offer load", () => {
  assert.equal(canLoadLifetimeOffer(account("basic"), true), false);
  assert.equal(canLoadLifetimeOffer(account("beta_free"), true), false);
});

test("only current recurring subscribers can dismiss the offer", () => {
  assert.equal(canDismissLifetimeOffer(account("basic")), true);
  assert.equal(canDismissLifetimeOffer(account("unlimited")), true);
  assert.equal(canDismissLifetimeOffer(account("beta_free")), false);
  assert.equal(canDismissLifetimeOffer(account("lifetime")), false);
});

test("dismissal persists in storage", () => {
  const values = new Map<string, string>();
  const storage = {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
  };

  assert.equal(readLifetimeOfferDismissed(storage), false);
  writeLifetimeOfferDismissed(storage);
  assert.equal(values.get(LIFETIME_OFFER_DISMISSED_STORAGE_KEY), "1");
  assert.equal(readLifetimeOfferDismissed(storage), true);
});

test("dismissal notifies simultaneous mounts when storage rejects writes", () => {
  let notifications = 0;
  const unsubscribe = subscribeLifetimeOfferDismissed(() => {
    notifications += 1;
  });
  const storage = {
    getItem: () => null,
    setItem: () => {
      throw new DOMException("Storage unavailable", "SecurityError");
    },
  };

  assert.doesNotThrow(() => writeLifetimeOfferDismissed(storage));
  assert.equal(notifications, 1);
  unsubscribe();
  writeLifetimeOfferDismissed(storage);
  assert.equal(notifications, 1);
});
