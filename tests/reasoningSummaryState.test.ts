import assert from "node:assert/strict";
import test from "node:test";

import {
  readReasoningSummaryOpen,
  REASONING_SUMMARY_STORAGE_KEY,
  writeReasoningSummaryOpen,
  type ReasoningSummaryStorage,
} from "../src/reasoningSummaryState.ts";

function memoryStorage(initialValue: string | null = null) {
  let value = initialValue;
  const storage: ReasoningSummaryStorage = {
    getItem(key) {
      assert.equal(key, REASONING_SUMMARY_STORAGE_KEY);
      return value;
    },
    setItem(key, nextValue) {
      assert.equal(key, REASONING_SUMMARY_STORAGE_KEY);
      value = nextValue;
    },
  };
  return { storage, value: () => value };
}

test("reasoning summary defaults to open", () => {
  assert.equal(readReasoningSummaryOpen(memoryStorage().storage), true);
  assert.equal(readReasoningSummaryOpen(null), true);
});

test("reasoning summary restores the saved closed state", () => {
  assert.equal(readReasoningSummaryOpen(memoryStorage("false").storage), false);
});

test("reasoning summary writes and restores each user choice", () => {
  const memory = memoryStorage();

  writeReasoningSummaryOpen(memory.storage, false);
  assert.equal(memory.value(), "false");
  assert.equal(readReasoningSummaryOpen(memory.storage), false);

  writeReasoningSummaryOpen(memory.storage, true);
  assert.equal(memory.value(), "true");
  assert.equal(readReasoningSummaryOpen(memory.storage), true);
});
