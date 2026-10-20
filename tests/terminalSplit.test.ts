import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  openTerminalSplitChoice,
  refreshLanesWhenSplitMenuOpens,
  requireTerminalSplitAgentLane,
  TERMINAL_SPLIT_TERMINAL_CHOICE_ID,
  terminalSplitAgentChoice,
  terminalSplitMenuChoices,
} from "../src/terminalSplit.ts";
import type { AgentSessionLaneOptions } from "../src/agentSessions.ts";

const profile = {
  id: "profile-1",
  provider: "anthropic",
  name: "Work key",
  kind: "api_key",
  priority: 0,
  status: "active" as const,
  lastValidatedAt: null,
  metadataJson: null,
  createdAt: "2026-08-17T00:00:00Z",
  updatedAt: "2026-08-17T00:00:00Z",
};

test("renders Terminal after Duplicate and before agent choices", () => {
  const choices = terminalSplitMenuChoices(
    true,
    "Terminal",
    "opens a clean shell",
    [
      {
        id: "claude",
        label: "Claude Code",
        ready: true,
        readiness: "Signed in",
      },
    ],
    null,
  );
  assert.deepEqual(
    ["duplicate", ...choices.map((choice) => choice.id)],
    ["duplicate", TERMINAL_SPLIT_TERMINAL_CHOICE_ID, "claude"],
  );

  const source = readFileSync(
    new URL("../src/components/TerminalPane.tsx", import.meta.url),
    "utf8",
  );
  const duplicate = source.indexOf('<strong>{t("Duplicate")}</strong>');
  const choicesRender = source.indexOf("{choices.map((choice) =>", duplicate);
  assert.notEqual(duplicate, -1);
  assert.notEqual(choicesRender, -1);
  assert.ok(duplicate < choicesRender);
});

test("the Terminal choice is ready in every pane menu", () => {
  const [choice] = terminalSplitMenuChoices(
    false,
    "Terminal",
    "opens a clean shell",
    [],
    null,
  );
  assert.deepEqual(choice, {
    id: TERMINAL_SPLIT_TERMINAL_CHOICE_ID,
    label: "Terminal",
    ready: false,
    readiness: "opens a clean shell",
  });

  const source = readFileSync(
    new URL("../src/components/TerminalPane.tsx", import.meta.url),
    "utf8",
  );
  assert.match(source, /disabled=\{!choice\.ready\}/);

  // The source pane names the folder, so the agent surface (no project)
  // gets a working choice too.
  const panel = readFileSync(
    new URL("../src/components/TerminalPanel.tsx", import.meta.url),
    "utf8",
  );
  assert.match(
    panel,
    /terminalSplitMenuChoices\(\s*true,\s*t\("Terminal"\),\s*t\("opens a clean shell"\)/,
  );
});

test("opens a clean terminal with the selected split direction", async () => {
  const session = { id: "plain-terminal" };
  let openCalls = 0;
  const result = await openTerminalSplitChoice(
    TERMINAL_SPLIT_TERMINAL_CHOICE_ID,
    "horizontal",
    async () => {
      openCalls += 1;
      return session;
    },
  );
  assert.equal(openCalls, 1);
  assert.deepEqual(result, { direction: "horizontal", session });

  const source = readFileSync(
    new URL("../src/components/TerminalPanel.tsx", import.meta.url),
    "utf8",
  );
  assert.match(
    source,
    /async function openTerminal\(\)[\s\S]*?openCleanTerminal\(\)/,
  );
  assert.match(
    source,
    /async function openCleanTerminal\(\)[\s\S]*?ensureSessionCapacity\(\)[\s\S]*?openPlainTerminalSession\(/,
  );
  // A split shell starts next to its source pane, not at the project root.
  assert.match(
    source,
    /openTerminalSplitChoice\([\s\S]*?direction,[\s\S]*?\(\) => openShellBeside\(source\)/,
  );
  assert.match(
    source,
    /async function openShellBeside\([\s\S]*?ensureSessionCapacity\(\)[\s\S]*?ipc\.terminal\.openShell\(\{[\s\S]*?sessionId: source\.id,[\s\S]*?scope: storageKeyRef\.current,/,
  );
});

test("uses a signed-in subscription before a stored split profile", () => {
  const options: AgentSessionLaneOptions = [
    {
      agentKind: "claude",
      lanes: [
        {
          kind: "claudeSubscription",
          loggedIn: true,
          credentialProfiles: [],
        },
        {
          kind: "claudeApiKey",
          loggedIn: null,
          credentialProfiles: [profile],
        },
      ],
    },
  ];
  assert.deepEqual(requireTerminalSplitAgentLane(options, "claude"), {
    kind: "claudeSubscription",
  });
  assert.equal(terminalSplitAgentChoice(options, "claude").readiness, "Signed in");
});

test("uses an active stored profile when the split subscription is signed out", () => {
  const options: AgentSessionLaneOptions = [
    {
      agentKind: "claude",
      lanes: [
        {
          kind: "claudeSubscription",
          loggedIn: false,
          credentialProfiles: [],
        },
        {
          kind: "claudeApiKey",
          loggedIn: null,
          credentialProfiles: [profile],
        },
      ],
    },
  ];
  assert.deepEqual(requireTerminalSplitAgentLane(options, "claude"), {
    kind: "claudeApiKey",
    credentialProfileId: "profile-1",
  });
});

test("rejects an unavailable split lane instead of falling back silently", () => {
  const options: AgentSessionLaneOptions = [
    {
      agentKind: "kimi",
      lanes: [
        {
          kind: "kimiSubscription",
          loggedIn: false,
          credentialProfiles: [],
        },
      ],
    },
  ];
  assert.throws(
    () => requireTerminalSplitAgentLane(options, "kimi"),
    /signed-in subscription or saved credential profile/,
  );
});

test("resolves Codex login plus GLM and Grok stored-profile defaults", () => {
  const options: AgentSessionLaneOptions = [
    {
      agentKind: "codex",
      lanes: [
        { kind: "codexChatGpt", loggedIn: true, credentialProfiles: [] },
      ],
    },
    {
      agentKind: "zai",
      lanes: [
        {
          kind: "zaiGlm",
          loggedIn: null,
          credentialProfiles: [{ ...profile, id: "zai-1", provider: "zai" }],
        },
      ],
    },
    {
      agentKind: "kimi",
      lanes: [
        { kind: "kimiSubscription", loggedIn: true, credentialProfiles: [] },
      ],
    },
    {
      agentKind: "grok",
      lanes: [
        {
          kind: "grokApiKey",
          loggedIn: null,
          credentialProfiles: [{ ...profile, id: "xai-1", provider: "xai" }],
        },
      ],
    },
  ];

  assert.deepEqual(requireTerminalSplitAgentLane(options, "codex"), {
    kind: "codexChatGpt",
    proxied: true,
  });
  assert.deepEqual(requireTerminalSplitAgentLane(options, "zai"), {
    kind: "zaiGlm",
    credentialProfileId: "zai-1",
  });
  assert.deepEqual(requireTerminalSplitAgentLane(options, "kimi"), {
    kind: "kimiSubscription",
  });
  assert.deepEqual(requireTerminalSplitAgentLane(options, "grok"), {
    kind: "grokApiKey",
    credentialProfileId: "xai-1",
  });
});

test("refreshes lanes without blocking when the split menu opens", async () => {
  let refreshes = 0;
  let release = () => {};
  const pending = new Promise<void>((resolve) => {
    release = resolve;
  });
  const result = refreshLanesWhenSplitMenuOpens(true, () => {
    refreshes += 1;
    return pending;
  });
  assert.equal(result, undefined);
  assert.equal(refreshes, 1);
  release();

  refreshLanesWhenSplitMenuOpens(false, async () => {
    refreshes += 1;
  });
  assert.equal(refreshes, 1);

  // A failed fetch must not surface as an unhandled rejection.
  refreshLanesWhenSplitMenuOpens(true, async () => {
    throw new Error("ipc down");
  });
  await new Promise((resolve) => setImmediate(resolve));

  const source = readFileSync(
    new URL("../src/components/TerminalPane.tsx", import.meta.url),
    "utf8",
  );
  assert.match(
    source,
    /className="terminal-split-menu"[\s\S]*?refreshLanesWhenSplitMenuOpens\(\s*event\.currentTarget\.open/,
  );
});
