import React from "react";
import ReactDOM from "react-dom/client";
import { LocaleProvider, setActiveLocale } from "../i18n/runtime";
import { matchLanguageTag } from "../i18n/config";
import { TrayPanel } from "../tray/TrayPanel";
import { EMPTY_SNAPSHOT, type TraySnapshot } from "../tray/model";
import "../tray/tray.css";

const query = new URLSearchParams(location.search);
const dark = query.get("theme") !== "light";
const state = query.get("state") ?? "normal";
const now = Date.parse("2026-09-29T19:42:00Z");
setActiveLocale(matchLanguageTag(query.get("locale") ?? "en") ?? "en");
document.documentElement.classList.toggle("dark", dark);
document.body.style.background = dark ? "#37362f" : "#d8d4c9";
document.body.style.width = "376px";
const snapshot: TraySnapshot = state === "loading" ? EMPTY_SNAPSHOT : {
  today: 1_800_000, total: 124_600_000, today_usd: 1.86, total_usd: 412.5,
  checked_at: new Date(now - (state === "stale" ? 180_000 : 0)).toISOString(),
  windows: (["Claude", "Codex"] as const).flatMap((provider, providerIndex) => ["5h", "7d"].map((label, index) => ({
    provider, label,
    used: state === "unavailable" ? null : 100 - (state === "low" ? [[8, 26], [0, 0.5]] : [[82, 61], [68, 42]])[providerIndex][index],
    reset: new Date(now + [[134, 4800], [92, 6780]][providerIndex][index] * 60_000).toISOString(),
  }))),
};
ReactDOM.createRoot(document.getElementById("root")!).render(<LocaleProvider>
  <TrayPanel snapshot={snapshot} now={now} dark={dark} error={false} busy={false}
    onAction={action => { document.body.dataset.action = action; }} />
</LocaleProvider>);
