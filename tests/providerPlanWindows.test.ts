import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  buildProviderPlanWindowGroups,
  formatCodexPlanLabel,
  planProviderConnections,
  selectPlanProviders,
  type PlanProviderConnections,
} from "../src/providerPlanWindows.ts";
import type { UsageWindow } from "../src/types.ts";

const allConnected: PlanProviderConnections = {
  claude: true,
  codex: true,
  kimi: true,
  glm: true,
  grok: true,
};

test("selects all connected providers by priority", () => {
  assert.deepEqual(selectPlanProviders(allConnected), [
    "claude",
    "codex",
    "kimi",
    "glm",
    "grok",
  ]);
});

test("falls back to Kimi and GLM when priority providers are absent", () => {
  assert.deepEqual(
    selectPlanProviders(
      { claude: false, codex: false, kimi: true, glm: true, grok: false },
    ),
    ["kimi", "glm"],
  );
  assert.deepEqual(
    selectPlanProviders(
      { claude: false, codex: true, kimi: true, glm: true, grok: false },
    ),
    ["codex", "kimi", "glm"],
  );
});

test("merges subscription and credential-profile connections", () => {
  const base = {
    anthropic: false,
    claude: false,
    codex: false,
    grok: true,
    kimi: false,
    moonshot: true,
    openai: true,
    zai: true,
  };

  assert.deepEqual(planProviderConnections(base, null, null), {
    claude: false,
    codex: true,
    kimi: true,
    glm: true,
    grok: true,
  });
  assert.equal(
    planProviderConnections(
      { ...base, openai: false },
      {
        chatgpt: {
          source: "app",
          accountId: "account",
          accessTokenValid: true,
        },
      },
      null,
    ).codex,
    true,
  );
  assert.equal(
    planProviderConnections(
      { ...base, openai: false },
      {
        chatgpt: {
          source: "app",
          accountId: "account",
          accessTokenValid: false,
        },
      },
      null,
    ).codex,
    false,
  );
});

test("builds Claude and Codex windows with percent left", () => {
  const groups = buildProviderPlanWindowGroups({
    connections: allConnected,
    claudeWindows: [
      { label: "5h", usedPercent: 11, resetsAt: "2026-08-17T14:20:00Z" },
      { label: "7d", usedPercent: 17, resetsAt: "2026-08-21T23:00:00Z" },
    ],
    codexWindows: [
      { label: "7d", usedPercent: 42, resetsAt: 1_788_000_000 },
    ],
    normalizedWindows: [],
    nowMs: Date.parse("2026-08-17T12:00:00Z"),
  });

  assert.deepEqual(
    groups.map((group) => [
      group.id,
      group.windows.map((window) => window.remainingPercent),
    ]),
    [
      ["claude", [89, 83]],
      ["codex", [58]],
      ["kimi", [null]],
      ["glm", [null]],
      ["grok", [null]],
    ],
  );
});

test("shows unknown Claude windows without a readable local snapshot", () => {
  const [group] = buildProviderPlanWindowGroups({
    connections: {
      claude: true,
      codex: false,
      kimi: false,
      glm: false,
      grok: false,
    },
    claudeWindows: [],
    codexWindows: [],
    normalizedWindows: [],
    nowMs: Date.parse("2026-08-17T12:00:00Z"),
  });

  assert.deepEqual(group.windows, [
    { label: "5h", remainingPercent: null, resetsAt: null },
    { label: "7d", remainingPercent: null, resetsAt: null },
  ]);
});

test("carries Claude's reason when both usage windows are unknown", () => {
  const [group] = buildProviderPlanWindowGroups({
    connections: {
      claude: true,
      codex: false,
      kimi: false,
      glm: false,
      grok: false,
    },
    claudeUnavailableReason: "Sign in to Claude Code to read plan usage.",
    claudeWindows: [
      {
        label: "5h",
        usedPercent: null,
        resetsAt: null,
      },
      {
        label: "7d",
        usedPercent: null,
        resetsAt: null,
      },
    ],
    codexWindows: [],
    normalizedWindows: [],
  });

  assert.equal(
    group.unavailableReason,
    "Sign in to Claude Code to read plan usage.",
  );

  const panel = readFileSync(
    new URL("../src/components/SavingsPanel.tsx", import.meta.url),
    "utf8",
  );
  const hook = readFileSync(
    new URL("../src/usePlanWindowGroups.ts", import.meta.url),
    "utf8",
  );
  assert.match(panel, /\{group\.unavailableReason\}/);
  assert.match(hook, /CLAUDE_USAGE_REFRESH_INTERVAL_MS = 60_000/);
  assert.match(hook, /window\.clearInterval\(interval\)/);
});

test("hides Claude's unavailable reason when either window is known", () => {
  const [group] = buildProviderPlanWindowGroups({
    connections: {
      claude: true,
      codex: false,
      kimi: false,
      glm: false,
      grok: false,
    },
    claudeUnavailableReason: "stale reason",
    claudeWindows: [
      {
        label: "5h",
        usedPercent: 25,
        resetsAt: new Date(Date.now() + 3_600_000).toISOString(),
      },
      {
        label: "7d",
        usedPercent: null,
        resetsAt: null,
      },
    ],
    codexWindows: [],
    normalizedWindows: [],
  });

  assert.equal(group.unavailableReason, null);
});

test("uses normalized provider windows for fallback providers", () => {
  const window: UsageWindow = {
    id: 1,
    provider: "moonshot",
    credentialProfileId: "kimi",
    model: "kimi-k2",
    windowKind: "requests",
    limitValue: 100,
    remaining: 75,
    used: 25,
    resetsAt: "2026-08-17T13:00:00Z",
    retryAfterSeconds: null,
    source: "headers",
    confidence: "known",
    capturedAt: "2026-08-17T12:00:00Z",
  };
  const [group] = buildProviderPlanWindowGroups({
    connections: {
      claude: false,
      codex: false,
      kimi: true,
      glm: false,
      grok: false,
    },
    claudeWindows: [],
    codexWindows: [],
    normalizedWindows: [window],
    nowMs: Date.parse("2026-08-17T12:00:00Z"),
  });

  assert.equal(group.id, "kimi");
  assert.equal(group.windows[0].remainingPercent, 75);
});

test("keeps profiles and removes legacy settings inputs", () => {
  const source = readFileSync(
    new URL("../src/components/SettingsPanel.tsx", import.meta.url),
    "utf8",
  );

  assert.match(source, /Credential profiles/);
  assert.doesNotMatch(source, /Sidebar plan windows/);
  assert.doesNotMatch(source, /All connected providers/);
  assert.doesNotMatch(source, /aria-label="OpenAI API key"/);
  assert.doesNotMatch(source, /aria-label="Anthropic API key"/);
  assert.doesNotMatch(source, /Set OpenAI key|Clear OpenAI key/);
  assert.doesNotMatch(source, /Set Anthropic key|Clear Anthropic key/);
});

test("uses xAI usage windows for the Grok plan provider", () => {
  const window: UsageWindow = {
    id: 2,
    provider: "xai",
    credentialProfileId: "grok",
    model: "grok-4.6",
    windowKind: "tokens",
    limitValue: 1_000,
    remaining: 640,
    used: 360,
    resetsAt: "2026-08-17T13:00:00Z",
    retryAfterSeconds: null,
    source: "headers",
    confidence: "known",
    capturedAt: "2026-08-17T12:00:00Z",
  };
  const [group] = buildProviderPlanWindowGroups({
    connections: {
      claude: false,
      codex: false,
      kimi: false,
      glm: false,
      grok: true,
    },
    claudeWindows: [],
    codexWindows: [],
    normalizedWindows: [window],
    nowMs: Date.parse("2026-08-17T12:00:00Z"),
  });

  assert.equal(group.id, "grok");
  assert.equal(group.label, "Grok");
  assert.equal(group.windows[0].remainingPercent, 64);
});

test("20-dollar Codex plan renders 5h before 7d", () => {
  const [group] = buildProviderPlanWindowGroups({
    connections: {
      claude: false,
      codex: true,
      kimi: false,
      glm: false,
      grok: false,
    },
    claudeWindows: [],
    codexWindows: [
      {
        label: "7d",
        usedPercent: 86,
        resetsAt: new Date(Date.now() + 3_600_000).toISOString(),
        windowSeconds: 604_800,
      },
      {
        label: "5h",
        usedPercent: 12,
        resetsAt: Math.floor(Date.now() / 1000) + 1_800,
        windowSeconds: 18_000,
      },
    ],
    codexPlanType: "plus",
    normalizedWindows: [],
  });

  assert.deepEqual(
    group.windows.map((window) => [window.label, window.remainingPercent]),
    [
      ["5h", 88],
      ["7d", 14],
    ],
  );
  assert.equal(group.label, "Codex · Plus");
});

test("Pro Codex plan renders only its reported 7d window", () => {
  const [group] = buildProviderPlanWindowGroups({
    connections: {
      claude: false,
      codex: true,
      kimi: false,
      glm: false,
      grok: false,
    },
    claudeWindows: [],
    codexWindows: [
      {
        label: "7d",
        usedPercent: 21,
        resetsAt: null,
        windowSeconds: 604_800,
      },
    ],
    codexPlanType: "pro",
    normalizedWindows: [],
  });

  assert.equal(group.label, "Codex · Pro");
  assert.deepEqual(group.windows.map((window) => window.label), ["7d"]);
});

test("unknown Codex window length sorts last and keeps an empty label", () => {
  const [group] = buildProviderPlanWindowGroups({
    connections: {
      claude: false,
      codex: true,
      kimi: false,
      glm: false,
      grok: false,
    },
    claudeWindows: [],
    codexWindows: [
      {
        label: "",
        usedPercent: 30,
        resetsAt: null,
        windowSeconds: null,
      },
      {
        label: "5h",
        usedPercent: 10,
        resetsAt: null,
        windowSeconds: 18_000,
      },
    ],
    normalizedWindows: [],
  });

  assert.deepEqual(group.windows.map((window) => window.label), ["5h", ""]);
});

test("empty Codex usage renders one placeholder with an empty label", () => {
  const [group] = buildProviderPlanWindowGroups({
    connections: {
      claude: false,
      codex: true,
      kimi: false,
      glm: false,
      grok: false,
    },
    claudeWindows: [],
    codexWindows: [],
    normalizedWindows: [],
  });

  assert.deepEqual(
    group.windows,
    [{ label: "", remainingPercent: null, resetsAt: null }],
  );
});

test("formats Codex plan labels", () => {
  assert.equal(formatCodexPlanLabel("pro"), "Codex · Pro");
  assert.equal(formatCodexPlanLabel("PLUS"), "Codex · Plus");
  assert.equal(formatCodexPlanLabel(null), "Codex");
  assert.equal(formatCodexPlanLabel("student"), "Codex · Student");
});
