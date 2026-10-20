import assert from "node:assert/strict";
import test from "node:test";

import { availableAccountOAuthButtons } from "../src/accountOAuth.ts";

test("shows account OAuth buttons only for providers returned by the server", () => {
  assert.deepEqual(availableAccountOAuthButtons(["apple"]), [
    { provider: "apple", label: "Continue with Apple" },
  ]);
  assert.deepEqual(availableAccountOAuthButtons(["google"]), [
    { provider: "google", label: "Continue with Google" },
  ]);
  assert.deepEqual(availableAccountOAuthButtons([]), []);
});

test("ignores unknown, duplicate, and malformed provider responses", () => {
  assert.deepEqual(
    availableAccountOAuthButtons(["apple", "github", "google", "apple"]),
    [
      { provider: "google", label: "Continue with Google" },
      { provider: "apple", label: "Continue with Apple" },
    ],
  );
  assert.deepEqual(availableAccountOAuthButtons({ google: true }), []);
  assert.deepEqual(availableAccountOAuthButtons(null), []);
});
