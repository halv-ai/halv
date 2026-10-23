import assert from "node:assert/strict";
import test from "node:test";

import {
  dotShortcutVisible,
  parseSubagentConcurrencyLimit,
} from "../src/preferences.ts";

test("treats an empty subagent concurrency preference as Unlimited", () => {
  assert.deepEqual(parseSubagentConcurrencyLimit(""), {
    valid: true,
    limit: null,
  });
  assert.deepEqual(parseSubagentConcurrencyLimit("   "), {
    valid: true,
    limit: null,
  });
});

test("accepts positive integer subagent concurrency limits without a product cap", () => {
  assert.deepEqual(parseSubagentConcurrencyLimit("1"), {
    valid: true,
    limit: 1,
  });
  assert.deepEqual(parseSubagentConcurrencyLimit("1000000"), {
    valid: true,
    limit: 1_000_000,
  });
  assert.deepEqual(
    parseSubagentConcurrencyLimit(String(Number.MAX_SAFE_INTEGER)),
    { valid: true, limit: Number.MAX_SAFE_INTEGER },
  );
});

test("hides the Dot shortcut unless the preference is explicitly true", () => {
  assert.equal(dotShortcutVisible(null), false);
  assert.equal(dotShortcutVisible(undefined), false);
  // A preferences object from an older release has no key.
  assert.equal(dotShortcutVisible({}), false);
  assert.equal(dotShortcutVisible({ showDotShortcut: false }), false);
  assert.equal(dotShortcutVisible({ showDotShortcut: true }), true);
});

test("rejects zero, negative, fractional, exponent, and non-numeric limits", () => {
  for (const value of ["0", "-1", "1.5", "1e3", "agents"]) {
    assert.deepEqual(parseSubagentConcurrencyLimit(value), { valid: false });
  }
});
