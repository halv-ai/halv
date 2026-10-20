import assert from "node:assert/strict";
import test from "node:test";
import {
  artifactBelongsToUserTurn,
  shouldShowTaskPlanForUserTurn,
} from "../src/turnArtifacts.ts";

test("completed task plans disappear when their turn stops streaming", () => {
  assert.equal(
    shouldShowTaskPlanForUserTurn("latest", "latest", false, true),
    false,
  );
  assert.equal(
    shouldShowTaskPlanForUserTurn("latest", "latest", true, true),
    true,
  );
  assert.equal(
    shouldShowTaskPlanForUserTurn("older", "latest", true, true),
    false,
  );
});

test("turn artifacts stay with their originating user message", () => {
  const knownUsers = new Set(["older", "latest"]);
  assert.equal(
    artifactBelongsToUserTurn(
      "older",
      "2026-07-14T10:00:00.000Z",
      "older",
      "2026-07-14T10:00:00.000Z",
      "latest",
      false,
      knownUsers,
    ),
    true,
  );
  assert.equal(
    artifactBelongsToUserTurn(
      "older",
      "2026-07-14T10:00:00.000Z",
      "latest",
      "2026-07-14T10:05:00.000Z",
      "latest",
      false,
      knownUsers,
    ),
    false,
  );
});

test("a live artifact can follow an optimistic user message until storage reloads", () => {
  const knownUsers = new Set(["older", "local-latest"]);
  assert.equal(
    artifactBelongsToUserTurn(
      "persisted-latest",
      "2026-07-14T10:05:00.500Z",
      "local-latest",
      "2026-07-14T10:05:00.000Z",
      "local-latest",
      true,
      knownUsers,
    ),
    true,
  );
  assert.equal(
    artifactBelongsToUserTurn(
      "persisted-latest",
      "2026-07-14T10:05:00.500Z",
      "local-latest",
      "2026-07-14T10:05:00.000Z",
      "local-latest",
      false,
      knownUsers,
    ),
    false,
  );
});

test("an orphaned artifact from an edited-away turn does not jump to a new turn", () => {
  const knownUsers = new Set(["local-latest"]);
  assert.equal(
    artifactBelongsToUserTurn(
      "deleted-old-turn",
      "2026-07-14T09:00:00.000Z",
      "local-latest",
      "2026-07-14T10:05:00.000Z",
      "local-latest",
      true,
      knownUsers,
    ),
    false,
  );
});
