import type {
  AgentSessionLaneOptions,
  AgentSessionProbe,
} from "./agentSessions";
import type { ConnectGateConnections } from "./connectGate";
import type { AgentBackend, AuthStatus, ChatModel } from "./types";

export const CLAUDE_CODE_MODELS: ChatModel[] = [
  sessionModel(
    "fable",
    "Fable 5.1",
    "claude_session",
    "Claude",
    "via Claude Code",
    {
      reasoningEfforts: ["low", "medium", "high", "xhigh", "max"],
      defaultEffort: "high",
    },
  ),
  sessionModel(
    "opus",
    "Opus 5.5",
    "claude_session",
    "Claude",
    "via Claude Code",
    {
      reasoningEfforts: ["low", "medium", "high", "xhigh", "max"],
      defaultEffort: "high",
    },
  ),
  sessionModel(
    "sonnet",
    "Sonnet 5",
    "claude_session",
    "Claude",
    "via Claude Code",
    {
      reasoningEfforts: ["low", "medium", "high", "xhigh", "max"],
      defaultEffort: "high",
    },
  ),
  sessionModel("haiku", "Haiku 4.5", "claude_session", "Claude", "via Claude Code"),
];

export const KIMI_CODE_MODELS: ChatModel[] = [
  {
    ...sessionModel(
      "kimi-cli/kimi-for-coding",
      "Kimi for Coding",
      "kimi_session",
      "Kimi",
      "via Kimi Code",
    ),
    agentSessionModel: "kimi-for-coding",
    provider: "moonshot",
  },
];

export const ZAI_SESSION_MODELS: ChatModel[] = [
  {
    ...sessionModel(
      "zai-cli/glm-5.2",
      "GLM 5.2",
      "zai_session",
      "GLM",
      "via GLM Coding Plan",
    ),
    agentSessionModel: "sonnet",
    provider: "zai",
  },
  {
    ...sessionModel(
      "zai-cli/glm-4.7",
      "GLM 4.7",
      "zai_session",
      "GLM",
      "via GLM Coding Plan",
    ),
    agentSessionModel: "haiku",
    provider: "zai",
  },
];

export const GROK_SESSION_MODELS: ChatModel[] = [
  {
    ...sessionModel(
      "grok-cli/grok-4.6",
      "Grok 4.6",
      "grok_session",
      "Grok",
      "via xAI API",
    ),
    agentSessionModel: "sonnet",
    provider: "xai",
  },
  {
    ...sessionModel(
      "grok-cli/grok-build-0.1",
      "Grok Build 0.1",
      "grok_session",
      "Grok",
      "via xAI API",
    ),
    agentSessionModel: "haiku",
    provider: "xai",
  },
];

export function claudeCodeLoggedIn(options: AgentSessionLaneOptions) {
  return laneLoggedIn(options, "claude", "claudeSubscription");
}

export function codexCliLoggedIn(options: AgentSessionLaneOptions) {
  return laneLoggedIn(options, "codex", "codexChatGpt");
}

export function kimiCliPresent(probe: AgentSessionProbe | null | undefined) {
  return nestedProbeFound(probe, "kimi");
}

export function zaiLaneAvailable(
  options: AgentSessionLaneOptions,
  probe: AgentSessionProbe | null | undefined,
) {
  const lane = options
    .find((option) => option.agentKind === "zai")
    ?.lanes.find((candidate) => candidate.kind === "zaiGlm");
  return (
    nestedProbeFound(probe, "claude") &&
    (lane?.credentialProfiles.some((profile) => profile.status === "active") ??
      false)
  );
}

export function grokLaneAvailable(
  options: AgentSessionLaneOptions,
  probe: AgentSessionProbe | null | undefined,
) {
  const lane = options
    .find((option) => option.agentKind === "grok")
    ?.lanes.find((candidate) => candidate.kind === "grokApiKey");
  return (
    nestedProbeFound(probe, "claude") &&
    (lane?.credentialProfiles.some((profile) => profile.status === "active") ??
      false)
  );
}

export function composerModelsWithAgentSessions(
  nativeModels: ChatModel[],
  options: AgentSessionLaneOptions,
  probe: AgentSessionProbe | null | undefined,
) {
  const codexCatalog = nativeModels.filter((model) => model.provider === "codex_cli");
  const claudeCatalog = nativeModels.filter((model) => model.provider === "claude_cli");
  const models = nativeModels.filter((model) => !model.agentBackend && model.provider !== "codex_cli" && model.provider !== "claude_cli");
  const additions: ChatModel[] = [];
  if (codexCliLoggedIn(options)) {
    additions.push(...codexSessionModels(codexCatalog.length ? codexCatalog.map((model) => ({ ...model, provider: "openai" })) : models));
  }
  if (claudeCodeLoggedIn(options)) {
    additions.push(...(claudeCatalog.length ? claudeCatalog.map<ChatModel>((model) => ({
      ...model, id: `claude-cli/${model.id}`, agentBackend: "claude_session", agentSessionModel: model.id,
      modelGroup: "Claude", modelBadge: "via Claude Code", supportsFastMode: false,
    })) : CLAUDE_CODE_MODELS));
  }
  if (kimiCliPresent(probe)) {
    additions.push(...KIMI_CODE_MODELS);
  }
  if (zaiLaneAvailable(options, probe)) {
    additions.push(...ZAI_SESSION_MODELS);
  }
  if (grokLaneAvailable(options, probe)) {
    additions.push(...GROK_SESSION_MODELS);
  }
  return [...models, ...additions];
}

export function composerModelsWithClaudeCode(
  nativeModels: ChatModel[],
  options: AgentSessionLaneOptions,
) {
  return composerModelsWithAgentSessions(nativeModels, options, null);
}

export function firstRunComposerModel(
  models: ChatModel[],
  defaultModel: string | null | undefined,
  connections: ConnectGateConnections,
  auth: AuthStatus | null,
) {
  const visibleModels = visibleComposerModels(models, {
    proMode: false,
    agentBackend: null,
    directApiModelsAvailable:
      connections.openai || connections.anthropic ||
      Boolean(auth && ("chatgpt" in auth || "apiKey" in auth || "anthropicApiKey" in auth)),
  });
  const fallbackModel = visibleModels[0];
  if (defaultModel != null) {
    return (
      visibleModels.find((model) => model.id === defaultModel) ?? fallbackModel
    );
  }
  if (connections.codex) {
    return visibleModels.find((model) => model.agentBackend === "codex_session") ?? fallbackModel;
  }
  if (auth && ("chatgpt" in auth || "apiKey" in auth)) {
    return visibleModels.find((model) => model.provider === "openai" && !model.agentBackend) ?? fallbackModel;
  }
  if (auth && "anthropicApiKey" in auth) {
    return visibleModels.find((model) => model.provider === "anthropic" && !model.agentBackend) ?? fallbackModel;
  }
  if (!onlyClaudeConnected(connections, auth)) {
    return fallbackModel;
  }
  return (
    visibleModels.find((model) => model.agentBackend === "claude_session") ??
    fallbackModel
  );
}

export function requestModelForChatModel(model: ChatModel | undefined, fallback: string) {
  return model?.agentSessionModel ?? model?.id ?? fallback;
}

export function agentBackendForModel(
  model: ChatModel | undefined,
  proMode: boolean,
): AgentBackend | null {
  if (model?.agentBackend) {
    return model.agentBackend;
  }
  if (proMode && model?.provider === "openai" && model.id.startsWith("gpt-")) {
    return "codex_session";
  }
  return null;
}

export function visibleComposerModels(
  models: ChatModel[],
  options: {
    proMode: boolean;
    agentBackend?: AgentBackend | null;
    directApiModelsAvailable: boolean;
  },
) {
  const availableModels = options.directApiModelsAvailable
    ? models
    : models.filter((model) => model.agentBackend != null);
  if (!options.proMode && !isSessionBackend(options.agentBackend)) {
    return availableModels;
  }
  return availableModels.filter(
    (model) =>
      model.agentBackend != null ||
      agentBackendForModel(model, true) !== "codex_session",
  );
}

export function resolveComposerModel(
  visibleModels: ChatModel[],
  models: ChatModel[],
  selectedModel: string,
) {
  return (
    visibleModels.find((model) => model.id === selectedModel) ??
    visibleModels.find(
      (model) =>
        model.agentBackend === "codex_session" &&
        model.agentSessionModel === selectedModel,
    ) ??
    visibleModels[0] ??
    models.find((model) => model.id === selectedModel) ??
    models[0]
  );
}

export function isSessionBackend(
  backend: AgentBackend | null | undefined,
): backend is AgentBackend {
  return backend != null;
}

function codexSessionModels(models: ChatModel[]) {
  return models
    .filter((model) => model.provider === "openai" && model.id.startsWith("gpt-"))
    .map<ChatModel>((model) => ({
      ...model,
      id: `codex-cli/${model.id}`,
      agentBackend: "codex_session",
      agentSessionModel: model.id,
      modelGroup: "GPT",
      modelBadge: "via Codex CLI",
      supportsFastMode: false,
    }));
}

function sessionModel(
  id: string,
  label: string,
  agentBackend: AgentBackend,
  modelGroup: string,
  modelBadge: string,
  effort?: Pick<ChatModel, "reasoningEfforts" | "defaultEffort">,
): ChatModel {
  return {
    id,
    label,
    reasoningEfforts: effort?.reasoningEfforts ?? ["medium"],
    defaultEffort: effort?.defaultEffort ?? "medium",
    supportsFastMode: false,
    agentBackend,
    modelGroup,
    modelBadge,
  };
}

function laneLoggedIn(
  options: AgentSessionLaneOptions,
  agentKind: "claude" | "codex",
  laneKind: "claudeSubscription" | "codexChatGpt",
) {
  return (
    options
      .find((option) => option.agentKind === agentKind)
      ?.lanes.some((lane) => lane.kind === laneKind && lane.loggedIn === true) ?? false
  );
}

function nestedProbeFound(
  probe: AgentSessionProbe | null | undefined,
  key: "claude" | "kimi",
) {
  if (!probe || !(key in probe)) {
    return false;
  }
  const probes = probe as Record<string, { found?: boolean } | undefined>;
  return probes[key]?.found === true;
}

function onlyClaudeConnected(
  connections: ConnectGateConnections,
  auth: AuthStatus | null,
) {
  const chatGptConnected =
    connections.codex ||
    Boolean(auth && ("chatgpt" in auth || "apiKey" in auth));
  return (
    connections.claude &&
    !chatGptConnected &&
    !connections.openai &&
    !connections.kimi &&
    !connections.moonshot &&
    !connections.zai &&
    !connections.grok
  );
}
