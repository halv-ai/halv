import assert from "node:assert/strict";
import test from "node:test";
import {
  allowsDelivery,
  notificationEnabled,
  notificationPermissionAction,
  sendTestNotificationLater,
  TEST_NOTIFICATION_DELAY_MS,
  type NotificationPermissionState,
} from "../src/notifications.ts";
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

test("an error banner ignores the never setting but keeps the focus rule", () => {
  const never = preferences({ completionNotifications: "never" });
  assert.equal(notificationEnabled(never, "error", false), true);
  assert.equal(notificationEnabled(never, "error", true), false);
  assert.equal(notificationEnabled(preferences(), "error", false), true);
  assert.equal(notificationEnabled(preferences(), "error", true), false);
  const always = preferences({ completionNotifications: "always" });
  assert.equal(notificationEnabled(always, "error", true), true);
  assert.equal(notificationEnabled(null, "error", false), true);
  assert.equal(notificationEnabled(null, "error", true), false);
});

test("an error banner ignores the permission and question toggles", () => {
  const value = preferences({
    permissionNotifications: false,
    questionNotifications: false,
  });
  assert.equal(notificationEnabled(value, "error", false), true);
});

function gate(
  state: NotificationPermissionState | Error,
  requested: NotificationPermissionState = "granted",
  fallback = true,
) {
  const calls: string[] = [];
  const allowed = allowsDelivery({
    read: async () => {
      calls.push("read");
      if (state instanceof Error) throw state;
      return state;
    },
    request: async () => {
      calls.push("request");
      return requested;
    },
    fallback: async () => {
      calls.push("fallback");
      return fallback;
    },
  });
  return { allowed, calls };
}

test("a denied macOS state blocks a banner without asking again", async () => {
  const { allowed, calls } = gate("denied");
  assert.equal(await allowed, false);
  assert.deepEqual(calls, ["read"]);
});

test("a granted macOS state allows a banner", async () => {
  const { allowed, calls } = gate("granted");
  assert.equal(await allowed, true);
  assert.deepEqual(calls, ["read"]);
});

test("banners off in macOS still allow the call, because the history keeps them", async () => {
  assert.equal(await gate("bannersOff").allowed, true);
});

test("an undecided macOS state asks once and follows the answer", async () => {
  const granted = gate("notDetermined", "granted");
  assert.equal(await granted.allowed, true);
  assert.deepEqual(granted.calls, ["read", "request"]);
  const denied = gate("notDetermined", "denied");
  assert.equal(await denied.allowed, false);
});

test("an unknown or unreadable state falls back to the plugin answer", async () => {
  const unknown = gate("unknown", "granted", false);
  assert.equal(await unknown.allowed, false);
  assert.deepEqual(unknown.calls, ["read", "fallback"]);
  const failed = gate(new Error("no ipc"), "granted", true);
  assert.equal(await failed.allowed, true);
  assert.deepEqual(failed.calls, ["read", "fallback"]);
});

test("the settings notice names the fix for each macOS state", () => {
  assert.equal(notificationPermissionAction("denied"), "openSettings");
  assert.equal(notificationPermissionAction("bannersOff"), "openSettings");
  assert.equal(notificationPermissionAction("notDetermined"), "request");
  assert.equal(notificationPermissionAction("granted"), null);
  assert.equal(notificationPermissionAction("unknown"), null);
});

test("the settings test banner waits so the user can put another app in front", async () => {
  // macOS shows no banner while Halv is the frontmost app. The legacy
  // notification center delegate has no shouldPresentNotification override.
  const events: string[] = [];
  let releaseWait: () => void = () => {};
  const waits: number[] = [];
  const done = sendTestNotificationLater({
    send: async () => {
      events.push("send");
    },
    wait: (ms) => {
      waits.push(ms);
      return new Promise<void>((resolve) => {
        releaseWait = resolve;
      });
    },
  });
  await Promise.resolve();
  assert.deepEqual(events, [], "the banner must not go out while Halv is in front");
  assert.deepEqual(waits, [TEST_NOTIFICATION_DELAY_MS]);
  assert.ok(TEST_NOTIFICATION_DELAY_MS >= 3_000);
  releaseWait();
  await done;
  assert.deepEqual(events, ["send"]);
});
