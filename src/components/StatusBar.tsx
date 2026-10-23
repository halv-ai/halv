// The status line at the bottom of the window: plan usage per provider, the
// account switcher, and the weekly savings. The layout adapts Orca's
// StatusBar (stablyai/orca, MIT).
import {
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
  type ReactNode,
  type RefObject,
} from "react";
import { ipc } from "../ipc.ts";
import {
  activeWalletLabel,
  formatWalletPlan,
  walletAccountDetail,
  walletAccountProblem,
  walletProviderName,
  walletStore,
  walletSyncedWithOrca,
  type WalletAccountUsage,
  type WalletProvider,
  type WalletState,
} from "../accountWallet";
import { formatMoney, formatNumber, useT } from "../i18n/runtime";
import type {
  PlanProviderConnections,
  PlanProviderId,
  ProviderPlanWindowGroup,
} from "../providerPlanWindows";
import {
  canRefreshUsage,
  findWalletUsage,
  isWalletPlanProvider,
  popoverLeft,
  providerLetter,
  statusBarDensity,
  statusBarProviderGroups,
  statusProviderUsage,
  usageTone,
  walletUsageWindows,
  type StatusBarDensity,
  type StatusProviderUsage,
  type StatusUsageWindow,
  type UsageTone,
} from "../statusBar";
import type { SidecarStats, TerminalSession } from "../types";
import { usePlanWindowGroups } from "../usePlanWindowGroups";
import { accountRestartStore } from "../accountRestart";
import { errorText, useWalletSnapshot } from "./AccountWallet";
import { AccountRestartPrompt } from "./AccountRestartChip";
import { AccountSwapNotice } from "./AccountSwapNotice";
import { AppUpdateSegment } from "./AppUpdateSegment";
import { MobileStatusButton } from "./MobileConnect";
import { ResourceUsageSegment } from "./ResourceUsageSegment";
import { formatResetCountdown, SavingsSparkline } from "./SavingsPanel";

const SYSTEM_DEFAULT_KEY = "system-default";
const CLOCK_TICK_MS = 30_000;
// The popover keeps this gap above the top edge of the status line.
const POPOVER_GAP_PX = 6;

type StatusBarProps = {
  planProviderConnections: PlanProviderConnections;
  stats: SidecarStats | null;
  weeklyTokensSaved: number | null;
  /** Dollars saved in the last 7 days (ADR 0015). */
  weeklyDollarsUsd?: number | null;
  /** True when the weekly dollars hold a routing estimate (ADR 0016). */
  weeklyApproximate?: boolean;
  weeklyRecentDollars?: number[] | null;
  savingsDrawerOpen: boolean;
  /** The open terminals, for the resource chip. */
  terminalSessions?: readonly TerminalSession[];
  /** The first item of the bar, such as the attention queue chip. */
  leading?: ReactNode;
  onToggleSavingsDrawer: () => void;
  onOpenUsageDetails: () => void;
  onManageAccounts: (provider: WalletProvider) => void;
  onAddAccount: (provider: WalletProvider) => void;
};

type SwitchMessage = { kind: "notice" | "error"; text: string };

type PopoverAnchor = { left: number; bottom: number };

export function StatusBar({
  planProviderConnections,
  stats,
  weeklyTokensSaved,
  weeklyDollarsUsd = null,
  weeklyApproximate = false,
  weeklyRecentDollars = null,
  savingsDrawerOpen,
  terminalSessions = [],
  leading = null,
  onToggleSavingsDrawer,
  onOpenUsageDetails,
  onManageAccounts,
  onAddAccount,
}: StatusBarProps) {
  const t = useT();
  const { groups, loadFailed, refresh, refreshing, walletVersions } = usePlanWindowGroups(
    planProviderConnections,
    stats,
  );
  const wallet = useWalletSnapshot();
  useEffect(() => accountRestartStore.ensureStarted(), []);
  const density = statusBarDensity(useWindowWidth());
  const nowMs = useClock(CLOCK_TICK_MS);
  const popoverId = useId();
  const [openProvider, setOpenProvider] = useState<PlanProviderId | null>(null);
  const [anchor, setAnchor] = useState<PopoverAnchor | null>(null);
  const pillRefs = useRef(new Map<PlanProviderId, HTMLButtonElement>());
  const popoverRef = useRef<HTMLDivElement>(null);
  const lastRefreshRef = useRef<number | null>(null);
  const [switching, setSwitching] = useState<{
    provider: WalletProvider;
    key: string;
  } | null>(null);
  const [switchMessages, setSwitchMessages] = useState<
    Partial<Record<WalletProvider, SwitchMessage>>
  >({});
  const [walletUsage, setWalletUsage] = useState<WalletAccountUsage[]>([]);
  const [walletUsageLoaded, setWalletUsageLoaded] = useState(false);

  const visibleGroups = statusBarProviderGroups(groups);
  const openGroup =
    visibleGroups.find((group) => group.id === openProvider) ?? null;
  const labels = { week: t("wk") };

  const measureAnchor = useCallback((provider: PlanProviderId) => {
    const pill = pillRefs.current.get(provider);
    if (!pill) return null;
    const rect = pill.getBoundingClientRect();
    return {
      left: popoverLeft(rect.left, window.innerWidth),
      bottom: window.innerHeight - rect.top + POPOVER_GAP_PX,
    };
  }, []);

  const closePopover = useCallback(
    (restoreFocus: boolean) => {
      const provider = openProvider;
      setOpenProvider(null);
      if (!provider) return;
      // A finished switch leaves its message until the user closes the
      // popover. A pending switch keeps it for the next open.
      if (isWalletPlanProvider(provider) && switching?.provider !== provider) {
        setSwitchMessages((current) => ({ ...current, [provider]: undefined }));
      }
      if (restoreFocus) pillRefs.current.get(provider)?.focus();
    },
    [openProvider, switching],
  );

  function togglePopover(provider: PlanProviderId) {
    if (openProvider === provider) {
      closePopover(false);
      return;
    }
    setAnchor(measureAnchor(provider));
    setOpenProvider(provider);
    // The switcher shows the accounts as Orca has them now.
    if (isWalletPlanProvider(provider)) {
      void walletStore.refresh().catch(() => undefined);
    }
  }

  // Outside clicks and Escape close the popover. The resize handler keeps
  // the popover over its pill when the density changes.
  useEffect(() => {
    if (!openProvider) return;
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (!target) return;
      if (popoverRef.current?.contains(target)) return;
      if (pillRefs.current.get(openProvider)?.contains(target)) return;
      closePopover(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      closePopover(true);
    };
    const onResize = () => setAnchor(measureAnchor(openProvider));
    document.addEventListener("pointerdown", onPointerDown, true);
    document.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("resize", onResize);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown, true);
      document.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("resize", onResize);
    };
  }, [closePopover, measureAnchor, openProvider]);

  useEffect(() => {
    if (openProvider) popoverRef.current?.focus();
  }, [openProvider]);

  // The provider can disconnect while its popover is open.
  useEffect(() => {
    if (openProvider && !openGroup) setOpenProvider(null);
  }, [openGroup, openProvider]);

  // Usage of the accounts that are not active loads when the switcher opens,
  // and again when the wallet changes while it is open.
  const openWalletProvider =
    openProvider && isWalletPlanProvider(openProvider) ? openProvider : null;
  const openWalletHasAccounts = openWalletProvider
    ? wallet.state[openWalletProvider].accounts.length > 0
    : false;
  useEffect(() => {
    if (!openWalletProvider || !openWalletHasAccounts) {
      setWalletUsageLoaded(true);
      return;
    }
    let cancelled = false;
    setWalletUsageLoaded(false);
    ipc.wallet
      .usage()
      .then(
        (usage) => {
          if (!cancelled) setWalletUsage(usage);
        },
        () => undefined,
      )
      .finally(() => {
        if (!cancelled) setWalletUsageLoaded(true);
      });
    return () => {
      cancelled = true;
    };
  }, [openWalletHasAccounts, openWalletProvider, wallet.version]);

  function handleRefresh() {
    const now = Date.now();
    if (!canRefreshUsage(lastRefreshRef.current, now)) return;
    lastRefreshRef.current = now;
    refresh();
  }

  async function selectAccount(provider: WalletProvider, accountId: string | null) {
    if (switching) return;
    if (accountId === wallet.state[provider].activeId) return;
    setSwitching({ provider, key: accountId ?? SYSTEM_DEFAULT_KEY });
    setSwitchMessages((current) => ({ ...current, [provider]: undefined }));
    let message: SwitchMessage | undefined;
    try {
      // The popover lists the panes that still run the old account.
      const result = await walletStore.select(provider, accountId);
      accountRestartStore.apply(result.staleSessions);
    } catch (cause) {
      message = { kind: "error", text: errorText(cause) };
    } finally {
      setSwitching(null);
    }
    setSwitchMessages((current) => ({ ...current, [provider]: message }));
  }

  const hasSavings = (weeklyTokensSaved ?? 0) > 0 || (weeklyDollarsUsd ?? 0) > 0;

  return (
    <div
      aria-label={t("Status bar")}
      className="flex h-6 min-h-[24px] w-full flex-none select-none items-center gap-4 whitespace-nowrap border-t border-surface-border/25 bg-surface-sunken px-3 text-xs text-content-secondary"
      role="region"
    >
      {leading}
      {/* Padding inside the clipped row leaves room for focus rings. */}
      <div className="-mx-1 flex min-w-0 flex-1 items-center gap-3 self-stretch overflow-hidden px-1">
        {visibleGroups.map((group) => {
          const usage = statusProviderUsage(group, nowMs, labels);
          const walletProvider = isWalletPlanProvider(group.id) ? group.id : null;
          return (
            <ProviderPill
              activeLabel={
                walletProvider && wallet.loaded
                  ? activeWalletLabel(wallet.state[walletProvider], t)
                  : null
              }
              buttonRef={(element) => {
                if (element) pillRefs.current.set(group.id, element);
                else pillRefs.current.delete(group.id);
              }}
              density={density}
              failed={
                walletProvider !== null &&
                switchMessages[walletProvider]?.kind === "error"
              }
              group={group}
              key={group.id}
              onToggle={() => togglePopover(group.id)}
              open={openProvider === group.id}
              pending={walletProvider !== null && switching?.provider === walletProvider}
              popoverId={popoverId}
              usage={usage}
            />
          );
        })}
        {visibleGroups.length > 0 ? (
          <button
            aria-label={t("Refresh usage")}
            className={`flex h-5 w-5 shrink-0 items-center justify-center rounded transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40 disabled:opacity-40 ${
              loadFailed ? "text-warning" : "text-content-muted"
            }`}
            disabled={refreshing}
            onClick={handleRefresh}
            title={loadFailed ? t("Could not refresh savings data.") : t("Refresh usage")}
            type="button"
          >
            <RefreshIcon spinning={refreshing} />
          </button>
        ) : null}
        <button
          aria-controls="savings-drawer"
          aria-expanded={savingsDrawerOpen}
          className="inline-flex h-5 shrink-0 items-center gap-1.5 rounded px-1 text-content-primary transition-colors hover:bg-surface-raised focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40"
          onClick={onToggleSavingsDrawer}
          title={
            hasSavings
              ? t("Saved in the last 7 days at API prices. Click for details.")
              : t("Savings start with your first reply")
          }
          type="button"
        >
          <span aria-hidden className="text-[9px] text-accent">{"▲"}</span>
          <span className="font-medium">
            {hasSavings
              ? t("{amount} saved", {
                  amount: weeklyApproximate
                    ? t("\u2248 {amount}", {
                        amount: formatMoney(weeklyDollarsUsd ?? 0),
                      })
                    : formatMoney(weeklyDollarsUsd ?? 0),
                })
              : t("Tracking savings")}
          </span>
          {hasSavings ? (
            <span className="font-mono text-[10px] text-content-muted">
              {"· "}
              {t("7d")}
            </span>
          ) : null}
          {density === "minimal" || !hasSavings ? null : (
            <SavingsSparkline
              compact
              values={(weeklyRecentDollars ?? []).filter((value) => value > 0)}
            />
          )}
        </button>
      </div>
      <AppUpdateSegment />
      <MobileStatusButton />
      <ResourceUsageSegment
        compact={density !== "full"}
        terminalSessions={terminalSessions}
      />
      <AccountSwapNotice
        groups={visibleGroups}
        nowMs={nowMs}
        walletVersions={walletVersions}
      />
      {openGroup && anchor ? (
        <ProviderPopover
          anchor={anchor}
          group={openGroup}
          id={popoverId}
          message={
            isWalletPlanProvider(openGroup.id)
              ? switchMessages[openGroup.id]
              : undefined
          }
          nowMs={nowMs}
          onAddAccount={(provider) => {
            closePopover(false);
            onAddAccount(provider);
          }}
          onManageAccounts={(provider) => {
            closePopover(false);
            onManageAccounts(provider);
          }}
          onOpenUsageDetails={() => {
            closePopover(false);
            onOpenUsageDetails();
          }}
          onSelectAccount={(provider, accountId) =>
            void selectAccount(provider, accountId)
          }
          popoverRef={popoverRef}
          switchingKey={
            switching && switching.provider === openGroup.id ? switching.key : null
          }
          switchBusy={switching !== null}
          walletLoaded={wallet.loaded}
          walletState={wallet.state}
          walletUsage={walletUsage}
          walletUsageLoaded={walletUsageLoaded}
        />
      ) : null}
    </div>
  );
}

function ProviderPill({
  activeLabel,
  buttonRef,
  density,
  failed,
  group,
  onToggle,
  open,
  pending,
  popoverId,
  usage,
}: {
  activeLabel: string | null;
  buttonRef: (element: HTMLButtonElement | null) => void;
  density: StatusBarDensity;
  failed: boolean;
  group: ProviderPlanWindowGroup;
  onToggle: () => void;
  open: boolean;
  pending: boolean;
  popoverId: string;
  usage: StatusProviderUsage;
}) {
  const t = useT();
  const name = providerName(group);
  const windowText = (window: StatusUsageWindow) =>
    t("{percent}% used {reset}", {
      percent: formatNumber(window.usedPercent),
      reset: window.resetLabel,
    });
  const summary =
    usage.windows.length > 0
      ? usage.windows.map(windowText).join(" · ")
      : "—";
  // Compact and minimal pills show only the tightest window.
  const shownWindows =
    density === "full" ? usage.windows : usage.tightest ? [usage.tightest] : [];

  return (
    <button
      aria-controls={open ? popoverId : undefined}
      aria-expanded={open}
      aria-haspopup="dialog"
      aria-label={t("{provider} usage: {summary}", { provider: name, summary })}
      className={`inline-flex h-5 shrink-0 items-center gap-1.5 rounded px-1 transition-colors hover:bg-surface-raised focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40 ${
        open ? "bg-surface-raised" : ""
      }`}
      data-status-provider={group.id}
      onClick={onToggle}
      ref={buttonRef}
      title={activeLabel ? `${name} · ${activeLabel}` : name}
      type="button"
    >
      {density === "minimal" ? (
        <span className="inline-flex items-center gap-1 text-content-secondary">
          <span
            aria-hidden
            className={`inline-block h-2 w-2 rounded-full ${
              usage.tightest ? usageBarClass(usage.tightest.tone) : "bg-content-muted/40"
            }`}
          />
          {providerLetter(group.id)}
        </span>
      ) : (
        <ProviderIcon id={group.id} />
      )}
      {density === "full" && usage.tightest ? (
        <span
          aria-hidden
          className="h-[6px] w-[48px] shrink-0 overflow-hidden rounded-full bg-surface-border/30"
          data-usage-bar
        >
          <span
            className={`block h-full rounded-full ${usageBarClass(usage.tightest.tone)}`}
            style={{ width: `${usage.tightest.usedPercent}%` }}
          />
        </span>
      ) : null}
      {shownWindows.length === 0 ? (
        <span className="text-content-muted">{"—"}</span>
      ) : (
        shownWindows.map((window, index) => (
          <span className="tabular-nums" key={window.key}>
            {index > 0 ? (
              <span className="text-content-muted">{"· "}</span>
            ) : null}
            <span className={usageTextClass(window.tone)}>
              {density === "minimal"
                ? `${formatNumber(window.usedPercent)}%`
                : windowText(window)}
            </span>
          </span>
        ))
      )}
      {pending ? <Spinner /> : null}
      {failed && !pending ? (
        <span aria-hidden className="inline-block h-1.5 w-1.5 rounded-full bg-danger" />
      ) : null}
    </button>
  );
}

function ProviderPopover({
  anchor,
  group,
  id,
  message,
  nowMs,
  onAddAccount,
  onManageAccounts,
  onOpenUsageDetails,
  onSelectAccount,
  popoverRef,
  switchBusy,
  switchingKey,
  walletLoaded,
  walletState,
  walletUsage,
  walletUsageLoaded,
}: {
  anchor: PopoverAnchor;
  group: ProviderPlanWindowGroup;
  id: string;
  message: SwitchMessage | undefined;
  nowMs: number;
  onAddAccount: (provider: WalletProvider) => void;
  onManageAccounts: (provider: WalletProvider) => void;
  onOpenUsageDetails: () => void;
  onSelectAccount: (provider: WalletProvider, accountId: string | null) => void;
  popoverRef: RefObject<HTMLDivElement>;
  switchBusy: boolean;
  switchingKey: string | null;
  walletLoaded: boolean;
  walletState: WalletState;
  walletUsage: WalletAccountUsage[];
  walletUsageLoaded: boolean;
}) {
  const t = useT();
  const name = providerName(group);
  const walletProvider = isWalletPlanProvider(group.id) ? group.id : null;
  const providerState = walletProvider ? walletState[walletProvider] : null;
  const usageLabels = { week: t("wk"), fable: "Fable" };

  return (
    <div
      aria-label={t("{provider} usage and accounts", { provider: name })}
      className="fixed z-[60] flex max-h-[calc(100vh-5rem)] w-[360px] max-w-[calc(100vw-1rem)] flex-col overflow-hidden whitespace-normal rounded-lg border border-surface-border/30 bg-surface-panel text-content-primary shadow-2xl shadow-black/25 outline-none"
      data-status-popover={group.id}
      data-wallet-usage={walletUsageLoaded ? "loaded" : "loading"}
      id={id}
      ref={popoverRef}
      role="dialog"
      style={{ left: anchor.left, bottom: anchor.bottom }}
      tabIndex={-1}
    >
      <header className="flex items-center gap-2 border-b border-surface-border/25 px-3 py-2">
        <ProviderIcon id={group.id} />
        <span className="min-w-0 flex-1 truncate text-xs font-semibold">
          {group.label}
        </span>
        {walletProvider && walletSyncedWithOrca(walletState) ? (
          <span className="shrink-0 text-[10px] text-content-muted">
            {t("Synced with Orca")}
          </span>
        ) : null}
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="space-y-2.5 px-3 py-2.5">
          {group.windows.map((window, index) => (
            <UsageRow key={`${window.label}-${index}`} window={window} />
          ))}
          {group.unavailableReason ? (
            <p className="text-[10px] leading-relaxed text-content-muted">
              {group.unavailableReason}
            </p>
          ) : null}
        </div>
        {walletProvider && providerState && walletLoaded &&
        providerState.accounts.length > 0 ? (
          <div className="border-t border-surface-border/25 px-3 py-2.5">
            <div className="mb-1.5 font-mono text-[10px] uppercase tracking-[0.16em] text-content-muted">
              {t("Accounts")}
            </div>
            <div
              aria-label={t("{provider} accounts", {
                provider: walletProviderName(walletProvider),
              })}
              className="space-y-0.5"
              role="group"
            >
              <AccountRow
                active={providerState.activeId === null}
                busy={switchBusy}
                detail={
                  [
                    providerState.systemDefault.email,
                    formatWalletPlan(providerState.systemDefault.plan),
                  ]
                    .filter((part): part is string => Boolean(part))
                    .join(" · ") ||
                  t("Your own {provider} login", {
                    provider: walletProviderName(walletProvider),
                  })
                }
                label={t("System default")}
                onSelect={() => onSelectAccount(walletProvider, null)}
                pending={switchingKey === SYSTEM_DEFAULT_KEY}
                usage={[]}
              />
              {providerState.accounts.map((account) => {
                const active = providerState.activeId === account.id;
                const problem = walletAccountProblem(account, t);
                return (
                  <AccountRow
                    active={active}
                    blocked={problem !== null}
                    busy={switchBusy}
                    detail={problem ?? walletAccountDetail(account)}
                    key={account.id}
                    label={account.email}
                    onSelect={() => onSelectAccount(walletProvider, account.id)}
                    pending={switchingKey === account.id}
                    usage={
                      active
                        ? []
                        : walletUsageWindows(
                            findWalletUsage(walletUsage, walletProvider, account.id),
                            nowMs,
                            usageLabels,
                          )
                    }
                  />
                );
              })}
            </div>
          </div>
        ) : null}
        {walletProvider ? <AccountRestartPrompt provider={walletProvider} /> : null}
        {message ? (
          <p
            className={`border-t border-surface-border/25 px-3 py-2 text-[11px] leading-4 ${
              message.kind === "error" ? "text-danger" : "text-content-muted"
            }`}
            role={message.kind === "error" ? "alert" : "status"}
          >
            {message.text}
          </p>
        ) : null}
      </div>
      <footer className="flex flex-wrap items-center gap-x-3 gap-y-1 border-t border-surface-border/25 px-3 py-2 text-[11px]">
        {walletProvider ? (
          <>
            <PopoverLink onClick={() => onAddAccount(walletProvider)}>
              {t("Add account…")}
            </PopoverLink>
            <PopoverLink onClick={() => onManageAccounts(walletProvider)}>
              {t("Manage accounts…")}
            </PopoverLink>
          </>
        ) : null}
        <span className="ml-auto">
          <PopoverLink onClick={onOpenUsageDetails}>{t("Usage details")}</PopoverLink>
        </span>
      </footer>
    </div>
  );
}

function UsageRow({ window }: { window: ProviderPlanWindowGroup["windows"][number] }) {
  const t = useT();
  const usedPercent =
    window.remainingPercent == null ? null : 100 - window.remainingPercent;
  const tone = usedPercent == null ? "normal" : usageTone(usedPercent);
  return (
    <div>
      <div className="flex items-baseline gap-2 text-[11px]">
        <span className="w-7 shrink-0 font-mono text-content-secondary">
          {window.label || "—"}
        </span>
        {usedPercent == null ? (
          <span className="text-content-muted">
            {t("No activity in this window yet.")}
          </span>
        ) : (
          <>
            <span className={`tabular-nums ${usageTextClass(tone)}`}>
              {t("{percent}% used", { percent: formatNumber(usedPercent) })}
            </span>
            <span className="ml-auto tabular-nums text-content-muted">
              {t("resets {reset}", {
                reset: formatResetCountdown(window.resetsAt, t),
              })}
            </span>
          </>
        )}
      </div>
      <div
        aria-hidden
        className="mt-1 h-1.5 overflow-hidden rounded-full bg-surface-border/30"
      >
        {usedPercent == null ? null : (
          <span
            className={`block h-full rounded-full ${usageBarClass(tone)}`}
            style={{ width: `${usedPercent}%` }}
          />
        )}
      </div>
    </div>
  );
}

function AccountRow({
  active,
  blocked = false,
  busy,
  detail,
  label,
  onSelect,
  pending,
  usage,
}: {
  active: boolean;
  // The account cannot be picked now. The detail tells why.
  blocked?: boolean;
  busy: boolean;
  detail: string;
  label: string;
  onSelect: () => void;
  pending: boolean;
  usage: StatusUsageWindow[];
}) {
  const t = useT();
  return (
    <button
      aria-current={active ? "true" : undefined}
      className={`block w-full rounded-md px-2 py-1.5 text-left transition-colors focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40 disabled:cursor-not-allowed ${
        active ? "bg-accent/[0.08]" : "hover:bg-surface-raised"
      } ${busy && !pending ? "opacity-60" : ""} ${blocked ? "opacity-60" : ""}`}
      disabled={busy || (blocked && !active)}
      onClick={onSelect}
      type="button"
    >
      <span className="flex min-w-0 items-center gap-2">
        <span className="min-w-0 flex-1 truncate text-xs text-content-primary">
          {label}
        </span>
        {pending ? (
          <span className="inline-flex shrink-0 items-center gap-1 text-[10px] text-content-muted">
            <Spinner />
            {t("Switching…")}
          </span>
        ) : active ? (
          // Some catalogs translate "Active" as "running". The lowercase
          // label fits an account in every catalog.
          <span className="shrink-0 rounded bg-accent/15 px-1.5 py-px text-[10px] font-medium capitalize text-accent">
            {t("active")}
          </span>
        ) : null}
      </span>
      {detail ? (
        <span
          className={`block truncate text-[10px] ${
            blocked ? "text-warning" : "text-content-muted"
          }`}
        >
          {detail}
        </span>
      ) : null}
      {usage.length > 0 ? (
        <span
          className="mt-1 grid w-full items-center gap-1.5"
          data-account-usage
          style={{ gridTemplateColumns: `repeat(${usage.length}, minmax(0, 1fr))` }}
        >
          {usage.map((window) => (
            <span className="flex min-w-0 items-center gap-1" key={window.key}>
              <span
                aria-hidden
                className="h-1 min-w-0 flex-1 overflow-hidden rounded-full bg-surface-border/30"
              >
                <span
                  className={`block h-full rounded-full ${usageBarClass(window.tone)}`}
                  style={{ width: `${window.usedPercent}%` }}
                />
              </span>
              <span className="shrink-0 text-[10px] tabular-nums text-content-muted">
                {`${formatNumber(window.usedPercent)}% ${window.resetLabel}`}
              </span>
            </span>
          ))}
        </span>
      ) : null}
    </button>
  );
}

function PopoverLink({
  children,
  onClick,
}: {
  children: ReactNode;
  onClick: () => void;
}) {
  return (
    <button
      className="rounded text-accent underline-offset-4 transition-colors hover:text-accent-strong hover:underline focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40"
      onClick={onClick}
      type="button"
    >
      {children}
    </button>
  );
}

function ProviderIcon({ id }: { id: PlanProviderId }) {
  if (id === "claude") {
    return (
      <svg
        aria-hidden
        className="shrink-0 text-accent"
        fill="none"
        height={13}
        stroke="currentColor"
        strokeWidth="2"
        viewBox="0 0 24 24"
        width={13}
      >
        <path d="M12 1v22M1 12h22M4.2 4.2l15.6 15.6M4.2 19.8 19.8 4.2M7.8 1.8l8.4 20.4M1.8 7.8l20.4 8.4M1.8 16.2l20.4-8.4M7.8 22.2l8.4-20.4" />
      </svg>
    );
  }
  if (id === "codex") {
    return (
      <svg
        aria-hidden
        className="shrink-0 text-content-primary"
        fill="none"
        height={13}
        stroke="currentColor"
        strokeWidth="2"
        viewBox="0 0 24 24"
        width={13}
      >
        <path d="m8 6-6 6 6 6m8-12 6 6-6 6M14 3l-4 18" />
      </svg>
    );
  }
  return (
    <span
      aria-hidden
      className="inline-flex h-[13px] min-w-[13px] shrink-0 items-center justify-center rounded-sm bg-surface-raised px-px font-mono text-[9px] font-semibold leading-none text-content-secondary"
    >
      {providerLetter(id)}
    </span>
  );
}

function RefreshIcon({ spinning }: { spinning: boolean }) {
  return (
    <svg
      aria-hidden
      className={spinning ? "animate-spin" : undefined}
      fill="none"
      height={11}
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      strokeWidth="2"
      viewBox="0 0 24 24"
      width={11}
    >
      <path d="M21 12a9 9 0 0 1-15.5 6.2M3 12A9 9 0 0 1 18.5 5.8" />
      <path d="M21 3v6h-6M3 21v-6h6" />
    </svg>
  );
}

function Spinner() {
  return (
    <span
      aria-hidden
      className="inline-block h-2.5 w-2.5 shrink-0 animate-spin rounded-full border border-current border-t-transparent text-content-muted"
    />
  );
}

function providerName(group: ProviderPlanWindowGroup) {
  return isWalletPlanProvider(group.id)
    ? walletProviderName(group.id)
    : group.label;
}

function usageBarClass(tone: UsageTone) {
  if (tone === "danger") return "bg-danger";
  if (tone === "warning") return "bg-warning";
  return "bg-content-muted/70";
}

function usageTextClass(tone: UsageTone) {
  if (tone === "danger") return "text-danger";
  if (tone === "warning") return "text-warning";
  return "text-content-secondary";
}

function useWindowWidth() {
  const [width, setWidth] = useState(() => window.innerWidth);
  useEffect(() => {
    const onResize = () => setWidth(window.innerWidth);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  return width;
}

/** The current time, updated on a timer so countdowns stay current. */
function useClock(intervalMs: number) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const interval = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(interval);
  }, [intervalMs]);
  return now;
}
