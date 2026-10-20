import assert from "node:assert/strict";
import test from "node:test";
// @ts-expect-error Node's test runner needs the source extension.
import { resolveAgentTitleLayers } from "../src/agentSessions.ts";

const titles = {
  defaultTitle: "Codex 1",
  generatedTitle: "Fix the release checks",
  aiVaultTitle: {
    sessionId: "provider-session-1",
    title: "Release check repair",
  },
  customTitle: "Ship blocker",
  generation: 3,
};

test("agent titles resolve custom then AI then prompt then default", () => {
  assert.equal(resolveAgentTitleLayers(titles, "Codex"), "Ship blocker");
  assert.equal(
    resolveAgentTitleLayers({ ...titles, customTitle: null }, "Codex"),
    "Release check repair",
  );
  assert.equal(
    resolveAgentTitleLayers(
      { ...titles, customTitle: null, aiVaultTitle: null },
      "Codex",
    ),
    "Fix the release checks",
  );
  assert.equal(
    resolveAgentTitleLayers(
      {
        ...titles,
        customTitle: null,
        aiVaultTitle: null,
        generatedTitle: null,
      },
      "Codex",
    ),
    "Codex 1",
  );
});
