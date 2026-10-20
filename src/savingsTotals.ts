import type {
  AccountAuthStatus,
  SavingsTotals,
  SavingsTotalsPayload,
} from "./types";

export function mergeAllTimeSavings(
  local: SavingsTotalsPayload,
  account: AccountAuthStatus | null,
): SavingsTotals {
  const serverTokens = account?.entitlements?.savings.allTimeTokensSaved;
  const serverUsd = account?.entitlements?.savings.allTimeSavingsUsd;
  const serverDollars = account?.entitlements?.savings.allTimeV2Usd;
  const localDollars = local.dollarsUsd ?? 0;
  // A server without the dollar fields leaves the local dollars in place.
  const dollarsUsd =
    account?.signedIn === true &&
    typeof serverDollars === "number" &&
    Number.isFinite(serverDollars)
      ? Math.max(localDollars, serverDollars + (local.pendingDollarsUsd ?? 0))
      : localDollars;
  if (
    account?.signedIn !== true ||
    typeof serverTokens !== "number" ||
    !Number.isFinite(serverTokens) ||
    typeof serverUsd !== "number" ||
    !Number.isFinite(serverUsd)
  ) {
    return {
      ...local,
      dollarsUsd,
      localTokensSaved: local.tokensSaved,
      localSavingsUsd: local.savingsUsd,
      localDollarsUsd: localDollars,
    };
  }

  // The server already includes acknowledged local events. Add only pending
  // queue values so one event never appears in both parts of the total.
  return {
    ...local,
    tokensSaved: serverTokens + local.pendingTokensSaved,
    savingsUsd: serverUsd + local.pendingSavingsUsd,
    dollarsUsd,
    localTokensSaved: local.tokensSaved,
    localSavingsUsd: local.savingsUsd,
    localDollarsUsd: localDollars,
  };
}

export function calculatePlanStretchPercent(
  totals: Pick<SavingsTotals, "localTokensSaved"> | null,
  inputTokens: number | null | undefined,
): number | null {
  const localTokensSaved = totals?.localTokensSaved ?? 0;
  const localInputTokens = inputTokens ?? 0;
  return localTokensSaved > 0 && localInputTokens > 0
    ? (localTokensSaved / (localInputTokens + localTokensSaved)) * 100
    : null;
}
