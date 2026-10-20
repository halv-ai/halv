import type {
  AgentBinaryProbe,
  AgentKind,
  AgentSessionCreateLaneKind,
  AgentSessionLaneOptions,
  AgentSessionProbe,
} from "./agentSessions";
import type { CredentialProfile } from "./types";

export type ConnectGateState =
  | "resolving"
  | "required"
  | "optional"
  | "passed";

export type ConnectGateOutcome = "none" | "some" | "all";

export const CONNECT_GATE_DISMISSED_STORAGE_KEY = "halv.connectGateDismissed";

export type ConnectGateConnections = {
  anthropic: boolean;
  claude: boolean;
  codex: boolean;
  grok: boolean;
  kimi: boolean;
  moonshot: boolean;
  openai: boolean;
  zai: boolean;
};

const agentKindsThatNeedCliToRun = new Set<AgentKind>([
  "antigravity",
  "claude",
  "gemini",
  "kimi",
  // GLM runs through the Claude Code CLI.
  "zai",
]);

export function agentNeedsCliToRun(agentKind: AgentKind): boolean {
  return agentKindsThatNeedCliToRun.has(agentKind);
}

export function anyAgentCliInstalled(
  probe: AgentSessionProbe | null,
): boolean {
  if (!probe) {
    return false;
  }
  if (typeof (probe as AgentBinaryProbe).found === "boolean") {
    return (probe as AgentBinaryProbe).found;
  }
  const agentProbes = probe as {
    antigravity?: AgentBinaryProbe;
    claude?: AgentBinaryProbe;
    codex?: AgentBinaryProbe;
    gemini?: AgentBinaryProbe;
    kimi?: AgentBinaryProbe;
  };
  return [
    agentProbes.antigravity,
    agentProbes.claude,
    agentProbes.codex,
    agentProbes.gemini,
    agentProbes.kimi,
  ].some((agentProbe) => agentProbe?.found === true);
}

export function shouldOpenApiKeySection(
  probe: AgentSessionProbe | null,
  connections: ConnectGateConnections,
): boolean {
  return (
    probe !== null &&
    !anyAgentCliInstalled(probe) &&
    !Object.values(connections).some(Boolean)
  );
}

const subscriptionLanes: Partial<
  Record<AgentKind, AgentSessionCreateLaneKind>
> = {
  claude: "claudeSubscription",
  codex: "codexChatGpt",
  kimi: "kimiSubscription",
};

const supportedProfileProviders = [
  "anthropic",
  "openai",
  "moonshot",
  "zai",
  "xai",
] as const;

export function connectGateConnections(
  laneOptions: AgentSessionLaneOptions,
  credentialProfiles: CredentialProfile[],
): ConnectGateConnections {
  const activeProviders = new Set(
    [...credentialProfiles, ...laneCredentialProfiles(laneOptions)]
      .filter((profile) => profile.status === "active")
      .map((profile) => profile.provider),
  );

  return {
    anthropic: activeProviders.has("anthropic"),
    claude: subscriptionConnected(laneOptions, "claude"),
    codex: subscriptionConnected(laneOptions, "codex"),
    grok: activeProviders.has("xai"),
    kimi: subscriptionConnected(laneOptions, "kimi"),
    moonshot: activeProviders.has("moonshot"),
    openai: activeProviders.has("openai"),
    zai: activeProviders.has("zai"),
  };
}

export function allConnectGateAccountsConnected(
  laneOptions: AgentSessionLaneOptions,
  credentialProfiles: CredentialProfile[],
) {
  const connections = connectGateConnections(laneOptions, credentialProfiles);
  const allSubscriptions =
    connections.claude &&
    connections.codex &&
    connections.kimi &&
    connections.zai &&
    connections.grok;
  const allProviderProfiles = supportedProfileProviders.every(
    (provider) =>
      provider === "xai" ? connections.grok : connections[provider],
  );
  return allSubscriptions || allProviderProfiles;
}

export function connectGateOutcome(
  laneOptions: AgentSessionLaneOptions,
  credentialProfiles: CredentialProfile[],
  hasLegacyCredentials: boolean,
): ConnectGateOutcome {
  if (allConnectGateAccountsConnected(laneOptions, credentialProfiles)) {
    return "all";
  }
  const connections = connectGateConnections(laneOptions, credentialProfiles);
  return hasLegacyCredentials || Object.values(connections).some(Boolean)
    ? "some"
    : "none";
}

export function nextConnectGateState(
  current: ConnectGateState,
  accountsResolved: boolean,
  laneOptions: AgentSessionLaneOptions,
  credentialProfiles: CredentialProfile[],
  hasLegacyCredentials: boolean,
  partialGateDismissed = false,
): ConnectGateState {
  if (current === "passed") {
    return current;
  }
  if (!accountsResolved) {
    return current;
  }
  const outcome = connectGateOutcome(
    laneOptions,
    credentialProfiles,
    hasLegacyCredentials,
  );
  if (outcome === "all" || (outcome === "some" && partialGateDismissed)) {
    return "passed";
  }
  return outcome === "some" ? "optional" : "required";
}

function subscriptionConnected(
  laneOptions: AgentSessionLaneOptions,
  agentKind: AgentKind,
) {
  const laneKind = subscriptionLanes[agentKind];
  if (!laneKind) {
    return false;
  }
  return laneOptions
    .find((option) => option.agentKind === agentKind)
    ?.lanes.some((lane) => lane.kind === laneKind && lane.loggedIn === true) ??
    false;
}

function laneCredentialProfiles(laneOptions: AgentSessionLaneOptions) {
  return laneOptions.flatMap((option) =>
    option.lanes.flatMap((lane) => lane.credentialProfiles),
  );
}
