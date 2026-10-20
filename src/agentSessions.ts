import { ipc } from "./ipc.ts";
import { deriveSavingsMeasurement } from "./savingsMeasurement.ts";
import type {
  CredentialProfile,
  PonytailMode,
  SavingsPricingSource,
} from "./types";

export type AgentKind =
  | "antigravity"
  | "claude"
  | "codex"
  | "gemini"
  | "kimi"
  | "zai"
  | "grok";

export type AgentSessionStatus =
  | "starting"
  | "running"
  | "waiting_input"
  | "exited"
  | "failed";

export type AgentSessionMode =
  | "gui"
  | "terminal_embedded"
  | "terminal_ghostty";

export type AiVaultSessionTitle = {
  sessionId: string;
  title: string;
};

export type AgentSessionTitleState = {
  defaultTitle: string;
  generatedTitle?: string | null;
  aiVaultTitle?: AiVaultSessionTitle | null;
  customTitle?: string | null;
  generation: number;
};

export type AgentSessionSavingsSources = {
  rtk: "missing" | "present" | "instruction";
  crux: "missing" | "present";
  proxy: boolean;
};

export type AgentSessionLane =
  | { kind: "claudeSubscription" }
  | {
      kind: "claudeApiKey";
      credentialProfileId: string;
      baseUrl?: string;
    }
  | { kind: "custom"; env: Array<[string, string]> };

export type AgentSessionLaneKind = AgentSessionLane["kind"];

export type AgentSessionAdditionalLane =
  | { kind: "codexChatGpt"; proxied: boolean }
  | {
      kind: "zaiGlm";
      credentialProfileId: string;
      modelOverrides?: {
        sonnet?: string;
        opus?: string;
        haiku?: string;
      };
    }
  | {
      kind: "grokApiKey";
      credentialProfileId: string;
      modelOverrides?: {
        sonnet?: string;
        opus?: string;
        haiku?: string;
      };
    }
  | { kind: "kimiApiKey"; credentialProfileId: string }
  | { kind: "kimiSubscription" };

export type AgentSessionCreateLane =
  | AgentSessionLane
  | AgentSessionAdditionalLane;

export type AgentSessionCreateLaneKind = AgentSessionCreateLane["kind"];

export type AgentSessionLaneOption = {
  kind: AgentSessionCreateLaneKind;
  loggedIn: boolean | null;
  credentialProfiles: CredentialProfile[];
};

export type AgentSessionAgentOptions = {
  agentKind: AgentKind;
  lanes: AgentSessionLaneOption[];
};

export type AgentSessionLaneOptions = AgentSessionAgentOptions[];

export type AgentSession = {
  id: string;
  defaultTitle?: string;
  generatedTitle?: string | null;
  aiVaultTitle?: AiVaultSessionTitle | null;
  customTitle?: string | null;
  generation?: number;
  agentKind: AgentKind;
  laneKind: AgentSessionCreateLaneKind;
  mode: AgentSessionMode;
  ponytailMode?: PonytailMode | null;
  projectId?: number | null;
  conversationId?: string | null;
  conversationTitle?: string | null;
  model?: string | null;
  cwd: string;
  pid: number;
  status: AgentSessionStatus;
  cliSessionId?: string | null;
  createdAt: string;
  lastActiveAt: string;
  exitCode?: number | null;
  // Old saved sessions and test data do not have this field. Live Rust
  // sessions always include it.
  savingsSources?: AgentSessionSavingsSources;
};

export type AgentSessionCheckpoint = {
  id: string;
  sessionId: string;
  turnId?: string | null;
  repoRoot: string;
  commitSha: string;
  treeSha: string;
  headSha?: string | null;
  branch?: string | null;
  kind: "turn" | "safety";
  label: string;
  createdAt: string;
};

export type AgentSessionCheckpointPreview = {
  checkpointId: string;
  modified: string[];
  created: string[];
  deleted: string[];
};

export type AgentSessionCheckpointRestoreResult =
  AgentSessionCheckpointPreview & {
    safetyCheckpointId: string;
  };

export type AgentBinaryProbe = {
  found: boolean;
  binary?: string | null;
  version?: string | null;
  error?: string | null;
};

export type AgentSessionProbe =
  | AgentBinaryProbe
  | {
      claude?: AgentBinaryProbe;
      codex?: AgentBinaryProbe;
      kimi?: AgentBinaryProbe;
      ghostty?: GhosttyProbe;
      [key: string]: AgentBinaryProbe | GhosttyProbe | undefined;
    };

export type GhosttyProbe = {
  found: boolean;
  version?: string | null;
  applescriptReady: boolean;
};

export type AgentSessionUsage = {
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  totalCostUsd?: number | null;
  savingsTokens?: number | null;
  savingsSource?: string | null;
  savingsUsd?: number | null;
  /** Dollars saved with the methods of ADR 0015. */
  dollarsUsd?: number | null;
  pricingSource?: SavingsPricingSource | null;
  rtkTokensSaved?: number | null;
  rtkIncludedInTotal?: boolean;
  cruxTokensSaved?: number | null;
  cruxIncludedInTotal?: boolean;
  turnBoundary?: boolean;
};

export type AgentSessionEvent =
  | { type: "sessionInit"; cliSessionId: string; model: string }
  | { type: "turnStarted"; turnId?: string | null }
  | { type: "embeddedTurnStarted"; turnId?: string | null }
  | {
      type: "embeddedTurnFinished";
      turnId?: string | null;
      outcome: "completed" | "aborted";
    }
  | { type: "textDelta"; text: string }
  | { type: "toolCallStarted"; name: string; summaryJson: string }
  | { type: "toolCallCompleted"; name: string }
  | {
      type: "permissionRequest";
      requestId: string;
      description: string;
      payloadJson: string;
    }
  | { type: "permissionResolved"; requestId: string; approved: boolean }
  | { type: "turnCompleted"; usage: AgentSessionUsage }
  | {
      type: "savingsSample";
      savingsTokens: number;
      savingsSource: string;
      savingsUsd?: number | null;
      dollarsUsd?: number;
      pricingSource?: SavingsPricingSource | null;
      rtkTokensSaved?: number;
      rtkIncludedInTotal?: boolean;
      cruxTokensSaved?: number;
      cruxIncludedInTotal?: boolean;
      cacheReadTokens?: number;
      turnBoundary?: boolean;
    }
  | {
      type: "savingsSourcesChanged";
      savingsSources: AgentSessionSavingsSources;
    }
  | { type: "sessionExited"; code: number | null }
  | { type: "protocolError"; message: string };

export type AgentSessionEventPayload = {
  sessionId: string;
  event: AgentSessionEvent;
};

type TranscriptBase = {
  id: string;
  turn: number;
};

export type AgentSessionTranscriptItem =
  | (TranscriptBase & { kind: "userText"; text: string })
  | (TranscriptBase & { kind: "assistantText"; text: string })
  | (TranscriptBase & {
      kind: "toolCall";
      name: string;
      summaryJson: string;
      completed: boolean;
      permissionRequest?: {
        requestId: string;
        description: string;
        payloadJson: string;
        approved?: boolean;
      };
    })
  | (TranscriptBase & { kind: "error"; text: string })
  | (TranscriptBase & { kind: "system"; text: string });

type AgentSessionTranscriptDraft =
  AgentSessionTranscriptItem extends infer Item
    ? Item extends TranscriptBase
      ? Omit<Item, keyof TranscriptBase>
      : never
    : never;

export type AgentSessionUsageTotals = {
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  totalCostUsd: number;
  savingsTokens: number;
  savingsUsd: number;
  dollarsUsd: number;
  pricingSource: SavingsPricingSource | null;
  rtkTokensSaved: number;
  rtkIncludedInTotal: boolean;
  cruxTokensSaved: number;
  cruxIncludedInTotal: boolean;
};

export type AgentSessionEntry = {
  session: AgentSession;
  transcript: AgentSessionTranscriptItem[];
  turns: AgentSessionUsage[];
  totals: AgentSessionUsageTotals;
  model: string | null;
  turnSequence: number;
  turnActive: boolean;
};

export type AgentSessionsState = {
  entries: AgentSessionEntry[];
  selectedSessionId: string | null;
  probe: AgentSessionProbe | null;
  laneOptions: AgentSessionLaneOptions;
  pendingEventsBySessionId: Record<string, AgentSessionEvent[]>;
  loading: boolean;
  error: string | null;
  nextItemId: number;
};

export type AgentSessionCreateInput = {
  agentKind: AgentKind;
  lane: AgentSessionCreateLane;
  cwd: string;
  projectId?: number;
  conversationId?: string;
  conversationTitle?: string;
  model?: string;
  effort?: string;
  permissionMode?: string;
  initialPrompt?: string;
  resumeCliSessionId?: string;
  previousSessionId?: string;
  mode?: AgentSessionMode;
  cols?: number;
  rows?: number;
  titleState?: AgentSessionTitleState;
};

// Savings accumulated since the current message started. Poll samples
// mark a boundary when the request count grew; the burst is the trailing
// run from the latest boundary.
export function currentTurnSavingsTokens(turns: AgentSessionUsage[]): number | null {
  if (turns.length === 0) return null;
  let sum = 0;
  let measured = false;
  for (let index = turns.length - 1; index >= 0; index -= 1) {
    const turn = turns[index];
    if (!turn) continue;
    if (turn.savingsTokens != null) {
      sum += turn.savingsTokens;
      measured = true;
    }
    if (turn.turnBoundary) return measured ? sum : null;
  }
  return measured ? sum : null;
}

export const SESSION_SAVINGS_PROXY_OFF_NOTE =
  "crux + RTK only \u00b7 proxy off";
export const CLAUDE_SUBSCRIPTION_PROXY_ON_NOTE = "crux + RTK \u00b7 proxy on";
export const SESSION_SAVINGS_RTK_MISSING_NOTE = "RTK hook not active";
export const SESSION_SAVINGS_RTK_INSTRUCTION_NOTE =
  "crux + RTK (by instruction)";

export function sessionSavingsUnmeasured(
  _laneKind: string,
  savingsSources?: AgentSessionSavingsSources,
) {
  if (!savingsSources) {
    return false;
  }
  return (
    deriveSavingsMeasurement({ tokensSaved: 0, savingsSources }).state ===
    "unmeasured"
  );
}

export function sessionSavingsMeasurementNote(
  laneKind: string,
  savingsSources?: AgentSessionSavingsSources,
): string | null {
  if (laneKind === "claudeSubscription" && savingsSources?.proxy) {
    return CLAUDE_SUBSCRIPTION_PROXY_ON_NOTE;
  }
  if (savingsSources?.rtk === "instruction") {
    return `${SESSION_SAVINGS_RTK_INSTRUCTION_NOTE} \u00b7 proxy ${savingsSources.proxy ? "on" : "off"}`;
  }
  if (!savingsSources && laneKind === "claudeSubscription") {
    return SESSION_SAVINGS_PROXY_OFF_NOTE;
  }
  if (
    !savingsSources &&
    (laneKind === "kimiSubscription" || laneKind === "kimiApiKey")
  ) {
    return `${SESSION_SAVINGS_RTK_INSTRUCTION_NOTE} · proxy on`;
  }
  if (!savingsSources) {
    return null;
  }
  if (laneKind === "kimiSubscription" && savingsSources.rtk === "missing") {
    return SESSION_SAVINGS_RTK_MISSING_NOTE;
  }
  if (savingsSources.proxy) {
    return null;
  }
  const measurement = deriveSavingsMeasurement({
    tokensSaved: 0,
    savingsSources,
  });
  if (measurement.state === "unmeasured") {
    return SESSION_SAVINGS_RTK_MISSING_NOTE;
  }
  if (
    measurement.state === "estimated" &&
    savingsSources.crux === "present" &&
    savingsSources.rtk === "missing"
  ) {
    return "crux only \u00b7 proxy off";
  }
  return SESSION_SAVINGS_PROXY_OFF_NOTE;
}

export function liveSavingsMeasurementState(sessions: readonly AgentSession[]) {
  const live = sessions.filter(
    (session) =>
      ["starting", "running", "waiting_input"].includes(session.status),
  );
  const measurements = live.map((session) =>
    deriveSavingsMeasurement({
      tokensSaved: 0,
      savingsSources: session.savingsSources,
    }),
  );
  const state = measurements.some((measurement) => measurement.state === "measured")
    ? "measured"
    : measurements.some((measurement) => measurement.state === "estimated")
      ? "estimated"
      : "unmeasured";
  if (live.length > 0 && state === "unmeasured") {
    return {
      active: true,
      state,
      unmeasured: true,
      note: SESSION_SAVINGS_RTK_MISSING_NOTE,
    };
  }
  return {
    active: live.length > 0,
    state,
    unmeasured: false,
    note:
      state === "estimated"
        ? live.every(
            (session) =>
              session.savingsSources?.proxy === false &&
              session.savingsSources.crux === "present" &&
              session.savingsSources.rtk === "missing",
          )
          ? "crux only \u00b7 proxy off"
          : SESSION_SAVINGS_PROXY_OFF_NOTE
        : null,
  };
}

export function ponytailBadgeLabel(
  mode: PonytailMode | null | undefined,
): string | null {
  return mode && mode !== "off" ? `ponytail ${mode}` : null;
}

// These CLIs take the first message as a positional argument at launch.
// Kimi and Antigravity have no documented interactive prompt argument.
// Their first message uses typed delivery after the TUI is ready.
export function agentSupportsInitialPrompt(agentKind: AgentKind) {
  return (
    agentKind === "claude" ||
    agentKind === "codex" ||
    agentKind === "gemini" ||
    agentKind === "zai" ||
    agentKind === "grok"
  );
}

export function agentSessionAgentLabel(agentKind: AgentKind) {
  if (agentKind === "antigravity") return "Antigravity";
  if (agentKind === "claude") return "Claude";
  if (agentKind === "codex") return "Codex";
  if (agentKind === "gemini") return "Gemini";
  if (agentKind === "kimi") return "Kimi";
  if (agentKind === "grok") return "Grok";
  return "GLM";
}

export function agentSessionAgentSubtitle(agentKind: AgentKind) {
  if (agentKind === "antigravity") return "Antigravity CLI";
  if (agentKind === "gemini") return "Gemini CLI";
  if (agentKind === "zai") return "Z.ai Coding Plan";
  if (agentKind === "grok") return "xAI API";
  return null;
}

export function agentSessionTitle(session: AgentSession) {
  return (
    session.customTitle?.trim() ||
    session.aiVaultTitle?.title.trim() ||
    session.generatedTitle?.trim() ||
    session.defaultTitle?.trim() ||
    session.conversationTitle?.trim() ||
    agentSessionAgentLabel(session.agentKind)
  );
}

export function resolveAgentTitleLayers(
  titles: Partial<AgentSessionTitleState>,
  fallback: string,
) {
  return (
    titles.customTitle?.trim() ||
    titles.aiVaultTitle?.title.trim() ||
    titles.generatedTitle?.trim() ||
    titles.defaultTitle?.trim() ||
    fallback
  );
}

export function agentSessionLaneLabel(laneKind: AgentSessionCreateLaneKind) {
  if (laneKind === "claudeSubscription") return "subscription";
  if (laneKind === "claudeApiKey") return "API key";
  if (laneKind === "codexChatGpt") return "ChatGPT";
  if (laneKind === "zaiGlm") return "GLM Coding Plan";
  if (laneKind === "grokApiKey") return "Grok API key";
  if (laneKind === "kimiApiKey") return "API key";
  if (laneKind === "kimiSubscription") return "subscription";
  return "custom";
}

export type SessionSavingsDisplay = {
  label: "measured" | "estimated" | "unmeasured";
  text: string;
  tokenCount: number;
  source: string | null;
  costUsd: number | null;
};

export function emptyAgentSessionsState(): AgentSessionsState {
  return {
    entries: [],
    selectedSessionId: null,
    probe: null,
    laneOptions: [],
    pendingEventsBySessionId: {},
    loading: true,
    error: null,
    nextItemId: 1,
  };
}

export const AGENT_SESSION_PRO_MODE_STORAGE_KEY =
  "halv.agent-session-pro-mode";
export const AGENT_SESSION_PRO_MODE_HINT_SEEN_STORAGE_KEY =
  "halv.agent-session-pro-mode-hint-seen";

type ProModeStorage = Pick<Storage, "getItem" | "setItem">;

export function readAgentSessionProMode(
  storage: Pick<ProModeStorage, "getItem"> | null = browserStorage(),
) {
  if (!storage) {
    return true;
  }
  try {
    // Pro is the default experience; a stored value is the user's own
    // choice and always wins.
    const stored = storage.getItem(AGENT_SESSION_PRO_MODE_STORAGE_KEY);
    return stored === null ? true : stored === "true";
  } catch {
    return true;
  }
}

export function writeAgentSessionProMode(
  enabled: boolean,
  storage: Pick<ProModeStorage, "setItem"> | null = browserStorage(),
) {
  if (!storage) {
    return;
  }
  try {
    storage.setItem(AGENT_SESSION_PRO_MODE_STORAGE_KEY, String(enabled));
  } catch {
    // Storage can be unavailable in private or restricted webviews.
  }
}

export function claimAgentSessionProModeHint(
  storage: ProModeStorage | null = browserStorage(),
) {
  if (!storage) {
    return false;
  }
  try {
    if (
      storage.getItem(AGENT_SESSION_PRO_MODE_HINT_SEEN_STORAGE_KEY) === "true"
    ) {
      return false;
    }
    storage.setItem(AGENT_SESSION_PRO_MODE_HINT_SEEN_STORAGE_KEY, "true");
    return true;
  } catch {
    return false;
  }
}

export class AgentSessionProModeStore {
  private enabled: boolean;
  private listeners = new Set<() => void>();
  private readonly storage: ProModeStorage | null;

  constructor(storage: ProModeStorage | null = browserStorage()) {
    this.storage = storage;
    this.enabled = readAgentSessionProMode(storage);
  }

  getSnapshot = () => this.enabled;

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  set(enabled: boolean) {
    writeAgentSessionProMode(enabled, this.storage);
    if (enabled === this.enabled) {
      return;
    }
    this.enabled = enabled;
    for (const listener of this.listeners) {
      listener();
    }
  }

  toggle() {
    this.set(!this.enabled);
  }
}

export const agentSessionProModeStore = new AgentSessionProModeStore();

export function agentSessionModeForProMode(
  enabled: boolean,
  ghostty: GhosttyProbe | null | undefined,
): AgentSessionMode {
  if (!enabled) {
    return "gui";
  }
  return ghostty?.found && ghostty.applescriptReady
    ? "terminal_ghostty"
    : "terminal_embedded";
}

export type AgentSessionModeDecision =
  | { action: "keep"; mode: AgentSessionMode }
  | { action: "wait"; mode: AgentSessionMode }
  | { action: "replace"; mode: AgentSessionMode };

export function agentSessionModeDecision(
  enabled: boolean,
  ghostty: GhosttyProbe | null | undefined,
  session: Pick<AgentSession, "cliSessionId" | "mode" | "status">,
): AgentSessionModeDecision {
  const mode = agentSessionModeForProMode(enabled, ghostty);
  if (session.mode === mode) {
    return { action: "keep", mode };
  }
  if (!session.cliSessionId) {
    return { action: "wait", mode };
  }
  if (session.mode === "gui" && session.status !== "waiting_input") {
    return { action: "wait", mode };
  }
  if (
    session.mode !== "gui" &&
    !["running", "waiting_input"].includes(session.status)
  ) {
    return { action: "wait", mode };
  }
  return { action: "replace", mode };
}

export function restartLaneForAgentSession(
  session: Pick<AgentSession, "laneKind">,
  credentialProfileId?: string | null,
): AgentSessionCreateLane | null {
  if (session.laneKind === "claudeSubscription") {
    return { kind: "claudeSubscription" };
  }
  if (session.laneKind === "codexChatGpt") {
    return { kind: "codexChatGpt", proxied: true };
  }
  if (session.laneKind === "kimiSubscription") {
    return { kind: "kimiSubscription" };
  }
  const profileId = credentialProfileId?.trim();
  if (!profileId) {
    return null;
  }
  if (session.laneKind === "claudeApiKey") {
    return { kind: "claudeApiKey", credentialProfileId: profileId };
  }
  if (session.laneKind === "kimiApiKey") {
    return { kind: "kimiApiKey", credentialProfileId: profileId };
  }
  if (session.laneKind === "zaiGlm") {
    return { kind: "zaiGlm", credentialProfileId: profileId };
  }
  if (session.laneKind === "grokApiKey") {
    return { kind: "grokApiKey", credentialProfileId: profileId };
  }
  return null;
}

export function agentSessionProModeCaption(
  ghostty: GhosttyProbe | null | undefined,
) {
  return ghostty?.found && ghostty.applescriptReady
    ? "New conversations open in Ghostty"
    : "New conversations open in the built-in terminal";
}

export function laneOptionsForAgent(
  options: AgentSessionLaneOptions,
  agentKind: AgentKind,
) {
  return options.find((option) => option.agentKind === agentKind)?.lanes ?? [];
}

export function defaultLaneKindForAgent(
  options: AgentSessionLaneOptions,
  agentKind: AgentKind,
): AgentSessionCreateLaneKind {
  return defaultLaneSelectionForAgent(options, agentKind).laneKind;
}

export type AgentSessionDefaultLaneSelection = {
  laneKind: AgentSessionCreateLaneKind;
  credentialProfileId: string | null;
};

export function defaultLaneSelectionForAgent(
  options: AgentSessionLaneOptions,
  agentKind: AgentKind,
): AgentSessionDefaultLaneSelection {
  const preferred: Record<AgentKind, AgentSessionCreateLaneKind> = {
    antigravity: "custom",
    claude: "claudeSubscription",
    codex: "codexChatGpt",
    gemini: "custom",
    kimi: "kimiSubscription",
    zai: "zaiGlm",
    grok: "grokApiKey",
  };
  const lanes = laneOptionsForAgent(options, agentKind);
  const signedInSubscription = lanes.find(
    (lane) =>
      lane.kind === preferred[agentKind] && lane.loggedIn === true,
  );
  if (signedInSubscription) {
    return {
      laneKind: signedInSubscription.kind,
      credentialProfileId: null,
    };
  }

  const storedProfileLane = lanes.find((lane) =>
    lane.credentialProfiles.some((profile) => profile.status === "active"),
  );
  if (storedProfileLane) {
    return {
      laneKind: storedProfileLane.kind,
      credentialProfileId:
        storedProfileLane.credentialProfiles.find(
          (profile) => profile.status === "active",
        )?.id ?? null,
    };
  }

  const fallback =
    lanes.find((lane) => lane.kind === preferred[agentKind]) ??
    lanes.find((lane) => lane.kind !== "custom") ??
    lanes[0];
  return {
    laneKind: fallback?.kind ?? preferred[agentKind],
    credentialProfileId:
      fallback?.credentialProfiles.find(
        (profile) => profile.status === "active",
      )?.id ?? null,
  };
}

export function buildAgentSessionLane(
  laneKind: AgentSessionCreateLaneKind,
  credentialProfileId: string | null,
  customEnvironment: string,
): AgentSessionCreateLane {
  if (laneKind === "claudeSubscription" || laneKind === "kimiSubscription") {
    return { kind: laneKind };
  }
  if (laneKind === "codexChatGpt") {
    return { kind: laneKind, proxied: true };
  }
  if (laneKind === "custom") {
    return {
      kind: laneKind,
      env: parseCustomAgentEnvironment(customEnvironment),
    };
  }
  const profileId = credentialProfileId?.trim();
  if (!profileId) {
    throw new Error("Choose a credential profile for this API lane.");
  }
  if (laneKind === "zaiGlm") {
    return { kind: laneKind, credentialProfileId: profileId };
  }
  if (laneKind === "grokApiKey") {
    return { kind: laneKind, credentialProfileId: profileId };
  }
  if (laneKind === "kimiApiKey") {
    return { kind: laneKind, credentialProfileId: profileId };
  }
  return { kind: laneKind, credentialProfileId: profileId };
}

export function parseCustomAgentEnvironment(value: string) {
  return value
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line): [string, string] => {
      const separator = line.indexOf("=");
      const name = separator < 0 ? "" : line.slice(0, separator).trim();
      if (!name || !/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) {
        throw new Error(`Invalid environment entry: ${line}`);
      }
      return [name, line.slice(separator + 1)];
    });
}

export function emptyAgentSessionUsageTotals(): AgentSessionUsageTotals {
  return {
    inputTokens: 0,
    outputTokens: 0,
    cacheReadTokens: 0,
    cacheCreationTokens: 0,
    totalCostUsd: 0,
    savingsTokens: 0,
    savingsUsd: 0,
    dollarsUsd: 0,
    pricingSource: null,
    rtkTokensSaved: 0,
    rtkIncludedInTotal: false,
    cruxTokensSaved: 0,
    cruxIncludedInTotal: false,
  };
}

export function probeForAgent(
  probe: AgentSessionProbe | null,
  agentKind: AgentKind,
): AgentBinaryProbe | null {
  if (!probe) {
    return null;
  }
  if (isAgentBinaryProbe(probe)) {
    if (
      agentKind === "claude" ||
      agentKind === "zai" ||
      agentKind === "grok"
    ) {
      return probe;
    }
    return null;
  }
  const probeKind =
    agentKind === "zai" || agentKind === "grok" ? "claude" : agentKind;
  return (probe[probeKind] as AgentBinaryProbe | undefined) ?? null;
}

export function probeForGhostty(
  probe: AgentSessionProbe | null,
): GhosttyProbe | null {
  if (!probe || isAgentBinaryProbe(probe)) {
    return null;
  }
  return probe.ghostty ?? null;
}

export function resolvedAgentSessionModelForConversation(
  state: AgentSessionsState,
  conversationId: string | null,
) {
  if (!conversationId) {
    return null;
  }
  return (
    state.entries.find(
      (entry) =>
        entry.session.conversationId === conversationId && entry.model !== null,
    )?.model ?? null
  );
}

export function sessionSavingsDisplay(
  usage: AgentSessionUsage | null | undefined,
): SessionSavingsDisplay {
  const measurement = deriveSavingsMeasurement({
    tokensSaved: usage?.savingsTokens,
    savingsUsd: usage?.savingsUsd,
    savingsSource: usage?.savingsSource,
    pricingSource: usage?.pricingSource,
    rtkTokensSaved: usage?.rtkTokensSaved,
    rtkIncludedInTotal: usage?.rtkIncludedInTotal,
    cruxTokensSaved: usage?.cruxTokensSaved,
    cruxIncludedInTotal: usage?.cruxIncludedInTotal,
    cacheReadTokens: usage?.cacheReadTokens,
  });
  if (measurement.state !== "unmeasured" && measurement.tokens !== null) {
    return {
      label: measurement.state,
      text: `~${Math.round(measurement.tokens).toLocaleString()} tokens saved`,
      tokenCount: measurement.tokens,
      source: usage?.savingsSource?.trim() || null,
      costUsd: finiteNonNegative(usage?.totalCostUsd),
    };
  }

  const cached = measurement.composition?.cacheRead ?? 0;
  return {
    label: measurement.state,
    text:
      measurement.state === "unmeasured"
        ? "Savings not measured"
        : `${Math.round(cached).toLocaleString()} tokens served from cache`,
    tokenCount: cached,
    source: null,
    costUsd: finiteNonNegative(usage?.totalCostUsd),
  };
}

export function reduceAgentSessionEvent(
  state: AgentSessionsState,
  payload: AgentSessionEventPayload,
): AgentSessionsState {
  const index = state.entries.findIndex(
    (entry) => entry.session.id === payload.sessionId,
  );
  if (index < 0) {
    return {
      ...state,
      pendingEventsBySessionId: {
        ...state.pendingEventsBySessionId,
        [payload.sessionId]: [
          ...(state.pendingEventsBySessionId[payload.sessionId] ?? []),
          payload.event,
        ],
      },
    };
  }

  const result = applyEventToEntry(
    state.entries[index],
    payload.event,
    state.nextItemId,
  );
  const entries = [...state.entries];
  entries[index] = result.entry;
  return {
    ...state,
    entries,
    nextItemId: result.nextItemId,
  };
}

export function upsertAgentSession(
  state: AgentSessionsState,
  session: AgentSession,
  select = false,
): AgentSessionsState {
  const existingIndex = state.entries.findIndex(
    (entry) => entry.session.id === session.id,
  );
  let entry =
    existingIndex >= 0
      ? { ...state.entries[existingIndex], session }
      : createAgentSessionEntry(session);
  let nextItemId = state.nextItemId;
  const pendingEvents = state.pendingEventsBySessionId[session.id] ?? [];
  for (const event of pendingEvents) {
    const result = applyEventToEntry(entry, event, nextItemId);
    entry = result.entry;
    nextItemId = result.nextItemId;
  }

  const entries = [...state.entries];
  if (existingIndex >= 0) {
    entries[existingIndex] = entry;
  } else {
    entries.unshift(entry);
  }
  entries.sort(compareEntriesByActivity);

  const pendingEventsBySessionId = { ...state.pendingEventsBySessionId };
  delete pendingEventsBySessionId[session.id];
  return {
    ...state,
    entries,
    nextItemId,
    pendingEventsBySessionId,
    selectedSessionId:
      select || !state.selectedSessionId
        ? session.id
        : state.selectedSessionId,
  };
}

export function replaceAgentSessions(
  state: AgentSessionsState,
  sessions: AgentSession[],
): AgentSessionsState {
  let next = state;
  const sessionIds = new Set(sessions.map((session) => session.id));
  for (const session of sessions) {
    next = upsertAgentSession(next, session);
  }
  const entries = next.entries.filter((entry) =>
    sessionIds.has(entry.session.id),
  );
  const selectedSessionId = entries.some(
    (entry) => entry.session.id === next.selectedSessionId,
  )
    ? next.selectedSessionId
    : entries[0]?.session.id ?? null;
  return { ...next, entries, selectedSessionId, loading: false };
}

// Live totals restart at zero with the app; the persisted aggregates put
// each session's meter back where it was. Live numbers stay when larger:
// persistence lags one poll behind the event stream.
export function seedPersistedSavings(
  state: AgentSessionsState,
  saved: AgentSessionSavedTotals[],
): AgentSessionsState {
  if (saved.length === 0) return state;
  const bySession = new Map(saved.map((row) => [row.sessionId, row]));
  const entries = state.entries.map((entry) => {
    if (entry.session.mode === "gui") return entry;
    const row = bySession.get(entry.session.id);
    if (!row) return entry;
    const savingsTokens = Math.max(entry.totals.savingsTokens, row.savingsTokens);
    const dollarsUsd = Math.max(entry.totals.dollarsUsd, row.dollarsUsd ?? 0);
    const cruxTokensSaved = Math.max(
      entry.totals.cruxTokensSaved,
      row.cruxTokensSaved ?? 0,
    );
    const cacheReadTokens = Math.max(
      entry.totals.cacheReadTokens,
      row.cacheReadTokens,
    );
    const pricingSource = mergePricingSource(
      entry.totals.pricingSource,
      row.pricingSource,
    );
    const cruxIncludedInTotal =
      row.cruxIncludedInTotal ?? entry.totals.cruxIncludedInTotal;
    if (
      savingsTokens === entry.totals.savingsTokens &&
      dollarsUsd === entry.totals.dollarsUsd &&
      cruxTokensSaved === entry.totals.cruxTokensSaved &&
      cruxIncludedInTotal === entry.totals.cruxIncludedInTotal &&
      pricingSource === entry.totals.pricingSource &&
      cacheReadTokens === entry.totals.cacheReadTokens
    ) {
      return entry;
    }
    return {
      ...entry,
      totals: {
        ...entry.totals,
        savingsTokens,
        dollarsUsd,
        cruxTokensSaved,
        cruxIncludedInTotal,
        cacheReadTokens,
        pricingSource,
      },
    };
  });
  return { ...state, entries };
}

export function appendAgentSessionUserText(
  state: AgentSessionsState,
  sessionId: string,
  text: string,
): AgentSessionsState {
  return appendTranscriptItem(state, sessionId, {
    kind: "userText",
    text,
  });
}

export function appendAgentSessionError(
  state: AgentSessionsState,
  sessionId: string,
  text: string,
): AgentSessionsState {
  return appendTranscriptItem(state, sessionId, { kind: "error", text });
}

export async function probeAgentSessions() {
  return ipc.agent.sessionProbe();
}

export type AgentSessionSavedTotals = {
  sessionId: string;
  conversationId?: string | null;
  savingsTokens: number;
  cruxTokensSaved?: number;
  cruxIncludedInTotal?: boolean;
  cacheReadTokens: number;
  pricingSource?: SavingsPricingSource | null;
  dollarsUsd?: number;
};

// Closing a tab kills one PTY, not the conversation's history: the meter
// sums every terminal session the conversation ever ran.
export function conversationSavingsTotals(
  rows: AgentSessionSavedTotals[],
  conversationId: string | null,
): { savingsTokens: number; cacheReadTokens: number; dollarsUsd: number } {
  const scoped = conversationId
    ? rows.filter((row) => row.conversationId === conversationId)
    : [];
  return {
    savingsTokens: scoped.reduce((total, row) => total + row.savingsTokens, 0),
    dollarsUsd: scoped.reduce((total, row) => total + (row.dollarsUsd ?? 0), 0),
    cacheReadTokens: scoped.reduce(
      (total, row) => total + row.cacheReadTokens,
      0,
    ),
  };
}

export async function listAgentSessionSavedTotals() {
  return ipc.agent.sessionSavedTotals();
}

export async function listAgentSessions() {
  return ipc.agent.sessionList();
}

export async function listAgentSessionCheckpoints(sessionId: string) {
  return ipc.agent.sessionCheckpoints({
    sessionId,
  });
}

export async function previewAgentSessionCheckpoint(checkpointId: string) {
  return ipc.agent.sessionCheckpointPreview({ checkpointId });
}

export async function restoreAgentSessionCheckpoint(checkpointId: string) {
  return ipc.agent.sessionCheckpointRestore({ checkpointId });
}

export async function listAgentSessionLaneOptions() {
  return ipc.agent.sessionLaneOptions();
}

export async function launchAgentLogin(agentKind: AgentKind) {
  return ipc.agent.loginLaunch({ agentKind });
}

export async function createAgentSession(input: AgentSessionCreateInput) {
  const { mode = "gui", ...commandInput } = input;
  if (mode === "terminal_embedded") {
    const {
      agentKind,
      lane,
      cwd,
      projectId,
      conversationId,
      conversationTitle,
      model,
      effort,
      resumeCliSessionId,
      cols,
      rows,
      titleState,
      permissionMode,
      initialPrompt,
      previousSessionId,
    } = commandInput;
    return ipc.agent.terminalCreate({
      agentKind,
      lane,
      cwd,
      projectId,
      conversationId,
      conversationTitle,
      model,
      effort,
      resumeCliSessionId,
      cols,
      rows,
      titleState,
      permissionMode,
      initialPrompt,
      previousSessionId,
    });
  }
  if (mode === "terminal_ghostty") {
    const {
      agentKind,
      lane,
      cwd,
      projectId,
      conversationId,
      conversationTitle,
      model,
      effort,
      resumeCliSessionId,
      titleState,
      permissionMode,
    } = commandInput;
    return ipc.agent.ghosttyLaunch({
      agentKind,
      lane,
      cwd,
      projectId,
      conversationId,
      conversationTitle,
      model,
      effort,
      resumeCliSessionId,
      titleState,
      permissionMode,
    });
  }
  return ipc.agent.sessionCreate(commandInput);
}

export async function sendAgentSessionMessage(sessionId: string, text: string) {
  return ipc.agent.sessionSend({ sessionId, text });
}

export async function respondAgentSessionPermission(
  sessionId: string,
  requestId: string,
  approve: boolean,
) {
  return ipc.agent.sessionRespondPermission({
    sessionId,
    requestId,
    approve,
  });
}

export async function killAgentSession(sessionId: string) {
  return ipc.agent.sessionKill({ sessionId });
}

export async function forgetAgentSession(sessionId: string) {
  return ipc.agent.sessionForget({ sessionId });
}

export async function renameAgentSession(
  sessionId: string,
  title: string | null,
) {
  return ipc.agent.sessionRename({ sessionId, title });
}

export async function subscribeAgentSessionEvents(
  handler: (payload: AgentSessionEventPayload) => void,
) {
  return ipc.events.listen<AgentSessionEventPayload>(
    "agent-session-event",
    handler,
  );
}

type AgentSessionStoreDependencies = {
  probe: typeof probeAgentSessions;
  list: typeof listAgentSessions;
  savedTotals: typeof listAgentSessionSavedTotals;
  laneOptions: typeof listAgentSessionLaneOptions;
  create: typeof createAgentSession;
  send: typeof sendAgentSessionMessage;
  kill: typeof killAgentSession;
  rename: typeof renameAgentSession;
  subscribe: typeof subscribeAgentSessionEvents;
};

const defaultDependencies: AgentSessionStoreDependencies = {
  probe: probeAgentSessions,
  list: listAgentSessions,
  savedTotals: listAgentSessionSavedTotals,
  laneOptions: listAgentSessionLaneOptions,
  create: createAgentSession,
  send: sendAgentSessionMessage,
  kill: killAgentSession,
  rename: renameAgentSession,
  subscribe: subscribeAgentSessionEvents,
};

export class AgentSessionStore {
  private state = emptyAgentSessionsState();
  private listeners = new Set<() => void>();
  private unlisten: (() => void) | null = null;
  private connectionGeneration = 0;
  private readonly dependencies: AgentSessionStoreDependencies;

  constructor(
    dependencies: AgentSessionStoreDependencies = defaultDependencies,
  ) {
    this.dependencies = dependencies;
  }

  getSnapshot = () => this.state;

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  async connect() {
    const generation = ++this.connectionGeneration;
    this.unlisten?.();
    this.unlisten = null;
    this.commit({ ...this.state, loading: true, error: null });

    try {
      const unlisten = await this.dependencies.subscribe((payload) => {
        if (generation !== this.connectionGeneration) {
          return;
        }
        this.commit(reduceAgentSessionEvent(this.state, payload));
        if (payload.event.type === "sessionExited") {
          void this.refreshSessions().catch(() => undefined);
        }
      });
      if (generation !== this.connectionGeneration) {
        unlisten();
        return;
      }
      this.unlisten = unlisten;
    } catch (error) {
      if (generation === this.connectionGeneration) {
        this.commit({ ...this.state, error: errorMessage(error) });
      }
    }

    const [sessions, probe, laneOptions] = await Promise.allSettled([
      this.dependencies.list(),
      this.dependencies.probe(),
      this.dependencies.laneOptions(),
    ]);
    if (generation !== this.connectionGeneration) {
      return;
    }

    let next = this.state;
    if (sessions.status === "fulfilled") {
      next = replaceAgentSessions(next, sessions.value);
    } else {
      next = {
        ...next,
        loading: false,
        error: errorMessage(sessions.reason),
      };
    }
    if (probe.status === "fulfilled") {
      next = { ...next, probe: probe.value };
    }
    if (laneOptions.status === "fulfilled") {
      next = { ...next, laneOptions: laneOptions.value };
    } else if (!next.error) {
      next = { ...next, error: errorMessage(laneOptions.reason) };
    }
    this.commit(next);
  }

  // Other login paths (ConnectGate, window focus) push fresh lanes here.
  // Without this push, launches keep a stale logged-out lane.
  applyLaneOptions(laneOptions: AgentSessionLaneOptions) {
    this.commit({ ...this.state, laneOptions });
  }

  async refreshLaneOptions() {
    try {
      const laneOptions = await this.dependencies.laneOptions();
      this.applyLaneOptions(laneOptions);
      return laneOptions;
    } catch (error) {
      this.commit({ ...this.state, error: errorMessage(error) });
      throw error;
    }
  }

  async refreshSessions() {
    try {
      const sessions = await this.dependencies.list();
      // Commit the list immediately: holding it across another await
      // clobbered concurrent commits (resume renames, status events) with
      // a stale snapshot and broke the terminal restore.
      this.commit(replaceAgentSessions(this.state, sessions));
      try {
        const saved = await this.dependencies.savedTotals();
        this.commit(seedPersistedSavings(this.state, saved));
      } catch {
        // Best effort: the live meter still works without the seed.
      }
      return sessions;
    } catch (error) {
      this.commit({ ...this.state, error: errorMessage(error) });
      throw error;
    }
  }

  disconnect() {
    this.connectionGeneration += 1;
    this.unlisten?.();
    this.unlisten = null;
  }

  select(sessionId: string) {
    if (!this.state.entries.some((entry) => entry.session.id === sessionId)) {
      return;
    }
    this.commit({ ...this.state, selectedSessionId: sessionId });
  }

  clearError() {
    this.commit({ ...this.state, error: null });
  }

  async create(input: AgentSessionCreateInput) {
    this.clearError();
    try {
      const session = await this.dependencies.create(input);
      this.commit(upsertAgentSession(this.state, session, true));
      if (session.mode !== "gui") {
        // A resumed terminal inherits migrated history; seed its totals
        // without blocking the create - callers await the session, not
        // the meter backfill.
        void this.dependencies
          .savedTotals()
          .then((saved) => this.commit(seedPersistedSavings(this.state, saved)))
          .catch(() => undefined);
      }
      return session;
    } catch (error) {
      this.commit({ ...this.state, error: errorMessage(error) });
      throw error;
    }
  }

  async send(sessionId: string, text: string) {
    const trimmed = text.trim();
    if (!trimmed) {
      return;
    }
    this.clearError();
    this.commit(appendAgentSessionUserText(this.state, sessionId, trimmed));
    try {
      await this.dependencies.send(sessionId, trimmed);
    } catch (error) {
      const message = errorMessage(error);
      this.commit(appendAgentSessionError(this.state, sessionId, message));
      this.commit({ ...this.state, error: message });
      throw error;
    }
  }

  async kill(sessionId: string) {
    this.clearError();
    try {
      const session = await this.dependencies.kill(sessionId);
      this.commit(upsertAgentSession(this.state, session));
      return session;
    } catch (error) {
      this.commit({ ...this.state, error: errorMessage(error) });
      throw error;
    }
  }

  async rename(sessionId: string, title: string | null) {
    this.clearError();
    try {
      const session = await this.dependencies.rename(sessionId, title);
      this.commit(upsertAgentSession(this.state, session));
      return session;
    } catch (error) {
      this.commit({ ...this.state, error: errorMessage(error) });
      throw error;
    }
  }

  private commit(state: AgentSessionsState) {
    if (state === this.state) {
      return;
    }
    this.state = state;
    for (const listener of this.listeners) {
      listener();
    }
  }
}

export const agentSessionStore = new AgentSessionStore();

function createAgentSessionEntry(session: AgentSession): AgentSessionEntry {
  return {
    session,
    transcript: [],
    turns: [],
    totals: emptyAgentSessionUsageTotals(),
    model: null,
    turnSequence: 0,
    turnActive: false,
  };
}

function applyEventToEntry(
  entry: AgentSessionEntry,
  event: AgentSessionEvent,
  nextItemId: number,
): { entry: AgentSessionEntry; nextItemId: number } {
  let next = { ...entry, transcript: [...entry.transcript] };
  let itemId = nextItemId;
  const append = (
    item: AgentSessionTranscriptDraft,
  ) => {
    next.transcript.push({
      ...item,
      id: `agent-session-item-${itemId++}`,
      turn: next.turnSequence,
    } as AgentSessionTranscriptItem);
  };

  switch (event.type) {
    case "sessionInit":
      next = {
        ...next,
        // Discovery-driven init events can omit the model; keep the known one.
        model: event.model || next.model,
        session: {
          ...next.session,
          cliSessionId: event.cliSessionId,
          status: "running",
        },
      };
      append({
        kind: "system",
        text: event.model
          ? `Session started with ${event.model}.`
          : "Session started.",
      });
      break;
    case "turnStarted":
      next = {
        ...next,
        turnSequence: next.turnSequence + 1,
        turnActive: true,
        session: { ...next.session, status: "running" },
      };
      break;
    case "embeddedTurnStarted":
    case "embeddedTurnFinished":
      // Embedded CLI lifecycle drives terminal activity only. Its process-backed
      // agent session remains running until the terminal process exits.
      break;
    case "textDelta": {
      if (!next.turnActive) {
        next = beginTurn(next);
      }
      const last = next.transcript.at(-1);
      if (last?.kind === "assistantText" && last.turn === next.turnSequence) {
        next.transcript[next.transcript.length - 1] = {
          ...last,
          text: last.text + event.text,
        };
      } else {
        append({ kind: "assistantText", text: event.text });
      }
      break;
    }
    case "toolCallStarted":
      if (!next.turnActive) {
        next = beginTurn(next);
      }
      append({
        kind: "toolCall",
        name: event.name,
        summaryJson: event.summaryJson,
        completed: false,
      });
      break;
    case "toolCallCompleted": {
      const toolIndex = findPendingToolIndex(next.transcript, event.name);
      if (toolIndex >= 0) {
        const tool = next.transcript[toolIndex];
        if (tool.kind === "toolCall") {
          next.transcript[toolIndex] = { ...tool, completed: true };
        }
      } else {
        append({
          kind: "toolCall",
          name: event.name,
          summaryJson: "",
          completed: true,
        });
      }
      break;
    }
    case "permissionRequest":
      if (!next.turnActive) {
        next = beginTurn(next);
      }
      next.session = { ...next.session, status: "waiting_input" };
      append({
        kind: "toolCall",
        name: "Permission request",
        summaryJson: event.payloadJson,
        completed: false,
        permissionRequest: {
          requestId: event.requestId,
          description: event.description,
          payloadJson: event.payloadJson,
        },
      });
      break;
    case "permissionResolved": {
      const permissionIndex = next.transcript.findIndex(
        (item) =>
          item.kind === "toolCall" &&
          item.permissionRequest?.requestId === event.requestId,
      );
      if (permissionIndex >= 0) {
        const item = next.transcript[permissionIndex];
        if (item.kind === "toolCall" && item.permissionRequest) {
          next.transcript[permissionIndex] = {
            ...item,
            completed: true,
            permissionRequest: {
              ...item.permissionRequest,
              approved: event.approved,
            },
          };
        }
      }
      const hasPendingPermission = next.transcript.some(
        (item) =>
          item.kind === "toolCall" &&
          Boolean(item.permissionRequest) &&
          !item.completed,
      );
      next.session = {
        ...next.session,
        status: hasPendingPermission
          ? "waiting_input"
          : next.turnActive
            ? "running"
            : "waiting_input",
      };
      break;
    }
    case "turnCompleted":
      next = {
        ...next,
        turnActive: false,
        turns: [...next.turns, event.usage],
        totals: addUsage(next.totals, event.usage),
        session: { ...next.session, status: "waiting_input" },
      };
      break;
    case "savingsSample": {
      const usage: AgentSessionUsage = {
        inputTokens: 0,
        outputTokens: 0,
        cacheReadTokens: event.cacheReadTokens ?? 0,
        cacheCreationTokens: 0,
        totalCostUsd: 0,
        savingsTokens: event.savingsTokens,
        savingsSource: event.savingsSource,
        savingsUsd: event.savingsUsd,
        dollarsUsd: event.dollarsUsd ?? 0,
        pricingSource: event.pricingSource,
        rtkTokensSaved: event.rtkTokensSaved,
        rtkIncludedInTotal: event.rtkIncludedInTotal,
        cruxTokensSaved: event.cruxTokensSaved,
        cruxIncludedInTotal: event.cruxIncludedInTotal,
        turnBoundary: event.turnBoundary ?? false,
      };
      next = {
        ...next,
        turns: [...next.turns, usage],
        totals: addUsage(next.totals, usage),
      };
      break;
    }
    case "savingsSourcesChanged":
      next = {
        ...next,
        session: {
          ...next.session,
          savingsSources: event.savingsSources,
        },
      };
      break;
    case "sessionExited":
      next = {
        ...next,
        turnActive: false,
        transcript: next.transcript.map((item) =>
          item.kind === "toolCall" && item.permissionRequest && !item.completed
            ? {
                ...item,
                completed: true,
                permissionRequest: {
                  ...item.permissionRequest,
                  approved: false,
                },
              }
            : item,
        ),
        session: {
          ...next.session,
          status: event.code === 0 ? "exited" : "failed",
          exitCode: event.code,
        },
      };
      append({
        kind: "system",
        text:
          event.code === null
            ? "Session exited without an exit code."
            : `Session exited with code ${event.code}.`,
      });
      break;
    case "protocolError":
      append({ kind: "error", text: event.message });
      break;
  }

  return { entry: next, nextItemId: itemId };
}

function beginTurn(entry: AgentSessionEntry) {
  if (entry.turnActive) {
    return entry;
  }
  return {
    ...entry,
    turnSequence: entry.turnSequence + 1,
    turnActive: true,
  };
}

function appendTranscriptItem(
  state: AgentSessionsState,
  sessionId: string,
  item: AgentSessionTranscriptDraft,
) {
  const index = state.entries.findIndex(
    (entry) => entry.session.id === sessionId,
  );
  if (index < 0) {
    return state;
  }
  const entry = state.entries[index];
  const entries = [...state.entries];
  entries[index] = {
    ...entry,
    transcript: [
      ...entry.transcript,
      {
        ...item,
        id: `agent-session-item-${state.nextItemId}`,
        turn: entry.turnSequence + 1,
      } as AgentSessionTranscriptItem,
    ],
  };
  return { ...state, entries, nextItemId: state.nextItemId + 1 };
}

function addUsage(
  totals: AgentSessionUsageTotals,
  usage: AgentSessionUsage,
): AgentSessionUsageTotals {
  return {
    inputTokens: totals.inputTokens + finiteOrZero(usage.inputTokens),
    outputTokens: totals.outputTokens + finiteOrZero(usage.outputTokens),
    cacheReadTokens:
      totals.cacheReadTokens + finiteOrZero(usage.cacheReadTokens),
    cacheCreationTokens:
      totals.cacheCreationTokens + finiteOrZero(usage.cacheCreationTokens),
    totalCostUsd: totals.totalCostUsd + finiteOrZero(usage.totalCostUsd),
    savingsTokens: totals.savingsTokens + finiteOrZero(usage.savingsTokens),
    savingsUsd: totals.savingsUsd + finiteOrZero(usage.savingsUsd),
    dollarsUsd: totals.dollarsUsd + finiteOrZero(usage.dollarsUsd),
    pricingSource: mergePricingSource(
      totals.pricingSource,
      usage.pricingSource,
    ),
    rtkTokensSaved:
      totals.rtkTokensSaved + finiteOrZero(usage.rtkTokensSaved),
    rtkIncludedInTotal:
      totals.rtkIncludedInTotal && usage.rtkIncludedInTotal !== false,
    cruxTokensSaved:
      totals.cruxTokensSaved + finiteOrZero(usage.cruxTokensSaved),
    cruxIncludedInTotal:
      totals.cruxIncludedInTotal && usage.cruxIncludedInTotal !== false,
  };
}

function mergePricingSource(
  current: SavingsPricingSource | null,
  next: SavingsPricingSource | null | undefined,
): SavingsPricingSource | null {
  if (!next || current === next) return current;
  if (!current) return next;
  if (current === "fallback" || next === "fallback") return "fallback";
  if (current === "model_table" || next === "model_table") {
    return "model_table";
  }
  return "headroom";
}

function findPendingToolIndex(
  transcript: AgentSessionTranscriptItem[],
  name: string,
) {
  for (let index = transcript.length - 1; index >= 0; index -= 1) {
    const item = transcript[index];
    if (
      item.kind === "toolCall" &&
      item.name === name &&
      !item.completed &&
      !item.permissionRequest
    ) {
      return index;
    }
  }
  return -1;
}

function compareEntriesByActivity(
  left: AgentSessionEntry,
  right: AgentSessionEntry,
) {
  return (
    Date.parse(right.session.lastActiveAt) -
    Date.parse(left.session.lastActiveAt)
  );
}

function finiteNonNegative(value: number | null | undefined) {
  return typeof value === "number" && Number.isFinite(value) && value >= 0
    ? value
    : null;
}

function isAgentBinaryProbe(
  probe: AgentSessionProbe,
): probe is AgentBinaryProbe {
  return typeof (probe as AgentBinaryProbe).found === "boolean";
}

function finiteOrZero(value: number | null | undefined) {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function browserStorage(): Storage | null {
  return typeof window === "undefined" ? null : window.localStorage;
}
