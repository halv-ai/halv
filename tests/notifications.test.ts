import assert from "node:assert/strict";
import test from "node:test";
import { notificationEnabled } from "../src/notifications.ts";
import type { AppPreferences } from "../src/types.ts";

function preferences(
  overrides: Partial<AppPreferences> = {},
): AppPreferences {
  return {
    subagentConcurrencyLimit: null,
    completionNotifications: "background",
    permissionNotifications: true,
    questionNotifications: true,
    ...overrides,
  };
}

test("completion delivery supports never, background, and always", () => {
  assert.equal(notificationEnabled(preferences({ completionNotifications: "never" }), "completion", false), false);
  assert.equal(notificationEnabled(preferences(), "completion", false), true);
  assert.equal(notificationEnabled(preferences(), "completion", true), false);
  assert.equal(notificationEnabled(preferences({ completionNotifications: "always" }), "completion", true), true);
});

test("permission and question toggles are independent", () => {
  const value = preferences({ permissionNotifications: false, questionNotifications: true });
  assert.equal(notificationEnabled(value, "permission", false), false);
  assert.equal(notificationEnabled(value, "question", false), true);
  assert.equal(notificationEnabled(value, "question", true), false);
});
