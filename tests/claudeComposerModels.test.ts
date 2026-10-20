import assert from "node:assert/strict";
import test from "node:test";

import {
  CLAUDE_CODE_MODELS,
  GROK_SESSION_MODELS,
  KIMI_CODE_MODELS,
  ZAI_SESSION_MODELS,
  agentBackendForModel,
  claudeCodeLoggedIn,
  composerModelsWithAgentSessions,
  composerModelsWithClaudeCode,
  firstRunComposerModel,
  requestModelForChatModel,
  resolveComposerModel,
  visibleComposerModels,
} from "../src/claudeComposerModels.ts";

test("new account models reach CLI selectors without a hardcoded model update", () => {
  const models = composerModelsWithAgentSessions([
    { id: "gpt-future", label: "Future Codex", provider: "codex_cli", reasoningEfforts: ["high", "ultra"], defaultEffort: "high" },
    { id: "claude-future[1m]", label: "Future Claude 9.1", provider: "claude_cli", reasoningEfforts: ["medium", "max"], defaultEffort: "medium" },
  ], [{ agentKind: "claude", lanes: [{ kind: "claudeSubscription", loggedIn: true, credentialProfiles: [] }] }, { agentKind: "codex", lanes: [{ kind: "codexChatGpt", loggedIn: true, credentialProfiles: [] }] }], false);
  const codex = models.find((model) => model.agentSessionModel === "gpt-future");
  const claude = models.find((model) => model.agentSessionModel === "claude-future[1m]");
  assert.equal(codex?.agentBackend, "codex_session");
  assert.deepEqual(codex?.reasoningEfforts, ["high", "ultra"]);
  assert.equal(claude?.agentBackend, "claude_session");
  assert.equal(claude?.label, "Future Claude 9.1");
  assert.equal(models.some((model) => model.id === "gpt-future"), false);
  assert.equal(models.some((model) => model.id === "claude-future[1m]"), false);
  assert.equal(models.some((model) => model.agentSessionModel === "claude-opus-5"), false);
});
import type { AgentSessionLaneOptions } from "../src/agentSessions.ts";
import type { ChatModel } from "../src/types.ts";

const nativeModel: ChatModel = {
  id: "gpt-5.5",
  label: "gpt-5.5",
  reasoningEfforts: ["medium"],
  defaultEffort: "medium",
  provider: "openai",
  supportsFastMode: false,
};

const codexSessionModel: ChatModel = {
  ...nativeModel,
  id: "codex-cli/gpt-5.5",
  agentBackend: "codex_session",
  agentSessionModel: nativeModel.id,
};

function lanes(loggedIn: boolean | null): AgentSessionLaneOptions {
  return [
    {
      agentKind: "claude",
      lanes: [
        {
          kind: "claudeSubscription",
          loggedIn,
          credentialProfiles: [],
        },
      ],
    },
  ];
}

test("gates Claude Code models on the Claude subscription login probe", () => {
  assert.equal(claudeCodeLoggedIn(lanes(true)), true);
  assert.equal(claudeCodeLoggedIn(lanes(false)), false);
  assert.equal(claudeCodeLoggedIn(lanes(null)), false);
  assert.deepEqual(composerModelsWithClaudeCode([nativeModel], lanes(false)), [
    nativeModel,
  ]);
  assert.deepEqual(
    composerModelsWithClaudeCode([nativeModel], lanes(true)).map(
      (model) => model.id,
    ),
    [nativeModel.id, ...CLAUDE_CODE_MODELS.map((model) => model.id)],
  );
});

test("uses stable Claude CLI aliases as wire model IDs", () => {
  assert.deepEqual(
    CLAUDE_CODE_MODELS.map((model) => [
      model.id,
      model.label,
      model.modelBadge,
    ]),
    [
      ["fable", "Fable 5.1", "via Claude Code"],
      ["opus", "Opus 5.5", "via Claude Code"],
      ["sonnet", "Sonnet 5", "via Claude Code"],
      ["haiku", "Haiku 4.5", "via Claude Code"],
    ],
  );
});

test("offers Claude Code effort only on supported models", () => {
  for (const model of CLAUDE_CODE_MODELS.slice(0, 3)) {
    assert.deepEqual(model.reasoningEfforts, [
      "low",
      "medium",
      "high",
      "xhigh",
      "max",
    ]);
    assert.equal(model.defaultEffort, "high");
  }

  const singleEffortModels = [
    CLAUDE_CODE_MODELS[3],
    ...KIMI_CODE_MODELS,
    ...ZAI_SESSION_MODELS,
    ...GROK_SESSION_MODELS,
  ];
  for (const model of singleEffortModels) {
    assert.deepEqual(model.reasoningEfforts, ["medium"]);
    assert.equal(model.defaultEffort, "medium");
  }
});

test("adds distinct CLI entries without replacing the native GPT entry", () => {
  const options: AgentSessionLaneOptions = [
    ...lanes(true),
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
          credentialProfiles: [
            {
              id: "zai-1",
              provider: "zai",
              name: "GLM",
              kind: "api_key",
              priority: 0,
              status: "active",
              lastValidatedAt: null,
              metadataJson: null,
              createdAt: "2026-08-17T00:00:00Z",
              updatedAt: "2026-08-17T00:00:00Z",
            },
          ],
        },
      ],
    },
    {
      agentKind: "grok",
      lanes: [
        {
          kind: "grokApiKey",
          loggedIn: null,
          credentialProfiles: [
            {
              id: "xai-1",
              provider: "xai",
              name: "Grok",
              kind: "api_key",
              priority: 0,
              status: "active",
              lastValidatedAt: null,
              metadataJson: null,
              createdAt: "2026-08-17T00:00:00Z",
              updatedAt: "2026-08-17T00:00:00Z",
            },
          ],
        },
      ],
    },
  ];
  const models = composerModelsWithAgentSessions([nativeModel], options, {
    claude: { found: true },
    codex: { found: true },
    kimi: { found: true },
  });

  assert.equal(models[0], nativeModel);
  assert.deepEqual(
    models.map((model) => model.id),
    [
      "gpt-5.5",
      "codex-cli/gpt-5.5",
      ...CLAUDE_CODE_MODELS.map((model) => model.id),
      ...KIMI_CODE_MODELS.map((model) => model.id),
      ...ZAI_SESSION_MODELS.map((model) => model.id),
      ...GROK_SESSION_MODELS.map((model) => model.id),
    ],
  );
  const codex = models.find((model) => model.agentBackend === "codex_session");
  assert.equal(requestModelForChatModel(codex, "missing"), "gpt-5.5");
  assert.equal(agentBackendForModel(nativeModel, false), null);
  assert.equal(agentBackendForModel(nativeModel, true), "codex_session");
});

test("defaults a Claude-only first run to a visible Claude model", () => {
  const models = [nativeModel, ...CLAUDE_CODE_MODELS];
  const connections = {
    anthropic: false,
    claude: true,
    codex: false,
    grok: false,
    kimi: false,
    moonshot: false,
    openai: false,
    zai: false,
  };

  const claudeDefault = firstRunComposerModel(
    models,
    null,
    connections,
    null,
  );

  assert.equal(claudeDefault?.id, CLAUDE_CODE_MODELS[0].id);
  assert.equal(claudeDefault?.agentBackend, "claude_session");
  assert.equal(
    visibleComposerModels(models, {
      proMode: true,
      agentBackend: null,
      directApiModelsAvailable: false,
    }).some((model) => model.id === claudeDefault?.id),
    true,
  );
});

test("hides native models without API keys and defaults to a session", () => {
  const models = [nativeModel, codexSessionModel];
  const connections = {
    anthropic: false,
    claude: false,
    codex: true,
    grok: false,
    kimi: false,
    moonshot: false,
    openai: false,
    zai: false,
  };

  assert.deepEqual(
    visibleComposerModels(models, {
      proMode: false,
      agentBackend: null,
      directApiModelsAvailable: false,
    }),
    [codexSessionModel],
  );
  assert.equal(
    firstRunComposerModel(models, null, connections, null)?.id,
    codexSessionModel.id,
  );
  assert.equal(
    firstRunComposerModel(models, nativeModel.id, connections, null)?.id,
    codexSessionModel.id,
  );
});

test("keeps a model during the Pro mode lane loading race", () => {
  const visibleBeforeLanes = visibleComposerModels([nativeModel], {
    proMode: true,
    agentBackend: null,
    directApiModelsAvailable: false,
  });

  assert.deepEqual(visibleBeforeLanes, []);
  assert.equal(
    resolveComposerModel(visibleBeforeLanes, [nativeModel], nativeModel.id),
    nativeModel,
  );
  assert.equal(
    resolveComposerModel(
      [codexSessionModel],
      [nativeModel, codexSessionModel],
      nativeModel.id,
    ),
    codexSessionModel,
  );
});

test("shows native models and respects their default with an OpenAI key", () => {
  const models = [nativeModel, codexSessionModel];
  const connections = {
    anthropic: false,
    claude: false,
    codex: true,
    grok: false,
    kimi: false,
    moonshot: false,
    openai: true,
    zai: false,
  };

  assert.equal(
    visibleComposerModels(models, {
      proMode: false,
      agentBackend: null,
      directApiModelsAvailable: true,
    }),
    models,
  );
  assert.equal(
    firstRunComposerModel(models, nativeModel.id, connections, null)?.id,
    nativeModel.id,
  );
});

test("hides direct GPT models in pro mode or a session-backed conversation", () => {
  const nativeAnthropic: ChatModel = {
    ...nativeModel,
    id: "claude-api",
    provider: "anthropic",
  };
  const models = [nativeModel, nativeAnthropic, codexSessionModel];
  const expectedIds = [nativeAnthropic.id, codexSessionModel.id];

  assert.deepEqual(
    visibleComposerModels(models, {
      proMode: true,
      agentBackend: null,
      directApiModelsAvailable: true,
    }).map((model) => model.id),
    expectedIds,
  );
  assert.deepEqual(
    visibleComposerModels(models, {
      proMode: false,
      agentBackend: "claude_session",
      directApiModelsAvailable: true,
    }).map((model) => model.id),
    expectedIds,
  );
});

test("maps the live Grok catalog to stable Claude Code model slots", () => {
  assert.deepEqual(
    GROK_SESSION_MODELS.map((model) => [
      model.id,
      model.label,
      model.agentBackend,
      model.agentSessionModel,
      model.provider,
    ]),
    [
      ["grok-cli/grok-4.6", "Grok 4.6", "grok_session", "sonnet", "xai"],
      [
        "grok-cli/grok-build-0.1",
        "Grok Build 0.1",
        "grok_session",
        "haiku",
        "xai",
      ],
    ],
  );
});

test("a browser ChatGPT sign-in starts with its native model without a CLI", () => {
  const connections = { anthropic: false, claude: false, codex: false, grok: false, kimi: false, moonshot: false, openai: false, zai: false };
  const auth = { chatgpt: { source: "app", accountId: "new-account", accessTokenValid: true } } as const;
  const model = firstRunComposerModel([...CLAUDE_CODE_MODELS, nativeModel], null, connections, auth);
  assert.equal(model?.id, nativeModel.id);
  assert.equal(model?.agentBackend, nativeModel.agentBackend);
});
