import { calculateInputSavings } from "./savings.ts";
import { deriveSavingsMeasurement } from "./savingsMeasurement.ts";
import type {
  SavingsMeasurement,
  TokenUsage,
  UiMessage,
} from "./types.ts";

export function savingsReplyLabel(measurement: SavingsMeasurement) {
  const { composition } = measurement;
  if ((composition?.headroom ?? 0) > 0) return "compressed";
  if ((composition?.rtk ?? 0) > 0 && (composition?.crux ?? 0) > 0) {
    return "RTK + crux (est.)";
  }
  if ((composition?.rtk ?? 0) > 0) return "RTK";
  if ((composition?.crux ?? 0) > 0) return "crux (est.)";
  return "saved";
}

export type ReplySavingsLabel = ReturnType<typeof savingsReplyLabel>;

/**
 * The per-reply savings line as data. The message bubble formats the
 * numbers in the app locale and translates the template, so the line
 * never stays in English.
 * - `dollars`: a saving with a dollar method (ADR 0015). Dollars lead.
 * - `compressed`, `tokens`: an older saving; tokens only.
 * - `usage`: a reply with usage but no savings measurement.
 */
export type ReplySavings =
  | { kind: "dollars"; amountUsd: number; tokens: number }
  | {
      kind: "compressed";
      originalInputTokens: number;
      compressedInputTokens: number;
      percent: number;
    }
  | { kind: "tokens"; label: ReplySavingsLabel; tokens: number }
  | { kind: "usage"; input: number; output: number; cached: number };

export function replySavings(
  message: UiMessage,
  usage: TokenUsage | null,
  sessionBacked: boolean,
): ReplySavings | null {
  if (message.role !== "assistant" || message.streaming) {
    return null;
  }
  const measurement = deriveSavingsMeasurement({
    tokensSaved: message.tokensSaved,
    savingsUsd: message.savingsUsd,
    savingsSource: message.savingsSource,
    pricingSource: message.pricingSource,
    rtkTokensSaved: message.rtkTokensSaved,
    rtkIncludedInTotal: message.rtkIncludedInTotal,
    cruxTokensSaved: message.cruxTokensSaved,
    cruxIncludedInTotal: message.cruxIncludedInTotal,
    cacheReadTokens: message.cachedTokens,
  });
  if (measurement.state !== "unmeasured" && measurement.tokens !== null) {
    const tokens = measurement.tokens;
    if (message.usdMethod && measurement.usd !== null) {
      return { kind: "dollars", amountUsd: measurement.usd, tokens };
    }
    const label = savingsReplyLabel(measurement);
    const sentTokens = message.inputTokens ?? usage?.inputTokens ?? null;
    const savings = calculateInputSavings(
      sentTokens,
      measurement.composition?.headroom,
    );
    if (label === "compressed" && savings) {
      return {
        kind: "compressed",
        originalInputTokens: savings.originalInputTokens,
        compressedInputTokens: savings.compressedInputTokens,
        percent: Math.round(savings.percent),
      };
    }
    return { kind: "tokens", label, tokens };
  }
  if (!sessionBacked || !usage) {
    return null;
  }
  return {
    kind: "usage",
    input: usage.inputTokens,
    output: usage.outputTokens,
    cached: usage.cachedTokens,
  };
}
