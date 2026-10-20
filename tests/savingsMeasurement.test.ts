import assert from "node:assert/strict";
import test from "node:test";

import { savingsReplyLabel } from "../src/messageSavings.ts";
import {
  deriveSavingsMeasurement,
  deriveSavingsMeasurementBreakdown,
  savingsMeasurementIsEstimated,
} from "../src/savingsMeasurement.ts";

test("derives measured savings with a zero-safe token value", () => {
  assert.deepEqual(
    deriveSavingsMeasurement({
      tokensSaved: 0,
      savingsUsd: 0,
      savingsSource: "measured",
      pricingSource: "headroom",
    }),
    {
      state: "measured",
      tokens: 0,
      usd: 0,
      composition: { headroom: 0 },
      pricingSource: "headroom",
    },
  );
});

test("derives estimated savings from Crux and API pricing", () => {
  const measurement = deriveSavingsMeasurement({
    tokensSaved: 40,
    savingsUsd: 0.02,
    pricingSource: "model_table",
    cruxTokensSaved: 40,
    cruxIncludedInTotal: true,
  });

  assert.deepEqual(measurement, {
    state: "estimated",
    tokens: 40,
    usd: 0.02,
    composition: { headroom: 0, crux: 40 },
    pricingSource: "model_table",
  });
  assert.equal(savingsMeasurementIsEstimated(measurement), true);
});

test("keeps inactive sources unmeasured instead of forcing zero", () => {
  assert.deepEqual(
    deriveSavingsMeasurement({
      tokensSaved: 0,
      savingsSources: { proxy: false, rtk: "missing", crux: "missing" },
    }),
    { state: "unmeasured", tokens: null, usd: null },
  );
});

test("treats instruction-based RTK as measured evidence", () => {
  assert.deepEqual(
    deriveSavingsMeasurement({
      tokensSaved: 0,
      savingsSources: {
        proxy: true,
        rtk: "instruction",
        crux: "missing",
      },
    }),
    { state: "measured", tokens: 0, usd: null },
  );
});

test("labels each savings composition without inventing compression", () => {
  const headroom = deriveSavingsMeasurement({
    tokensSaved: 80,
    headroomTokensSaved: 80,
    savingsSource: "measured",
  });
  const rtk = deriveSavingsMeasurement({
    tokensSaved: 50,
    rtkTokensSaved: 50,
    rtkIncludedInTotal: true,
  });
  const crux = deriveSavingsMeasurement({
    tokensSaved: 30,
    cruxTokensSaved: 30,
    cruxIncludedInTotal: true,
  });
  const mixed = deriveSavingsMeasurement({
    tokensSaved: 70,
    rtkTokensSaved: 40,
    rtkIncludedInTotal: true,
    cruxTokensSaved: 30,
    cruxIncludedInTotal: true,
  });
  const unknown = deriveSavingsMeasurement({
    tokensSaved: 25,
    savingsSource: "measured",
  });

  assert.equal(savingsReplyLabel(headroom), "compressed");
  assert.equal(savingsReplyLabel(rtk), "RTK");
  assert.equal(savingsReplyLabel(crux), "crux (est.)");
  assert.equal(savingsReplyLabel(mixed), "RTK + crux (est.)");
  assert.equal(savingsReplyLabel(unknown), "saved");
});

test("keeps included savings components within the session total", () => {
  const breakdown = deriveSavingsMeasurementBreakdown({
    tokensSaved: 100,
    savingsSource: "headroom",
    pricingSource: "headroom",
    rtkTokensSaved: 20,
    rtkIncludedInTotal: true,
    cruxTokensSaved: 10,
    cruxIncludedInTotal: true,
    cacheReadTokens: 60,
  });

  assert.deepEqual(breakdown.included, {
    headroom: 70,
    rtk: 20,
    crux: 10,
    other: 0,
  });
  assert.deepEqual(breakdown.separate, {
    rtk: 0,
    crux: 0,
    cacheRead: 60,
  });
  assert.equal(breakdown.measurement.state, "estimated");
  assert.equal(
    Object.values(breakdown.included).reduce(
      (total, value) => total + value,
      0,
    ),
    100,
  );
});

test("keeps provider cache reuse separate without explicit inclusion", () => {
  const breakdown = deriveSavingsMeasurementBreakdown({
    tokensSaved: 50,
    pricingSource: "model_table",
    cacheReadTokens: 100,
  });

  assert.deepEqual(breakdown.included, {
    headroom: 0,
    rtk: 0,
    crux: 0,
    other: 50,
  });
  assert.equal(breakdown.separate.cacheRead, 100);
});

test("does not add separately reported RTK or Crux savings", () => {
  const breakdown = deriveSavingsMeasurementBreakdown({
    tokensSaved: 100,
    savingsSource: "headroom",
    pricingSource: "headroom",
    rtkTokensSaved: 20,
    rtkIncludedInTotal: false,
    cruxTokensSaved: 10,
    cruxIncludedInTotal: false,
  });

  assert.deepEqual(breakdown.included, {
    headroom: 100,
    rtk: 0,
    crux: 0,
    other: 0,
  });
  assert.deepEqual(breakdown.separate, {
    rtk: 20,
    crux: 10,
    cacheRead: 0,
  });
});

test("keeps included components separate when the total is unavailable", () => {
  const breakdown = deriveSavingsMeasurementBreakdown({
    savingsSource: "measured",
    rtkTokensSaved: 20,
    rtkIncludedInTotal: true,
  });

  assert.deepEqual(breakdown.included, {
    headroom: 0,
    rtk: 0,
    crux: 0,
    other: 0,
  });
  assert.equal(breakdown.separate.rtk, 20);
});

test("keeps conflicting included components visible but outside the total", () => {
  const breakdown = deriveSavingsMeasurementBreakdown({
    tokensSaved: 50,
    savingsSource: "measured",
    rtkTokensSaved: 40,
    rtkIncludedInTotal: true,
    cruxTokensSaved: 30,
    cruxIncludedInTotal: true,
  });

  assert.deepEqual(breakdown.included, {
    headroom: 0,
    rtk: 0,
    crux: 0,
    other: 50,
  });
  assert.deepEqual(breakdown.separate, {
    rtk: 40,
    crux: 30,
    cacheRead: 0,
  });
});
