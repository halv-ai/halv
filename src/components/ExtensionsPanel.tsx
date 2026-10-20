import { ipc } from "../ipc.ts";
import { requestCruxIndexApproval } from "../cruxIndexApproval.ts";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { formatNumber, useT, type Translate } from "../i18n/runtime";
import type {
  CruxActivation,
  ExtensionsSnapshot,
  McpAuthMode,
  McpConnectionResult,
  McpServerConfig,
  PluginDescriptor,
  PluginCandidate,
  SkillDescriptor,
} from "../types";

type ExtensionsPanelProps = {
  open: boolean;
  projectId: number | null;
  workspaceId: string | null;
  onClose: () => void;
};

type McpApprovalMode = McpServerConfig["approvalMode"];

const emptySnapshot: ExtensionsSnapshot = {
  skills: [],
  plugins: [],
  availablePlugins: [],
  mcpServers: [],
  mcpAuth: [],
  cruxActivation: "auto",
  cruxEnabled: true,
  cruxCensus: null,
  cruxIndexStatus: {
    state: "waiting_for_index",
    documentCount: null,
  },
};

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function CruxStatusChip({
  status,
}: {
  status: ExtensionsSnapshot["cruxIndexStatus"];
}) {
  const t = useT();
  const label =
    status.state === "indexed"
      ? status.documentCount === null || status.documentCount === undefined
        ? t("Indexed")
        : t("Indexed · {documentCount} docs", {
            documentCount: formatNumber(status.documentCount),
          })
      : status.state === "indexing"
        ? t("Indexing…")
        : status.state === "waiting_for_index"
          ? t("Waiting for index")
          : status.state === "approval_required"
            ? t("Needs approval")
            : t("Off");
  const tone =
    status.state === "indexed"
      ? "border-accent/25 bg-accent/10 text-accent"
      : status.state === "indexing"
        ? "border-warning/25 bg-warning-surface/40 text-warning"
        : "border-surface-border/30 bg-surface-raised/60 text-content-muted";

  return (
    <span
      className={`shrink-0 rounded-full border px-2 py-1 font-mono text-[10px] ${tone}`}
    >
      {label}
    </span>
  );
}

export function ExtensionsPanel({
  open,
  projectId,
  workspaceId,
  onClose,
}: ExtensionsPanelProps) {
  const t = useT();
  const [snapshot, setSnapshot] = useState(emptySnapshot);
  const [tab, setTab] = useState<"skills" | "plugins" | "mcp">("skills");
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pluginPath, setPluginPath] = useState("");
  const [skillContents, setSkillContents] = useState<{
    name: string;
    contents: string;
  } | null>(null);
  const [mcpName, setMcpName] = useState("");
  const [mcpTransport, setMcpTransport] = useState<
    "stdio" | "streamable_http"
  >("stdio");
  const [mcpEndpoint, setMcpEndpoint] = useState("");
  const [mcpArgs, setMcpArgs] = useState("");
  const [mcpEnvVars, setMcpEnvVars] = useState("");
  const [mcpAuthMode, setMcpAuthMode] = useState<McpAuthMode>("none");
  const [mcpOauthScopes, setMcpOauthScopes] = useState("");
  const [mcpOauthClientId, setMcpOauthClientId] = useState("");
  const [mcpBearerToken, setMcpBearerToken] = useState("");
  const [mcpApprovalMode, setMcpApprovalMode] =
    useState<McpApprovalMode>("writes");
  const [trustToolAnnotations, setTrustToolAnnotations] = useState(false);
  const [mcpTest, setMcpTest] = useState<McpConnectionResult | null>(null);

  const authByServer = useMemo(
    () =>
      new Map((snapshot.mcpAuth ?? []).map((auth) => [auth.serverId, auth])),
    [snapshot.mcpAuth],
  );

  async function refresh(showLoading = true) {
    if (showLoading) {
      setLoading(true);
    }
    setError(null);
    try {
      setSnapshot(
        await ipc.extensions.snapshot({
          projectId,
          workspaceId,
        }),
      );
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      if (showLoading) {
        setLoading(false);
      }
    }
  }

  useEffect(() => {
    if (open) {
      void refresh();
    }
  }, [open, projectId, workspaceId]);

  useEffect(() => {
    if (!open || snapshot.cruxIndexStatus.state !== "indexing") {
      return;
    }
    const timer = window.setTimeout(() => void refresh(false), 1_000);
    return () => window.clearTimeout(timer);
  }, [
    open,
    projectId,
    snapshot.cruxIndexStatus.state,
    workspaceId,
  ]);

  async function mutate(action: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await action();
      await refresh();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function inspectSkill(skill: SkillDescriptor) {
    setBusy(true);
    setError(null);
    try {
      const contents = await ipc.skill.read({
        skillId: skill.id,
        projectId,
        workspaceId,
      });
      setSkillContents({ name: skill.displayName, contents });
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  function installPlugin() {
    const path = pluginPath.trim();
    if (!path) {
      return;
    }
    void mutate(async () => {
      await ipc.plugins.installLocal({ path });
      setPluginPath("");
    });
  }

  async function browseForPlugin() {
    const selected = await openDialog({
      directory: true,
      multiple: false,
      title: t("Choose a Codex-compatible plugin folder"),
    });
    if (typeof selected === "string") {
      setPluginPath(selected);
    }
  }

  function addMcpServer() {
    const name = mcpName.trim();
    const endpoint = mcpEndpoint.trim();
    if (!name || !endpoint) {
      setError(t("Name and command/URL are required."));
      return;
    }
    const authMode =
      mcpTransport === "streamable_http" ? mcpAuthMode : "none";
    const config: McpServerConfig = {
      id: "",
      name,
      transport: mcpTransport,
      command: mcpTransport === "stdio" ? endpoint : null,
      args:
        mcpTransport === "stdio"
          ? mcpArgs
              .split("\n")
              .map((value) => value.trim())
              .filter(Boolean)
          : [],
      cwd: null,
      env: {},
      envVars:
        mcpTransport === "stdio"
          ? mcpEnvVars
              .split(",")
              .map((value) => value.trim())
              .filter(Boolean)
          : [],
      url: mcpTransport === "streamable_http" ? endpoint : null,
      headers: {},
      envHttpHeaders: {},
      bearerTokenEnvVar: null,
      authMode,
      oauthScopes:
        authMode === "oauth"
          ? mcpOauthScopes
              .split(",")
              .map((value) => value.trim())
              .filter(Boolean)
          : [],
      oauthClientId:
        authMode === "oauth" ? mcpOauthClientId.trim() || null : null,
      enabled: true,
      required: false,
      startupTimeoutMs: 10_000,
      toolTimeoutMs: 60_000,
      enabledTools: [],
      disabledTools: [],
      approvalMode: mcpApprovalMode,
      toolApprovalModes: {},
      trustToolAnnotations,
      source: "user",
      pluginId: null,
      pluginName: null,
    };
    void mutate(async () => {
      const saved = await ipc.mcp.serverUpsert({
        server: config,
      });
      if (authMode === "bearer" && mcpBearerToken.trim()) {
        await ipc.mcp.serverSetBearerToken({
          serverId: saved.id,
          token: mcpBearerToken.trim(),
        });
      }
      setMcpName("");
      setMcpEndpoint("");
      setMcpArgs("");
      setMcpEnvVars("");
      setMcpBearerToken("");
      setMcpOauthScopes("");
      setMcpOauthClientId("");
    });
  }

  function reindexCrux() {
    if (projectId === null) {
      return;
    }
    void mutate(() =>
      snapshot.cruxIndexStatus.state === "approval_required"
        ? requestCruxIndexApproval(t, { projectId, workspaceId })
        : ipc.crux.reindexNow({
            projectId,
            workspaceId,
          }),
    );
  }

  async function testMcpServer(serverId: string) {
    setBusy(true);
    setError(null);
    setMcpTest(null);
    try {
      setMcpTest(
        await ipc.mcp.serverTest({
          serverId,
          projectId,
          workspaceId,
        }),
      );
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  if (!open) {
    return null;
  }

  return (
    <div className="fixed inset-0 z-[75] flex justify-end bg-black/30 backdrop-blur-sm">
      <section
        aria-labelledby="extensions-title"
        aria-modal="true"
        className="flex h-full w-full max-w-2xl flex-col border-0 bg-surface-base shadow-2xl shadow-black/30 sm:border-l sm:border-surface-border/35"
        role="dialog"
      >
        <div className="flex items-center gap-3 border-b border-surface-border/30 px-5 py-4">
          <div className="min-w-0 flex-1">
            <h2
              className="text-base font-semibold text-content-primary"
              id="extensions-title"
            >
              {t("Extensions")}
            </h2>
            <p className="mt-1 text-xs text-content-muted">
              {t(
                "Skills, validated plugins, and secret-safe MCP configuration.",
              )}
            </p>
          </div>
          <button
            aria-label={t("Close extensions")}
            className="rounded-md px-2 py-1 text-xl text-content-muted hover:bg-surface-raised hover:text-content-primary"
            onClick={onClose}
            type="button"
          >
            ×
          </button>
        </div>

        <div className="flex gap-1 overflow-x-auto border-b border-surface-border/25 px-4 pt-2">
          {(["skills", "plugins", "mcp"] as const).map((item) => (
            <button
              aria-pressed={tab === item}
              className={`rounded-t-lg px-4 py-2 text-sm capitalize ${
                tab === item
                  ? "bg-surface-panel text-content-primary"
                  : "text-content-muted hover:text-content-primary"
              }`}
              key={item}
              onClick={() => setTab(item)}
              type="button"
            >
              {item === "mcp"
                ? t("MCP")
                : item === "skills"
                  ? t("Skills")
                  : t("Plugins")}
            </button>
          ))}
          <button
            className="ml-auto shrink-0 px-3 py-2 text-xs text-content-muted hover:text-content-primary disabled:opacity-50"
            disabled={loading}
            onClick={() => void refresh()}
            type="button"
          >
            {loading ? t("Refreshing…") : t("Refresh")}
          </button>
        </div>

        {error ? (
          <div className="border-b border-danger/25 bg-danger/10 px-5 py-2 text-xs text-danger">
            {error}
          </div>
        ) : null}

        <div className="min-h-0 flex-1 overflow-y-auto p-5">
          {tab === "skills" ? (
            <SkillsTab
              busy={busy}
              inspectSkill={inspectSkill}
              mutate={mutate}
              skills={snapshot.skills}
            />
          ) : tab === "plugins" ? (
            <PluginsTab
              availablePlugins={snapshot.availablePlugins}
              browseForPlugin={browseForPlugin}
              busy={busy}
              installPlugin={installPlugin}
              mutate={mutate}
              pluginPath={pluginPath}
              plugins={snapshot.plugins}
              setPluginPath={setPluginPath}
            />
          ) : (
            <div>
              <div className="mb-4 rounded-xl border border-surface-border/30 bg-surface-panel/55 p-4">
                <div className="flex flex-wrap items-end gap-4">
                  <label className="flex items-center gap-2 text-xs text-content-muted">
                    <input
                      checked={snapshot.cruxEnabled}
                      disabled={busy}
                      onChange={(event) =>
                        void mutate(() =>
                          ipc.mcp.serverSetEnabled({
                            serverId: "builtin.crux",
                            enabled: event.target.checked,
                            projectId,
                            workspaceId,
                          }),
                        )
                      }
                      type="checkbox"
                    />
                    {t("Enable built-in Crux")}
                  </label>
                  <label className="text-xs text-content-muted">
                    {t("Project size gate")}
                    <select
                      className="ml-2 rounded-lg border border-surface-border/35 bg-surface-base px-3 py-2 text-sm text-content-primary"
                      disabled={busy}
                      onChange={(event) =>
                        void mutate(() =>
                          ipc.crux.setActivation({
                            activation: event.target.value as CruxActivation,
                            projectId,
                            workspaceId,
                          }),
                        )
                      }
                      value={snapshot.cruxActivation}
                    >
                      <option value="auto">{t("Auto")}</option>
                      <option value="always_on">{t("Always on")}</option>
                      <option value="always_off">{t("Always off")}</option>
                    </select>
                  </label>
                  <CruxStatusChip status={snapshot.cruxIndexStatus} />
                  <button
                    className="rounded-lg border border-surface-border/35 px-3 py-2 text-xs text-content-secondary hover:border-accent/45 hover:bg-surface-raised hover:text-content-primary disabled:cursor-not-allowed disabled:opacity-50"
                    disabled={
                      busy ||
                      projectId === null ||
                      snapshot.cruxIndexStatus.state === "off"
                    }
                    onClick={reindexCrux}
                    type="button"
                  >
                    {snapshot.cruxIndexStatus.state === "approval_required"
                      ? t("Allow indexing")
                      : t("Reindex")}
                  </button>
                </div>
                <p className="mt-2 text-[10px] leading-4 text-content-muted">
                  {t(
                "Auto enables Crux when one language has 200 files or 75,000 lines.",
                  )}
                </p>
              </div>
              <div className="rounded-xl border border-surface-border/30 bg-surface-panel/55 p-4">
                <div className="grid grid-cols-2 gap-3">
                  <label className="text-xs text-content-muted">
                    {t("Name")}
                    <input
                      className="mt-1 w-full rounded-lg border border-surface-border/35 bg-surface-base px-3 py-2 text-sm text-content-primary"
                      onChange={(event) => setMcpName(event.target.value)}
                      value={mcpName}
                    />
                  </label>
                  <label className="text-xs text-content-muted">
                    {t("Transport")}
                    <select
                      className="mt-1 w-full rounded-lg border border-surface-border/35 bg-surface-base px-3 py-2 text-sm text-content-primary"
                      onChange={(event) => {
                        const transport = event.target.value as typeof mcpTransport;
                        setMcpTransport(transport);
                        if (transport === "stdio") {
                          setMcpAuthMode("none");
                        }
                      }}
                      value={mcpTransport}
                    >
                      <option value="stdio">{t("STDIO")}</option>
                      <option value="streamable_http">
                        {t("Streamable HTTP")}
                      </option>
                    </select>
                  </label>
                </div>
                <label className="mt-3 block text-xs text-content-muted">
                  {mcpTransport === "stdio" ? t("Command") : t("HTTPS URL")}
                  <input
                    className="mt-1 w-full rounded-lg border border-surface-border/35 bg-surface-base px-3 py-2 text-sm text-content-primary"
                    onChange={(event) => setMcpEndpoint(event.target.value)}
                    placeholder={
                      mcpTransport === "stdio"
                        ? "npx"
                        : "https://example.com/mcp"
                    }
                    value={mcpEndpoint}
                  />
                </label>
                {mcpTransport === "stdio" ? (
                  <>
                    <label className="mt-3 block text-xs text-content-muted">
                      {t("Arguments, one per line")}
                      <textarea
                        className="mt-1 min-h-20 w-full rounded-lg border border-surface-border/35 bg-surface-base px-3 py-2 font-mono text-xs text-content-primary"
                        onChange={(event) => setMcpArgs(event.target.value)}
                        value={mcpArgs}
                      />
                    </label>
                    <label className="mt-3 block text-xs text-content-muted">
                      {t("Environment variable names, comma separated")}
                      <input
                        className="mt-1 w-full rounded-lg border border-surface-border/35 bg-surface-base px-3 py-2 text-sm text-content-primary"
                        onChange={(event) => setMcpEnvVars(event.target.value)}
                        placeholder="GITHUB_TOKEN"
                        value={mcpEnvVars}
                      />
                    </label>
                  </>
                ) : (
                  <div className="mt-3 grid grid-cols-2 gap-3">
                    <label className="text-xs text-content-muted">
                      {t("Authentication")}
                      <select
                        className="mt-1 w-full rounded-lg border border-surface-border/35 bg-surface-base px-3 py-2 text-sm text-content-primary"
                        onChange={(event) =>
                          setMcpAuthMode(event.target.value as McpAuthMode)
                        }
                        value={mcpAuthMode}
                      >
                        <option value="none">{t("None")}</option>
                        <option value="bearer">{t("Bearer token")}</option>
                        <option value="oauth">{t("OAuth")}</option>
                      </select>
                    </label>
                    {mcpAuthMode === "bearer" ? (
                      <label className="text-xs text-content-muted">
                        {t("Bearer token")}
                        <input
                          className="mt-1 w-full rounded-lg border border-surface-border/35 bg-surface-base px-3 py-2 text-sm text-content-primary"
                          onChange={(event) =>
                            setMcpBearerToken(event.target.value)
                          }
                          type="password"
                          value={mcpBearerToken}
                        />
                      </label>
                    ) : null}
                  </div>
                )}
                {mcpAuthMode === "oauth" &&
                mcpTransport === "streamable_http" ? (
                  <div className="mt-2 grid gap-2">
                    <label className="block text-xs text-content-muted">
                      {t("OAuth scopes, comma separated")}
                      <input
                        className="mt-1 w-full rounded-lg border border-surface-border/35 bg-surface-base px-3 py-2 text-sm text-content-primary"
                        onChange={(event) =>
                          setMcpOauthScopes(event.target.value)
                        }
                        placeholder="repo, read:user"
                        value={mcpOauthScopes}
                      />
                    </label>
                    <label className="block text-xs text-content-muted">
                      {t("Client ID or metadata URL (optional)")}
                      <input
                        className="mt-1 w-full rounded-lg border border-surface-border/35 bg-surface-base px-3 py-2 font-mono text-xs text-content-primary"
                        onChange={(event) =>
                          setMcpOauthClientId(event.target.value)
                        }
                        placeholder={t("Leave blank for dynamic registration")}
                        value={mcpOauthClientId}
                      />
                    </label>
                    <p className="rounded-lg border border-accent/20 bg-accent/5 px-3 py-2 text-[10px] leading-4 text-content-muted">
                      {t(
                        "Halv discovers the authorization server, uses PKCE in your browser, and stores access and refresh tokens in the system Keychain.",
                      )}{" "}
                      {t(
                        "Add a client ID only when the server does not offer dynamic registration.",
                      )}
                    </p>
                  </div>
                ) : null}
                <div className="mt-3 grid grid-cols-2 gap-3">
                  <label className="text-xs text-content-muted">
                    {t("Tool approval")}
                    <select
                      className="mt-1 w-full rounded-lg border border-surface-border/35 bg-surface-base px-3 py-2 text-sm text-content-primary"
                      onChange={(event) =>
                        setMcpApprovalMode(
                          event.target.value as McpApprovalMode,
                        )
                      }
                      value={mcpApprovalMode}
                    >
                      <option value="writes">{t("Prompt for writes")}</option>
                      <option value="prompt">
                        {t("Prompt for every tool")}
                      </option>
                      <option value="auto">{t("Automatic")}</option>
                      <option value="approve">{t("Pre-approved")}</option>
                    </select>
                  </label>
                  <label className="flex items-end gap-2 pb-2 text-xs text-content-muted">
                    <input
                      checked={trustToolAnnotations}
                      onChange={(event) =>
                        setTrustToolAnnotations(event.target.checked)
                      }
                      type="checkbox"
                    />
                    {t("Trust server read-only hints")}
                  </label>
                </div>
                <p className="mt-3 text-[10px] leading-4 text-content-muted">
                  {t("Secrets are never written to extensions.json.")}{" "}
                  {t(
                    "Tool annotations are untrusted hints unless you opt in; missing hints classify a tool as a write.",
                  )}
                </p>
                <button
                  className="mt-3 rounded-lg bg-accent px-4 py-2 text-sm text-accent-contrast disabled:opacity-50"
                  disabled={busy}
                  onClick={addMcpServer}
                  type="button"
                >
                  {t("Add server")}
                </button>
              </div>

              <div className="mt-4 space-y-2">
                {snapshot.mcpServers.length === 0 ? (
                  <Empty text={t("No MCP servers configured.")} />
                ) : (
                  snapshot.mcpServers.map((server) => {
                    const auth = authByServer.get(server.id);
                    const fromPlugin = server.source === "plugin";
                    const builtIn = server.source === "built_in";
                    return (
                      <div
                        className="rounded-xl border border-surface-border/30 p-3"
                        key={server.id}
                      >
                        <div className="flex items-start gap-3">
                          <div className="min-w-0 flex-1">
                            <div className="flex flex-wrap items-center gap-2">
                              <p className="text-sm font-medium">{server.name}</p>
                              <Badge>
                                {fromPlugin
                                  ? t("plugin managed")
                                  : builtIn
                                    ? t("built-in")
                                    : t("user")}
                              </Badge>
                              {!server.enabled ? (
                                <Badge>{t("disabled")}</Badge>
                              ) : null}
                            </div>
                            <p className="mt-1 truncate font-mono text-[10px] text-content-muted">
                              {server.transport === "stdio"
                                ? [server.command, ...server.args].join(" ")
                                : server.url}
                            </p>
                            <p className="mt-1 text-[10px] text-accent">
                              {server.trustToolAnnotations
                                ? t(
                                    "approval: {approvalMode} · read-only hints trusted",
                                    {
                                      approvalMode: mcpApprovalModeLabel(
                                        server.approvalMode,
                                        t,
                                      ),
                                    },
                                  )
                                : t(
                                    "approval: {approvalMode} · annotations untrusted",
                                    {
                                      approvalMode: mcpApprovalModeLabel(
                                        server.approvalMode,
                                        t,
                                      ),
                                    },
                                  )}
                            </p>
                            {auth ? (
                              <p className="mt-1 text-[10px] leading-4 text-content-muted">
                                {t("auth {state} · {message}", {
                                  state: mcpAuthStateLabel(auth.state, t),
                                  message: auth.message,
                                })}
                              </p>
                            ) : null}
                          </div>
                          <button
                            className="text-xs text-accent hover:underline"
                            disabled={busy || !server.enabled}
                            onClick={() => void testMcpServer(server.id)}
                            type="button"
                          >
                            {t("Test")}
                          </button>
                          {server.transport === "streamable_http" &&
                          auth?.mode === "oauth" &&
                          auth.loginSupported ? (
                            <button
                              className="text-xs text-accent hover:underline"
                              disabled={busy || !server.enabled}
                              onClick={() =>
                                void mutate(() =>
                                  ipc.mcp.serverOauthLogin({
                                    serverId: server.id,
                                  }),
                                )
                              }
                              type="button"
                            >
                              {auth.state === "login_required" ||
                              auth.state === "expired"
                                ? t("Sign in")
                                : t("Reconnect")}
                            </button>
                          ) : null}
                          {server.transport === "streamable_http" &&
                          auth?.mode === "oauth" &&
                          auth.state !== "login_required" ? (
                            <button
                              className="text-xs text-content-muted hover:text-danger"
                              disabled={busy}
                              onClick={() =>
                                void mutate(() =>
                                  ipc.mcp.serverOauthLogout({
                                    serverId: server.id,
                                  }),
                                )
                              }
                              type="button"
                            >
                              {t("Sign out")}
                            </button>
                          ) : null}
                          {!fromPlugin &&
                          !builtIn &&
                          server.transport === "streamable_http" &&
                          auth?.mode === "bearer" ? (
                            <button
                              className="text-xs text-content-muted hover:text-content-primary"
                              disabled={busy}
                              onClick={() =>
                                void mutate(() =>
                                  ipc.mcp.serverClearBearerToken({
                                    serverId: server.id,
                                  }),
                                )
                              }
                              type="button"
                            >
                              {t("Clear auth")}
                            </button>
                          ) : null}
                          {!fromPlugin && !builtIn ? (
                            <>
                              <button
                                className="text-xs text-content-muted hover:text-content-primary"
                                disabled={busy}
                                onClick={() =>
                                  void mutate(() =>
                                    ipc.mcp.serverSetEnabled({
                                      serverId: server.id,
                                      enabled: !server.enabled,
                                    }),
                                  )
                                }
                                type="button"
                              >
                                {server.enabled ? t("Disable") : t("Enable")}
                              </button>
                              <button
                                className="text-xs text-content-muted hover:text-danger"
                                disabled={busy}
                                onClick={() =>
                                  void mutate(() =>
                                    ipc.mcp.serverRemove({
                                      serverId: server.id,
                                    }),
                                  )
                                }
                                type="button"
                              >
                                {t("Remove")}
                              </button>
                            </>
                          ) : null}
                        </div>
                      </div>
                    );
                  })
                )}
              </div>

              {mcpTest ? <McpTestResult result={mcpTest} /> : null}
            </div>
          )}
        </div>
      </section>

      {skillContents ? (
        <div
          aria-labelledby="skill-contents-title"
          aria-modal="true"
          className="fixed inset-0 z-[90] flex items-center justify-center bg-black/45 p-6"
          onMouseDown={(event) =>
            event.target === event.currentTarget && setSkillContents(null)
          }
          role="dialog"
        >
          <div className="flex max-h-[80vh] w-full max-w-3xl flex-col overflow-hidden rounded-xl border border-surface-border/35 bg-surface-panel shadow-2xl">
            <div className="flex items-center border-b border-surface-border/25 px-4 py-3">
              <p className="font-medium" id="skill-contents-title">
                {skillContents.name}
              </p>
              <button
                aria-label={t("Close skill contents")}
                className="ml-auto text-content-muted"
                onClick={() => setSkillContents(null)}
                type="button"
              >
                ×
              </button>
            </div>
            <pre className="min-h-0 flex-1 overflow-auto whitespace-pre-wrap p-4 font-mono text-xs leading-5 text-content-secondary">
              {skillContents.contents}
            </pre>
          </div>
        </div>
      ) : null}
    </div>
  );
}

function SkillsTab({
  busy,
  inspectSkill,
  mutate,
  skills,
}: {
  busy: boolean;
  inspectSkill: (skill: SkillDescriptor) => Promise<void>;
  mutate: (action: () => Promise<unknown>) => Promise<void>;
  skills: SkillDescriptor[];
}) {
  const t = useT();
  const skillNameToken = "$skill-name";
  const skillSelectionDescription = t(
    "Trusted project skills and global {skillName} selections are loaded progressively.",
    { skillName: skillNameToken },
  );
  const [
    skillSelectionDescriptionBeforeName,
    skillSelectionDescriptionAfterName,
  ] = skillSelectionDescription.split(skillNameToken);

  return (
    <div className="space-y-2">
      <p className="mb-4 text-xs leading-5 text-content-muted">
        {skillSelectionDescriptionBeforeName}
        <code>{skillNameToken}</code>
        {skillSelectionDescriptionAfterName}{" "}
        {t("Catalog metadata remains visible while a skill is disabled.")}
      </p>
      {skills.length === 0 ? (
        <Empty
          text={t("No skills found in global, project, or plugin roots.")}
        />
      ) : (
        skills.map((skill) => (
          <div
            className="rounded-xl border border-surface-border/30 bg-surface-panel/55 p-3"
            key={skill.id}
          >
            <div className="flex items-start gap-3">
              <div className="min-w-0 flex-1">
                <div className="flex flex-wrap items-center gap-2">
                  <p className="text-sm font-medium text-content-primary">
                    {skill.displayName}
                  </p>
                  {!skill.allowImplicitInvocation ? (
                    <Badge>{t("explicit only")}</Badge>
                  ) : null}
                </div>
                <p className="mt-1 text-xs leading-5 text-content-muted">
                  {skill.shortDescription}
                </p>
                {skill.displayName !== skill.name ? (
                  <p className="mt-1 font-mono text-[10px] text-content-muted">
                    ${skill.name}
                  </p>
                ) : null}
                <p className="mt-1 truncate font-mono text-[9px] uppercase tracking-[0.1em] text-accent">
                  {t("{scope} · {source}", {
                    scope: skillScopeLabel(skill.scope, t),
                    source: skill.source,
                  })}
                </p>
                {skill.dependencyTools.length ? (
                  <p className="mt-1 text-[10px] text-content-muted">
                    {t("MCP dependencies: {dependencies}", {
                      dependencies: skill.dependencyTools.join(", "),
                    })}
                  </p>
                ) : null}
                {skill.defaultPrompt ? (
                  <p className="mt-1 truncate text-[10px] italic text-content-muted">
                    {skill.defaultPrompt}
                  </p>
                ) : null}
              </div>
              <label className="flex items-center gap-2 text-xs text-content-muted">
                <input
                  checked={skill.enabled}
                  disabled={busy}
                  onChange={(event) =>
                    void mutate(() =>
                      ipc.skills.setEnabled({
                        skillId: skill.id,
                        enabled: event.target.checked,
                      }),
                    )
                  }
                  type="checkbox"
                />
                {t("Enabled")}
              </label>
            </div>
            <button
              className="mt-2 text-xs text-accent hover:underline disabled:opacity-50"
              disabled={busy}
              onClick={() => void inspectSkill(skill)}
              type="button"
            >
              {t("Read instructions")}
            </button>
          </div>
        ))
      )}
    </div>
  );
}

function PluginsTab({
  availablePlugins,
  browseForPlugin,
  busy,
  installPlugin,
  mutate,
  pluginPath,
  plugins,
  setPluginPath,
}: {
  availablePlugins: PluginCandidate[];
  browseForPlugin: () => Promise<void>;
  busy: boolean;
  installPlugin: () => void;
  mutate: (action: () => Promise<unknown>) => Promise<void>;
  pluginPath: string;
  plugins: PluginDescriptor[];
  setPluginPath: (value: string) => void;
}) {
  const t = useT();

  return (
    <div>
      <div className="rounded-xl border border-surface-border/30 bg-surface-panel/55 p-4">
        <label className="text-xs font-medium text-content-secondary">
          {t("Local plugin folder")}
        </label>
        <div className="mt-2 flex gap-2">
          <input
            className="min-w-0 flex-1 rounded-lg border border-surface-border/35 bg-surface-base px-3 py-2 text-sm outline-none focus:border-accent"
            onChange={(event) => setPluginPath(event.target.value)}
            placeholder="/path/to/plugin"
            value={pluginPath}
          />
          <button
            className="rounded-lg border border-surface-border/35 px-3 py-2 text-sm text-content-secondary hover:bg-surface-raised disabled:opacity-50"
            disabled={busy}
            onClick={() => void browseForPlugin()}
            type="button"
          >
            {t("Browse…")}
          </button>
          <button
            className="rounded-lg bg-accent px-4 py-2 text-sm text-accent-contrast disabled:opacity-50"
            disabled={busy || !pluginPath.trim()}
            onClick={installPlugin}
            type="button"
          >
            {t("Install")}
          </button>
        </div>
        <p className="mt-2 text-[10px] text-content-muted">
          {t("Requires .codex-plugin/plugin.json.")}{" "}
          {t(
            "Bundled skills and MCP servers are validated and loaded from the copied plugin.",
          )}{" "}
          {t(
            "Local packages up to 256 MiB are supported so Computer Use can be imported when its licensed runtime is already installed on this Mac.",
          )}
        </p>
      </div>
      {availablePlugins.length > 0 ? (
        <div className="mt-4 rounded-xl border border-accent/25 bg-accent/5 p-4">
          <div className="mb-3">
            <p className="text-sm font-medium text-content-primary">
              {t("Available from Codex")}
            </p>
            <p className="mt-1 text-[10px] text-content-muted">
              {t(
                "Halv found compatible packages in your local Codex plugin cache.",
              )}{" "}
              {t("Import is explicit; nothing is copied until you choose it.")}
            </p>
          </div>
          <div className="space-y-2">
            {availablePlugins.map((plugin) => (
              <div
                className="flex items-start gap-3 rounded-lg border border-surface-border/25 bg-surface-base/70 p-3"
                key={`${plugin.id}-${plugin.version}`}
              >
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="text-sm font-medium text-content-primary">
                      {plugin.displayName || plugin.name}
                    </span>
                    <span className="font-mono text-[10px] text-content-muted">
                      {t("v{version}", { version: plugin.version })}
                    </span>
                    {plugin.installed ? <Badge>{t("installed")}</Badge> : null}
                  </div>
                  <p className="mt-1 line-clamp-2 text-xs text-content-muted">
                    {plugin.description}
                  </p>
                </div>
                <button
                  className="shrink-0 rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-accent-contrast disabled:opacity-50"
                  disabled={busy}
                  onClick={() => {
                    setPluginPath(plugin.path);
                    void mutate(() =>
                      ipc.plugins.installLocal({
                        path: plugin.path,
                      }),
                    );
                  }}
                  type="button"
                >
                  {plugin.installed ? t("Reinstall") : t("Import")}
                </button>
              </div>
            ))}
          </div>
        </div>
      ) : null}
      <div className="mt-4 space-y-2">
        {plugins.length === 0 ? (
          <Empty text={t("No local plugins installed.")} />
        ) : (
          plugins.map((plugin) => (
            <div
              className="rounded-xl border border-surface-border/30 p-3"
              key={plugin.id}
            >
              <div className="flex items-start gap-3">
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <p className="text-sm font-medium">
                      {plugin.displayName || plugin.name}
                    </p>
                    <span className="font-mono text-[10px] text-content-muted">
                      {t("v{version}", { version: plugin.version })}
                    </span>
                    {!plugin.enabled ? <Badge>{t("disabled")}</Badge> : null}
                  </div>
                  <p className="mt-1 text-xs leading-5 text-content-muted">
                    {plugin.description}
                  </p>
                  <p className="mt-1 text-[10px] text-content-muted">
                    {plugin.developerName
                      ? t(
                          "{skillCount} skills · {mcpServerCount} MCP servers · by {developerName}",
                          {
                            skillCount: plugin.skillCount ?? 0,
                            mcpServerCount: plugin.mcpServerCount ?? 0,
                            developerName: plugin.developerName,
                          },
                        )
                      : t(
                          "{skillCount} skills · {mcpServerCount} MCP servers",
                          {
                            skillCount: plugin.skillCount ?? 0,
                            mcpServerCount: plugin.mcpServerCount ?? 0,
                          },
                        )}
                  </p>
                  {plugin.capabilities?.length ? (
                    <p className="mt-1 text-[10px] text-accent">
                      {plugin.capabilities.join(" · ")}
                    </p>
                  ) : null}
                  {plugin.unsupportedComponents.length ? (
                    <p className="mt-1 text-[10px] text-warning">
                      {t("Not loaded by Halv yet: {components}", {
                        components: plugin.unsupportedComponents.join(", "),
                      })}
                    </p>
                  ) : null}
                </div>
                <button
                  className="text-xs text-content-muted hover:text-content-primary"
                  disabled={busy}
                  onClick={() =>
                    void mutate(() =>
                      ipc.plugins.setEnabled({
                        pluginId: plugin.id,
                        enabled: !plugin.enabled,
                      }),
                    )
                  }
                  type="button"
                >
                  {plugin.enabled ? t("Disable") : t("Enable")}
                </button>
                <button
                  className="text-xs text-content-muted hover:text-danger"
                  disabled={busy}
                  onClick={() =>
                    void mutate(() =>
                      ipc.plugins.uninstall({ pluginId: plugin.id }),
                    )
                  }
                  type="button"
                >
                  {t("Uninstall")}
                </button>
              </div>
            </div>
          ))
        )}
      </div>
    </div>
  );
}

function McpTestResult({ result }: { result: McpConnectionResult }) {
  const t = useT();
  const metadataByName = new Map(
    (result.toolMetadata ?? []).map((metadata) => [metadata.name, metadata]),
  );
  return (
    <div className="mt-4 rounded-xl border border-accent/30 bg-accent/5 p-3 text-xs">
      <p className="font-medium text-accent">
        {t("Connected with MCP {protocolVersion}", {
          protocolVersion: result.protocolVersion,
        })}
      </p>
      {result.serverInstructions ? (
        <p className="mt-2 whitespace-pre-wrap text-content-secondary">
          {t("Server instructions: {serverInstructions}", {
            serverInstructions: result.serverInstructions,
          })}
        </p>
      ) : null}
      <p className="mt-2 font-mono text-[10px] text-content-muted">
        {t("Capabilities: {capabilities}", {
          capabilities: JSON.stringify(result.serverCapabilities ?? {}),
        })}
      </p>
      {result.tools.length ? (
        <div className="mt-3 space-y-2">
          {result.tools.map((tool) => {
            const metadata = metadataByName.get(tool.name);
            return (
              <div
                className="rounded-lg border border-surface-border/25 bg-surface-base/60 px-3 py-2"
                key={tool.wireName}
              >
                <div className="flex items-center gap-2">
                  <span className="font-medium text-content-primary">
                    {tool.name}
                  </span>
                  <Badge>
                    {metadata?.classifiedAsWrite ? t("write") : t("read only")}
                  </Badge>
                  <Badge>
                    {t("approval {approvalMode}", {
                      approvalMode: mcpApprovalModeLabel(tool.approvalMode, t),
                    })}
                  </Badge>
                </div>
                {metadata ? (
                  <p className="mt-1 text-[10px] text-content-muted">
                    {mcpToolAnnotationsLabel(metadata, t)}
                  </p>
                ) : null}
              </div>
            );
          })}
        </div>
      ) : (
        <p className="mt-1 text-content-muted">
          {t("Server exposes no tools.")}
        </p>
      )}
    </div>
  );
}

function skillScopeLabel(scope: SkillDescriptor["scope"], t: Translate) {
  switch (scope) {
    case "global":
      return t("global");
    case "project":
      return t("project");
    case "plugin":
      return t("plugin");
  }
}

function mcpApprovalModeLabel(mode: string, t: Translate) {
  switch (mode) {
    case "auto":
      return t("auto");
    case "prompt":
      return t("prompt");
    case "writes":
      return t("writes");
    case "approve":
      return t("approve");
    default:
      return mode;
  }
}

function mcpAuthStateLabel(state: string, t: Translate) {
  switch (state) {
    case "environment":
      return t("environment");
    case "refresh_required":
      return t("refresh required");
    case "expired":
      return t("expired");
    case "configured":
      return t("configured");
    case "login_required":
      return t("login required");
    case "not_required":
      return t("not required");
    case "missing":
      return t("missing");
    case "error":
      return t("error");
    default:
      return state.replace(/_/g, " ");
  }
}

function mcpToolAnnotationsLabel(
  metadata: McpConnectionResult["toolMetadata"][number],
  t: Translate,
) {
  const destructive = metadata.annotations.destructiveHint;
  const openWorld = metadata.annotations.openWorldHint;

  if (metadata.annotationsTrusted) {
    if (destructive && openWorld) {
      return t(
        "annotations trusted · destructive hint · open-world hint",
      );
    }
    if (destructive) {
      return t("annotations trusted · destructive hint");
    }
    if (openWorld) {
      return t("annotations trusted · open-world hint");
    }
    return t("annotations trusted");
  }
  if (destructive && openWorld) {
    return t(
      "annotations untrusted · destructive hint · open-world hint",
    );
  }
  if (destructive) {
    return t("annotations untrusted · destructive hint");
  }
  if (openWorld) {
    return t("annotations untrusted · open-world hint");
  }
  return t("annotations untrusted");
}

function Badge({ children }: { children: ReactNode }) {
  return (
    <span className="rounded-full border border-surface-border/30 px-1.5 py-0.5 text-[9px] uppercase tracking-wide text-content-muted">
      {children}
    </span>
  );
}

function Empty({ text }: { text: string }) {
  return (
    <div className="rounded-xl border border-dashed border-surface-border/30 px-4 py-8 text-center text-sm text-content-muted">
      {text}
    </div>
  );
}
