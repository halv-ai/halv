import assert from "node:assert/strict";
import test from "node:test";

import {
  formatMemoryBytes,
  resourceUsageRows,
  runningResourceSessions,
  type ResourceUsageSession,
} from "../src/resourceUsage.ts";

const MB = 1024 * 1024;

function session(
  id: string,
  overrides: Partial<ResourceUsageSession> = {},
): ResourceUsageSession {
  return {
    id,
    title: null,
    projectName: "halv",
    agentKind: null,
    status: "running",
    ...overrides,
  };
}

test("formats memory with a short unit", () => {
  assert.equal(formatMemoryBytes(0), "<1 MB");
  assert.equal(formatMemoryBytes(-5), "<1 MB");
  assert.equal(formatMemoryBytes(Number.NaN), "<1 MB");
  assert.equal(formatMemoryBytes(0.4 * MB), "<1 MB");
  assert.equal(formatMemoryBytes(412.4 * MB), "412 MB");
  assert.equal(formatMemoryBytes(1023.6 * MB), "1.0 GB");
  assert.equal(formatMemoryBytes(1.44 * 1024 * MB), "1.4 GB");
  assert.equal(formatMemoryBytes(12.6 * 1024 * MB), "13 GB");
});

test("counts only the terminals with a live process", () => {
  const sessions = [
    session("a"),
    session("b", { status: "exited" }),
    session("c", { status: "failed" }),
  ];
  assert.deepEqual(
    runningResourceSessions(sessions).map((item) => item.id),
    ["a"],
  );
});

test("lists the app first and the terminals by memory", () => {
  const rows = resourceUsageRows(
    {
      appBytes: 300 * MB,
      totalBytes: 600 * MB,
      terminals: [
        { sessionId: "small", bytes: 100 * MB },
        { sessionId: "big", bytes: 200 * MB },
      ],
    },
    [
      session("small", { title: "zsh" }),
      session("big", { agentKind: "claude" }),
      session("unknown", { title: "", projectName: "web" }),
    ],
    { app: "Halv app" },
  );
  assert.deepEqual(rows, [
    { id: "app", kind: "app", label: "Halv app", bytes: 300 * MB },
    { id: "big", kind: "terminal", label: "claude · halv", bytes: 200 * MB },
    { id: "small", kind: "terminal", label: "zsh", bytes: 100 * MB },
    { id: "unknown", kind: "terminal", label: "web", bytes: null },
  ]);
});

test("rows without a snapshot keep the terminals and no memory", () => {
  const rows = resourceUsageRows(null, [session("a", { title: "zsh" })], {
    app: "Halv app",
  });
  assert.deepEqual(rows, [
    { id: "app", kind: "app", label: "Halv app", bytes: null },
    { id: "a", kind: "terminal", label: "zsh", bytes: null },
  ]);
});
