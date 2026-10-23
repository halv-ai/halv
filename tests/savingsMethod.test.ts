import assert from "node:assert/strict";
import test from "node:test";

import {
  countedRoutingUsd,
  includesRoutingEstimate,
  laneIsPlan,
  ROUTING_NOT_ESTIMATED,
  routingRange,
  routingSavingsFromTotals,
} from "../src/savingsMethod.ts";

test("subscription lanes are plans in both lane spellings", () => {
  for (const lane of ["claudeSubscription", "codexChatGpt", "kimiSubscription", "zaiGlm"]) {
    assert.equal(laneIsPlan(lane), true, lane);
  }
  assert.equal(laneIsPlan("claude_subscription"), true);
  for (const lane of ["claudeApiKey", "grokApiKey", "kimiApiKey", "custom", null, undefined]) {
    assert.equal(laneIsPlan(lane), false, String(lane));
  }
});

test("only a routing estimate makes a total approximate", () => {
  assert.equal(includesRoutingEstimate(ROUTING_NOT_ESTIMATED), false);
  assert.equal(
    includesRoutingEstimate({
      state: "estimated",
      netUsd: 1,
      lowUsd: 0.5,
      highUsd: 2,
      chains: 1,
    }),
    true,
  );
});

const totals = {
  chains: 3,
  unpricedChains: 1,
  lowUsd: -0.2,
  pointUsd: 0.4,
  highUsd: 1.1,
  benchChains: 0,
  unreportedChains: 1,
};

test("settled chains with a price make routing estimated at the low end", () => {
  const routing = routingSavingsFromTotals(totals);
  assert.deepEqual(routing, {
    state: "estimated",
    netUsd: 0,
    lowUsd: 0,
    highUsd: 1.1,
    chains: 3,
  });
  assert.equal(countedRoutingUsd(routing), 0);
});

test("no priced chain keeps routing not estimated", () => {
  assert.equal(routingSavingsFromTotals(null), ROUTING_NOT_ESTIMATED);
  assert.equal(
    routingSavingsFromTotals({ ...totals, chains: 0, lowUsd: 0, highUsd: 0 }),
    ROUTING_NOT_ESTIMATED,
  );
  assert.equal(countedRoutingUsd(ROUTING_NOT_ESTIMATED), 0);
});

function estimated(lowUsd: number, highUsd: number) {
  return { state: "estimated" as const, netUsd: lowUsd, lowUsd, highUsd, chains: 2 };
}

test("routing shows a range only when the high end is above the low end at cents", () => {
  assert.equal(routingRange(ROUTING_NOT_ESTIMATED), null);
  assert.equal(routingRange(estimated(10.76, 10.76)), null);
  assert.equal(routingRange(estimated(10.76, 10.764)), null);
  assert.equal(routingRange(estimated(0, 0)), null);
  assert.deepEqual(routingRange(estimated(0.84, 2.65)), { lowUsd: 0.84, highUsd: 2.65 });
  assert.deepEqual(routingRange(estimated(0, 0.01)), { lowUsd: 0, highUsd: 0.01 });
});
