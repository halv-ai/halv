import assert from "node:assert/strict";
import test from "node:test";
import { createNotificationThrottle } from "../src/chatNotificationThrottle.ts";

test("a repeat for one conversation and kind waits for the window", () => {
  const throttle = createNotificationThrottle(60_000);
  assert.equal(throttle.allow("chat-1:done", 1_000), true);
  assert.equal(throttle.allow("chat-1:done", 30_000), false);
  assert.equal(throttle.allow("chat-1:done", 60_999), false);
  assert.equal(throttle.allow("chat-1:done", 61_000), true);
});

test("a done banner does not block an error banner of the same conversation", () => {
  const throttle = createNotificationThrottle(60_000);
  assert.equal(throttle.allow("chat-1:done", 1_000), true);
  assert.equal(throttle.allow("chat-1:error", 2_000), true);
});

test("one conversation does not block another", () => {
  const throttle = createNotificationThrottle(60_000);
  assert.equal(throttle.allow("chat-1:done", 1_000), true);
  assert.equal(throttle.allow("chat-2:done", 1_500), true);
});

test("a blocked call does not extend the window", () => {
  const throttle = createNotificationThrottle(60_000);
  assert.equal(throttle.allow("chat-1:done", 0), true);
  assert.equal(throttle.allow("chat-1:done", 59_000), false);
  assert.equal(throttle.allow("chat-1:done", 60_000), true);
});

test("old keys leave the memory of the throttle", () => {
  const throttle = createNotificationThrottle(1_000);
  for (let index = 0; index < 50; index += 1) {
    throttle.allow(`chat-${index}:done`, index);
  }
  assert.equal(throttle.size(), 50);
  throttle.allow("chat-new:done", 10_000);
  assert.equal(throttle.size(), 1);
});
