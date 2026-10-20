import { useCallback, useEffect, useMemo, useState } from "react";
import { ipc } from "./ipc.ts";
import {
  buildProviderPlanWindowGroups,
  type ClaudeLocalUsageWindow,
  type CodexUsageSummary,
  type PlanProviderConnections,
  type ProviderPlanWindowGroup,
} from "./providerPlanWindows";
import type { SidecarStats, UsageWindow } from "./types";
import { useWalletSnapshot } from "./components/AccountWallet";

export const CLAUDE_USAGE_REFRESH_INTERVAL_MS = 60_000;

export type PlanWindowGroupsState = {
  groups: ProviderPlanWindowGroup[];
  // A usage source failed on its last load.
  loadFailed: boolean;
  // A usage request is in flight.
  refreshing: boolean;
  refresh: () => void;
  // The wallet version that the live windows of each wallet provider
  // belong to. Null before the first load. A switch makes the windows old
  // until the next load.
  walletVersions: { claude: number | null; codex: number | null };
};

/**
 * Loads the plan windows of the connected providers and groups them per
 * provider. Codex and stored windows reload when the request count changes.
 * Claude windows reload on a timer. A wallet switch reloads both, because
 * the active account changes.
 */
export function usePlanWindowGroups(
  connections: PlanProviderConnections,
  stats: SidecarStats | null,
): PlanWindowGroupsState {
  const [normalizedWindows, setNormalizedWindows] = useState<UsageWindow[]>([]);
  const [claudeWindows, setClaudeWindows] = useState<ClaudeLocalUsageWindow[]>([]);
  const [claudeUnavailableReason, setClaudeUnavailableReason] = useState<
    string | null
  >(null);
  const [codexLiveUsage, setCodexLiveUsage] = useState<CodexUsageSummary>({
    planType: null,
    windows: [],
  });
  const [usageWindowsLoadFailed, setUsageWindowsLoadFailed] = useState(false);
  const [claudeWindowsLoadFailed, setClaudeWindowsLoadFailed] = useState(false);
  const [usagePending, setUsagePending] = useState(false);
  const [claudePending, setClaudePending] = useState(false);
  const [refreshRequest, setRefreshRequest] = useState(0);
  const walletVersion = useWalletSnapshot().version;
  const [claudeWalletVersion, setClaudeWalletVersion] = useState<number | null>(null);
  const [codexWalletVersion, setCodexWalletVersion] = useState<number | null>(null);

  useEffect(() => {
    let cancelled = false;
    const codexRequest = connections.codex
      ? ipc.agent.codexUsageWindows()
      : Promise.resolve<CodexUsageSummary>({ planType: null, windows: [] });
    setUsagePending(true);
    void Promise.allSettled([
      ipc.storage.listUsageWindows(),
      codexRequest,
    ]).then(([usageResult, codexResult]) => {
      if (cancelled) return;
      if (usageResult.status === "fulfilled") {
        setNormalizedWindows(usageResult.value);
      }
      if (codexResult.status === "fulfilled") {
        setCodexLiveUsage(codexResult.value);
        setCodexWalletVersion(walletVersion);
      }
      setUsageWindowsLoadFailed(
        usageResult.status === "rejected" ||
          (connections.codex && codexResult.status === "rejected"),
      );
      setUsagePending(false);
    });
    return () => {
      cancelled = true;
    };
  }, [connections.codex, refreshRequest, stats?.requestsTotal, walletVersion]);

  useEffect(() => {
    let cancelled = false;
    if (!connections.claude) {
      setClaudeWindows([]);
      setClaudeUnavailableReason(null);
      setClaudeWindowsLoadFailed(false);
      setClaudePending(false);
      return () => {
        cancelled = true;
      };
    }
    const refresh = () => {
      setClaudePending(true);
      void ipc.agent
        .claudeUsageWindows()
        .then((result) => {
          if (cancelled) return;
          setClaudeWindows(result.windows);
          setClaudeUnavailableReason(result.unavailableReason?.trim() || null);
          setClaudeWindowsLoadFailed(false);
          setClaudeWalletVersion(walletVersion);
        })
        .catch(() => {
          if (!cancelled) setClaudeWindowsLoadFailed(true);
        })
        .finally(() => {
          if (!cancelled) setClaudePending(false);
        });
    };
    refresh();
    const interval = window.setInterval(
      refresh,
      CLAUDE_USAGE_REFRESH_INTERVAL_MS,
    );
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [connections.claude, refreshRequest, walletVersion]);

  const { claude, codex, kimi, glm, grok } = connections;
  const groups = useMemo(
    () =>
      buildProviderPlanWindowGroups({
        connections: { claude, codex, kimi, glm, grok },
        claudeWindows,
        claudeUnavailableReason,
        codexWindows:
          codexLiveUsage.windows.length > 0
            ? codexLiveUsage.windows
            : stats?.planWindows ?? [],
        codexPlanType:
          codexLiveUsage.windows.length > 0 ? codexLiveUsage.planType : null,
        normalizedWindows,
      }),
    [
      claudeUnavailableReason,
      claudeWindows,
      claude,
      codex,
      codexLiveUsage,
      glm,
      grok,
      kimi,
      normalizedWindows,
      stats?.planWindows,
    ],
  );
  const refresh = useCallback(() => {
    setRefreshRequest((current) => current + 1);
  }, []);

  return {
    groups,
    loadFailed: usageWindowsLoadFailed || claudeWindowsLoadFailed,
    refreshing: usagePending || claudePending,
    refresh,
    walletVersions: { claude: claudeWalletVersion, codex: codexWalletVersion },
  };
}
