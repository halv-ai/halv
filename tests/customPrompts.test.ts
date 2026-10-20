import assert from "node:assert/strict";
import test from "node:test";
import {
  expandCustomPrompt,
  resolveCustomPromptNames,
  type CustomPrompt,
} from "../src/customPrompts.ts";

const prompts: CustomPrompt[] = [
  { name: "review", body: "Review this", scope: "project" },
  { name: "ship", body: "Ship this", scope: "user" },
  { name: "NEW", body: "Start this", scope: "codex" },
];

test("custom prompts use a user prefix when names conflict with built-ins or aliases", () => {
  assert.deepEqual(
    resolveCustomPromptNames(prompts, ["review", "new"]).map(
      ({ name, commandName }) => [name, commandName],
    ),
    [
      ["review", "user:review"],
      ["ship", "ship"],
      ["NEW", "user:NEW"],
    ],
  );
});

test("custom prompt arguments replace every ARGUMENTS placeholder", () => {
  assert.equal(
    expandCustomPrompt("Compare $ARGUMENTS, then test $ARGUMENTS.", "src tests"),
    "Compare src tests, then test src tests.",
  );
  assert.equal(expandCustomPrompt("Run $ARGUMENTS", ""), "Run ");
});

test("custom prompt arguments append on a new line when no placeholder exists", () => {
  assert.equal(expandCustomPrompt("Review the change.", "src/app.ts"), "Review the change.\nsrc/app.ts");
  assert.equal(expandCustomPrompt("Review the change.\n", "src/app.ts"), "Review the change.\nsrc/app.ts");
  assert.equal(expandCustomPrompt("Review the change.", ""), "Review the change.");
});
