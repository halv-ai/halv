import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import { shortAge, shortAgeLabel } from "../src/shortAge.ts";

const NOW = Date.parse("2026-08-13T15:00:00.000Z");
const MINUTE = 60_000;

test("ages read now, minutes, hours, then days", () => {
  assert.deepEqual(shortAge(NOW, NOW), { unit: "now" });
  assert.deepEqual(shortAge(NOW - 59_999, NOW), { unit: "now" });
  assert.deepEqual(shortAge(NOW - MINUTE, NOW), { unit: "m", amount: 1 });
  assert.deepEqual(shortAge(NOW - 18 * MINUTE - 40_000, NOW), { unit: "m", amount: 18 });
  assert.deepEqual(shortAge(NOW - 59 * MINUTE, NOW), { unit: "m", amount: 59 });
  assert.deepEqual(shortAge(NOW - 60 * MINUTE, NOW), { unit: "h", amount: 1 });
  assert.deepEqual(shortAge(NOW - 3.9 * 60 * MINUTE, NOW), { unit: "h", amount: 3 });
  assert.deepEqual(shortAge(NOW - 24 * 60 * MINUTE, NOW), { unit: "d", amount: 1 });
  assert.deepEqual(shortAge(NOW - 5 * 24 * 60 * MINUTE, NOW), { unit: "d", amount: 5 });
  assert.deepEqual(shortAge(NOW - 400 * 24 * 60 * MINUTE, NOW), { unit: "d", amount: 400 });
});

test("ISO strings parse, and future or bad times read as now", () => {
  assert.deepEqual(shortAge("2026-08-13T14:42:00.000Z", NOW), { unit: "m", amount: 18 });
  assert.deepEqual(shortAge(NOW + 5 * MINUTE, NOW), { unit: "now" });
  assert.deepEqual(shortAge("not a date", NOW), { unit: "now" });
});

test("labels use the short source strings", () => {
  const t = (source: string, values?: Record<string, string | number>) =>
    source.replace(/\{(\w+)\}/g, (_, name: string) => String(values?.[name] ?? ""));
  assert.equal(shortAgeLabel({ unit: "now" }, t), "now");
  assert.equal(shortAgeLabel({ unit: "m", amount: 18 }, t), "18m");
  assert.equal(shortAgeLabel({ unit: "h", amount: 3 }, t), "3h");
  assert.equal(shortAgeLabel({ unit: "d", amount: 5 }, t), "5d");
});
