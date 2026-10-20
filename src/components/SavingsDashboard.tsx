import {
  formatCompactNumber,
  formatCompactTokens,
  formatPercent,
  formatUsd,
  formatPlanStretch,
} from "../format";
import { formatDateTime, formatMoney, useT } from "../i18n/runtime";
import type {
  SavingsToday,
  SavingsTotals,
  SidecarStats,
  SidecarStatus,
} from "../types";

type SavingsDashboardProps = {
  stats: SidecarStats | null;
  totals: SavingsTotals | null;
  today: SavingsToday | null;
  sidecar: SidecarStatus | null;
};

export function SavingsDashboard({
  stats,
  totals,
  today,
  sidecar,
}: SavingsDashboardProps) {
  const t = useT();
  const tokensDetail = (tokens: number) =>
    t("{tokens} tokens kept out of context", {
      tokens: formatCompactTokens(tokens),
    });
  const engineVersion =
    sidecar?.state === "running" && sidecar.version
      ? sidecar.managedByHalv && sidecar.pinnedVersion
        ? t("{version} · pinned {pinnedVersion}", {
            version: sidecar.version,
            pinnedVersion: sidecar.pinnedVersion,
          })
        : sidecar.version
      : "\u2014";

  return (
    <div className="space-y-4">
      <div className="grid gap-3 sm:grid-cols-3">
        <Metric
          label={t("All-time saved")}
          value={totals ? formatMoney(totals.dollarsUsd ?? 0) : "\u2014"}
          detail={totals ? tokensDetail(totals.tokensSaved) : "\u2014"}
        />
        <Metric
          label={t("Today")}
          value={today ? formatMoney(today.dollarsUsd ?? 0) : "\u2014"}
          detail={today ? tokensDetail(today.tokensSaved) : "\u2014"}
        />
        <Metric
          label={t("This app run")}
          value={totals ? formatMoney(totals.sessionDollarsUsd ?? 0) : "\u2014"}
          detail={totals ? tokensDetail(totals.sessionTokensSaved) : "\u2014"}
        />
      </div>

      <SavingsMethod totals={totals} />

      <details className="rounded-md border border-surface-border/25 bg-surface-raised/25">
        <summary className="cursor-pointer px-3 py-2.5 text-sm font-medium text-content-secondary hover:text-content-primary">
          {t("Advanced diagnostics")}
        </summary>
        {stats ? (
          <div className="grid gap-3 border-t border-surface-border/20 p-3 sm:grid-cols-2">
            <Metric label={t("Proxy version")} value={engineVersion} />
            <Metric
              label={t("Port")}
              value={sidecar ? String(sidecar.port) : "\u2014"}
            />
            <Metric
              label={t("Requests · all time")}
              value={formatCompactNumber(stats.lifetimeRequests)}
              detail={t("{count} since the proxy started", {
                count: formatCompactNumber(stats.requestsTotal),
              })}
            />
            <Metric
              label={t("Savings percent · all time")}
              value={formatPercent(
                (stats.lifetimeTokensSaved ?? 0) > 0 && stats.inputTokens > 0
                  ? (((stats.lifetimeTokensSaved ?? 0) /
                      (stats.inputTokens + (stats.lifetimeTokensSaved ?? 0))) *
                      100)
                  : null,
              )}
              detail={t("saved vs lifetime engine input")}
            />
            <Metric
              label={t("Plan stretch · all time")}
              value={formatPlanStretch(
                (stats.lifetimeTokensSaved ?? 0) > 0 && stats.inputTokens > 0
                  ? (((stats.lifetimeTokensSaved ?? 0) /
                      (stats.inputTokens + (stats.lifetimeTokensSaved ?? 0))) *
                      100)
                  : null,
              )}
              detail={t("how much further the plan goes")}
            />
            <Metric
              label={t("Cache savings · all time")}
              value={formatUsd(stats.lifetimeCacheSavingsUsd)}
              detail={t("provider cache discount, not counted")}
            />
            <Metric
              label={t("Proxy input · all time")}
              value={t("{count} in", {
                count: formatCompactNumber(stats.inputTokens),
              })}
              detail={t("lifetime input through the engine")}
            />
            <Metric
              label={t("Engine session tokens")}
              value={formatCompactNumber(stats.sessionTokensSaved)}
              detail={t(
                "the engine's own durable session, not a Halv session",
              )}
            />
            <Metric
              label={t("Engine session USD")}
              value={formatUsd(stats.sessionCostSavedUsd)}
              detail={t("the engine's own durable session")}
            />
            <Metric
              label={t("Estimated output tokens avoided")}
              value={formatCompactNumber(stats.estimatedOutputTokensSaved)}
              detail={t(
                "counterfactual estimate · excluded from exact total",
              )}
            />
            <Metric
              label={t("MCP compression tokens")}
              value={formatCompactNumber(stats.mcpTokensSaved)}
              detail={t("machine-wide · excluded from Halv turn totals")}
            />
            <Metric
              label={t("Live WS unit tokens")}
              value={formatCompactNumber(stats.liveWsTokensSaved)}
              detail={t(
                "diagnostic overlap · excluded to prevent double-counting",
              )}
            />
          </div>
        ) : (
          <p className="border-t border-surface-border/20 p-3 text-sm text-content-muted">
            {t("Proxy stats unavailable until the proxy is running.")}
          </p>
        )}
      </details>
    </div>
  );
}

/** "How Halv estimates savings" (ADR 0015, design section 8.3). */
export function SavingsMethod({ totals }: { totals: SavingsTotals | null }) {
  const t = useT();
  const since = totals?.dollarsSince
    ? formatDateTime(new Date(totals.dollarsSince), { dateStyle: "medium" })
    : null;
  const pricesDate = totals?.priceVersion
    ? formatDateTime(new Date(`${totals.priceVersion}T12:00:00Z`), {
        dateStyle: "medium",
      })
    : null;
  return (
    <section
      aria-labelledby="savings-method-title"
      className="rounded-md border border-surface-border/25 bg-surface-raised/25 p-3"
      id="savings-method"
    >
      <h5
        className="text-sm font-medium text-content-primary"
        id="savings-method-title"
      >
        {t("How Halv estimates savings")}
      </h5>
      <ul className="mt-2 list-disc space-y-1.5 pl-4 text-xs leading-5 text-content-secondary">
        <li>
          {t(
            "Halv counts the dollars that the same work would cost at API list prices.",
          )}
        </li>
        <li>
          {t(
            "Context reduction: tokens that Halv keeps out of a request, at the price that the session paid per prompt token.",
          )}
        </li>
        <li>
          {t(
            "Routing: the cost of the task on your default model, minus the real cost on the routed model. Halv measures the real cost. It estimates the default cost from Halv benchmark tasks that ran on both models.",
          )}
        </li>
        <li>
          {t(
            "When the benchmark has too few runs for a model pair, Halv counts a route only if it still saves money when the routed model uses 4 times the tokens.",
          )}
        </li>
        <li>
          {t(
            "Halv shows the low end of the estimate. A failed or repeated attempt reduces the total.",
          )}
        </li>
        <li>
          {t(
            "Savings use API prices for the model that ran. On a subscription plan, they are not a refund.",
          )}
        </li>
      </ul>
      <p className="mt-2 font-mono text-[10px] text-content-muted">
        {pricesDate ? t("Prices as of {date}.", { date: pricesDate }) : null}
        {pricesDate && since ? " " : null}
        {since
          ? t("Dollars count savings since {date}. Earlier savings show as tokens.", {
              date: since,
            })
          : null}
        {totals?.routingCalibrationVersion ? " " : null}
        {totals?.routingCalibrationVersion
          ? t("Routing calibration {version}.", {
              version: totals.routingCalibrationVersion,
            })
          : null}
      </p>
    </section>
  );
}

function Metric({
  label,
  value,
  detail,
}: {
  label: string;
  value: string;
  detail?: string;
}) {
  return (
    <div className="rounded-md border border-surface-border/25 bg-surface-raised/55 p-3">
      <div className="font-mono text-[10px] uppercase tracking-[0.16em] text-content-muted">
        {label}
      </div>
      <div className="mt-1 font-mono text-base font-semibold text-content-primary">
        {value}
      </div>
      {detail ? (
        <div className="mt-1 font-mono text-xs text-content-muted">{detail}</div>
      ) : null}
    </div>
  );
}
