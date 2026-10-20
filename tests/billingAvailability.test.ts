import assert from "node:assert/strict";
import test from "node:test";
import { canManageBilling } from "../src/billingAvailability.ts";
import type { Entitlements } from "../src/types.ts";

function entitlement(overrides: Partial<Entitlements> = {}): Entitlements {
  return { plan: "unlimited", beta: false, billingRequired: true, canUseApp: true, message: null, checkoutUrl: null, billingPortalUrl: null, savings: { period: "2026-09", tokensSaved: 0, capTokens: null, exhausted: false }, ...overrides };
}

test("paid accounts can open a dynamically created portal without a static URL", () => {
  assert.equal(canManageBilling(entitlement()), true);
  assert.equal(canManageBilling(entitlement({ plan: "basic" })), true);
  assert.equal(canManageBilling(entitlement({ billingPortalAvailable: true })), true);
});

test("the server capability covers former subscribers and overrides plan guesses", () => {
  assert.equal(canManageBilling(entitlement({ plan: "free", billingPortalAvailable: true })), true);
  assert.equal(canManageBilling(entitlement({ billingPortalAvailable: false })), false);
});

test("billing issues retain access to payment management", () => {
  assert.equal(canManageBilling(entitlement({ plan: "free", paymentIssue: "past_due" })), true);
});

test("free and beta accounts do not get a broken portal action", () => {
  assert.equal(canManageBilling(null), false);
  assert.equal(canManageBilling(entitlement({ plan: "free" })), false);
  assert.equal(canManageBilling(entitlement({ beta: true, billingRequired: false })), false);
  assert.equal(canManageBilling(entitlement({ plan: "free", billingPortalUrl: "   " })), false);
});

test("a configured external portal remains supported", () => {
  assert.equal(canManageBilling(entitlement({ plan: "free", billingPortalAvailable: false, billingPortalUrl: "https://billing.example/portal" })), true);
});
