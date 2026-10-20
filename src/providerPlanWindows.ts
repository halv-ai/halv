import type { ConnectGateConnections } from "./connectGate";
import type {
  AnthropicCredentialStatus,
  AuthStatus,
  PlanWindowStats,
  UsageWindow,
} from "./types";

export type PlanProviderId = "claude" | "codex" | "kimi" | "glm" | "grok";

export type PlanProviderConnections = Record<PlanProviderId, boolean>;

export type ClaudeLocalUsageWindow = {
  label: string;
  usedPercent: number | null;
  resetsAt: string | null;
};

export type ClaudeUsageWindowsResult = {
  windows: ClaudeLocalUsageWindow[];
  unavailableReason: string | null;
};

// Live Codex windows include a duration. Headroom fallback resets use epochs.
export type CodexUsageWindow = {
  label: string;
  usedPercent: number | null;
  resetsAt: string | number | null;
  windowSeconds?: number | null;
};

export type CodexUsageSummary = {
  planType: string | null;
  windows: CodexUsageWindow[];
};

export type ProviderPlanWindow = {
  label: string;
  remainingPercent: number | null;
  resetsAt: number | null;
};

export type ProviderPlanWindowGroup = {
  id: PlanProviderId;
  label: string;
  windows: ProviderPlanWindow[];
  unavailableReason: string | null;
};

const PROVIDER_ORDER: PlanProviderId[] = [
  "claude",
  "codex",
  "kimi",
  "glm",
  "grok",
];
const PROVIDER_LABELS: Record<PlanProviderId, string> = {
  claude: "Claude",
  codex: "Codex",
  kimi: "Kimi",
  glm: "GLM",
  grok: "Grok",
};
const CODEX_PLAN_LABELS: Record<string, string> = {
  pro: "Pro",
  plus: "Plus",
  business: "Business",
  team: "Team",
  free: "Free",
  enterprise: "Enterprise",
};

export function formatCodexPlanLabel(
  planType: string | null | undefined,
): string {
  const value = planType?.trim();
  if (!value) return "Codex";
  const label =
    CODEX_PLAN_LABELS[value.toLowerCase()] ??
    `${value.slice(0, 1).toUpperCase()}${value.slice(1)}`;
  return `Codex · ${label}`;
}

export function planProviderConnections(
  connections: ConnectGateConnections,
  auth: AuthStatus | null,
  anthropic: AnthropicCredentialStatus | null,
): PlanProviderConnections {
  return {
    claude:
      connections.claude ||
      connections.anthropic ||
      Boolean(auth && "anthropicApiKey" in auth) ||
      anthropic?.configured === true,
    codex:
      connections.codex ||
      connections.openai ||
      Boolean(
        auth &&
          (("chatgpt" in auth && auth.chatgpt.accessTokenValid) ||
            "apiKey" in auth),
      ),
    kimi: connections.kimi || connections.moonshot,
    glm: connections.zai,
    grok: connections.grok,
  };
}

export function selectPlanProviders(
  connections: PlanProviderConnections,
) {
  return PROVIDER_ORDER.filter((provider) => connections[provider]);
}

export function buildProviderPlanWindowGroups({
  connections,
  claudeWindows,
  claudeUnavailableReason = null,
  codexWindows,
  codexPlanType = null,
  normalizedWindows,
  nowMs = Date.now(),
}: {
  connections: PlanProviderConnections;
  claudeWindows: ClaudeLocalUsageWindow[];
  claudeUnavailableReason?: string | null;
  codexWindows: CodexUsageWindow[];
  codexPlanType?: string | null;
  normalizedWindows: UsageWindow[];
  nowMs?: number;
}): ProviderPlanWindowGroup[] {
  return selectPlanProviders(connections).map((provider) => {
    const windows = windowsForProvider(
      provider,
      claudeWindows,
      codexWindows,
      normalizedWindows,
      nowMs,
    );
    const unavailableReason =
      provider === "claude" &&
      windows.every((window) => window.remainingPercent === null)
        ? claudeUnavailableReason?.trim() || null
        : null;
    return {
      id: provider,
      label:
        provider === "codex"
          ? formatCodexPlanLabel(codexPlanType)
          : PROVIDER_LABELS[provider],
      windows,
      unavailableReason,
    };
  });
}

function windowsForProvider(
  provider: PlanProviderId,
  claudeWindows: ClaudeLocalUsageWindow[],
  codexWindows: CodexUsageWindow[],
  normalizedWindows: UsageWindow[],
  nowMs: number,
) {
  if (provider === "claude") {
    const byLabel = new Map(claudeWindows.map((window) => [window.label, window]));
    return ["5h", "7d"].map((label) => {
      const window = byLabel.get(label);
      return completeWindow(
        label,
        window?.usedPercent == null ? null : 100 - window.usedPercent,
        window?.resetsAt ?? null,
        nowMs,
      );
    });
  }

  if (provider === "codex") {
    if (codexWindows.length === 0) {
      return [completeWindow("", null, null, nowMs)];
    }
    return [...codexWindows]
      .sort((left, right) => {
        if (left.windowSeconds == null) {
          return right.windowSeconds == null ? 0 : 1;
        }
        if (right.windowSeconds == null) return -1;
        return left.windowSeconds - right.windowSeconds;
      })
      .map((window) =>
        completeWindow(
          window.label,
          window.usedPercent == null ? null : 100 - window.usedPercent,
          window.resetsAt ?? null,
          nowMs,
        ),
      );
  }

  const storageProvider =
    provider === "kimi"
      ? "moonshot"
      : provider === "glm"
        ? "zai"
        : "xai";
  const latestByKind = new Map<string, UsageWindow>();
  for (const window of normalizedWindows) {
    if (window.provider !== storageProvider) continue;
    const previous = latestByKind.get(window.windowKind);
    if (!previous || Date.parse(window.capturedAt) > Date.parse(previous.capturedAt)) {
      latestByKind.set(window.windowKind, window);
    }
  }
  const windows = [...latestByKind.values()].map((window) =>
    completeWindow(
      window.windowKind,
      remainingPercent(window),
      window.resetsAt,
      nowMs,
    ),
  );
  return windows.length > 0
    ? windows
    : [{ label: "", remainingPercent: null, resetsAt: null }];
}

function remainingPercent(window: UsageWindow) {
  if (window.limitValue == null || window.limitValue <= 0) return null;
  if (window.remaining != null) {
    return (window.remaining / window.limitValue) * 100;
  }
  if (window.used != null) {
    return 100 - (window.used / window.limitValue) * 100;
  }
  return null;
}

function completeWindow(
  label: string,
  remainingPercent: number | null,
  reset: string | number | null,
  nowMs: number,
): ProviderPlanWindow {
  const resetsAt = resetEpochSeconds(reset);
  if (
    remainingPercent == null ||
    !Number.isFinite(remainingPercent) ||
    resetsAt == null ||
    resetsAt * 1000 <= nowMs
  ) {
    return { label, remainingPercent: null, resetsAt: null };
  }
  return {
    label,
    remainingPercent: Math.max(0, Math.min(100, Math.round(remainingPercent))),
    resetsAt,
  };
}

function resetEpochSeconds(reset: string | number | null) {
  if (typeof reset === "number") {
    return Number.isFinite(reset) ? reset : null;
  }
  if (!reset) return null;
  const timestamp = Date.parse(reset);
  return Number.isFinite(timestamp) ? Math.floor(timestamp / 1000) : null;
}
