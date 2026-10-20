import { formatUsdCompact } from "../format.ts";

export type TrayAction = "tray.usage" | "tray.new-chat" | "tray.show" | "tray.quit" | "tray.quit-stop-agents" | "dismiss";

export interface TrayWindow {
  provider: "Claude" | "Codex";
  label: string;
  used: number | null;
  reset: string | null;
}

export interface TraySnapshot {
  today: number | null;
  total: number | null;
  /** Dollars saved with the methods of ADR 0015. */
  today_usd?: number | null;
  total_usd?: number | null;
  windows: TrayWindow[];
  checked_at: string | null;
}

export const EMPTY_SNAPSHOT: TraySnapshot = { today: null, total: null, today_usd: null, total_usd: null, windows: [], checked_at: null };

export function trayIsFresh(snapshot: TraySnapshot, now: number): boolean {
  const age = now - Date.parse(snapshot.checked_at ?? "");
  return Number.isFinite(age) && age >= -5000 && age < 120_000;
}

export function trayWindows(snapshot: TraySnapshot, provider: TrayWindow["provider"]): TrayWindow[] {
  const windows = snapshot.windows.filter(window => window.provider === provider);
  const canonical = provider === "Claude" || windows.every(window => window.label === "5h" || window.label === "7d");
  return ["5h", "7d"].map((label, index) => {
    const window = canonical ? windows.find(window => window.label === label) : windows[index];
    return window ?? { provider, label, used: null, reset: null };
  });
}

export function remainingQuota(window: TrayWindow, fresh: boolean, now: number): number | null {
  const reset = Date.parse(window.reset ?? "");
  if (!fresh || window.used == null || !Number.isFinite(window.used) || !Number.isFinite(reset) || reset <= now) return null;
  return Math.max(0, Math.min(100, 100 - window.used));
}

export function formatTrayPercent(remaining: number): string {
  return remaining > 0 && remaining < 1 ? "<1" : String(Math.floor(remaining));
}

export function formatTrayReset(reset: string | null, now: number): string {
  const minutes = Math.max(1, Math.ceil((Date.parse(reset ?? "") - now) / 60_000));
  if (!Number.isFinite(minutes)) return "—";
  if (minutes >= 1440) return `${Math.floor(minutes / 1440)}d ${Math.floor(minutes % 1440 / 60)}h`;
  if (minutes >= 60) return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
  return `${minutes}m`;
}

export function formatTrayDollars(value: number | null | undefined, locale: string): string {
  if (value == null || !Number.isFinite(value)) return "—";
  return formatUsdCompact(value, locale);
}

export function formatTrayTokens(value: number | null, locale: string): string {
  if (value == null || !Number.isFinite(value)) return "—";
  return new Intl.NumberFormat(locale, { notation: "compact", maximumFractionDigits: 1 }).format(value);
}
