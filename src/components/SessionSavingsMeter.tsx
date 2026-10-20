import {
  ponytailBadgeLabel,
  sessionSavingsDisplay,
  type AgentSessionUsage,
  type AgentSessionUsageTotals,
} from "../agentSessions";
import { formatCompactTokens, formatUsd } from "../format";
import {
  formatMoney,
  formatNumber,
  useT,
  type Translate,
} from "../i18n/runtime";
import {
  countedRoutingUsd,
  includesRoutingEstimate,
  openSavingsMethod,
  ROUTING_NOT_ESTIMATED,
  type RoutingSavings,
} from "../savingsMethod";
import {
  deriveSavingsMeasurementBreakdown,
  type SavingsMeasurementBreakdown,
} from "../savingsMeasurement";
import { buildSavingsSparkline } from "../savingsSparkline";
import type { PonytailMode } from "../types";

type SessionSavingsMeterProps = {
  turns: AgentSessionUsage[];
  totals: AgentSessionUsageTotals;
  compact?: boolean;
  measurementNote?: string | null;
  unmeasured?: boolean;
  ponytailMode?: PonytailMode | null;
  conversationTotals?: {
    savingsTokens: number;
    cacheReadTokens: number;
    dollarsUsd?: number;
  } | null;
  /** True for a flat subscription lane (ADR 0015). */
  planBilling?: boolean;
  /** The JEV routing part (ADR 0016). The headline adds its low estimate. */
  routing?: RoutingSavings;
};

export function SessionSavingsMeter({
  turns,
  totals,
  compact = false,
  measurementNote = null,
  unmeasured = false,
  ponytailMode = null,
  conversationTotals = null,
  planBilling = false,
  routing = ROUTING_NOT_ESTIMATED,
}: SessionSavingsMeterProps) {
  const t = useT();
  const lastTurn = turns.at(-1) ?? null;
  const savedTokens = Math.max(totals.savingsTokens, conversationTotals?.savingsTokens ?? 0);
  const savedDollars = Math.max(
    totals.dollarsUsd ?? 0,
    conversationTotals?.dollarsUsd ?? 0,
  );
  const approximate = includesRoutingEstimate(routing);
  // The headline is context plus the low routing estimate; the Details rows
  // show each part.
  const amount = formatMoney(savedDollars + countedRoutingUsd(routing));
  const contextAmount = formatMoney(savedDollars);
  const includesEarlierSessions = savedTokens > totals.savingsTokens;
  const includesEstimatedTurn = turns.some((usage) =>
    (usage.savingsTokens ?? 0) > 0 && sessionSavingsDisplay(usage).label === "estimated",
  );
  const measurementInput = {
    tokensSaved: unmeasured ? null : savedTokens,
    savingsUsd: totals.savingsUsd,
    pricingSource: totals.pricingSource,
    rtkTokensSaved: totals.rtkTokensSaved,
    rtkIncludedInTotal: totals.rtkIncludedInTotal,
    cruxTokensSaved: totals.cruxTokensSaved,
    cruxIncludedInTotal: totals.cruxIncludedInTotal,
    cacheReadTokens: Math.max(totals.cacheReadTokens, conversationTotals?.cacheReadTokens ?? 0),
    // Earlier sessions have no component attribution in the persisted aggregate.
    ...(includesEarlierSessions ? { headroomTokensSaved: 0 } : {}),
    savingsSource: unmeasured
      ? "unmeasured"
      : includesEarlierSessions || includesEstimatedTurn
        ? "estimated"
        : (lastTurn?.savingsSource ?? "measured"),
  };
  // The headline uses the aggregate for its labeled period. A latest-turn
  // sample of zero must not erase savings already observed in that period.
  const savings = sessionSavingsDisplay({
    inputTokens: totals.inputTokens,
    outputTokens: totals.outputTokens,
    cacheReadTokens: totals.cacheReadTokens,
    cacheCreationTokens: totals.cacheCreationTokens,
    savingsTokens: unmeasured ? null : savedTokens,
    savingsUsd: totals.savingsUsd,
    pricingSource: totals.pricingSource,
    rtkTokensSaved: totals.rtkTokensSaved,
    rtkIncludedInTotal: totals.rtkIncludedInTotal,
    cruxTokensSaved: totals.cruxTokensSaved,
    cruxIncludedInTotal: totals.cruxIncludedInTotal,
    savingsSource: measurementInput.savingsSource,
  });
  const breakdown = deriveSavingsMeasurementBreakdown(measurementInput);
  const includedBreakdown = formatIncludedSavingsBreakdown(breakdown, t);
  const separateBreakdown = formatSeparateSavingsBreakdown(breakdown, t);
  const lastTurnSavings = sessionSavingsDisplay(lastTurn);
  // Bars mark positive savings updates only; message dots with zero
  // savings stay off the chart.
  const dollarTurns = turns.filter((usage) => (usage.dollarsUsd ?? 0) > 0);
  const sparkline = buildSavingsSparkline(
    dollarTurns.length > 0
      ? dollarTurns.map((usage) => usage.dollarsUsd ?? 0)
      : turns
          .filter((usage) => (usage.savingsTokens ?? 0) > 0)
          .map((usage) => sessionSavingsDisplay(usage).tokenCount),
  );
  const ponytailBadge = translatePonytailBadge(ponytailMode, t);

  return (
    <section
      aria-label={t("Session savings meter")}
      className={`agent-session-savings-meter${compact ? " agent-session-savings-meter-compact" : ""}`}
    >
      <div className="agent-session-savings-heading">
        <span>{t("LIVE SAVINGS")}</span>
        <span className="agent-session-savings-always">
          {measurementNote ??
            (savings.label === "measured"
              ? t("proxy measured")
              : savings.label === "estimated"
                ? t("usage estimate")
                : t("not measured"))}
          {ponytailBadge && !compact ? (
            <span className="agent-session-savings-chip">{ponytailBadge}</span>
          ) : null}
        </span>
      </div>
      <div className="agent-session-savings-content">
        <div className="agent-session-savings-group">
          <span className="agent-session-savings-label">{t("Last turn")}</span>
          <TokenTriplet
            cached={lastTurn?.cacheReadTokens ?? 0}
            input={lastTurn?.inputTokens ?? 0}
            output={lastTurn?.outputTokens ?? 0}
          />
        </div>
        <div className="agent-session-savings-group">
          <span className="agent-session-savings-label">
            {t("Session total")}
          </span>
          <TokenTriplet
            cached={totals.cacheReadTokens}
            input={totals.inputTokens}
            output={totals.outputTokens}
          />
        </div>
        <div className="agent-session-savings-result">
          {ponytailBadge && compact ? (
            <span className="agent-session-savings-chip">{ponytailBadge}</span>
          ) : null}
          {savings.label === "unmeasured" ? null : (
            <span
              className={`agent-session-savings-chip agent-session-savings-chip-${savings.label}`}
            >
              {savings.label === "measured" ? t("measured") : t("estimated")}
            </span>
          )}
          <strong>
            {savings.label === "unmeasured"
              ? t("Not measured")
              : conversationTotals
                ? approximate
                  ? t("\u2248 {amount} saved in this conversation", { amount })
                  : t("{amount} saved in this conversation", { amount })
                : approximate
                  ? t("\u2248 {amount} saved this session", { amount })
                  : t("{amount} saved this session", { amount })}
          </strong>
          {compact && measurementNote ? (
            <span className="agent-session-savings-compact-note">
              {measurementNote}
            </span>
          ) : null}
          <details className="agent-session-savings-details">
            <summary>{t("Details")}</summary>
            <div className="agent-session-savings-explanation">
              {savings.label !== "unmeasured" ? (
                <span className="agent-session-savings-source">
                  {savings.label === "measured"
                    ? t("Measured means Halv observed the token reduction.")
                    : t(
                        "Estimated means Halv calculated savings from available usage data.",
                      )}
                </span>
              ) : null}
              {savings.label !== "unmeasured" ? (
                <span className="agent-session-savings-source">
                  {t("{source}: {amount} ({tokens} tokens)", {
                    source: t("Context reduction"),
                    amount: contextAmount,
                    tokens: formatNumber(Math.round(savings.tokenCount)),
                  })}
                </span>
              ) : null}
              <span className="agent-session-savings-source">
                {routing.state === "estimated"
                  ? t("{source}: {amount}", {
                      source: t("Routing"),
                      amount: t("\u2248 {amount} (range {low} to {high})", {
                        amount: formatMoney(routing.netUsd),
                        low: formatMoney(routing.lowUsd),
                        high: formatMoney(routing.highUsd),
                      }),
                    })
                  : t("Routing savings not estimated yet")}
              </span>
              {includedBreakdown ? (
                <span className="agent-session-savings-source">
                  {t("Included in total: {details}.", {
                    details: includedBreakdown,
                  })}
                </span>
              ) : null}
              {separateBreakdown ? (
                <span className="agent-session-savings-source">
                  {t("Observed separately, not added: {details}.", {
                    details: separateBreakdown,
                  })}
                </span>
              ) : null}
              {lastTurnSavings.costUsd !== null ? (
                <span className="agent-session-savings-cost">
                  {t("{amount} last-turn cost", {
                    amount: formatUsd(lastTurnSavings.costUsd),
                  })}
                </span>
              ) : null}
              {totals.totalCostUsd > 0 ? (
                <span className="agent-session-savings-cost">
                  {t("{amount} session cost", {
                    amount: formatUsd(totals.totalCostUsd),
                  })}
                </span>
              ) : null}
              <span className="agent-session-savings-source">
                {t(
                  "Savings use API prices for the model that ran. On a subscription plan, they are not a refund.",
                )}
              </span>
              {planBilling ? (
                <span className="agent-session-savings-source">
                  {t(
                    "You pay a flat plan. This is what the same work costs at API prices.",
                  )}
                </span>
              ) : null}
              <button
                className="agent-session-savings-method-link"
                onClick={openSavingsMethod}
                type="button"
              >
                {t("How Halv estimates savings")}
              </button>
            </div>
          </details>
        </div>
        <SavingsSparkline
          hasSavings={sparkline.hasSavings}
          values={sparkline.bars}
        />
      </div>
    </section>
  );
}

function formatIncludedSavingsBreakdown(
  breakdown: SavingsMeasurementBreakdown,
  t: Translate,
) {
  const labels: string[] = [];
  const { included } = breakdown;
  if (included.headroom > 0) {
    labels.push(
      t("Headroom: {count}", {
        count: formatNumber(Math.round(included.headroom)),
      }),
    );
  }
  if (included.rtk > 0) {
    labels.push(
      t("RTK: {count}", {
        count: formatNumber(Math.round(included.rtk)),
      }),
    );
  }
  if (included.crux > 0) {
    labels.push(
      t("Crux estimate: {count}", {
        count: formatNumber(Math.round(included.crux)),
      }),
    );
  }
  if (included.other > 0) {
    labels.push(
      t("Other savings: {count}", {
        count: formatNumber(Math.round(included.other)),
      }),
    );
  }
  return labels.join(" · ");
}

function formatSeparateSavingsBreakdown(
  breakdown: SavingsMeasurementBreakdown,
  t: Translate,
) {
  const labels: string[] = [];
  const { separate } = breakdown;
  if (separate.rtk > 0) {
    labels.push(
      t("RTK: {count}", {
        count: formatNumber(Math.round(separate.rtk)),
      }),
    );
  }
  if (separate.crux > 0) {
    labels.push(
      t("Crux estimate: {count}", {
        count: formatNumber(Math.round(separate.crux)),
      }),
    );
  }
  if (separate.cacheRead > 0) {
    labels.push(
      t("Cache reuse: {count}", {
        count: formatNumber(Math.round(separate.cacheRead)),
      }),
    );
  }
  return labels.join(" · ");
}

export function translateSavingsMeasurementNote(
  note: string | null,
  t: Translate,
) {
  switch (note) {
    case null:
      return null;
    case "crux + RTK only · proxy off":
      return t("crux + RTK only · proxy off");
    case "crux + RTK · proxy on":
      return t("crux + RTK · proxy on");
    case "RTK hook not active":
      return t("RTK hook not active");
    case "crux + RTK (by instruction) · proxy on":
      return t("crux + RTK (by instruction) · proxy on");
    case "crux + RTK (by instruction) · proxy off":
      return t("crux + RTK (by instruction) · proxy off");
    case "crux only · proxy off":
      return t("crux only · proxy off");
  }
  return null;
}

function translatePonytailBadge(
  mode: PonytailMode | null | undefined,
  t: Translate,
) {
  switch (ponytailBadgeLabel(mode)) {
    case null:
      return null;
    case "ponytail lite":
      return t("ponytail lite");
    case "ponytail full":
      return t("ponytail full");
    case "ponytail ultra":
      return t("ponytail ultra");
  }
  return null;
}

function TokenTriplet({
  input,
  output,
  cached,
}: {
  input: number;
  output: number;
  cached: number;
}) {
  const t = useT();
  return (
    <span className="agent-session-token-triplet">
      <span>
        {t("{count} in", { count: formatCompactTokens(input) })}
      </span>
      <span>
        {t("{count} out", { count: formatCompactTokens(output) })}
      </span>
      <span>
        {t("{count} cached", { count: formatCompactTokens(cached) })}
      </span>
    </span>
  );
}

function SavingsSparkline({
  hasSavings,
  values,
}: {
  hasSavings: boolean;
  values: Array<{ value: number; height: number }>;
}) {
  const t = useT();
  const rounded = values.map((bar) => Math.round(bar.value));
  const ariaLabel =
    values.length === 0
      ? t("No session savings samples yet.")
      : t("Session savings samples, oldest to newest: {values}.", {
          values: rounded.join(", "),
        });

  return (
    <div
      aria-label={ariaLabel}
      className="agent-session-savings-sparkline"
      role="img"
    >
      {values.length === 0 ? (
        <span>{t("no data yet")}</span>
      ) : !hasSavings ? (
        <span>{t("0 saved")}</span>
      ) : (
        values.map((bar, index) => (
          <i
            className={
              bar.value > 0
                ? "agent-session-savings-bar agent-session-savings-bar-active"
                : "agent-session-savings-bar"
            }
            key={`${index}-${bar.value}`}
            style={{ height: bar.height }}
            title={t("{count} tokens", {
              count: formatNumber(Math.round(bar.value)),
            })}
          />
        ))
      )}
    </div>
  );
}
