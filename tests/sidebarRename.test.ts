import assert from "node:assert/strict";
import test from "node:test";
// @ts-expect-error Node's test runner needs the source extension.
import { inlineRenameCommitValue } from "../src/sidebarRename.ts";

test("inline rename commits only changed non-empty titles", () => {
  assert.equal(inlineRenameCommitValue("Old title", "  New title  "), "New title");
  assert.equal(inlineRenameCommitValue("Old title", "   "), null);
  assert.equal(inlineRenameCommitValue("Old title", "Old title"), null);
});
