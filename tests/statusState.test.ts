import assert from "node:assert/strict";
import test from "node:test";

import { mergeStatus, statusStateEqual } from "../src/statusState.ts";
import type { AccountAuthStatus, StatusState } from "../src/types.ts";

function status(overrides: Partial<StatusState> = {}): StatusState {
  return {
    account: null,
    auth: null,
    anthropic: null,
    sidecar: null,
    sidecarStats: null,
    error: null,
    ...overrides,
  };
}

function account(overrides: Partial<AccountAuthStatus> = {}): AccountAuthStatus {
  return {
    signedIn: true,
    email: "user@example.com",
    plan: null,
    serverUrl: "https://example.com",
    entitlements: null,
    syncedAt: null,
    firstLaunchAt: "2026-08-27T00:00:00Z",
    freeTurnsUsed: 0,
    ...overrides,
  };
}

test("identical status returns the current reference", () => {
  const current = status({ account: account() });
  const next = status({ account: account() });

  assert.strictEqual(mergeStatus(current, next), current);
});

test("a changed field returns the next object", () => {
  const current = status();
  const next = status({ error: "status unavailable" });

  assert.strictEqual(mergeStatus(current, next), next);
});

test("the stale account rule keeps the current account", () => {
  const staleAccount = account({ stale: true });
  const current = status({ account: staleAccount });
  const unchanged = mergeStatus(current, status({ account: account() }));
  const next = status({ account: account(), error: "status unavailable" });

  const merged = mergeStatus(current, next);

  assert.strictEqual(unchanged, current);
  assert.notStrictEqual(merged, current);
  assert.strictEqual(merged.account, staleAccount);
  assert.equal(merged.error, "status unavailable");
});

test("nested and array differences are detected", () => {
  assert.equal(
    statusStateEqual(
      { nested: { values: ["one", { count: 2 }] } },
      { nested: { values: ["one", { count: 3 }] } },
    ),
    false,
  );
});

test("a missing key equals an undefined key", () => {
  assert.equal(
    statusStateEqual(
      { configured: true },
      { configured: true, masked: undefined },
    ),
    true,
  );
});
