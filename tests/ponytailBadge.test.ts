import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import { ponytailBadgeLabel } from "../src/agentSessions.ts";

test("Ponytail badge hides off and labels active modes", () => {
  assert.equal(ponytailBadgeLabel(undefined), null);
  assert.equal(ponytailBadgeLabel(null), null);
  assert.equal(ponytailBadgeLabel("off"), null);
  assert.equal(ponytailBadgeLabel("lite"), "ponytail lite");
  assert.equal(ponytailBadgeLabel("full"), "ponytail full");
  assert.equal(ponytailBadgeLabel("ultra"), "ponytail ultra");
});
