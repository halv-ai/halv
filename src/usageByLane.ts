import type { LaneUsage } from "./types";

export type ProviderUsageRow = {
  id: string;
  label: string;
  requests: number;
};

export type BillingUsageSummary = {
  providers: ProviderUsageRow[];
  inputTokens: number;
  outputTokens: number | null;
  costUsd: number;
};

export type UsageByBilling = {
  plan: BillingUsageSummary;
  api: BillingUsageSummary;
};

export type UsageSectionVisibility = {
  showApi: boolean;
  showUnattributedFallback: boolean;
};

export type SavingsByLaneRow = {
  lane: string;
  label: string;
  requests: number;
  tokensSaved: number;
  percent: number;
};

export const PROVIDER_LABELS: Record<string, string> = {
  anthropic: "Claude",
  claude: "Claude",
  custom: "Custom",
  gemini: "Gemini",
  google: "Gemini",
  grok: "Grok",
  kimi: "Kimi",
  openai: "OpenAI",
  zai: "Z.ai",
};

export const LANE_LABELS: Record<string, string> = {
  claude_api_key: "Claude Code",
  claude_subscription: "Claude Code",
  codex_chatgpt: "Codex",
  custom: "Custom",
  gemini_api_key: "Gemini CLI",
  gemini_cli: "Gemini CLI",
  grok_api_key: "Grok",
  kimi_api_key: "Kimi",
  kimi_subscription: "Kimi",
  zai_glm: "Z.ai GLM",
};

function emptySummary(): BillingUsageSummary {
  return {
    providers: [],
    inputTokens: 0,
    outputTokens: null,
    costUsd: 0,
  };
}

function addLaneUsage(summary: BillingUsageSummary, usage: LaneUsage) {
  let provider = summary.providers.find((row) => row.id === usage.provider);
  if (!provider) {
    provider = {
      id: usage.provider,
      label: PROVIDER_LABELS[usage.provider] ?? usage.provider,
      requests: 0,
    };
    summary.providers.push(provider);
  }
  provider.requests += usage.requests;
  summary.inputTokens += usage.inputTokens;
  if (usage.outputTokens !== null) {
    summary.outputTokens = (summary.outputTokens ?? 0) + usage.outputTokens;
  }
  if (usage.billing === "api") {
    summary.costUsd += usage.costUsd ?? 0;
  }
}

export function getUsageSectionVisibility(
  apiKeyConnected: boolean,
  unattributedRequests: number,
): UsageSectionVisibility {
  return {
    showApi: apiKeyConnected,
    showUnattributedFallback: !apiKeyConnected && unattributedRequests > 0,
  };
}

export function summarizeUsageByLane(usageByLane: LaneUsage[]): UsageByBilling {
  const summaries = {
    plan: emptySummary(),
    api: emptySummary(),
  };

  for (const usage of usageByLane) {
    if (usage.billing === "plan" || usage.billing === "api") {
      addLaneUsage(summaries[usage.billing], usage);
    }
  }

  return summaries;
}

export function summarizeSavingsByLane(
  usageByLane: readonly LaneUsage[],
): SavingsByLaneRow[] {
  const rowsByLane = new Map<
    string,
    Omit<SavingsByLaneRow, "percent">
  >();

  for (const usage of usageByLane) {
    const row = rowsByLane.get(usage.lane) ?? {
      lane: usage.lane,
      label:
        LANE_LABELS[usage.lane] ??
        PROVIDER_LABELS[usage.provider] ??
        usage.lane,
      requests: 0,
      tokensSaved: 0,
    };
    row.requests += finiteNonnegative(usage.requests);
    row.tokensSaved += finiteNonnegative(usage.tokensSaved);
    rowsByLane.set(usage.lane, row);
  }

  const rows = [...rowsByLane.values()];
  const totalTokensSaved = rows.reduce(
    (total, row) => total + row.tokensSaved,
    0,
  );

  return rows
    .sort(
      (left, right) =>
        right.tokensSaved - left.tokensSaved ||
        left.label.localeCompare(right.label),
    )
    .map((row) => ({
      ...row,
      percent:
        totalTokensSaved === 0
          ? 0
          : (row.tokensSaved / totalTokensSaved) * 100,
    }));
}

function finiteNonnegative(value: number) {
  return Number.isFinite(value) ? Math.max(0, value) : 0;
}
