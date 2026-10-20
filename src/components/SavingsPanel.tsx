import { useEffect, useState, useSyncExternalStore } from "react";
import {
  formatCompactTokens,
  formatPlanResetDuration,
  formatPlanStretch,
  formatSignedWholePercent,
  formatUsd,
} from "../format";
import {
  formatDateTime,
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
  getUsageSectionVisibility,
  summarizeSavingsByLane,
  summarizeUsageByLane,
  type BillingUsageSummary,
  type ProviderUsageRow,
} from "../usageByLane";
import { buildSavingsSparkline } from "../savingsSparkline";
import { buildReceiptData, type ReceiptData } from "../receiptCard";
import { calculatePlanStretchPercent } from "../savingsTotals";
import { deriveSavingsMeasurement } from "../savingsMeasurement";
import type {
  PlanProviderConnections,
  ProviderPlanWindowGroup,
} from "../providerPlanWindows";
import type {
  DailyTokenActivity,
  AccountAuthStatus,
  LaneUsage,
  SavingsTotals,
  SidecarStats,
} from "../types";
import {
  agentSessionStore,
  liveSavingsMeasurementState,
} from "../agentSessions";
import { translateSavingsMeasurementNote } from "./SessionSavingsMeter";
import { TokenActivityGrid } from "./TokenActivityGrid";
import { ReceiptShareDialog } from "./ReceiptShareDialog";
import { usePlanWindowGroups } from "../usePlanWindowGroups";

const STORAGE_KEY = "halv.savingsPanel";

type SavingsPanelProps = {
  totals: SavingsTotals | null;
  weeklyTokensSaved: number | null;
  weeklyRecentSavings?: number[] | null;
  /** Dollars saved in the last 7 days (ADR 0015). */
  weeklyDollarsUsd?: number | null;
  weeklyRecentDollars?: number[] | null;
  /**
   * The JEV routing part of the last 7 days (ADR 0016). The weekly dollars
   * already hold its low estimate.
   */
  routing?: RoutingSavings;
  stats: SidecarStats | null;
  apiKeyConnected: boolean;
  dailyTokenActivity: DailyTokenActivity[];
  planProviderConnections: PlanProviderConnections;
  account?: AccountAuthStatus | null;
  onSignIn?: () => void;
  variant?: "sidebar" | "drawer";
};

export function SavingsPanel({
  totals,
  weeklyTokensSaved,
  weeklyDollarsUsd = null,
  weeklyRecentDollars = null,
  routing = ROUTING_NOT_ESTIMATED,
  stats,
  apiKeyConnected,
  dailyTokenActivity,
  planProviderConnections,
  account = null,
  onSignIn = () => undefined,
  variant = "sidebar",
}: SavingsPanelProps) {
  const t = useT();
  const [collapsed, setCollapsed] = useState(() => {
    try {
      return window.localStorage.getItem(STORAGE_KEY) === "collapsed";
    } catch {
      return false;
    }
  });
  const [userToggled, setUserToggled] = useState(false);
  const [activityUnit, setActivityUnit] = useState<"dollars" | "tokens">(
    "dollars",
  );
  const {
    groups: providerGroups,
    loadFailed: savingsDataLoadFailed,
    refresh: refreshPlanWindows,
  } = usePlanWindowGroups(planProviderConnections, stats);
  const [receiptData, setReceiptData] = useState<ReceiptData | null>(null);
  const agentSessionsState = useSyncExternalStore(
    agentSessionStore.subscribe,
    agentSessionStore.getSnapshot,
    agentSessionStore.getSnapshot,
  );
  const liveMeasurement = liveSavingsMeasurementState(
    agentSessionsState.entries.map((entry) => entry.session),
  );
  const weekMeasurement = deriveSavingsMeasurement({
    tokensSaved:
      liveMeasurement.active && liveMeasurement.unmeasured
        ? null
        : weeklyTokensSaved,
    savingsSource: liveMeasurement.active
      ? liveMeasurement.state
      : weeklyTokensSaved === null
        ? null
        : "measured",
  });
  // The input total is local, so the ratio must use local savings too.
  const planStretchPercent = calculatePlanStretchPercent(
    totals,
    stats?.inputTokens,
  );

  const weekTokens =
    weekMeasurement.tokens === null
      ? "\u2014"
      : formatCompactTokens(weekMeasurement.tokens);
  const noSavingsYet =
    !liveMeasurement.active &&
    weeklyTokensSaved === 0 &&
    (totals?.tokensSaved ?? 0) === 0;
  const measuredZero =
    liveMeasurement.active &&
    weekMeasurement.state === "measured" &&
    weekMeasurement.tokens === 0;
  const estimatedZero =
    liveMeasurement.active &&
    weekMeasurement.state === "estimated" &&
    weekMeasurement.tokens === 0;
  const notMeasured =
    liveMeasurement.active && weekMeasurement.state === "unmeasured";
  const effectiveCollapsed = noSavingsYet && !userToggled ? true : collapsed;
  const drawer = variant === "drawer";
  const contentCollapsed = drawer ? false : effectiveCollapsed;
  const collapsedCaption = notMeasured
    ? t("Not measured")
    : measuredZero
      ? t("Measured: 0")
      : estimatedZero
        ? t("Estimated: 0")
        : noSavingsYet
          ? t("No savings yet")
          : t("{amount} saved", {
              amount: includesRoutingEstimate(routing)
                ? t("\u2248 {amount}", {
                    amount: formatMoney(weeklyDollarsUsd ?? 0),
                  })
                : formatMoney(weeklyDollarsUsd ?? 0),
            });
  // The all-time dollars hold the low routing estimate of every settled chain.
  const allTimeApproximate = (totals?.routing?.chains ?? 0) > 0;
  const allTimeAmount = totals
    ? allTimeApproximate
      ? t("\u2248 {amount}", { amount: formatMoney(totals.dollarsUsd ?? 0) })
      : formatMoney(totals.dollarsUsd ?? 0)
    : null;
  const collapsedAllTime = allTimeAmount
    ? t("{amount} all-time", { amount: allTimeAmount })
    : null;
  const liveMeasurementNote = translateSavingsMeasurementNote(
    liveMeasurement.note,
    t,
  );
  const savedDays = dailyTokenActivity.map((day) => ({
    date: day.date,
    value: Math.max(
      0,
      activityUnit === "dollars" ? (day.dollarsUsd ?? 0) : day.tokensSaved,
    ),
  }));
  const weekDollars =
    weekMeasurement.state === "unmeasured" && liveMeasurement.active
      ? null
      : (weeklyDollarsUsd ?? 0);
  const approximate = includesRoutingEstimate(routing);
  // The weekly dollars hold the low routing estimate. "By source" splits it.
  const contextDollars =
    weekDollars === null ? null : weekDollars - countedRoutingUsd(routing);
  const weekAmount =
    weekDollars === null
      ? "\u2014"
      : approximate
        ? t("\u2248 {amount}", { amount: formatMoney(weekDollars) })
        : formatMoney(weekDollars);
  const dollarsSince = totals?.dollarsSince
    ? formatDateTime(new Date(totals.dollarsSince), { dateStyle: "medium" })
    : null;
  const legacyTokens = totals?.legacyTokensSaved ?? 0;
  const knownCollapsedWindow = providerGroups
    .flatMap((group) =>
      group.windows.map((window) => ({ group, window })),
    )
    .find(({ window }) => window.remainingPercent !== null);
  const collapsedWindow =
    knownCollapsedWindow ?? (providerGroups[0]?.windows[0]
      ? { group: providerGroups[0], window: providerGroups[0].windows[0] }
      : null);

  useEffect(() => {
    if (noSavingsYet && !userToggled) {
      return;
    }
    try {
      window.localStorage.setItem(
        STORAGE_KEY,
        effectiveCollapsed ? "collapsed" : "expanded",
      );
    } catch {
      // Local panel state is optional.
    }
  }, [effectiveCollapsed, noSavingsYet, userToggled]);

  return (
    <section
      className={
        drawer
          ? "savings-panel-drawer text-content-primary"
          : effectiveCollapsed
          ? "text-content-primary"
          : "rounded-xl border border-surface-border/40 bg-surface-sunken p-3 text-content-primary shadow-xl shadow-[rgb(var(--shadow-color)/0.18)] dark:border-surface-border/50 dark:bg-surface-sunken dark:text-content-primary"
      }
    >
      {drawer ? (
        <div className="mb-3 flex items-center justify-between gap-3 font-mono text-[10px] text-content-muted">
          <span>{t("last 7 days")}</span>
          {(weeklyTokensSaved ?? 0) > 0 ? (
            <button
              className="rounded-md border border-surface-border/30 px-2 py-1 text-content-secondary transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
              onClick={() =>
                setReceiptData(
                  buildReceiptData({
                    dailyTokenActivity,
                    usageByLane: stats?.usageByLane ?? [],
                    weeklyTokensSaved,
                    weeklyDollarsUsd,
                    totals,
                    account,
                    now: new Date(),
                  }),
                )
              }
              type="button"
            >
              {t("Share")}
            </button>
          ) : null}
        </div>
      ) : (
        <button
          aria-controls="live-savings-details"
          aria-expanded={!effectiveCollapsed}
          className={`flex w-full items-center justify-between gap-3 text-left focus:outline-none focus:ring-2 focus:ring-accent/35 ${
            effectiveCollapsed ? "rounded-md px-1 py-1" : ""
          }`}
          onClick={() => {
            setUserToggled(true);
            setCollapsed(!effectiveCollapsed);
          }}
          type="button"
        >
        {effectiveCollapsed ? (
          <>
            <span className="min-w-0 font-mono text-[11px] text-content-muted">
              <span className="block truncate">
                <span className="font-semibold text-accent">
                  {collapsedCaption}
                </span>
                {collapsedAllTime ? ` · ${collapsedAllTime}` : ""}
              </span>
              {collapsedWindow ? (
                <span className="mt-0.5 block truncate text-[10px]">
                  {collapsedWindow.window.remainingPercent == null
                    ? t("{provider} {window} —", {
                        provider: collapsedWindow.group.label,
                        window: collapsedWindow.window.label,
                      })
                    : t("{provider} {window} {percent}% left · resets {reset}", {
                        provider: collapsedWindow.group.label,
                        window: collapsedWindow.window.label,
                        percent: formatNumber(
                          collapsedWindow.window.remainingPercent,
                        ),
                        reset: formatResetCountdown(
                          collapsedWindow.window.resetsAt,
                          t,
                        ),
                      })}
                </span>
              ) : null}
            </span>
            <span className="shrink-0 font-mono text-[10px] text-content-muted" aria-hidden>
              {"\u25be"}
            </span>
          </>
        ) : (
          <>
            <span className="flex min-w-0 items-center gap-2 font-mono text-[10px] uppercase tracking-[0.18em] text-accent dark:text-accent">
              <span aria-hidden>{"\u25b4"}</span>
              <span className="truncate">{t("LIVE SAVINGS")}</span>
            </span>
            <span className="shrink-0 font-mono text-[10px] tracking-[0.14em] text-content-muted dark:text-content-muted">
              {t("last 7 days")}
            </span>
          </>
        )}
        </button>
      )}

      {savingsDataLoadFailed ? (
        <div
          className={`${contentCollapsed ? "mt-1" : "mt-3"} flex items-center gap-2 rounded-lg border border-danger/25 bg-danger/10 px-3 py-2 text-xs text-danger`}
          role="alert"
        >
          <span className="min-w-0 flex-1">
            {t("Could not refresh savings data.")}
          </span>
          <button
            className="shrink-0 rounded-md border border-danger/30 px-2 py-1 font-medium hover:bg-danger/10"
            onClick={refreshPlanWindows}
            type="button"
          >
            {t("Retry")}
          </button>
        </div>
      ) : null}

      {contentCollapsed ? null : (
        <div className={drawer ? "" : "mt-3"} id="live-savings-details">
          <div className="flex items-end justify-between gap-3">
            <div className="min-w-0">
              <div className="truncate text-3xl font-bold leading-none text-accent dark:text-accent">
                {weekAmount}
              </div>
              <div className="mt-1 font-mono text-[10px] uppercase tracking-[0.16em] text-content-muted dark:text-content-muted">
                {t("saved at API prices")}
              </div>
              <div className="mt-1 font-mono text-[10px] text-content-muted dark:text-content-muted">
                {t("{tokens} tokens kept out of context", { tokens: weekTokens })}
              </div>
              {notMeasured || liveMeasurementNote ? (
                <div className="mt-1 font-mono text-[10px] text-content-muted dark:text-content-muted">
                  {liveMeasurementNote ?? t("Not measured")}
                </div>
              ) : null}
            </div>
            <SavingsSparkline
              values={(weeklyRecentDollars ?? []).filter((value) => value > 0)}
            />
          </div>

          <div className="my-3 h-px bg-surface-border/30 dark:bg-surface-border/30" />

          <div className="mb-2 font-mono text-[10px] uppercase tracking-[0.18em] text-content-muted dark:text-content-muted">
            {t("By source")}
          </div>
          <div className="space-y-2">
            <MetricRow
              label={t("Context reduction")}
              value={
                contextDollars === null ? "\u2014" : formatMoney(contextDollars)
              }
            />
            <MetricRow
              label={t("Routing")}
              value={
                routing.state === "estimated"
                  ? t("\u2248 {amount} (range {low} to {high})", {
                      amount: formatMoney(routing.netUsd),
                      low: formatMoney(routing.lowUsd),
                      high: formatMoney(routing.highUsd),
                    })
                  : t("Routing savings not estimated yet")
              }
            />
          </div>

          <div className="my-3 h-px bg-surface-border/30 dark:bg-surface-border/30" />

          <div className="space-y-1">
            <MetricRow
              label={t("All time")}
              value={
                allTimeAmount
                  ? t("{amount} all-time", { amount: allTimeAmount })
                  : "\u2014"
              }
            />
            {dollarsSince ? (
              <div className="text-right font-mono text-[10px] text-content-muted">
                {t("since {date}", { date: dollarsSince })}
              </div>
            ) : null}
            {dollarsSince && legacyTokens > 0 ? (
              <div className="text-right font-mono text-[10px] text-content-muted">
                {t("Before {date}: {tokens} tokens saved", {
                  date: dollarsSince,
                  tokens: formatCompactTokens(legacyTokens),
                })}
              </div>
            ) : null}
          </div>

          <div className="my-3 h-px bg-surface-border/30 dark:bg-surface-border/30" />

          <TokenActivityGrid
            controls={
              <span
                aria-label={t("Unit")}
                className="flex overflow-hidden rounded border border-surface-border/30 font-mono text-[10px]"
                role="group"
              >
                {(["dollars", "tokens"] as const).map((unit) => (
                  <button
                    aria-pressed={activityUnit === unit}
                    className={`px-1.5 py-0.5 ${
                      activityUnit === unit
                        ? "bg-surface-raised text-content-primary"
                        : "text-content-muted hover:text-content-secondary"
                    }`}
                    key={unit}
                    onClick={() => setActivityUnit(unit)}
                    type="button"
                  >
                    {unit === "dollars" ? t("Dollars") : t("Tokens")}
                  </button>
                ))}
              </span>
            }
            days={savedDays}
            formatTotal={(total) =>
              t("{amount} in the last year", { amount: total })
            }
            formatValue={
              activityUnit === "dollars" ? formatMoney : formatCompactTokens
            }
            title={activityUnit === "dollars" ? t("Saved") : t("Tokens saved")}
          />

          <PlanWindowSection groups={providerGroups} />
          <UsageByLaneSections
            apiKeyConnected={apiKeyConnected}
            stats={stats}
          />
          <SavingsByAgentSection usageByLane={stats?.usageByLane ?? []} />
          <p className="mt-4 text-[11px] leading-4 text-content-muted">
            {t(
              "Savings use API prices for the model that ran. On a subscription plan, they are not a refund.",
            )}{" "}
            <button
              className="text-accent underline-offset-2 hover:underline focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40"
              onClick={openSavingsMethod}
              type="button"
            >
              {t("How Halv estimates savings")}
            </button>
          </p>
        </div>
      )}
      {receiptData ? (
        <ReceiptShareDialog
          account={account}
          data={receiptData}
          onClose={() => setReceiptData(null)}
          onSignIn={onSignIn}
        />
      ) : null}
    </section>
  );
}

function UsageByLaneSections({
  apiKeyConnected,
  stats,
}: {
  apiKeyConnected: boolean;
  stats: SidecarStats | null;
}) {
  const usage = summarizeUsageByLane(stats?.usageByLane ?? []);
  const unattributedRequests = stats?.unattributedUsage.requests ?? 0;
  const { showApi, showUnattributedFallback } = getUsageSectionVisibility(
    apiKeyConnected,
    unattributedRequests,
  );

  if (!showApi && !showUnattributedFallback) {
    return null;
  }

  return (
    <>
      {showApi ? (
        <ApiUsageSection
          statsAvailable={stats !== null}
          summary={usage.api}
          unattributedRequests={unattributedRequests}
        />
      ) : null}
      {showUnattributedFallback ? (
        <div className="mt-3">
          <div className="my-3 h-px bg-surface-border/30 dark:bg-surface-border/30" />
          <UnattributedUsageLine requests={unattributedRequests} />
        </div>
      ) : null}
    </>
  );
}

function SavingsByAgentSection({ usageByLane }: { usageByLane: LaneUsage[] }) {
  const t = useT();
  const rows = summarizeSavingsByLane(usageByLane);

  if (rows.length === 0) {
    return null;
  }

  return (
    <div className="mt-3">
      <div className="my-3 h-px bg-surface-border/30 dark:bg-surface-border/30" />
      <div className="mb-2 font-mono text-[10px] uppercase tracking-[0.18em] text-content-muted dark:text-content-muted">
        {t("By agent")}
      </div>
      <div className="space-y-3">
        {rows.map((row) => (
          <div key={row.lane}>
            <div className="flex items-center justify-between gap-3">
              <span className="truncate text-xs text-content-secondary dark:text-content-secondary">
                {row.label}
              </span>
              <span className="shrink-0 whitespace-nowrap text-right font-mono text-[10px] text-content-muted dark:text-content-muted">
                {t("{count} requests", {
                  count: formatNumber(row.requests),
                })}
                {" \u00b7 "}
                {t("{tokens} saved", {
                  tokens: formatCompactTokens(row.tokensSaved),
                })}
                {" \u00b7 "}
                {formatNumber(Math.round(row.percent))}%
              </span>
            </div>
            <div
              aria-hidden="true"
              className="mt-1 h-1 overflow-hidden rounded-full bg-surface-border/25 dark:bg-surface-border/25"
            >
              <span
                className="block h-full rounded-full bg-accent"
                style={{ width: `${row.percent}%` }}
              />
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

function ApiUsageSection({
  statsAvailable,
  summary,
  unattributedRequests,
}: {
  statsAvailable: boolean;
  summary: BillingUsageSummary;
  unattributedRequests: number;
}) {
  const t = useT();
  const observedProviders = summary.providers.filter((row) => row.requests > 0);
  const providers =
    observedProviders.length > 0
      ? observedProviders
      : [{ id: "openai", label: "OpenAI", requests: 0 }];

  return (
    <div className="mt-3">
      <div className="my-3 h-px bg-surface-border/30 dark:bg-surface-border/30" />
      <div className="mb-2 font-mono text-[10px] uppercase tracking-[0.18em] text-content-muted dark:text-content-muted">
        {t("API USAGE")}
      </div>
      <div className="space-y-2">
        <UsageMetricRows providers={providers} summary={summary} />
        <MetricRow
          label={t("Estimated input cost")}
          value={statsAvailable ? formatUsd(summary.costUsd) : "\u2014"}
        />
        <MetricRow label={t("Balance")} value={t("Provider console")} />
      </div>
      <UnattributedUsageLine requests={unattributedRequests} />
      <p className="mt-2 text-[10px] leading-relaxed text-content-muted dark:text-content-muted">
        {t(
          "Proxy-observed traffic. Exact OpenAI and Claude balances require provider Admin credentials.",
        )}
      </p>
    </div>
  );
}

function UsageMetricRows({
  providers,
  summary,
}: {
  providers: ProviderUsageRow[];
  summary: BillingUsageSummary;
}) {
  const t = useT();
  const tokens =
    summary.outputTokens === null
      ? t("{count} in", {
          count: formatCompactTokens(summary.inputTokens),
        })
      : t("{input} in · {output} out", {
          input: formatCompactTokens(summary.inputTokens),
          output: formatCompactTokens(summary.outputTokens),
        });

  return (
    <>
      {providers.map((row) => (
        <MetricRow
          key={row.id}
          label={t("{provider} requests", { provider: row.label })}
          value={formatNumber(row.requests)}
        />
      ))}
      <MetricRow label={t("Tokens")} value={tokens} />
    </>
  );
}

function UnattributedUsageLine({ requests }: { requests: number }) {
  const t = useT();
  if (requests <= 0) {
    return null;
  }

  return (
    <p className="mt-2 text-[10px] leading-relaxed text-content-muted dark:text-content-muted">
      {t("{count} requests not attributed to a session", {
        count: formatNumber(requests),
      })}
    </p>
  );
}

export function SavingsSparkline({
  compact = false,
  values,
}: {
  compact?: boolean;
  values: number[];
}) {
  const t = useT();
  // Up to 24 actual saving calls, no zero padding - empty slots read as
  // dead bars.
  const series = buildSavingsSparkline(values, 24, false);
  const exactValues = series.bars.map((bar) => Math.round(bar.value));
  const ariaLabel = series.hasSavings
    ? t("Tokens saved on recent replies, oldest to newest: {values}.", {
        values: exactValues.join(", "),
      })
    : t("No recent replies saved tokens yet.");

  return (
    <div
      className={
        compact
          ? "w-16 shrink-0"
          : "w-36 min-w-[4.5rem] shrink text-right"
      }
    >
      <div
        aria-label={ariaLabel}
        className={`flex items-end justify-end ${
          compact ? "h-5 gap-px" : "h-10 gap-0.5"
        }`}
        role="img"
        title={t(
          "Each bar is one recent reply that saved tokens, oldest to newest.",
        )}
      >
        {series.bars.map((bar, index) => (
            <span
              className={`${compact ? "w-px" : "w-1"} rounded-t ${
                bar.value > 0
                  ? "bg-accent shadow-[0_0_10px_rgb(var(--accent)/0.18)] dark:bg-accent dark:shadow-[0_0_12px_rgb(var(--accent)/0.18)]"
                  : "bg-surface-border/60 dark:bg-surface-border/60"
              }`}
              key={`${index}-${bar.value}`}
              style={{
                height: compact ? Math.max(2, bar.height / 2) : bar.height,
              }}
              title={t("Call {index} of {total}: {count} tokens saved", {
                index: index + 1,
                total: series.bars.length,
                count: formatNumber(Math.round(bar.value)),
              })}
            />
          ))}
      </div>
      {compact ? null : (
        <div className="mt-1 flex flex-wrap justify-end gap-x-1 font-mono text-[9px] uppercase tracking-[0.04em] text-content-muted">
          <span>{t("recent calls")}</span>
        </div>
      )}
    </div>
  );
}

function MetricRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-center justify-between gap-3">
      <span className="text-xs text-content-secondary dark:text-content-secondary">
        {label}
      </span>
      <span className="font-mono text-xs text-content-primary dark:text-content-primary">
        {value}
      </span>
    </div>
  );
}

function PlanWindowSection({ groups }: { groups: ProviderPlanWindowGroup[] }) {
  const t = useT();
  if (groups.length === 0) {
    return null;
  }

  return (
    <div className="mt-3">
      <div className="my-3 h-px bg-surface-border/30 dark:bg-surface-border/30" />
      <div className="mb-2 font-mono text-[10px] uppercase tracking-[0.18em] text-content-muted dark:text-content-muted">
        {t("PLAN WINDOWS")}
      </div>
      <div className="space-y-2">
        {groups.map((group) => (
          <div className="space-y-1" key={group.id}>
            <div className="min-w-0 truncate font-mono text-[10px] font-semibold uppercase tracking-[0.12em] text-content-secondary">
              {group.label}
            </div>
            {group.windows.map((window) => (
              // One line per window keeps the panel short with several
              // providers; the full reset text lives in the tooltip.
              <div
                className="grid grid-cols-[1.6rem_minmax(0,1fr)_auto] items-center gap-x-2"
                key={`${group.id}-${window.label || "unknown"}`}
                title={
                  window.remainingPercent == null
                    ? undefined
                    : t("{percent}% left · resets {reset}", {
                        percent: formatNumber(window.remainingPercent),
                        reset: formatResetCountdown(window.resetsAt, t),
                      })
                }
              >
                <span className="font-mono text-[10px] text-content-secondary dark:text-content-secondary">
                  {window.label || "\u2014"}
                </span>
                <span className="h-1.5 overflow-hidden rounded-full bg-surface-border/30 dark:bg-surface-border/30">
                  {window.remainingPercent == null ? null : (
                    <span
                      className={`block h-full rounded-full ${planWindowFillClass(window.remainingPercent)}`}
                      style={{ width: `${window.remainingPercent}%` }}
                    />
                  )}
                </span>
                <span className="whitespace-nowrap text-right font-mono text-[10px] text-content-primary dark:text-content-primary">
                  {window.remainingPercent == null
                    ? t("No activity in this window yet.")
                    : `${formatNumber(window.remainingPercent)}% \u00b7 ${formatResetCountdown(window.resetsAt, t)}`}
                </span>
              </div>
            ))}
            {group.unavailableReason ? (
              <p className="text-[10px] leading-relaxed text-content-muted dark:text-content-muted">
                {group.unavailableReason}
              </p>
            ) : null}
          </div>
        ))}
      </div>
    </div>
  );
}

function planWindowFillClass(remaining: number) {
  if (remaining < 10) {
    return "bg-danger";
  }
  if (remaining <= 30) {
    return "bg-warning";
  }
  return "bg-accent";
}

export function formatResetCountdown(resetsAt: number | null, t: Translate) {
  if (resetsAt == null) {
    return t("unknown");
  }
  const seconds = Math.max(0, resetsAt - Math.floor(Date.now() / 1000));
  if (seconds === 0) {
    return t("now");
  }
  const totalMinutes = Math.ceil(seconds / 60);
  return formatPlanResetDuration(totalMinutes);
}
