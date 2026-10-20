import { ipc } from "../ipc";
import type { KeyboardShortcutOverrides } from "../shortcuts";
import type { TerminalEvent } from "../types";
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import { launchAgentLogin, listAgentSessionLaneOptions, probeAgentSessions, probeForAgent, type AgentSessionProbe, type AgentSessionLaneOptions } from "../agentSessions";
import { useT } from "../i18n/runtime";
import { useAgentCliInstall } from "../useAgentCliInstall";
import { walletProviderName, walletStore, type WalletPendingAdd, type WalletProvider } from "../accountWallet";
import { ACCOUNT_SWAP_NOTICE_PERCENT, accountSwapSetting } from "../accountSwap";
import { AUTO_ROTATION_MAX_PERCENT, AUTO_ROTATION_MIN_START_PERCENT, autoRotationSetting, type AutoRotationSettings } from "../accountAutoRotation";
import { AccountWalletList, useWalletSnapshot } from "./AccountWallet";
import { TerminalPanel } from "./TerminalPanel";

type ConnectableAgent = "antigravity" | "claude" | "codex" | "gemini";

const providerConfig: Record<
  ConnectableAgent,
  { install: string; name: string }
> = {
  antigravity: {
    install: "curl -fsSL https://antigravity.google/cli/install.sh | bash",
    name: "Antigravity CLI",
  },
  claude: {
    install: "npm i -g @anthropic-ai/claude-code",
    name: "Claude Code",
  },
  codex: {
    install: "npm i -g @openai/codex",
    name: "Codex",
  },
  gemini: {
    install: "npm i -g @google/gemini-cli",
    name: "Gemini CLI",
  },
};

function cliSubscriptionReady(lanes: AgentSessionLaneOptions, agent: ConnectableAgent) {
  const kind = agent === "codex" ? "codexChatGpt" : agent === "claude" ? "claudeSubscription" : "custom";
  return lanes.find((option) => option.agentKind === agent)?.lanes.some((lane) => lane.kind === kind && lane.loggedIn === true) ?? false;
}

type PendingWalletAdd = WalletPendingAdd & { provider: WalletProvider };

// How the save of a new wallet account starts. The login terminal is still
// open after "button"; it is gone after "closed" and "exited".
type WalletAddTrigger = "button" | "closed" | "exited";

/** Opens Connections from outside Settings. It can also start an account sign-in. */
export type SettingsConnectionsRequest = {
  signal: number;
  addProvider: WalletProvider | null;
};

type Props = {
  chatGptConnected: boolean;
  chatGptPending: boolean;
  logoutPending: boolean;
  onChatGptLogin: () => void;
  onChatGptLogout: () => void;
  onConnectionsChanged: () => void;
  // A provider whose account sign-in starts when the probe is ready.
  walletAddRequest?: WalletProvider | null;
  onWalletAddRequestHandled?: () => void;
};

export function SettingsConnections(props: Props) {
  const t = useT();
  const descriptions: Record<ConnectableAgent, string> = {
    antigravity: t(
      "Detached panes reconnect. Automatic conversation recovery after process exit is unavailable.",
    ),
    claude: t("Use your Claude subscription in Claude Code terminals."),
    codex: t("Use your ChatGPT subscription in Codex terminals."),
    gemini: t("Use your Google account in Gemini CLI terminals."),
  };
  const [probe, setProbe] = useState<AgentSessionProbe | null>(null);
  const [lanes, setLanes] = useState<AgentSessionLaneOptions>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState<ConnectableAgent | null>(null);
  const [terminal, setTerminal] = useState<string | null>(null);
  const installer = useAgentCliInstall();
  const [shortcutOverrides, setShortcutOverrides] = useState<KeyboardShortcutOverrides>({});
  const [walletAdd, setWalletAddState] = useState<PendingWalletAdd | null>(null);
  const [walletAddPending, setWalletAddPending] = useState(false);
  // Refs let the terminal exit listener see the current sign-in.
  const walletAddRef = useRef<PendingWalletAdd | null>(null);
  const walletAddSavingRef = useRef(false);
  const [walletAddMessage, setWalletAddMessage] = useState<string | null>(null);
  const walletAddSectionRef = useRef<HTMLElement>(null);
  const walletVersion = useWalletSnapshot().version;

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const [nextProbe, nextLanes, preferences] = await Promise.all([probeAgentSessions(), listAgentSessionLaneOptions(), ipc.preferences.get()]);
      setShortcutOverrides(preferences.keyboardShortcuts);
      setProbe(nextProbe);
      setLanes(nextLanes);
      setError(null);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
    const onFocus = () => { void refresh(); };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refresh]);

  // A wallet switch changes which login the lanes report.
  const walletVersionSeen = useRef(walletVersion);
  useEffect(() => {
    if (walletVersionSeen.current === walletVersion) return;
    walletVersionSeen.current = walletVersion;
    void refresh();
  }, [refresh, walletVersion]);

  function setWalletAdd(next: PendingWalletAdd | null) {
    walletAddRef.current = next;
    setWalletAddState(next);
  }

  // The login command exits after the sign-in, so its exit saves the account.
  const walletTerminalId = walletAdd?.terminalId ?? null;
  useEffect(() => {
    if (walletTerminalId) walletAddSectionRef.current?.scrollIntoView?.({ block: "nearest" });
  }, [walletTerminalId]);
  useEffect(() => {
    if (!walletTerminalId) return;
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void ipc.events.listen<TerminalEvent>("terminal_event", (event) => {
      if (event.sessionId === walletTerminalId && event.kind === "exit") void saveWalletAdd("exited");
    }).then((listener) => {
      if (disposed) listener(); else unlisten = listener;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
    // saveWalletAdd reads refs, so the listener needs no other inputs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [walletTerminalId]);

  // The status bar asks for a sign-in. The install check needs the probe.
  const { walletAddRequest, onWalletAddRequestHandled } = props;
  useEffect(() => {
    if (!walletAddRequest || loading || !probe) return;
    onWalletAddRequestHandled?.();
    if (walletAddRef.current) return;
    void startWalletAdd(walletAddRequest);
    // startWalletAdd reads the current probe, so it needs no other inputs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loading, probe, walletAddRequest]);

  async function startWalletAdd(provider: WalletProvider) {
    setError(null);
    setWalletAddMessage(null);
    try {
      if (!probeForAgent(probe, provider)?.found && !(await installer.install(provider))) return;
      const pending = await walletStore.startAdd(provider);
      setWalletAdd({ ...pending, provider });
    } catch (cause) {
      setWalletAddMessage(String(cause));
    }
  }

  function dismissWalletTerminal(terminalId: string) {
    void ipc.terminal.close({ sessionId: terminalId }).catch(() => undefined);
  }

  // After "closed" or "exited", an unfinished sign-in can no longer finish,
  // so Halv deletes its folder.
  async function saveWalletAdd(trigger: WalletAddTrigger) {
    const current = walletAddRef.current;
    if (!current || walletAddSavingRef.current) return;
    walletAddSavingRef.current = true;
    const terminalClosed = trigger === "closed";
    setWalletAddPending(true);
    setWalletAddMessage(null);
    const name = walletProviderName(current.provider);
    try {
      const result = await walletStore.finishAdd(current.provider, current.pendingId);
      if (!result.added) {
        if (trigger === "button") {
          setWalletAddMessage(t("The {provider} sign-in has not finished yet. Finish it in the terminal, then select Save account.", { provider: name }));
          return;
        }
        await walletStore.cancelAdd(current.provider, current.pendingId);
        if (!terminalClosed) dismissWalletTerminal(current.terminalId);
        setWalletAdd(null);
        setWalletAddMessage(t("The {provider} sign-in did not finish, so Halv did not add the account.", { provider: name }));
        return;
      }
      if (!terminalClosed) dismissWalletTerminal(current.terminalId);
      setWalletAdd(null);
      props.onConnectionsChanged();
    } catch (cause) {
      if (!terminalClosed) dismissWalletTerminal(current.terminalId);
      setWalletAdd(null);
      setWalletAddMessage(String(cause));
    } finally {
      walletAddSavingRef.current = false;
      setWalletAddPending(false);
    }
  }

  async function cancelWalletAdd() {
    const current = walletAddRef.current;
    if (walletAddSavingRef.current) return;
    if (!current) return;
    dismissWalletTerminal(current.terminalId);
    setWalletAdd(null);
    setWalletAddMessage(null);
    await walletStore.cancelAdd(current.provider, current.pendingId).catch(() => undefined);
  }

  async function connect(agent: ConnectableAgent) {
    setPending(agent);
    setError(null);
    try {
      if (!probeForAgent(probe, agent)?.found) {
        if (agent === "codex" || agent === "claude") {
          if (!(await installer.install(agent))) return;
        } else {
          const config = providerConfig[agent];
          setError(t("{name} was not found. Install it with: {command}", { name: config.name, command: config.install }));
          return;
        }
      }
      const result = await launchAgentLogin(agent);
      if (result) setTerminal(result);
      await refresh();
      props.onConnectionsChanged();
    } catch (cause) {
      setError(String(cause));
    } finally {
      setPending(null);
    }
  }

  const busy = pending !== null || props.chatGptPending || props.logoutPending || walletAdd !== null;
  return (
    <div className="space-y-4">
      <div className="flex justify-end"><button type="button" className="text-xs text-content-muted hover:text-content-primary disabled:opacity-50" onClick={() => void refresh()} disabled={loading || busy}>{t("Refresh")}</button></div>
      <div className="settings-provider-grid">
        {(["codex", "claude", "gemini", "antigravity"] as const).map((agent) => {
          const ready = cliSubscriptionReady(lanes, agent);
          const config = providerConfig[agent];
          const name = config.name;
          // The card action signs in the active wallet account again.
          const walletAgent = agent === "codex" || agent === "claude";
          return (
            <section className={`settings-provider-card settings-provider-${agent}`} key={agent} aria-label={name}>
              <div className="flex items-center justify-between gap-3">
                <span className="settings-provider-mark" aria-hidden="true">{agent === "codex" ? <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7"><path d="m8 6-6 6 6 6m8-12 6 6-6 6M14 3l-4 18" /></svg> : agent === "gemini" ? <span className="text-2xl text-sky-400">✦</span> : agent === "antigravity" ? <span className="text-2xl font-semibold">A</span> : <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7"><path d="M12 1v22M1 12h22M4.2 4.2l15.6 15.6M4.2 19.8 19.8 4.2M7.8 1.8l8.4 20.4M1.8 7.8l20.4 8.4M1.8 16.2l20.4-8.4M7.8 22.2l8.4-20.4" /></svg>}</span>
                <span className={`settings-connection-state ${ready ? "is-ready" : ""}`}><span aria-hidden="true" />{loading ? t("Checking…") : ready ? t("Connected") : t("Not connected")}</span>
              </div>
              <div className="mt-4"><h4 className="font-semibold text-content-primary">{name}</h4><p className="mt-1 text-xs leading-5 text-content-muted">{descriptions[agent]}</p></div>
              {walletAgent ? <AccountWalletList provider={agent} disabled={busy || loading} onAdd={(provider) => void startWalletAdd(provider)} /> : null}
              <button type="button" className="settings-provider-action" disabled={busy || loading || !probe} onClick={() => void connect(agent)}>
                {pending === agent ? t("Connecting…") : ready ? (walletAgent ? t("Reconnect") : t("Switch account")) : !probeForAgent(probe, agent)?.found ? (agent === "gemini" || agent === "antigravity" ? t("Installation instructions") : t("Install & connect")) : t("Connect")}
              </button>
            </section>
          );
        })}
      </div>
      <p className="text-xs leading-5 text-content-muted">{t("Your AI subscriptions are separate from your Halv plan.")}</p>
      <AccountSwitchSettings />
      {installer.progress ? <p className="text-xs text-content-muted" role="status">{installer.progress.line}</p> : null}
      {error || installer.error ? <p className="text-xs text-danger" role="alert">{error || installer.error?.message}</p> : null}
      <section className="settings-chat-connection">
        <div><h4 className="text-sm font-medium text-content-primary">{t("ChatGPT for Halv chat")}</h4><p className="mt-1 text-xs leading-5 text-content-muted">{t("Connect ChatGPT to use it in Halv's built-in chat.")}</p></div>
        <button type="button" className="settings-secondary-action" disabled={busy} onClick={props.chatGptConnected ? props.onChatGptLogout : props.onChatGptLogin}>{props.chatGptPending ? t("Connecting…") : props.logoutPending ? t("Signing out…") : props.chatGptConnected ? t("Sign out") : t("Connect ChatGPT")}</button>
      </section>
      {walletAddMessage ? <p className="text-xs text-danger" role="alert">{walletAddMessage}</p> : null}
      {walletAdd ? (
        <section className="space-y-2" aria-label={t("Add {provider} account", { provider: walletProviderName(walletAdd.provider) })} ref={walletAddSectionRef}>
          <div className="flex flex-wrap items-center justify-between gap-3">
            <p className="min-w-0 flex-1 text-xs leading-5 text-content-muted">{t("Sign in to the new {provider} account in the terminal. Then select Save account.", { provider: walletProviderName(walletAdd.provider) })}</p>
            <div className="flex shrink-0 gap-2">
              <button type="button" className="settings-secondary-action" disabled={walletAddPending} onClick={() => void cancelWalletAdd()}>{t("Cancel")}</button>
              <button type="button" className="settings-secondary-action" disabled={walletAddPending} onClick={() => void saveWalletAdd("button")}>{walletAddPending ? t("Saving...") : t("Save account")}</button>
            </div>
          </div>
          <div className="h-[26rem] overflow-hidden rounded-xl border border-surface-border/30"><TerminalPanel open project={null} workspaceId={null} focusSessionId={walletAdd.terminalId} shortcutOverrides={shortcutOverrides} onClose={() => void saveWalletAdd("closed")} /></div>
        </section>
      ) : null}
      {terminal ? <div className="h-[26rem] overflow-hidden rounded-xl border border-surface-border/30"><TerminalPanel open project={null} workspaceId={null} focusSessionId={terminal} shortcutOverrides={shortcutOverrides} onClose={() => { setTerminal(null); void refresh(); props.onConnectionsChanged(); }} /></div> : null}
    </div>
  );
}

/**
 * The account switch near usage limits: the assisted card, and the opt-in
 * automatic rotation of Claude accounts (ADR 0017). Both settings live in
 * the localStorage of this viewer. Halv switches without a click only when
 * the rotation is on.
 */
function AccountSwitchSettings() {
  const t = useT();
  const swapNotice = useSyncExternalStore(accountSwapSetting.subscribe, accountSwapSetting.getSnapshot, accountSwapSetting.getSnapshot);
  const rotation = useSyncExternalStore(autoRotationSetting.subscribe, autoRotationSetting.getSnapshot, autoRotationSetting.getSnapshot);
  return (
    <div className="space-y-3" data-account-switch-settings>
      <label className="flex items-start gap-2 text-xs leading-5 text-content-secondary"><input type="checkbox" className="mt-1 accent-accent" checked={swapNotice} onChange={(event) => accountSwapSetting.set(event.target.checked)} /><span><span className="block text-content-primary">{t("Suggest an account switch near usage limits")}</span><span className="block text-content-muted">{t("At {percent}% of a limit, Halv offers the account with the most room.", { percent: ACCOUNT_SWAP_NOTICE_PERCENT })}</span></span></label>
      <div>
        <label className="flex items-start gap-2 text-xs leading-5 text-content-secondary"><input type="checkbox" className="mt-1 accent-accent" checked={rotation.enabled} onChange={(event) => autoRotationSetting.set({ enabled: event.target.checked })} /><span><span className="block text-content-primary">{t("Rotate Claude accounts automatically")}</span><span className="block text-content-muted">{t("Between {start}% and {latest}% of a limit, Halv switches to the Claude account with the most room. It waits for a pause between turns until {latest}%.", { start: rotation.startPercent, latest: rotation.latestPercent })}</span></span></label>
        {rotation.enabled ? (
          <div className="ml-6 mt-2 flex flex-wrap items-center gap-x-5 gap-y-2">
            <RotationPercentField field="startPercent" label={t("Start at")} min={AUTO_ROTATION_MIN_START_PERCENT} value={rotation.startPercent} />
            <RotationPercentField field="latestPercent" label={t("Switch by")} min={rotation.startPercent} value={rotation.latestPercent} />
          </div>
        ) : null}
      </div>
    </div>
  );
}

/** One percent of the rotation. The setting clamps the value on blur or Enter. */
function RotationPercentField({ field, label, min, value }: { field: "startPercent" | "latestPercent"; label: string; min: number; value: number }) {
  const [draft, setDraft] = useState(String(value));
  useEffect(() => setDraft(String(value)), [value]);
  const commit = () => {
    // An empty field keeps the current value.
    const next = draft.trim() === "" ? Number.NaN : Number(draft);
    const change: Partial<AutoRotationSettings> = { [field]: next };
    autoRotationSetting.set(change);
    setDraft(String(autoRotationSetting.getSnapshot()[field]));
  };
  return (
    <label className="flex items-center gap-2 text-xs text-content-secondary">
      <span>{label}</span>
      <span className="flex items-center gap-1">
        <input
          className="w-16 rounded-md border border-surface-border/30 bg-surface-raised px-2 py-1 text-xs tabular-nums text-content-primary outline-none transition-colors duration-150 focus:border-accent focus:ring-2 focus:ring-accent/30"
          inputMode="numeric"
          max={AUTO_ROTATION_MAX_PERCENT}
          min={min}
          onBlur={commit}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key !== "Enter") return;
            event.preventDefault();
            commit();
          }}
          step={1}
          type="number"
          value={draft}
        />
        <span aria-hidden className="text-content-muted">%</span>
      </span>
    </label>
  );
}
