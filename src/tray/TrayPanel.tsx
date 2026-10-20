import React from "react";
import darkIcon from "../../src-tauri/app-icon.svg";
import lightIcon from "../../src-tauri/app-icon-light.svg";
import { getLocaleDefinition } from "../i18n/config";
import { useLocale, useT } from "../i18n/runtime";
import { formatTrayDollars, formatTrayPercent, formatTrayReset, remainingQuota, trayIsFresh, trayWindows, type TrayAction, type TraySnapshot, type TrayWindow } from "./model";

function Icon({ children }: { children: React.ReactNode }) {
  return <svg viewBox="0 0 24 24" aria-hidden="true">{children}</svg>;
}

function Quota({ window, fresh, loading, now }: { window: TrayWindow; fresh: boolean; loading: boolean; now: number }) {
  const t = useT();
  const remaining = remainingQuota(window, fresh, now);
  const label = window.label === "5h" ? t("Session · {window}", { window: window.label })
    : window.label === "7d" ? t("Weekly · {window}", { window: window.label }) : window.label;
  const level = remaining == null ? "unavailable" : remaining < 10 ? "danger" : remaining <= 30 ? "warn" : "";
  return <div className={`quota ${level}`}>
    <div className="quota-top"><span className="window">{label}</span><span className="value">
      {remaining == null ? loading ? "—" : t("Unavailable") : t("{percent}% left", { percent: formatTrayPercent(remaining) })}
    </span></div>
    <div className="bar" role={remaining == null ? undefined : "progressbar"}
      aria-label={`${window.provider} ${label}`} aria-valuemin={0} aria-valuemax={100}
      aria-valuenow={remaining ?? undefined} aria-valuetext={remaining == null ? undefined : t("{percent}% left", { percent: formatTrayPercent(remaining) })}>
      {remaining != null && <div className="fill" style={{ width: `${remaining}%` }} />}
    </div>
    <div className="reset">{remaining == null ? loading ? t("Loading usage…") : t("No recent data")
      : t("Resets in {time}", { time: formatTrayReset(window.reset, now) })}</div>
  </div>;
}

export function TrayPanel({ snapshot, now, dark, error, busy, onAction }: {
  snapshot: TraySnapshot; now: number; dark: boolean; error: boolean; busy: boolean;
  onAction: (action: TrayAction) => void;
}) {
  const t = useT();
  const locale = getLocaleDefinition(useLocale()).intlLocale;
  const fresh = !error && trayIsFresh(snapshot, now);
  const loading = !error && !snapshot.checked_at;
  const checked = snapshot.checked_at && Number.isFinite(Date.parse(snapshot.checked_at))
    ? new Intl.DateTimeFormat(locale, { hour: "2-digit", minute: "2-digit" }).format(new Date(snapshot.checked_at)) : null;
  return <article className={`panel ${fresh ? "" : "empty"}`} aria-label={t("Usage and savings")}>
    <header className="panel-header">
      <div className="brand"><img src={dark ? darkIcon : lightIcon} alt="" data-theme={dark ? "dark" : "light"} />Halv</div>
      <span className="updated" title={checked ? t("Last checked: {time}", { time: checked }) : undefined}>
        <Icon><path d="M20 10a8 8 0 0 0-14-5L3 8m0-5v5h5M4 14a8 8 0 0 0 14 5l3-3m0 5v-5h-5" /></Icon>
        {loading ? t("Loading usage…") : fresh && checked ? checked : t("Unavailable")}
      </span>
    </header>
    <button className="savings" disabled={busy} onClick={() => onAction("tray.usage")}>
      <span className="savings-label">{t("Saved at API prices")}<Icon><path d="m9 5 7 7-7 7" /></Icon></span>
      <span className="savings-grid"><span className="metric"><span className="number">{formatTrayDollars(fresh ? snapshot.today_usd : null, locale)}</span><span className="caption">{t("Today")}</span></span>
        <span className="metric"><span className="number">{formatTrayDollars(fresh ? snapshot.total_usd : null, locale)}</span><span className="caption">{t("All time")}</span></span></span>
    </button>
    <div className="section-title"><span>{t("PLAN WINDOWS")}</span><span>{t("Remaining capacity")}</span></div>
    <div>{(["Claude", "Codex"] as const).map(provider => <section className="provider" key={provider} aria-label={provider}>
      <div className="provider-name"><span className={`provider-symbol ${provider.toLowerCase()}`}><Icon>{provider === "Claude"
        ? <path d="m12 2 .3 6.8L17 4l-3 6 7-1-6.2 3L21 16l-7-1 2 7-4-6-4 6 2-7-7 1 6-4-7-3 7 1-3-6 5.7 4.8Z" />
        : <path d="m8 6-6 6 6 6m8-12 6 6-6 6M14 3l-4 18" />}</Icon></span>{provider}</div>
      {trayWindows(snapshot, provider).map((window, index) => <Quota key={`${provider}-${index}`} window={window} fresh={fresh} loading={loading} now={now} />)}
    </section>)}</div>
    {error && <div className="tray-error" role="status">{t("Unavailable")}</div>}
    <div className="actions">
      <button className="action primary" disabled={busy} onClick={() => onAction("tray.new-chat")}><Icon><path d="M12 5v14M5 12h14" /></Icon>{t("New chat")}</button>
      <button className="action" disabled={busy} onClick={() => onAction("tray.show")}><Icon><rect x="3" y="3" width="18" height="18" rx="3" /><path d="M9 3v18" /></Icon>{t("Show Halv")}</button>
    </div>
    <footer className="bottom">
      <button disabled={busy} onClick={() => onAction("tray.quit")}>{t("Quit Halv")}</button>
      <button disabled={busy} onClick={() => onAction("tray.quit-stop-agents")}>{t("Quit and Stop Agents")}</button>
    </footer>
  </article>;
}
