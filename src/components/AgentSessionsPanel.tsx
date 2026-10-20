import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  agentSessionModeForProMode,
  agentSessionProModeStore,
  agentSessionStore,
  agentSessionTitle,
  buildAgentSessionLane,
  defaultLaneSelectionForAgent,
  laneOptionsForAgent,
  launchAgentLogin,
  probeForAgent,
  probeForGhostty,
  type AgentKind,
  type AgentSession,
  type AgentSessionCreateInput,
  type AgentSessionCreateLaneKind,
  type AgentSessionLaneOption,
} from "../agentSessions";
import { useT, type Translate } from "../i18n/runtime";
import type { KeyboardShortcutOverrides } from "../shortcuts";
import {
  AgentSessionView,
  StatusIndicator,
  agentLabel,
  agentSubtitle,
  laneLabel,
} from "./AgentSessionView";
import { AgentSessionProModeToggle } from "./AgentSessionProModeToggle";
import { TerminalPanel } from "./TerminalPanel";

type AgentSessionsPanelProps = {
  conversationId: string | null;
  conversationModel: string | null;
  conversationTitle: string | null;
  defaultCwd: string;
  onClose: () => void;
  onManageKeys: () => void;
  open: boolean;
  projectId: number | null;
  shortcutOverrides: KeyboardShortcutOverrides;
  /** With workspace tabs: an agent of a conversation shows in the tab of
   *  that conversation, never in a second panel here (ADR 0010, P5). */
  onOpenInTab?: (conversationId: string, sessionId: string) => void;
};

const agentKinds: AgentKind[] = [
  "claude",
  "codex",
  "gemini",
  "antigravity",
  "kimi",
  "zai",
  "grok",
];

export function AgentSessionsPanel({
  conversationId,
  conversationModel,
  conversationTitle,
  defaultCwd,
  onClose,
  onManageKeys,
  open,
  projectId,
  shortcutOverrides,
  onOpenInTab,
}: AgentSessionsPanelProps) {
  const t = useT();
  const state = useSyncExternalStore(
    agentSessionStore.subscribe,
    agentSessionStore.getSnapshot,
  );
  const [newSessionOpen, setNewSessionOpen] = useState(false);
  const [agentKind, setAgentKind] = useState<AgentKind>("claude");
  const [laneKind, setLaneKind] =
    useState<AgentSessionCreateLaneKind>("claudeSubscription");
  const [credentialProfileId, setCredentialProfileId] = useState<string | null>(
    null,
  );
  const proMode = useSyncExternalStore(
    agentSessionProModeStore.subscribe,
    agentSessionProModeStore.getSnapshot,
  );
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [cwd, setCwd] = useState(defaultCwd);
  const [customEnv, setCustomEnv] = useState("");
  const [creating, setCreating] = useState(false);
  const [openingAgentKind, setOpeningAgentKind] = useState<AgentKind | null>(null);
  const [loginPending, setLoginPending] = useState(false);
  const [loginTerminalSessionId, setLoginTerminalSessionId] = useState<
    string | null
  >(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [killingSessionIds, setKillingSessionIds] = useState<Set<string>>(
    () => new Set(),
  );
  const selectedEntry = useMemo(
    () =>
      state.entries.find(
        (entry) => entry.session.id === state.selectedSessionId,
      ) ?? state.entries[0] ?? null,
    [state.entries, state.selectedSessionId],
  );
  const agentLanes = useMemo(
    () => laneOptionsForAgent(state.laneOptions, agentKind),
    [agentKind, state.laneOptions],
  );
  const selectedLane =
    agentLanes.find((lane) => lane.kind === laneKind) ?? null;
  const googleAgent = isGoogleAgent(agentKind);
  const standardLanes = agentLanes.filter(
    (lane) => lane.kind !== "custom" || googleAgent,
  );
  const customLane = googleAgent
    ? null
    : agentLanes.find((lane) => lane.kind === "custom") ?? null;
  const ghostty = probeForGhostty(state.probe);
  const mode = agentSessionModeForProMode(proMode, ghostty);
  const activeProfiles =
    selectedLane?.credentialProfiles.filter(
      (profile) => profile.status === "active",
    ) ?? [];

  useEffect(() => {
    if (open) {
      void agentSessionStore.refreshLaneOptions().catch(() => undefined);
    }
  }, [open]);

  useEffect(() => {
    if (!newSessionOpen && defaultCwd) {
      setCwd(defaultCwd);
    }
  }, [defaultCwd, newSessionOpen]);

  useEffect(() => {
    if (agentLanes.length === 0) {
      return;
    }
    if (!agentLanes.some((lane) => lane.kind === laneKind)) {
      const selection = defaultLaneSelectionForAgent(
        state.laneOptions,
        agentKind,
      );
      setLaneKind(selection.laneKind);
      setCredentialProfileId(selection.credentialProfileId);
    }
  }, [agentKind, agentLanes, laneKind, state.laneOptions]);

  useEffect(() => {
    if (!isApiLane(selectedLane)) {
      setCredentialProfileId(null);
      return;
    }
    if (!activeProfiles.some((profile) => profile.id === credentialProfileId)) {
      setCredentialProfileId(activeProfiles[0]?.id ?? null);
    }
  }, [activeProfiles, credentialProfileId, selectedLane]);

  useEffect(() => {
    if (!open) {
      return;
    }
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        onClose();
      }
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [onClose, open]);

  if (!open) {
    return null;
  }

  async function browseForFolder() {
    try {
      const selected = await openDialog({
        directory: true,
        multiple: false,
        title: t("Choose an agent session folder"),
      });
      if (typeof selected === "string") {
        setCwd(selected);
      }
    } catch (error) {
      setFormError(errorMessage(error));
    }
  }

  async function createSession(
    launch: {
      agentKind: AgentKind;
      credentialProfileId: string | null;
      cwd: string;
      laneKind: AgentSessionCreateLaneKind;
    } = { agentKind, credentialProfileId, cwd, laneKind },
  ) {
    const path = launch.cwd.trim();
    if (!path) {
      setFormError(t("Choose a folder for this session."));
      return;
    }
    const launchLane = laneOptionsForAgent(
      state.laneOptions,
      launch.agentKind,
    ).find((lane) => lane.kind === launch.laneKind);
    if (!launchLane) {
      setFormError(t("Choose an available connection."));
      return;
    }

    let lane: AgentSessionCreateInput["lane"];
    try {
      lane = buildAgentSessionLane(
        launchLane.kind,
        launch.credentialProfileId,
        isGoogleAgent(launch.agentKind) ? "" : customEnv,
      );
    } catch (error) {
      setFormError(errorMessage(error));
      return;
    }

    setCreating(true);
    setFormError(null);
    const input: AgentSessionCreateInput = {
      agentKind: launch.agentKind,
      lane,
      cwd: path,
      ...(projectId === null ? {} : { projectId }),
      ...(conversationId && conversationModel
        ? {
            conversationId,
            conversationTitle: conversationTitle ?? t("Conversation"),
            model: conversationModel,
          }
        : {}),
      mode,
    };
    try {
      await agentSessionStore.create(input);
      setNewSessionOpen(false);
      setCustomEnv("");
    } catch {
      // The shared store displays the command error.
    } finally {
      setCreating(false);
    }
  }

  function selectDefaultAgent(kind: AgentKind) {
    const selection = defaultLaneSelectionForAgent(state.laneOptions, kind);
    setAgentKind(kind);
    setLaneKind(selection.laneKind);
    setCredentialProfileId(selection.credentialProfileId);
    setAdvancedOpen(false);
    return selection;
  }

  async function openAgentTerminal(kind: AgentKind) {
    const selection = selectDefaultAgent(kind);
    setOpeningAgentKind(kind);
    try {
      await createSession({
        agentKind: kind,
        credentialProfileId: selection.credentialProfileId,
        cwd: defaultCwd,
        laneKind: selection.laneKind,
      });
    } finally {
      setOpeningAgentKind(null);
    }
  }

  async function launchLogin() {
    setLoginPending(true);
    setFormError(null);
    try {
      setLoginTerminalSessionId(await launchAgentLogin(agentKind));
    } catch (error) {
      setFormError(errorMessage(error));
    } finally {
      setLoginPending(false);
    }
  }

  async function closeLoginTerminal() {
    setLoginTerminalSessionId(null);
    await agentSessionStore.refreshLaneOptions().catch(() => undefined);
  }

  async function killSession(sessionId: string) {
    setKillingSessionIds((current) => new Set(current).add(sessionId));
    try {
      await agentSessionStore.kill(sessionId);
    } catch {
      // The shared store displays the command error.
    } finally {
      setKillingSessionIds((current) => {
        const next = new Set(current);
        next.delete(sessionId);
        return next;
      });
    }
  }

  const createDisabled =
    creating ||
    !cwd.trim() ||
    !selectedLane ||
    (isApiLane(selectedLane) && credentialProfileId === null) ||
    (isSubscriptionLane(agentKind, selectedLane.kind) &&
      selectedLane.loggedIn === false);

  function quickOpenDisabled(kind: AgentKind) {
    const selection = defaultLaneSelectionForAgent(state.laneOptions, kind);
    const lane = laneOptionsForAgent(state.laneOptions, kind).find(
      (option) => option.kind === selection.laneKind,
    );
    const probe = probeForAgent(state.probe, kind);
    return (
      creating ||
      !defaultCwd.trim() ||
      !lane ||
      probe?.found === false ||
      (isApiLane(lane) && selection.credentialProfileId === null) ||
      (isSubscriptionLane(kind, lane.kind) && lane.loggedIn !== true)
    );
  }

  return (
    <div className="agent-sessions-overlay">
      <section
        aria-labelledby="agent-sessions-title"
        aria-modal="true"
        className="agent-sessions-dialog"
        role="dialog"
      >
        <aside className="agent-sessions-sidebar">
          <div className="agent-sessions-sidebar-header">
            <div>
              <h2 id="agent-sessions-title">{t("Agent sessions")}</h2>
              <p>{t("Run official agent tools in your project.")}</p>
            </div>
            <button
              aria-label={t("Close agent sessions")}
              className="agent-sessions-close"
              onClick={onClose}
              type="button"
            >
              ×
            </button>
          </div>

          <AgentSessionProModeToggle ghostty={ghostty} variant="panel" />

          <button
            className="agent-session-new-button"
            onClick={() => {
              setNewSessionOpen((current) => {
                if (!current) {
                  selectDefaultAgent(agentKind);
                }
                return !current;
              });
              setFormError(null);
            }}
            type="button"
          >
            <span aria-hidden>{newSessionOpen ? "−" : "+"}</span>
            {newSessionOpen
              ? proMode
                ? t("Cancel")
                : t("Cancel new session")
              : proMode
                ? t("Open agent terminal")
                : t("New session")}
          </button>

          {newSessionOpen ? (
            <form
              className={`agent-session-new-form${
                proMode ? " agent-session-terminal-form" : ""
              }`}
              onSubmit={(event) => {
                event.preventDefault();
                void createSession();
              }}
            >
              <fieldset disabled={creating || loginPending}>
                <legend>{t("1. Agent")}</legend>
                <div className="agent-session-choice-grid agent-session-agent-grid">
                  {agentKinds.map((kind) => {
                    const probe = probeForAgent(state.probe, kind);
                    const options = laneOptionsForAgent(state.laneOptions, kind);
                    return (
                      <button
                        aria-pressed={agentKind === kind}
                        className="agent-session-choice agent-session-agent-card"
                        key={kind}
                        onClick={() => selectDefaultAgent(kind)}
                        title={probe?.error ?? probe?.version ?? undefined}
                        type="button"
                      >
                        <span>
                          {openingAgentKind === kind
                            ? t("Opening terminal…")
                            : agentLabel(kind)}
                        </span>
                        <small>
                          {agentSubtitle(kind, t) ??
                            agentCardStatus(kind, options, probe?.found, t)}
                        </small>
                      </button>
                    );
                  })}
                </div>
              </fieldset>

              <details
                className="agent-session-connection-disclosure"
                onToggle={(event) => {
                  if (proMode) {
                    setAdvancedOpen(event.currentTarget.open);
                  }
                }}
                open={!proMode || advancedOpen}
              >
                {proMode ? <summary>{t("Advanced options")}</summary> : null}
                <fieldset disabled={creating || loginPending}>
                  <legend>
                    {proMode ? t("Connection") : t("2. Connection")}
                  </legend>
                  <select
                    aria-label={t("Session connection")}
                    disabled={standardLanes.length === 0}
                    onChange={(event) => {
                      setLaneKind(
                        event.target.value as AgentSessionCreateLaneKind,
                      );
                      setAdvancedOpen(false);
                      setFormError(null);
                    }}
                    value={laneKind === "custom" && !googleAgent ? "" : laneKind}
                  >
                    {laneKind === "custom" && !googleAgent ? (
                      <option value="">
                        {t("Custom environment (Advanced)")}
                      </option>
                    ) : null}
                    {standardLanes.length === 0 && laneKind !== "custom" ? (
                      <option value="">{t("No connections available")}</option>
                    ) : null}
                    {standardLanes.map((lane) => (
                      <option key={lane.kind} value={lane.kind}>
                        {laneOptionLabel(agentKind, lane.kind, t)}
                      </option>
                    ))}
                  </select>

                {isSubscriptionLane(agentKind, selectedLane?.kind) ? (
                  <div className="agent-session-account-status">
                    <span>
                      {loginStatusLabel(
                        agentKind,
                        selectedLane?.loggedIn ?? null,
                        t,
                      )}
                    </span>
                    {selectedLane?.loggedIn !== true ? (
                      <button
                        disabled={loginPending}
                        onClick={() => void launchLogin()}
                        type="button"
                      >
                        {loginPending ? t("Opening…") : t("Log in")}
                      </button>
                    ) : null}
                  </div>
                ) : null}

                  {isApiLane(selectedLane) ? (
                  activeProfiles.length === 0 ? (
                    <button
                      className="agent-session-add-key"
                      onClick={onManageKeys}
                      type="button"
                    >
                      {t("Add a key in Settings")}
                    </button>
                  ) : (
                    <div className="agent-session-profile-row">
                      <select
                        aria-label={t("Credential profile")}
                        onChange={(event) =>
                          setCredentialProfileId(event.target.value || null)
                        }
                        value={credentialProfileId ?? ""}
                      >
                        {activeProfiles.map((profile) => (
                          <option key={profile.id} value={profile.id}>
                            {profile.name}
                          </option>
                        ))}
                      </select>
                      <button onClick={onManageKeys} type="button">
                        {t("Manage keys…")}
                      </button>
                    </div>
                  )
                ) : null}

                {customLane && !proMode ? (
                  <details
                    className="agent-session-advanced"
                    onToggle={(event) =>
                      setAdvancedOpen(event.currentTarget.open)
                    }
                    open={advancedOpen}
                  >
                    <summary>{t("Advanced")}</summary>
                    <button
                      aria-pressed={laneKind === "custom"}
                      className="agent-session-custom-choice"
                      onClick={() => setLaneKind("custom")}
                      type="button"
                    >
                      {t("Custom environment")}
                    </button>
                    {laneKind === "custom" ? (
                      <textarea
                        aria-label={t("Custom environment variables")}
                        onChange={(event) => setCustomEnv(event.target.value)}
                        placeholder={t("NAME=value\nSECOND=value")}
                        rows={3}
                        value={customEnv}
                      />
                    ) : null}
                  </details>
                ) : customLane ? (
                  <div className="agent-session-advanced">
                    <button
                      aria-pressed={laneKind === "custom"}
                      className="agent-session-custom-choice"
                      onClick={() => setLaneKind("custom")}
                      type="button"
                    >
                      {t("Custom environment")}
                    </button>
                    {laneKind === "custom" ? (
                    <textarea
                      aria-label={t("Custom environment variables")}
                      onChange={(event) => setCustomEnv(event.target.value)}
                      placeholder={t("NAME=value\nSECOND=value")}
                        rows={3}
                        value={customEnv}
                      />
                    ) : null}
                  </div>
                ) : null}
                {selectedLane?.kind === "zaiGlm" ? (
                  <p className="agent-session-pro-caption">
                    {t("Uses the API key from your GLM Coding Plan (Z.ai).")}
                  </p>
                ) : null}
              {selectedLane?.kind === "grokApiKey" ? (
                <p className="agent-session-pro-caption">
                  {t("Uses your xAI API key to run Grok through Claude Code.")}
                </p>
                ) : null}
                </fieldset>
              </details>

              <fieldset disabled={creating || loginPending}>
                <legend>{proMode ? t("Folder") : t("3. Folder")}</legend>
                <div className="agent-session-folder-field">
                  <input
                    aria-label={t("Session folder")}
                    onChange={(event) => setCwd(event.target.value)}
                placeholder={t("/Users/you/programming/repo")}
                    value={cwd}
                  />
                  <button onClick={() => void browseForFolder()} type="button">
                    {t("Browse…")}
                  </button>
                </div>
              </fieldset>

              {formError ? (
                <div className="agent-session-form-error">{formError}</div>
              ) : null}
              <button
                className="agent-session-create-button"
                disabled={createDisabled}
                type="submit"
              >
                {creating
                  ? proMode
                    ? t("Opening terminal…")
                    : t("Creating…")
                  : proMode
                    ? t("Open terminal")
                    : t("Create session")}
              </button>
            </form>
          ) : null}

          {state.error ? (
            <div className="agent-sessions-error" role="alert">
              <span>{state.error}</span>
              <button onClick={() => agentSessionStore.clearError()} type="button">
                {t("Dismiss")}
              </button>
            </div>
          ) : null}

          <div className="agent-session-list" role="list">
            {state.loading && state.entries.length === 0 ? (
              <div className="agent-session-list-empty">
                {t("Loading sessions…")}
              </div>
            ) : state.entries.length === 0 ? (
              <div className="agent-session-list-empty">
                {proMode
                  ? t("No agent terminals open.")
                  : t("No sessions yet. Create one above.")}
              </div>
            ) : (
              state.entries.map((entry) => {
                const terminal =
                  entry.session.status === "exited" ||
                  entry.session.status === "failed";
                const killing = killingSessionIds.has(entry.session.id);
                const external = entry.session.mode === "terminal_ghostty";
                return (
                  <div
                    className={`agent-session-list-row ${
                      selectedEntry?.session.id === entry.session.id
                        ? "agent-session-list-row-selected"
                        : ""
                    }`}
                    key={entry.session.id}
                    role="listitem"
                  >
                    <button
                      className="agent-session-list-select"
                      onClick={() => agentSessionStore.select(entry.session.id)}
                      type="button"
                    >
                      <StatusIndicator status={entry.session.status} />
                      <span className="agent-session-list-copy">
                      <strong title={agentSessionTitle(entry.session)}>
                        {agentSessionTitle(entry.session)}
                      </strong>
                      <small title={entry.session.cwd}>{entry.session.cwd}</small>
                      <small>
                        {sessionListMetadataLabel(entry.session, t)}
                      </small>
                      </span>
                    </button>
                    <button
                      aria-label={t("Kill {agent} session", {
                        agent: agentLabel(entry.session.agentKind),
                      })}
                      className="agent-session-list-kill"
                      disabled={terminal || killing || external}
                      onClick={() => void killSession(entry.session.id)}
                      title={
                        terminal
                          ? t("Session stopped")
                          : external
                            ? t("Close the Ghostty window to stop this session.")
                            : t("Kill session")
                      }
                      type="button"
                    >
                      {killing ? "…" : "×"}
                    </button>
                  </div>
                );
              })
            )}
          </div>
        </aside>

        <main className="agent-sessions-main">
          {loginTerminalSessionId ? (
            <TerminalPanel
              focusSessionId={loginTerminalSessionId}
              onClose={() => void closeLoginTerminal()}
              open
              project={null}
              shortcutOverrides={shortcutOverrides}
              workspaceId={null}
            />
          ) : selectedEntry ? (
            selectedEntry.session.mode === "terminal_embedded" &&
            onOpenInTab &&
            selectedEntry.session.conversationId ? (
              <div className="agent-sessions-open-in-tab">
                <span className="agent-sessions-welcome-mark" aria-hidden>
                  {">_"}
                </span>
                <h3>{agentSessionTitle(selectedEntry.session)}</h3>
                <p>{t("This agent runs in the tab of its conversation.")}</p>
                <button
                  onClick={() =>
                    onOpenInTab(
                      selectedEntry.session.conversationId!,
                      selectedEntry.session.id,
                    )
                  }
                  type="button"
                >
                  {t("Open in tab")}
                </button>
              </div>
            ) : selectedEntry.session.mode === "terminal_embedded" ? (
              <TerminalPanel
                agentEntry={selectedEntry}
                focusSessionId={selectedEntry.session.id}
                onClose={() => undefined}
                open
                project={null}
                shortcutOverrides={shortcutOverrides}
                workspaceId={null}
              />
            ) : (
              <AgentSessionView
                entry={selectedEntry}
                onKill={(sessionId) => agentSessionStore.kill(sessionId)}
                onSend={(sessionId, text) =>
                  agentSessionStore.send(sessionId, text)
                }
              />
            )
          ) : (
            proMode ? (
              <div className="agent-sessions-welcome">
                <div className="agent-sessions-pro-empty">
                  <span className="agent-sessions-welcome-mark" aria-hidden>
                    {">_"}
                  </span>
                  <h3>{t("Open an agent terminal")}</h3>
                  <p>
                    {t(
                      "Agents run in real terminals — savings still measured.",
                    )}
                  </p>
                  <div className="agent-sessions-quick-agents">
                    {agentKinds.map((kind) => (
                      <button
                        disabled={quickOpenDisabled(kind)}
                        key={kind}
                        onClick={() => void openAgentTerminal(kind)}
                        title={
                          quickOpenDisabled(kind)
                            ? t(
                                "{agent} needs a login or saved key first.",
                                { agent: agentLabel(kind) },
                              )
                            : t("Open {agent} in {folder}", {
                                agent: agentLabel(kind),
                                folder: defaultCwd,
                              })
                        }
                        type="button"
                      >
                        <span>{agentLabel(kind)}</span>
                        <small>
                          {agentCardStatus(
                            kind,
                            laneOptionsForAgent(state.laneOptions, kind),
                            probeForAgent(state.probe, kind)?.found,
                            t,
                          )}
                        </small>
                      </button>
                    ))}
                  </div>
                </div>
              </div>
            ) : (
              <div className="agent-sessions-welcome">
                <span className="agent-sessions-welcome-mark" aria-hidden>
                  ◌
                </span>
                <h3>{t("Start an agent session")}</h3>
                <p>{t("Choose an agent, connection, and folder.")}</p>
              </div>
            )
          )}
        </main>
      </section>
    </div>
  );
}

function isApiLane(
  lane: AgentSessionLaneOption | null | undefined,
): lane is AgentSessionLaneOption {
  return Boolean(lane && lane.credentialProfiles.length > 0) ||
    lane?.kind === "claudeApiKey" ||
    lane?.kind === "kimiApiKey" ||
    lane?.kind === "zaiGlm" ||
    lane?.kind === "grokApiKey";
}

function isGoogleAgent(agentKind: AgentKind) {
  return agentKind === "gemini" || agentKind === "antigravity";
}

function isSubscriptionLane(
  agentKind: AgentKind,
  laneKind: AgentSessionCreateLaneKind | null | undefined,
) {
  return (
    (isGoogleAgent(agentKind) && laneKind === "custom") ||
    laneKind === "claudeSubscription" ||
    laneKind === "codexChatGpt" ||
    laneKind === "kimiSubscription"
  );
}

function laneOptionLabel(
  agentKind: AgentKind,
  laneKind: AgentSessionCreateLaneKind,
  t: Translate,
) {
  if (agentKind === "gemini" && laneKind === "custom") return "Gemini CLI";
  if (agentKind === "antigravity" && laneKind === "custom") {
    return "Antigravity CLI";
  }
  switch (laneKind) {
    case "claudeSubscription":
      return t("Claude subscription");
    case "claudeApiKey":
      return t("Anthropic API");
    case "codexChatGpt":
      return t("ChatGPT subscription");
    case "kimiSubscription":
      return t("Kimi subscription");
    case "kimiApiKey":
      return t("Moonshot API");
    case "zaiGlm":
      return t("GLM Coding Plan");
    case "grokApiKey":
      return t("Grok API key");
    case "custom":
      return t("Custom environment");
  }
}

function loginStatusLabel(
  agentKind: AgentKind,
  loggedIn: boolean | null,
  t: Translate,
) {
  if (loggedIn === false) return t("Not signed in");
  if (loggedIn === null) return t("Sign-in status unavailable");
  if (agentKind === "claude") {
    return t("Signed in via Claude Code");
  }
  if (agentKind === "codex") {
    return t("Signed in via ChatGPT in Codex");
  }
  if (agentKind === "gemini") return "Signed in via Gemini CLI";
  if (agentKind === "antigravity") return "Signed in via Antigravity CLI";
  return t("Signed in via Kimi Code");
}

function agentCardStatus(
  agentKind: AgentKind,
  lanes: AgentSessionLaneOption[],
  binaryFound: boolean | undefined,
  t: Translate,
) {
  if (binaryFound === false) return t("CLI not found");
  const subscription = lanes.find((lane) =>
    isSubscriptionLane(agentKind, lane.kind),
  );
  if (subscription?.loggedIn === true) return t("Connected");
  if (subscription?.loggedIn === false) return t("Login required");
  const apiLane = lanes.find(isApiLane);
  if (apiLane) {
    return apiLane.credentialProfiles.some((profile) => profile.status === "active")
      ? t("Key ready")
      : t("Key required");
  }
  return lanes.length > 0 ? t("Available") : t("Loading…");
}

function sessionListMetadataLabel(session: AgentSession, t: Translate) {
  const lane = laneLabel(session.laneKind, t, session.agentKind);
  const relativeTime = relativeTimeLabel(session.lastActiveAt, t);
  return session.conversationTitle
    ? t("{agent} · {lane} · {relativeTime}", {
        agent: agentLabel(session.agentKind),
        lane,
        relativeTime,
      })
    : t("{lane} · {relativeTime}", { lane, relativeTime });
}

function relativeTimeLabel(value: string, t: Translate) {
  const timestamp = Date.parse(value);
  if (Number.isNaN(timestamp)) {
    return t("unknown");
  }

  const diffSeconds = Math.round((Date.now() - timestamp) / 1000);
  const absolute = Math.abs(diffSeconds);
  const units: Array<[RelativeTimeUnit, number]> = [
    ["y", 60 * 60 * 24 * 365],
    ["mo", 60 * 60 * 24 * 30],
    ["w", 60 * 60 * 24 * 7],
    ["d", 60 * 60 * 24],
    ["h", 60 * 60],
    ["m", 60],
  ];

  if (absolute < 45) {
    return t("just now");
  }

  for (const [unit, secondsInUnit] of units) {
    if (absolute >= secondsInUnit) {
      return relativeTimeUnitLabel(
        unit,
        Math.round(absolute / secondsInUnit),
        diffSeconds >= 0,
        t,
      );
    }
  }

  return t("just now");
}

type RelativeTimeUnit = "y" | "mo" | "w" | "d" | "h" | "m";

function relativeTimeUnitLabel(
  unit: RelativeTimeUnit,
  value: number,
  elapsed: boolean,
  t: Translate,
) {
  switch (unit) {
    case "y":
      return elapsed
        ? t("{value}y ago", { value })
        : t("in {value}y", { value });
    case "mo":
      return elapsed
        ? t("{value}mo ago", { value })
        : t("in {value}mo", { value });
    case "w":
      return elapsed
        ? t("{value}w ago", { value })
        : t("in {value}w", { value });
    case "d":
      return elapsed
        ? t("{value}d ago", { value })
        : t("in {value}d", { value });
    case "h":
      return elapsed
        ? t("{value}h ago", { value })
        : t("in {value}h", { value });
    case "m":
      return elapsed
        ? t("{value}m ago", { value })
        : t("in {value}m", { value });
  }
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}
