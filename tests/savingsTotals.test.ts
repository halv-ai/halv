import assert from "node:assert/strict";
import test from "node:test";
import {
  calculatePlanStretchPercent,
  mergeAllTimeSavings,
} from "../src/savingsTotals.ts";
import type {
  AccountAuthStatus,
  SavingsEntitlement,
  SavingsTotalsPayload,
} from "../src/types.ts";

const localTotals: SavingsTotalsPayload = {
  tokensSaved: 400,
  savingsUsd: 2,
  pendingTokensSaved: 100,
  pendingSavingsUsd: 0.5,
  sessionTokensSaved: 25,
  sessionSavingsUsd: 0.1,
  cruxTokensSaved: 10,
};

function accountStatus(
  signedIn: boolean,
  savings: SavingsEntitlement,
): AccountAuthStatus {
  return {
    signedIn,
    email: signedIn ? "user@example.com" : null,
    plan: signedIn ? "basic" : null,
    serverUrl: "https://api.halv.test",
    entitlements: signedIn
      ? {
          beta: false,
          billingRequired: true,
          canUseApp: true,
          plan: "basic",
          message: null,
          checkoutUrl: null,
          billingPortalUrl: null,
          savings,
        }
      : null,
    syncedAt: null,
    firstLaunchAt: "2026-08-01T00:00:00Z",
    freeTurnsUsed: 0,
  };
}

const monthlySavings: SavingsEntitlement = {
  period: "2026-08",
  tokensSaved: 40,
  capTokens: 1_000,
  exhausted: false,
};

test("merges signed-in server history with pending local savings", () => {
  const account = accountStatus(true, {
    ...monthlySavings,
    allTimeTokensSaved: 800,
    allTimeSavingsUsd: 4,
  });

  assert.deepEqual(mergeAllTimeSavings(localTotals, account), {
    ...localTotals,
    tokensSaved: 900,
    savingsUsd: 4.5,
    dollarsUsd: 0,
    localTokensSaved: 400,
    localSavingsUsd: 2,
    localDollarsUsd: 0,
  });
});

test("uses local totals while signed out", () => {
  const account = accountStatus(false, monthlySavings);
  assert.deepEqual(mergeAllTimeSavings(localTotals, account), {
    ...localTotals,
    dollarsUsd: 0,
    localTokensSaved: 400,
    localSavingsUsd: 2,
    localDollarsUsd: 0,
  });
});

test("uses local totals with an old server response", () => {
  const account = accountStatus(true, monthlySavings);
  assert.deepEqual(mergeAllTimeSavings(localTotals, account), {
    ...localTotals,
    dollarsUsd: 0,
    localTokensSaved: 400,
    localSavingsUsd: 2,
    localDollarsUsd: 0,
  });
});

test("dollars use the server history when the server knows them", () => {
  const local = {
    ...localTotals,
    dollarsUsd: 0.3,
    pendingDollarsUsd: 0.05,
  };
  const withDollars = accountStatus(true, {
    ...monthlySavings,
    allTimeTokensSaved: 800,
    allTimeSavingsUsd: 4,
    allTimeV2Usd: 1.2,
  });
  assert.equal(mergeAllTimeSavings(local, withDollars).dollarsUsd, 1.25);
  assert.equal(mergeAllTimeSavings(local, withDollars).localDollarsUsd, 0.3);

  // An old server has no dollar field: the local dollars stay.
  const oldServer = accountStatus(true, {
    ...monthlySavings,
    allTimeTokensSaved: 800,
    allTimeSavingsUsd: 4,
  });
  assert.equal(mergeAllTimeSavings(local, oldServer).dollarsUsd, 0.3);
});

test("does not add acknowledged local savings twice", () => {
  const account = accountStatus(true, {
    ...monthlySavings,
    allTimeTokensSaved: 800,
    allTimeSavingsUsd: 4,
  });

  const merged = mergeAllTimeSavings(localTotals, account);
  assert.equal(merged.tokensSaved, 900);
  assert.notEqual(merged.tokensSaved, 1_200);
  assert.equal(merged.savingsUsd, 4.5);
});

test("calculates plan stretch from local savings after account merge", () => {
  const account = accountStatus(true, {
    ...monthlySavings,
    allTimeTokensSaved: 800,
    allTimeSavingsUsd: 4,
  });

  const merged = mergeAllTimeSavings(localTotals, account);
  assert.equal(calculatePlanStretchPercent(merged, 600), 40);
  assert.notEqual(calculatePlanStretchPercent(merged, 600), 60);
});
