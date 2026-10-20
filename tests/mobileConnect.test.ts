import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import { createMobileConnectStore } from "../src/mobileConnect.ts";

test("the Connect phone dialog store opens, closes, and notifies on a change only", () => {
  const store = createMobileConnectStore();
  let notified = 0;
  const unsubscribe = store.subscribe(() => {
    notified += 1;
  });
  assert.equal(store.getSnapshot(), false);

  store.open();
  store.open();
  assert.equal(store.getSnapshot(), true);
  assert.equal(notified, 1, "a second open is not a change");

  store.close();
  assert.equal(store.getSnapshot(), false);
  assert.equal(notified, 2);

  unsubscribe();
  store.open();
  assert.equal(notified, 2, "an unsubscribed listener gets no call");
});
