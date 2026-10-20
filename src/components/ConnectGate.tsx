import { ipc } from "../ipc.ts";
import { useEffect, useRef, useState } from "react";
import { recordAnalyticsEvent } from "../analytics";
import {
  agentNeedsCliToRun,
  connectGateConnections,
  shouldOpenApiKeySection,
  type ConnectGateState,
} from "../connectGate";
import {
  launchAgentLogin,
  probeForAgent,
  type AgentBinaryProbe,
  type AgentKind,
  type AgentSessionLaneOptions,
  type AgentSessionProbe,
} from "../agentSessions";
import { formatNumber, useT, type Translate } from "../i18n/runtime";
import type { KeyboardShortcutOverrides } from "../shortcuts";
import type {
  AnthropicCredentialStatus,
  AppPreferences,
  AuthStatus,
  CredentialProfile,
  SidecarStatus,
} from "../types";
import {
  useAgentCliInstall,
  type AgentCliInstallProgress,
  type AgentCliKind,
} from "../useAgentCliInstall";
import { StepIndicator } from "./StepIndicator";
import { TerminalPanel } from "./TerminalPanel";

type ConnectGateProps = {
  anthropic: AnthropicCredentialStatus | null;
  auth: AuthStatus | null;
  credentialProfiles: CredentialProfile[];
  gateState: Extract<ConnectGateState, "required" | "optional">;
  laneOptions: AgentSessionLaneOptions;
  onAddGrokApiKey: (apiKey: string) => Promise<void>;
  onAddZaiApiKey: (apiKey: string) => Promise<void>;
  onConnectionsChanged: () => Promise<void>;
  onContinue: () => void;
  onLoginChatGpt: () => Promise<void>;
  onSetOpenAiApiKey: (apiKey: string) => Promise<void>;
  onSetAnthropicApiKey: (apiKey: string) => Promise<void>;
  probe: AgentSessionProbe | null;
  shortcutOverrides: KeyboardShortcutOverrides;
  sidecar: SidecarStatus | null;
};

// Matches LOGIN_TIMEOUT in src-tauri/src/auth/mod.rs. A first ChatGPT
// sign-in with 2FA can take minutes. The backend callback dies at the same
// time, so a shorter frontend timer would cancel a login that can still
// succeed.
const OAUTH_LOGIN_TIMEOUT_MS = 300_000;

const agentRows: Array<{
  agentKind: AgentKind;
  installHint: string;
  managedInstall: AgentCliKind | null;
}> = [
  {
    agentKind: "codex",
    installHint: "npm i -g @openai/codex",
    managedInstall: "codex",
  },
  {
    agentKind: "claude",
    installHint: "npm i -g @anthropic-ai/claude-code",
    managedInstall: "claude",
  },
  {
    agentKind: "gemini",
    installHint: "npm i -g @google/gemini-cli",
    managedInstall: null,
  },
  {
    agentKind: "antigravity",
    installHint: "curl -fsSL https://antigravity.google/cli/install.sh | bash",
    managedInstall: null,
  },
  {
    agentKind: "kimi",
    installHint: "uv tool install --python 3.13 kimi-cli",
    managedInstall: null,
  },
  {
    agentKind: "zai",
    installHint: "npm i -g @anthropic-ai/claude-code",
    managedInstall: "claude",
  },
  {
    agentKind: "grok",
    installHint: "npm i -g @anthropic-ai/claude-code",
    managedInstall: "claude",
  },
];

export function ConnectGate({
  anthropic,
  auth,
  credentialProfiles,
  gateState,
  laneOptions,
  onAddGrokApiKey,
  onAddZaiApiKey,
  onConnectionsChanged,
  onContinue,
  onLoginChatGpt,
  onSetOpenAiApiKey,
  onSetAnthropicApiKey,
  probe,
  shortcutOverrides,
  sidecar,
}: ConnectGateProps) {
  const t = useT();
  const {
    error: cliInstallError,
    install: installAgentCli,
    pendingAgent: cliInstallPendingAgent,
    progress: cliInstallProgress,
    reset: resetAgentCliInstall,
  } = useAgentCliInstall();
  const [cliLoginPending, setCliLoginPending] = useState<AgentKind | null>(null);
  const [cliInstallRow, setCliInstallRow] = useState<AgentKind | null>(null);
  const [oauthPending, setOauthPending] = useState(false);
  const [oauthElapsedSeconds, setOauthElapsedSeconds] = useState(0);
  const [loginTerminalSessionId, setLoginTerminalSessionId] = useState<
    string | null
  >(null);
  const [apiKeySectionOpen, setApiKeySectionOpen] = useState(false);
  const [otherAgentsOpen, setOtherAgentsOpen] = useState(false);
  const [showZaiKey, setShowZaiKey] = useState(false);
  const [zaiApiKey, setZaiApiKey] = useState("");
  const [showGrokKey, setShowGrokKey] = useState(false);
  const [grokApiKey, setGrokApiKey] = useState("");
  const [openAiApiKey, setOpenAiApiKey] = useState("");
  const [anthropicApiKey, setAnthropicApiKey] = useState("");
  const [keyPending, setKeyPending] = useState<
    "zai" | "xai" | "openai" | "anthropic" | null
  >(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (error || cliInstallError) void recordAnalyticsEvent("setup_failed", "connection");
  }, [error, cliInstallError]);
  const [claudeSubscriptionProxy, setClaudeSubscriptionProxy] = useState(true);
  const [claudeSubscriptionProxyLoaded, setClaudeSubscriptionProxyLoaded] =
    useState(false);
  const [claudeSubscriptionProxyPending, setClaudeSubscriptionProxyPending] =
    useState(false);
  const [claudeSubscriptionProxyError, setClaudeSubscriptionProxyError] =
    useState<string | null>(null);
  const oauthAttemptRef = useRef(0);
  const oauthPendingRef = useRef(false);
  const oauthTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const apiKeySectionManuallyToggledRef = useRef(false);
  const connections = connectGateConnections(laneOptions, credentialProfiles);
  const apiKeySectionShouldOpen = shouldOpenApiKeySection(probe, connections);
  const chatGptConnected =
    connections.codex || Boolean(auth && "chatgpt" in auth);
  const codexProbeError = probeErrorText(probeForAgent(probe, "codex"));
  const claudeConnected = connections.claude;
  const claudeSubscriptionConnected = connections.claude;
  const kimiConnected = connections.kimi || connections.moonshot;
  const openAiConnected =
    connections.openai || Boolean(auth && "apiKey" in auth);
  const anthropicConnected =
    connections.anthropic ||
    Boolean(anthropic?.configured) ||
    Boolean(auth && "anthropicApiKey" in auth);
  const connectedByAgent: Record<AgentKind, boolean> = {
    antigravity: laneConnected(laneOptions, "antigravity"),
    claude: claudeConnected,
    codex: chatGptConnected,
    gemini: laneConnected(laneOptions, "gemini"),
    kimi: kimiConnected,
    zai: connections.zai,
    grok: connections.grok,
  };

  useEffect(() => {
    return () => {
      oauthAttemptRef.current += 1;
      clearOauthTimeout();
      if (oauthPendingRef.current) {
        oauthPendingRef.current = false;
        void ipc.auth.oauthCancel().catch(() => undefined);
      }
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    void ipc.preferences.get()
      .then((preferences) => {
        if (!disposed) {
          setClaudeSubscriptionProxy(
            preferences.claudeSubscriptionProxy ?? true,
          );
          setClaudeSubscriptionProxyLoaded(true);
          setClaudeSubscriptionProxyError(null);
        }
      })
      .catch((cause) => {
        if (!disposed) {
          setClaudeSubscriptionProxyLoaded(false);
          setClaudeSubscriptionProxyError(errorText(cause));
        }
      });
    return () => {
      disposed = true;
    };
  }, []);

  useEffect(() => {
    if (!oauthPending) {
      setOauthElapsedSeconds(0);
      return;
    }
    const startedAt = Date.now();
    const interval = setInterval(() => {
      setOauthElapsedSeconds(Math.floor((Date.now() - startedAt) / 1000));
    }, 1000);
    return () => clearInterval(interval);
  }, [oauthPending]);

  useEffect(() => {
    if (
      !apiKeySectionManuallyToggledRef.current &&
      apiKeySectionShouldOpen
    ) {
      setApiKeySectionOpen(true);
    }
  }, [apiKeySectionShouldOpen]);

  function clearOauthTimeout() {
    if (oauthTimeoutRef.current !== null) {
      clearTimeout(oauthTimeoutRef.current);
      oauthTimeoutRef.current = null;
    }
  }

  async function loginChatGpt() {
    const attemptId = oauthAttemptRef.current + 1;
    oauthAttemptRef.current = attemptId;
    clearOauthTimeout();
    oauthPendingRef.current = true;
    setOauthPending(true);
    setError(null);

    oauthTimeoutRef.current = setTimeout(() => {
      if (oauthAttemptRef.current !== attemptId) {
        return;
      }
      oauthAttemptRef.current += 1;
      oauthPendingRef.current = false;
      oauthTimeoutRef.current = null;
      void ipc.auth.oauthCancel().catch(() => undefined);
      setOauthPending(false);
      setError(t("ChatGPT sign-in timed out."));
    }, OAUTH_LOGIN_TIMEOUT_MS);

    try {
      await onLoginChatGpt();
      if (oauthAttemptRef.current === attemptId) {
        clearOauthTimeout();
      }
    } catch (cause) {
      if (oauthAttemptRef.current === attemptId) {
        clearOauthTimeout();
        void ipc.auth.oauthCancel().catch(() => undefined);
        setError(errorText(cause));
      }
    } finally {
      if (oauthAttemptRef.current === attemptId) {
        oauthPendingRef.current = false;
        setOauthPending(false);
      }
    }
  }

  function cancelChatGptLogin() {
    oauthAttemptRef.current += 1;
    clearOauthTimeout();
    oauthPendingRef.current = false;
    void ipc.auth.oauthCancel().catch(() => undefined);
    setOauthPending(false);
    setError(null);
  }

  function handleApiKeySectionToggle(
    event: React.SyntheticEvent<HTMLDetailsElement>,
  ) {
    const open = event.currentTarget.open;
    if (open === apiKeySectionOpen) {
      return;
    }
    apiKeySectionManuallyToggledRef.current = true;
    setApiKeySectionOpen(open);
  }

  async function launchLogin(agentKind: AgentKind) {
    setCliLoginPending(agentKind);
    setError(null);
    try {
      setLoginTerminalSessionId(await launchAgentLogin(agentKind));
    } catch (cause) {
      setError(errorText(cause));
    } finally {
      setCliLoginPending(null);
    }
  }

  async function handleAgentCliInstall(
    rowAgentKind: AgentKind,
    managedInstall: AgentCliKind,
  ) {
    setCliInstallRow(rowAgentKind);
    setError(null);
    const installed = await installAgentCli(managedInstall);
    if (!installed) {
      return;
    }
    try {
      await onConnectionsChanged();
    } catch (cause) {
      setError(errorText(cause));
    } finally {
      resetAgentCliInstall();
      setCliInstallRow(null);
    }
  }

  async function closeLoginTerminal() {
    setLoginTerminalSessionId(null);
    await onConnectionsChanged();
  }

  async function saveKey(
    event: React.FormEvent,
    provider: "zai" | "xai" | "openai" | "anthropic",
  ) {
    event.preventDefault();
    const apiKey =
      provider === "zai"
        ? zaiApiKey.trim()
        : provider === "xai"
          ? grokApiKey.trim()
          : provider === "openai"
            ? openAiApiKey.trim()
            : anthropicApiKey.trim();
    if (!apiKey) {
      return;
    }
    setKeyPending(provider);
    setError(null);
    try {
      if (provider === "zai") {
        await onAddZaiApiKey(apiKey);
        setZaiApiKey("");
        setShowZaiKey(false);
      } else if (provider === "xai") {
        await onAddGrokApiKey(apiKey);
        setGrokApiKey("");
        setShowGrokKey(false);
      } else if (provider === "openai") {
        await onSetOpenAiApiKey(apiKey);
        setOpenAiApiKey("");
      } else {
        await onSetAnthropicApiKey(apiKey);
        setAnthropicApiKey("");
      }
    } catch (cause) {
      setError(errorText(cause));
    } finally {
      setKeyPending(null);
    }
  }

  async function saveClaudeSubscriptionProxy(nextEnabled: boolean) {
    setClaudeSubscriptionProxyPending(true);
    setClaudeSubscriptionProxyError(null);
    try {
      const preferences = await ipc.preferences.setClaudeSubscriptionProxy({ claudeSubscriptionProxy: nextEnabled });
      setClaudeSubscriptionProxy(preferences.claudeSubscriptionProxy);
      setClaudeSubscriptionProxyLoaded(true);
    } catch (cause) {
      setClaudeSubscriptionProxyError(errorText(cause));
    } finally {
      setClaudeSubscriptionProxyPending(false);
    }
  }

  return (
    // The update card has no status bar to sit above here, so the gate
    // scrolls and keeps room for the card (--app-update-card-space).
    <main className="flex h-screen flex-col overflow-y-auto bg-surface-base px-6 pb-[max(2rem,var(--app-update-card-space,0px))] pt-4 text-content-primary">
      <section className="mx-auto my-auto flex w-full max-w-[560px] flex-col gap-[18px] py-6">
        <header className="flex flex-col gap-3">
          <div className="mb-4"><StepIndicator step={1} /></div>
          <h1 className="text-[30px] font-semibold leading-tight tracking-[-0.025em] text-content-primary">
            {gateState === "optional" ? t("You're connected. Let's use it.") : t("Bring the account you already use.")}
          </h1>
          <p className="text-sm leading-[22px] text-content-secondary">
            {t(
              "Connect one account to start. You can add more later in Settings.",
            )}
          </p>
        </header>

        {gateState === "optional" ? (
          <div className="flex items-center justify-between gap-4 rounded-xl border border-accent/30 bg-accent/5 px-4 py-3" role="status">
            <p className="text-sm font-medium text-content-primary">{t("One account connected. You're ready.")}</p>
            <button className="shrink-0 rounded-lg bg-accent px-4 py-2.5 text-sm font-semibold text-accent-contrast hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40" onClick={onContinue} type="button">{t("Choose my first task")} →</button>
          </div>
        ) : null}
        <div className="relative flex flex-wrap items-center gap-3.5 rounded-xl border border-accent/35 bg-surface-raised px-[18px] py-4">
          <span className="absolute -top-[9px] left-4 rounded-full bg-accent-muted px-2 py-0.5 font-mono text-[10px] font-bold uppercase tracking-[0.14em] text-accent-strong">
            {t("Recommended")}
          </span>
          <span className="flex h-[38px] w-[38px] shrink-0 items-center justify-center rounded-full bg-accent-muted text-accent">
            <svg
              aria-hidden="true"
              className="h-[18px] w-[18px]"
              fill="none"
              stroke="currentColor"
              strokeLinecap="round"
              strokeLinejoin="round"
              strokeWidth="2"
              viewBox="0 0 24 24"
            >
              <path d="M13 2 4 14h6l-1 8 9-12h-6l1-8Z" />
            </svg>
          </span>
          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-baseline gap-x-2">
              <span className="text-sm font-semibold text-content-primary">
                {t("ChatGPT")}
              </span>
              <span className="text-xs text-content-muted">{t("Codex")}</span>
            </div>
            <p className="mt-0.5 text-xs leading-[18px] text-content-muted">
              {t(
                "No CLI needed — sign in and Halv uses your ChatGPT plan limits.",
              )}
            </p>
            {codexProbeError ? (
              <p className="mt-1 text-xs leading-[18px] text-content-muted" role="status">
                {t("{agent} CLI check failed: {error}", {
                  agent: t("Codex"),
                  error: codexProbeError,
                })}
              </p>
            ) : null}
          </div>
          {!chatGptConnected ? (
            <div className="flex shrink-0 flex-wrap items-center justify-end gap-2">
              <button
                className="rounded-md bg-content-primary px-4 py-[9px] text-[13px] font-semibold text-surface-base transition-colors hover:bg-content-secondary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-60"
                disabled={oauthPending || cliLoginPending !== null}
                onClick={() => void loginChatGpt()}
                type="button"
              >
                {oauthPending
                  ? t("Waiting for ChatGPT…")
                  : t("Sign in with ChatGPT")}
              </button>
              {oauthPending ? (
                <>
                  <span className="text-xs tabular-nums text-content-muted" role="timer">
                    {t("{seconds}s elapsed", { seconds: oauthElapsedSeconds })}
                  </span>
                  <button
                    className="px-1 py-1.5 text-xs font-medium text-content-muted underline-offset-2 hover:text-content-primary hover:underline focus:outline-none focus:ring-2 focus:ring-accent/30"
                    onClick={cancelChatGptLogin}
                    type="button"
                  >
                    {t("Cancel")}
                  </button>
                </>
              ) : null}
            </div>
          ) : (
            <span className="shrink-0 text-xs font-medium text-accent">
              {t("Connected")}
            </span>
          )}
        </div>

        {oauthPending ? <p className="rounded-lg border border-accent/20 bg-accent/5 px-4 py-3 text-xs leading-5 text-content-secondary" role="status">{t("Finish signing in in your browser, then return here. This screen will update automatically.")}</p> : null}
        <div className="flex items-center justify-between gap-3">
          <p className="text-xs font-medium text-content-secondary">{t("Or use another account")}</p>
          <button aria-expanded={otherAgentsOpen} className="rounded py-1 text-xs text-accent underline underline-offset-4 focus:outline-none focus:ring-2 focus:ring-accent/40" onClick={() => setOtherAgentsOpen(!otherAgentsOpen)} type="button">{otherAgentsOpen ? t("Show fewer options") : t("Kimi, GLM, or Grok")}</button>
        </div>

        <div className="divide-y divide-surface-border/20 overflow-hidden rounded-lg border border-surface-border/30 bg-surface-panel">
          {agentRows.filter((row) => row.agentKind !== "codex" && (row.agentKind === "claude" || connectedByAgent[row.agentKind] || otherAgentsOpen)).map((row) => {
            const binary = probeForAgent(probe, row.agentKind);
            const connected = connectedByAgent[row.agentKind];
            const cliMissing = binary?.found === false;
            const probeError = probeErrorText(binary);
            const pending = cliLoginPending === row.agentKind;
            const managedInstall = row.managedInstall;
            const rowInstalling =
              managedInstall !== null &&
              cliInstallRow === row.agentKind &&
              cliInstallPendingAgent === managedInstall;
            const rowInstallError =
              cliInstallRow === row.agentKind ? cliInstallError : null;
            const cliProgressPercent = rowInstalling
              ? agentCliInstallProgressPercent(cliInstallProgress)
              : null;
            const status = connected
              ? t("Connected")
              : agentNeedsCliToRun(row.agentKind) && cliMissing
                ? t("CLI not installed")
                : t("Not connected");
            return (
              <div className="px-4 py-3" key={row.agentKind}>
                <div className="flex flex-wrap items-center gap-3">
                  <ConnectionCheck connected={connected} />
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-baseline gap-x-2">
                      <span className="text-sm font-semibold text-content-primary">
                        {agentLabel(row.agentKind, t)}
                      </span>
                      <span className="text-xs text-content-muted">
                        {agentDetail(row.agentKind, t)}
                      </span>
                    </div>
                    <p
                      className={`mt-0.5 text-xs ${
                        connected ? "text-accent" : "text-content-muted"
                      }`}
                    >
                      {status}
                    </p>
                    {probeError ? (
                      <p className="mt-0.5 text-xs text-content-muted" role="status">
                        {t("{agent} CLI check failed: {error}", {
                          agent: agentLabel(row.agentKind, t),
                          error: probeError,
                        })}
                      </p>
                    ) : null}
                  </div>
                  {!connected ? (
                    row.agentKind === "zai" || row.agentKind === "grok" ? (
                      <button
                        className="rounded-md border border-surface-border/30 px-3 py-1.5 text-xs font-semibold text-content-secondary transition-colors hover:bg-surface-panel hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
                        onClick={() => {
                          if (row.agentKind === "zai") {
                            setShowZaiKey((current) => !current);
                          } else {
                            setShowGrokKey((current) => !current);
                          }
                          setError(null);
                        }}
                        type="button"
                      >
                        {row.agentKind === "zai"
                          ? t("Add GLM key")
                          : t("Add Grok key")}
                      </button>
                    ) : (
                      <button
                        className="rounded-md border border-surface-border/30 px-3 py-1.5 text-xs font-semibold text-content-secondary transition-colors hover:bg-surface-panel hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
                        disabled={
                          cliMissing ||
                          oauthPending ||
                          cliLoginPending !== null
                        }
                        onClick={() => void launchLogin(row.agentKind)}
                        title={
                          cliMissing
                            ? t("Install the CLI before you log in.")
                            : undefined
                        }
                        type="button"
                      >
                        {pending ? t("Opening…") : t("Log in")}
                      </button>
                    )
                  ) : null}
                </div>
                {cliMissing && managedInstall !== null ? (
                  <div className="mt-2 space-y-1.5 pl-9">
                    <button
                      className="rounded-md border border-surface-border/30 px-3 py-1.5 text-xs font-semibold text-content-secondary transition-colors hover:bg-surface-panel hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
                      disabled={cliInstallPendingAgent !== null}
                      onClick={() =>
                        void handleAgentCliInstall(
                          row.agentKind,
                          managedInstall,
                        )
                      }
                      type="button"
                    >
                      {rowInstalling
                        ? t("Installing…")
                        : t("Install {agent}", {
                            agent:
                              managedInstall === "claude"
                                ? t("Claude Code")
                                : t("Codex"),
                          })}
                    </button>
                    {rowInstalling ? (
                      <div className="space-y-1.5" aria-live="polite">
                        <p
                          className="text-xs text-content-secondary"
                          role="status"
                        >
                        {cliProgressPercent !== null
                          ? t("{percent}%", {
                              percent: formatNumber(cliProgressPercent),
                            })
                          : cliInstallProgress?.line ??
                            t("Preparing download…")}
                        </p>
                        {cliProgressPercent !== null ? (
                          <div
                          aria-label={t("{agent} CLI download progress", {
                            agent: agentLabel(row.agentKind, t),
                          })}
                            aria-valuemax={100}
                            aria-valuemin={0}
                            aria-valuenow={cliProgressPercent}
                            className="h-1.5 overflow-hidden rounded-full bg-surface-border/30"
                            role="progressbar"
                          >
                            <div
                              className="h-full rounded-full bg-accent transition-[width] duration-150"
                              style={{ width: `${cliProgressPercent}%` }}
                            />
                          </div>
                        ) : null}
                      </div>
                    ) : null}
                    {rowInstallError ? (
                      <>
                        <p
                          className="rounded-md border border-danger/40 bg-danger-surface/55 px-3 py-2 text-sm text-danger"
                          role="alert"
                        >
                          {rowInstallError.message}
                        </p>
                      <p className="font-mono text-[11px] text-content-muted">
                        {t("Or install manually: {command}", {
                          command: row.installHint,
                        })}
                        </p>
                      </>
                    ) : null}
                  </div>
                ) : null}
                {cliMissing && managedInstall === null ? (
                  <div className="mt-2 space-y-1.5 pl-9">
                    <p className="text-xs text-content-muted">
                      {t("Install the CLI before you log in.")}
                    </p>
                    <code className="block overflow-x-auto rounded bg-surface-base px-2 py-1 text-[11px] text-content-secondary">
                      {row.installHint}
                    </code>
                  </div>
                ) : null}
                {cliMissing && managedInstall === null ? (
                  <p className="mt-2 pl-9 font-mono text-[11px] text-content-muted">
                    {t("Install: {command}", { command: row.installHint })}
                  </p>
                ) : null}
                {cliMissing && row.agentKind === "claude" ? (
                  <p className="mt-1 pl-9 text-[11px] text-content-muted">
                    {connected
                      ? t(
                          "Claude Code CLI not found — install to run sessions",
                        )
                      : t(
                          "Claude Pro/Max sign-in works only through the Claude Code CLI.",
                        )}
                  </p>
                ) : null}
                {row.agentKind === "zai" && showZaiKey ? (
                  <form
                    className="mt-3 flex flex-col gap-2 pl-9 sm:flex-row"
                    onSubmit={(event) => void saveKey(event, "zai")}
                  >
                    <p className="w-full text-xs text-content-muted sm:basis-full">
                      {t("Uses the API key from your GLM Coding Plan (Z.ai).")}
                    </p>
                    <input
                      aria-label={t("GLM Coding Plan API key")}
                      autoComplete="off"
                      className="min-w-0 flex-1 rounded-md border border-surface-border/30 bg-surface-raised px-3 py-2 text-sm text-content-primary outline-none placeholder:text-content-muted focus:border-accent focus:ring-2 focus:ring-accent/30"
                      disabled={keyPending !== null}
                      onChange={(event) => setZaiApiKey(event.target.value)}
                      placeholder={t("GLM Coding Plan API key")}
                      type="password"
                      value={zaiApiKey}
                    />
                    <button
                      className="rounded-md bg-content-primary px-3 py-2 text-sm font-semibold text-surface-base disabled:cursor-not-allowed disabled:opacity-60"
                      disabled={keyPending !== null || !zaiApiKey.trim()}
                      type="submit"
                    >
                      {keyPending === "zai"
                        ? t("Validating…")
                        : t("Add key")}
                    </button>
                  </form>
                ) : null}
                {row.agentKind === "grok" && showGrokKey ? (
                  <form
                    className="mt-3 flex flex-col gap-2 pl-9 sm:flex-row"
                    onSubmit={(event) => void saveKey(event, "xai")}
                  >
                  <p className="w-full text-xs text-content-muted sm:basis-full">
                    {t("Uses your xAI API key to run Grok through Claude Code.")}
                  </p>
                  <input
                    aria-label={t("xAI API key")}
                      autoComplete="off"
                      className="min-w-0 flex-1 rounded-md border border-surface-border/30 bg-surface-raised px-3 py-2 text-sm text-content-primary outline-none placeholder:text-content-muted focus:border-accent focus:ring-2 focus:ring-accent/30"
                      disabled={keyPending !== null}
                      onChange={(event) => setGrokApiKey(event.target.value)}
                    placeholder={t("xAI API key")}
                      type="password"
                      value={grokApiKey}
                    />
                    <button
                      className="rounded-md bg-content-primary px-3 py-2 text-sm font-semibold text-surface-base disabled:cursor-not-allowed disabled:opacity-60"
                      disabled={keyPending !== null || !grokApiKey.trim()}
                      type="submit"
                    >
                    {keyPending === "xai" ? t("Validating…") : t("Add key")}
                    </button>
                  </form>
                ) : null}
              </div>
            );
          })}
        </div>

        {claudeSubscriptionConnected ? (
          <details className="space-y-2 rounded-lg border border-accent/25 bg-accent/5 px-4 py-3">
            <summary className="cursor-pointer text-xs font-medium text-content-secondary">{t("Claude context compression settings")}</summary>
            <p className="font-mono text-[10px] uppercase tracking-[0.16em] text-accent">
              {t("Optional")}
            </p>
            <label className="flex cursor-pointer items-start gap-3 text-sm text-content-primary">
              <input
                aria-describedby="connect-claude-subscription-proxy-warning"
                checked={claudeSubscriptionProxy}
                className="mt-0.5 accent-accent"
                disabled={
                  !claudeSubscriptionProxyLoaded ||
                  claudeSubscriptionProxyPending
                }
                onChange={(event) =>
                  void saveClaudeSubscriptionProxy(event.target.checked)
                }
                type="checkbox"
              />
              <span>
                <span className="font-medium">
                  {t("Route Claude subscription through Halv Engine")}
                </span>
                <span
                  className="mt-1 block text-xs leading-5 text-content-muted"
                  id="connect-claude-subscription-proxy-warning"
                >
                  {t(
                    "Halv Engine will compress your Claude Code traffic before it reaches Anthropic. Anthropic's consumer terms may not allow modifying subscription traffic; enable at your own risk. Only affects new terminal sessions.",
                  )}
                </span>
              </span>
            </label>
            <details className="pl-7 text-xs text-content-secondary">
              <summary className="cursor-pointer font-medium text-accent focus:outline-none focus:ring-2 focus:ring-accent/30">
                {t("Learn more")}
              </summary>
              <p className="mt-2 leading-5">
                {t(
                  "Headroom compresses repeated and low-value context before Claude Code sends it to Anthropic, which reduces input tokens. Anthropic's consumer terms may not allow modified subscription traffic, so this option carries terms risk and stays off by default.",
                )}
              </p>
            </details>
            {claudeSubscriptionProxyError ? (
              <p className="pl-7 text-xs text-danger" role="alert">
                {claudeSubscriptionProxyError}
              </p>
            ) : null}
          </details>
        ) : null}

        <details
          className="group"
          onToggle={handleApiKeySectionToggle}
          open={apiKeySectionOpen}
        >
          <summary className="w-fit cursor-pointer list-none text-xs text-content-secondary underline underline-offset-2 focus:outline-none focus:ring-2 focus:ring-accent/30">
            {t(
              "Prefer an API key? Add an OpenAI or Anthropic key instead",
            )}
          </summary>
          <div className="mt-3 divide-y divide-surface-border/20 rounded-lg border border-surface-border/25 bg-surface-raised/20 px-4 py-3">
            <ApiKeyRow
              connected={openAiConnected}
              disabled={keyPending !== null}
              label={t("OpenAI")}
              onChange={setOpenAiApiKey}
              onSubmit={(event) => void saveKey(event, "openai")}
              pending={keyPending === "openai"}
              placeholder="sk-..."
              value={openAiApiKey}
            />
            <ApiKeyRow
              connected={anthropicConnected}
              disabled={keyPending !== null}
              label={t("Anthropic")}
              onChange={setAnthropicApiKey}
              onSubmit={(event) => void saveKey(event, "anthropic")}
              pending={keyPending === "anthropic"}
              placeholder="sk-ant-..."
              value={anthropicApiKey}
            />
          </div>
        </details>

        {error ? (
          <p
            className="rounded-md border border-danger/40 bg-danger-surface/55 px-3 py-2 text-sm text-danger"
            role="alert"
          >
            {error}
          </p>
        ) : null}

        <footer className="border-t border-surface-border/20 pt-4 text-xs leading-5 text-content-muted">
          {t("Your AI provider's plan or API usage limits still apply.")}
        </footer>

        {loginTerminalSessionId ? (
          <div className="flex h-[26rem] min-h-0 overflow-hidden rounded-lg border border-surface-border/30 bg-surface-sunken">
            <TerminalPanel
              focusSessionId={loginTerminalSessionId}
              onClose={() => void closeLoginTerminal()}
              open
              project={null}
              shortcutOverrides={shortcutOverrides}
              workspaceId={null}
            />
          </div>
        ) : null}

        {sidecar?.state !== "running" ? (
          <p className="border-t border-surface-border/20 pt-4 text-center text-xs text-content-muted">
            {t("Starting the local engine…")}
          </p>
        ) : null}
      </section>
    </main>
  );
}

function ApiKeyRow({
  connected,
  disabled,
  label,
  onChange,
  onSubmit,
  pending,
  placeholder,
  value,
}: {
  connected: boolean;
  disabled: boolean;
  label: string;
  onChange: (value: string) => void;
  onSubmit: (event: React.FormEvent) => void;
  pending: boolean;
  placeholder: string;
  value: string;
}) {
  const t = useT();
  return (
    <form className="py-3 first:pt-0 last:pb-0" onSubmit={onSubmit}>
      <div className="flex items-center gap-3">
        <ConnectionCheck connected={connected} />
        <div className="min-w-0 flex-1">
          <p className="text-sm font-semibold text-content-primary">{label}</p>
          <p
            className={`mt-0.5 text-xs ${
              connected ? "text-accent" : "text-content-muted"
            }`}
          >
            {connected ? t("Connected") : t("Not connected")}
          </p>
        </div>
      </div>
      {!connected ? (
        <div className="mt-3 flex flex-col gap-2 pl-9 sm:flex-row">
          <input
            aria-label={t("{label} API key", { label })}
            autoComplete="off"
            className="min-w-0 flex-1 rounded-md border border-surface-border/30 bg-surface-raised px-3 py-2 text-sm text-content-primary outline-none placeholder:text-content-muted focus:border-accent focus:ring-2 focus:ring-accent/30"
            disabled={disabled}
            onChange={(event) => onChange(event.target.value)}
            placeholder={placeholder}
            type="password"
            value={value}
          />
          <button
            className="rounded-md bg-content-primary px-3 py-2 text-sm font-semibold text-surface-base disabled:cursor-not-allowed disabled:opacity-60"
            disabled={disabled || !value.trim()}
            type="submit"
          >
            {pending ? t("Saving…") : t("Save")}
          </button>
        </div>
      ) : null}
    </form>
  );
}

function agentLabel(agentKind: AgentKind, t: Translate) {
  switch (agentKind) {
    case "antigravity":
      return "Antigravity";
    case "codex":
      return t("ChatGPT");
    case "gemini":
      return "Gemini";
    case "claude":
      return t("Claude");
    case "kimi":
      return t("Kimi");
    case "zai":
      return t("GLM");
    case "grok":
      return t("Grok");
  }
}

function agentDetail(agentKind: AgentKind, t: Translate) {
  switch (agentKind) {
    case "antigravity":
      return "Antigravity CLI";
    case "codex":
      return t("Codex");
    case "gemini":
      return "Gemini CLI";
    case "claude":
      return t("Claude Code");
    case "kimi":
      return t("Kimi Code");
    case "zai":
      return t("Z.ai Coding Plan");
    case "grok":
      return t("xAI API");
  }
}

function laneConnected(lanes: AgentSessionLaneOptions, agentKind: AgentKind) {
  return (
    lanes
      .find((option) => option.agentKind === agentKind)
      ?.lanes.some((lane) => lane.loggedIn === true) ?? false
  );
}

function ConnectionCheck({ connected }: { connected: boolean }) {
  return (
    <span
      aria-hidden
      className={`flex h-[22px] w-[22px] shrink-0 items-center justify-center rounded-full border ${
        connected
          ? "border-accent/60 bg-accent/15 text-accent"
          : "border-surface-border/40 text-content-muted"
      }`}
    >
      {connected ? (
        <svg
          className="h-3.5 w-3.5"
          fill="none"
          stroke="currentColor"
          strokeLinecap="round"
          strokeLinejoin="round"
          strokeWidth="2"
          viewBox="0 0 24 24"
        >
          <path d="m5 12 4 4L19 6" />
        </svg>
      ) : null}
    </span>
  );
}

function agentCliInstallProgressPercent(
  progress: AgentCliInstallProgress | null,
) {
  if (
    typeof progress?.downloadedBytes !== "number" ||
    typeof progress.totalBytes !== "number" ||
    progress.totalBytes <= 0
  ) {
    return null;
  }
  return Math.min(
    100,
    Math.max(
      0,
      Math.round((progress.downloadedBytes / progress.totalBytes) * 100),
    ),
  );
}

// A CLI that exists but fails `--version` stays on its row with the reason.
// Windows picks a broken npm shim this way (os error 193).
function probeErrorText(binary: AgentBinaryProbe | null): string | null {
  if (!binary || !binary.found) {
    return null;
  }
  const error = binary.error?.trim();
  return error ? error : null;
}

function errorText(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}
