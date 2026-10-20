import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  collectEditedPaths,
  collectWorkedDirectories,
  createGitRepoDiscoveryCache,
} from "../src/diffRailRepos.ts";
import type { AgentSessionTranscriptItem } from "../src/agentSessions.ts";

test("expires git repository discovery and preserves generation invalidation", async () => {
  let now = 1_000;
  let calls = 0;
  const discover = createGitRepoDiscoveryCache(
    async (directory) => [`${directory}:${++calls}`],
    5_000,
    () => now,
  );

  const first = discover("/workspace", 0);
  assert.equal(discover("/workspace", 0), first);
  assert.deepEqual(await first, ["/workspace:1"]);

  now = 5_999;
  assert.equal(discover("/workspace", 0), first);
  now = 6_000;
  assert.deepEqual(await discover("/workspace", 0), ["/workspace:2"]);

  now = 6_001;
  assert.deepEqual(await discover("/workspace", 1), ["/workspace:3"]);
  assert.equal(calls, 3);
});

test("collects Claude file paths and ignores relative paths", () => {
  assert.deepEqual(
    collectEditedPaths([
      toolCall("Write", {
        file_path: "/repo/src/one.ts",
      }),
      toolCall("NotebookEdit", {
        notebook_path: "/repo/notebooks/work.ipynb",
        edits: [
          { file_path: "/other/src/two.ts" },
          { file_path: "relative.ts" },
        ],
      }),
    ]),
    [
      "/repo/src/one.ts",
      "/repo/notebooks/work.ipynb",
      "/other/src/two.ts",
    ],
  );
});

test("collects Codex applyPatch change keys once", () => {
  assert.deepEqual(
    collectEditedPaths([
      toolCall("applyPatch", {
        type: "fileChange",
        changes: {
          "/repo/src/one.ts": { kind: "update" },
          "/repo/src/two.ts": { kind: "add" },
          "relative.ts": { kind: "delete" },
        },
      }),
      toolCall("Write", { file_path: "/repo/src/one.ts" }),
    ]),
    ["/repo/src/one.ts", "/repo/src/two.ts"],
  );
});

test("collects Codex applyPatch array paths", () => {
  assert.deepEqual(
    collectEditedPaths([
      toolCall("applyPatch", {
        type: "fileChange",
        changes: [
          { path: "/repo/src/one.ts", kind: "update" },
          { path: "/repo/src/two.ts", kind: "add" },
          { path: "relative.ts", kind: "delete" },
          null,
        ],
      }),
    ]),
    ["/repo/src/one.ts", "/repo/src/two.ts"],
  );
});

test("collects absolute working directories once", () => {
  assert.deepEqual(
    collectWorkedDirectories([
      toolCall("commandExecution", { cwd: "/workspace/one" }),
      toolCall("commandExecution", { cwd: "/workspace/one" }),
      toolCall("commandExecution", { cwd: "relative/two" }),
      toolCall("commandExecution", { cwd: "C:\\workspace\\three" }),
    ]),
    ["/workspace/one", "C:\\workspace\\three"],
  );
});

test("ignores malformed summaries and non-tool transcript entries", () => {
  const malformed = {
    ...toolCall("Write", {}),
    summaryJson: "{",
  };
  const text: AgentSessionTranscriptItem = {
    id: "text",
    turn: 1,
    kind: "assistantText",
    text: "/repo/not-an-edit.ts",
  };
  assert.deepEqual(collectEditedPaths([malformed, text]), []);
  assert.deepEqual(collectWorkedDirectories([malformed, text]), []);
});

function toolCall(
  name: string,
  summary: Record<string, unknown>,
): AgentSessionTranscriptItem {
  return {
    id: `tool-${name}`,
    turn: 1,
    kind: "toolCall",
    name,
    summaryJson: JSON.stringify(summary),
    completed: true,
  };
}
