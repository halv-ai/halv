import {
  emptyAgentSessionUsageTotals,
  type AgentSessionUsage,
  type AgentSessionUsageTotals,
} from "./agentSessions.ts";
import type { SavingsPricingSource, UiMessage } from "./types.ts";

export function conversationAgentTurns(messages: UiMessage[]) {
  return messages.flatMap((message): AgentSessionUsage[] => {
    if (message.role !== "assistant" || message.error) {
      return [];
    }
    const inputTokens = message.usage?.inputTokens ?? message.inputTokens;
    const outputTokens = message.usage?.outputTokens ?? message.outputTokens;
    if (inputTokens == null && outputTokens == null) {
      return [];
    }
    return [
      {
        inputTokens: inputTokens ?? 0,
        outputTokens: outputTokens ?? 0,
        cacheReadTokens:
          message.usage?.cachedTokens ?? message.cachedTokens ?? 0,
        cacheCreationTokens: 0,
        savingsTokens: message.tokensSaved ?? null,
        ...(message.savingsUsd != null
          ? { savingsUsd: message.savingsUsd }
          : {}),
        ...(message.pricingSource
          ? { pricingSource: message.pricingSource }
          : {}),
        ...(message.rtkTokensSaved != null
          ? { rtkTokensSaved: message.rtkTokensSaved }
          : {}),
        ...(message.rtkIncludedInTotal !== undefined
          ? { rtkIncludedInTotal: message.rtkIncludedInTotal }
          : {}),
        ...(message.cruxTokensSaved != null
          ? { cruxTokensSaved: message.cruxTokensSaved }
          : {}),
        ...(message.cruxIncludedInTotal !== undefined
          ? { cruxIncludedInTotal: message.cruxIncludedInTotal }
          : {}),
        savingsSource:
          message.savingsSource ??
          (message.tokensSaved != null ? "measured" : null),
      },
    ];
  });
}

export function conversationAgentUsageTotals(
  turns: AgentSessionUsage[],
): AgentSessionUsageTotals {
  return turns.reduce<AgentSessionUsageTotals>((totals, turn) => {
    return {
      inputTokens: totals.inputTokens + turn.inputTokens,
      outputTokens: totals.outputTokens + turn.outputTokens,
      cacheReadTokens: totals.cacheReadTokens + turn.cacheReadTokens,
      cacheCreationTokens:
        totals.cacheCreationTokens + turn.cacheCreationTokens,
      totalCostUsd: totals.totalCostUsd + (turn.totalCostUsd ?? 0),
      savingsTokens: totals.savingsTokens + (turn.savingsTokens ?? 0),
      savingsUsd: totals.savingsUsd + (turn.savingsUsd ?? 0),
      dollarsUsd: totals.dollarsUsd + (turn.dollarsUsd ?? 0),
      pricingSource: mergePricingSource(
        totals.pricingSource,
        turn.pricingSource,
      ),
      rtkTokensSaved:
        totals.rtkTokensSaved + (turn.rtkTokensSaved ?? 0),
      rtkIncludedInTotal:
        totals.rtkIncludedInTotal || turn.rtkIncludedInTotal === true,
      cruxTokensSaved:
        totals.cruxTokensSaved + (turn.cruxTokensSaved ?? 0),
      cruxIncludedInTotal:
        totals.cruxIncludedInTotal || turn.cruxIncludedInTotal === true,
    };
  }, emptyAgentSessionUsageTotals());
}

function mergePricingSource(
  current: SavingsPricingSource | null,
  next: SavingsPricingSource | null | undefined,
) {
  if (!next || current === next) return current;
  if (!current) return next;
  if (current === "fallback" || next === "fallback") return "fallback";
  if (current === "model_table" || next === "model_table") {
    return "model_table";
  }
  return "headroom";
}
