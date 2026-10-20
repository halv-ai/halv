import { ipc } from "../ipc.ts";
import { useEffect, useMemo, useState } from "react";
import { confirmDialog } from "../dialogs";
import { useT, type Translate } from "../i18n/runtime";
import type {
  LocalEnvironmentAction,
  LocalEnvironmentConfig,
  LocalEnvironmentDescriptor,
  LocalEnvironmentLifecycle,
  Project,
} from "../types";

type LocalEnvironmentsPanelProps = {
  open: boolean;
  project: Project | null;
  environments: LocalEnvironmentDescriptor[];
  selectedConfigPath: string | null;
  onClose: () => void;
  onChanged: (environments: LocalEnvironmentDescriptor[]) => void;
  onSelect: (configPath: string | null) => void;
};

const inputClass =
  "mt-1 w-full rounded-lg border border-surface-border/35 bg-surface-base px-3 py-2 text-sm text-content-primary outline-none focus:border-accent/55 focus:ring-2 focus:ring-accent/15";
const scriptClass = `${inputClass} min-h-24 resize-y font-mono text-xs leading-relaxed`;

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function emptyLifecycle(): LocalEnvironmentLifecycle {
  return { script: "" };
}

function emptyConfig(): LocalEnvironmentConfig {
  return {
    version: 1,
    name: "Local dev",
    setup: emptyLifecycle(),
    cleanup: null,
    actions: [],
  };
}

function cloneConfig(config: LocalEnvironmentConfig): LocalEnvironmentConfig {
  return structuredClone(config);
}

function fileName(path: string) {
  return path.split(/[\\/]/).pop() ?? path;
}

export function LocalEnvironmentsPanel({
  open,
  project,
  environments,
  selectedConfigPath,
  onClose,
  onChanged,
  onSelect,
}: LocalEnvironmentsPanelProps) {
  const t = useT();
  const [editingPath, setEditingPath] = useState<string | null>(null);
  const [filename, setFilename] = useState("environment.toml");
  const [config, setConfig] = useState<LocalEnvironmentConfig>(emptyConfig);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const editingDescriptor = useMemo(
    () =>
      editingPath
        ? environments.find((environment) => environment.configPath === editingPath) ?? null
        : null,
    [editingPath, environments],
  );
  const inherited = editingDescriptor?.inherited ?? false;

  useEffect(() => {
    if (!open) return;
    setError(null);
    const selected = environments.find(
      (environment) =>
        environment.configPath === selectedConfigPath && environment.config,
    );
    if (selected?.config) {
      setEditingPath(selected.configPath);
      setFilename(fileName(selected.configPath));
      setConfig(cloneConfig(selected.config));
    } else {
      setEditingPath(null);
      setFilename("environment.toml");
      setConfig(emptyConfig());
    }
  }, [open, project?.id]);

  useEffect(() => {
    if (!open) return;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !busy) onClose();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [busy, onClose, open]);

  function edit(environment: LocalEnvironmentDescriptor) {
    setError(null);
    setEditingPath(environment.configPath);
    setFilename(fileName(environment.configPath));
    if (environment.config) setConfig(cloneConfig(environment.config));
  }

  function createNew() {
    setError(null);
    setEditingPath(null);
    setFilename("environment.toml");
    setConfig(emptyConfig());
  }

  async function refresh(projectId: number) {
    const rows = await ipc.local.environmentList({ projectId });
    onChanged(rows);
    return rows;
  }

  async function save() {
    if (!project || inherited || busy) return;
    setBusy(true);
    setError(null);
    try {
      const saved = await ipc.local.environmentSave({
          projectId: project.id,
          configPath: editingPath,
          filename: editingPath ? null : filename.trim(),
          config,
        });
      const rows = await refresh(project.id);
      const row = rows.find(
        (environment) => environment.configPath === saved.configPath,
      );
      setEditingPath(saved.configPath);
      setFilename(fileName(saved.configPath));
      if (row?.config) setConfig(cloneConfig(row.config));
      onSelect(saved.configPath);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function remove() {
    if (!project || !editingPath || inherited || busy) return;
    if (
      !(await confirmDialog({
        danger: true,
        message: t("Delete {name}?", { name: fileName(editingPath) }),
      }))
    ) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await ipc.local.environmentDelete({
        projectId: project.id,
        configPath: editingPath,
      });
      if (selectedConfigPath === editingPath) onSelect(null);
      await refresh(project.id);
      createNew();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  function updateLifecycle(
    key: "setup" | "cleanup",
    next: LocalEnvironmentLifecycle | null,
  ) {
    setConfig((current) => ({ ...current, [key]: next }));
  }

  function updateAction(index: number, patch: Partial<LocalEnvironmentAction>) {
    setConfig((current) => ({
      ...current,
      actions: current.actions.map((action, actionIndex) =>
        actionIndex === index ? { ...action, ...patch } : action,
      ),
    }));
  }

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-[78] flex justify-end bg-black/30 backdrop-blur-sm">
      <section
        aria-label={t("Local environments")}
        aria-modal="true"
        className="flex h-full w-full max-w-5xl flex-col border-l border-surface-border/35 bg-surface-base shadow-2xl shadow-black/30"
        role="dialog"
      >
        <div className="flex items-center gap-4 border-b border-surface-border/30 px-5 py-4">
          <div className="min-w-0 flex-1">
            <h2 className="text-base font-semibold text-content-primary">
              {t("Local environments")}
            </h2>
            <p className="mt-1 text-xs text-content-muted">
              {t(
                "Setup, cleanup, and one-click terminal actions for Codex-compatible worktrees.",
              )}
            </p>
          </div>
          <button
            className="rounded-lg border border-accent/40 px-3 py-1.5 text-xs text-accent hover:bg-accent/10 disabled:opacity-50"
            disabled={!project || busy}
            onClick={createNew}
            type="button"
          >
            {t("+ Environment")}
          </button>
          <button
            aria-label={t("Close local environments")}
            className="rounded-md px-2 py-1 text-xl text-content-muted hover:bg-surface-raised hover:text-content-primary"
            disabled={busy}
            onClick={onClose}
            type="button"
          >
            ×
          </button>
        </div>

        {error ? (
          <div className="border-b border-danger/25 bg-danger/10 px-5 py-2 text-xs text-danger">
            {error}
          </div>
        ) : null}

        <div className="grid min-h-0 flex-1 grid-cols-[17rem_minmax(0,1fr)]">
          <aside className="overflow-y-auto border-r border-surface-border/25 p-3">
            <p className="px-2 pb-2 font-mono text-[10px] uppercase tracking-[0.16em] text-content-muted">
              {project?.name ?? t("No project")}
            </p>
            {environments.length === 0 ? (
              <p className="rounded-lg border border-dashed border-surface-border/35 px-3 py-5 text-center text-xs text-content-muted">
                {t("No `.codex/environments/*.toml` files yet.")}
              </p>
            ) : (
              <div className="space-y-1">
                {environments.map((environment) => (
                  <button
                    className={`w-full rounded-lg border px-3 py-2 text-left transition-colors ${
                      environment.configPath === editingPath
                        ? "border-accent/45 bg-accent/10"
                        : "border-transparent hover:bg-surface-raised"
                    }`}
                    key={environment.configPath}
                    onClick={() => edit(environment)}
                    type="button"
                  >
                    <span className="flex items-center gap-2">
                      <span className="min-w-0 flex-1 truncate text-xs font-medium text-content-primary">
                        {environment.config?.name ?? fileName(environment.configPath)}
                      </span>
                      {environment.configPath === selectedConfigPath ? (
                        <span className="font-mono text-[9px] text-accent">
                          {t("selected")}
                        </span>
                      ) : null}
                    </span>
                    <span className="mt-1 block truncate font-mono text-[9px] text-content-muted">
                      {environmentScopeFileLabel(
                        environment.inherited,
                        fileName(environment.configPath),
                        t,
                      )}
                    </span>
                    {environment.error ? (
                      <span className="mt-1 block text-[10px] text-danger">
                        {environment.error}
                      </span>
                    ) : null}
                  </button>
                ))}
              </div>
            )}
          </aside>

          <div className="min-h-0 overflow-y-auto p-5">
            {!project ? (
              <p className="text-sm text-content-muted">
                {t("Select a project to configure local environments.")}
              </p>
            ) : (
              <div className="mx-auto max-w-3xl space-y-5">
                {inherited ? (
                  <div className="rounded-lg border border-warning/30 bg-warning-surface/40 px-3 py-2 text-xs text-content-secondary">
                    {t(
                      "This environment is inherited from {workspaceRoot}. Edit it from that workspace; you can still select and run it here.",
                      { workspaceRoot: editingDescriptor?.workspaceRoot ?? "" },
                    )}
                  </div>
                ) : null}

                <div className="rounded-xl border border-surface-border/30 bg-surface-panel/55 p-4">
                  <div className="grid gap-3 sm:grid-cols-2">
                    <label className="text-xs text-content-muted">
                      {t("Name")}
                      <input
                        className={inputClass}
                        disabled={inherited || busy}
                        onChange={(event) =>
                          setConfig((current) => ({
                            ...current,
                            name: event.target.value,
                          }))
                        }
                        value={config.name}
                      />
                    </label>
                    <label className="text-xs text-content-muted">
                      {t("File name")}
                      <input
                        className={`${inputClass} font-mono text-xs`}
                        disabled={editingPath !== null || inherited || busy}
                        onChange={(event) => setFilename(event.target.value)}
                        value={filename}
                      />
                    </label>
                  </div>
                </div>

                <LifecycleEditor
                  disabled={inherited || busy}
                  label={t("Setup")}
                  lifecycle={config.setup}
                  onChange={(lifecycle) => updateLifecycle("setup", lifecycle)}
                />

                <div className="rounded-xl border border-surface-border/30 bg-surface-panel/55 p-4">
                  <div className="mb-3 flex items-center justify-between gap-3">
                    <div>
                      <h3 className="text-sm font-semibold text-content-primary">
                        {t("Cleanup")}
                      </h3>
                      <p className="mt-1 text-xs text-content-muted">
                        {t("Runs before Halv removes a managed worktree.")}
                      </p>
                    </div>
                    <label className="flex items-center gap-2 text-xs text-content-muted">
                      <input
                        checked={Boolean(config.cleanup)}
                        disabled={inherited || busy}
                        onChange={(event) =>
                          updateLifecycle(
                            "cleanup",
                            event.target.checked ? emptyLifecycle() : null,
                          )
                        }
                        type="checkbox"
                      />
                      {t("Enabled")}
                    </label>
                  </div>
                  {config.cleanup ? (
                    <LifecycleFields
                      disabled={inherited || busy}
                      lifecycle={config.cleanup}
                      onChange={(lifecycle) => updateLifecycle("cleanup", lifecycle)}
                    />
                  ) : null}
                </div>

                <div className="rounded-xl border border-surface-border/30 bg-surface-panel/55 p-4">
                  <div className="mb-3 flex items-center justify-between gap-3">
                    <div>
                      <h3 className="text-sm font-semibold text-content-primary">
                        {t("Actions")}
                      </h3>
                      <p className="mt-1 text-xs text-content-muted">
                        {t(
                          "Header buttons run these commands in Halv’s integrated terminal.",
                        )}
                      </p>
                    </div>
                    <button
                      className="rounded-lg border border-surface-border/35 px-3 py-1.5 text-xs text-content-secondary hover:bg-surface-raised disabled:opacity-50"
                      disabled={inherited || busy}
                      onClick={() =>
                        setConfig((current) => ({
                          ...current,
                          actions: [
                            ...current.actions,
                            { name: "Run", icon: "run", command: "", platform: null },
                          ],
                        }))
                      }
                      type="button"
                    >
                      {t("+ Action")}
                    </button>
                  </div>
                  <div className="space-y-3">
                    {config.actions.length === 0 ? (
                      <p className="rounded-lg border border-dashed border-surface-border/35 px-3 py-4 text-center text-xs text-content-muted">
                        {t("No terminal actions.")}
                      </p>
                    ) : null}
                    {config.actions.map((action, index) => (
                      <div
                        className="rounded-lg border border-surface-border/25 bg-surface-base/65 p-3"
                        key={`${index}-${action.name}`}
                      >
                        <div className="grid gap-3 sm:grid-cols-[1fr_8rem_9rem_auto]">
                          <label className="text-xs text-content-muted">
                            {t("Label")}
                            <input
                              className={inputClass}
                              disabled={inherited || busy}
                              onChange={(event) => updateAction(index, { name: event.target.value })}
                              value={action.name}
                            />
                          </label>
                          <label className="text-xs text-content-muted">
                            {t("Icon")}
                            <select
                              className={inputClass}
                              disabled={inherited || busy}
                              onChange={(event) =>
                                updateAction(index, {
                                  icon: event.target.value as LocalEnvironmentAction["icon"],
                                })
                              }
                              value={action.icon}
                            >
                              {(["tool", "run", "debug", "test"] as const).map(
                                (icon) => (
                                  <option key={icon} value={icon}>
                                    {actionIconLabel(icon, t)}
                                  </option>
                                ),
                              )}
                            </select>
                          </label>
                          <label className="text-xs text-content-muted">
                            {t("Platform")}
                            <select
                              className={inputClass}
                              disabled={inherited || busy}
                              onChange={(event) =>
                                updateAction(index, {
                                  platform: (event.target.value || null) as LocalEnvironmentAction["platform"],
                                })
                              }
                              value={action.platform ?? ""}
                            >
                              <option value="">{t("All")}</option>
                              <option value="darwin">{t("macOS")}</option>
                              <option value="linux">{t("Linux")}</option>
                              <option value="win32">{t("Windows")}</option>
                            </select>
                          </label>
                          <button
                            aria-label={
                              action.name
                                ? t("Delete {name}", { name: action.name })
                                : t("Delete action")
                            }
                            className="self-end rounded-lg px-2 py-2 text-xs text-content-muted hover:bg-danger/10 hover:text-danger disabled:opacity-50"
                            disabled={inherited || busy}
                            onClick={() =>
                              setConfig((current) => ({
                                ...current,
                                actions: current.actions.filter((_, actionIndex) => actionIndex !== index),
                              }))
                            }
                            type="button"
                          >
                            {t("Delete")}
                          </button>
                        </div>
                        <label className="mt-3 block text-xs text-content-muted">
                          {commandPlatformLabel(action.platform, t)}
                          <textarea
                            className={`${scriptClass} min-h-20`}
                            disabled={inherited || busy}
                            onChange={(event) => updateAction(index, { command: event.target.value })}
                            spellCheck={false}
                            value={action.command}
                          />
                        </label>
                      </div>
                    ))}
                  </div>
                </div>

                <div className="sticky bottom-0 flex items-center gap-2 border-t border-surface-border/30 bg-surface-base/95 py-3 backdrop-blur">
                  {editingPath && config ? (
                    <button
                      className="rounded-lg border border-accent/40 px-3 py-2 text-xs text-accent hover:bg-accent/10 disabled:opacity-50"
                      disabled={busy || Boolean(editingDescriptor?.error)}
                      onClick={() => onSelect(editingPath)}
                      type="button"
                    >
                      {selectedConfigPath === editingPath
                        ? t("Selected for new worktrees")
                        : t("Use for new worktrees")}
                    </button>
                  ) : null}
                  {editingPath && !inherited ? (
                    <button
                      className="rounded-lg px-3 py-2 text-xs text-content-muted hover:bg-danger/10 hover:text-danger disabled:opacity-50"
                      disabled={busy}
                      onClick={() => void remove()}
                      type="button"
                    >
                      {t("Delete")}
                    </button>
                  ) : null}
                  <div className="flex-1" />
                  <button
                    className="rounded-lg bg-accent px-4 py-2 text-sm font-semibold text-accent-contrast hover:bg-accent-strong disabled:opacity-50"
                    disabled={busy || inherited || !config.name.trim() || (!editingPath && !filename.trim())}
                    onClick={() => void save()}
                    type="button"
                  >
                    {busy
                      ? t("Saving…")
                      : editingPath
                        ? t("Save environment")
                        : t("Create environment")}
                  </button>
                </div>
              </div>
            )}
          </div>
        </div>
      </section>
    </div>
  );
}

function LifecycleEditor({
  label,
  lifecycle,
  disabled,
  onChange,
}: {
  label: string;
  lifecycle: LocalEnvironmentLifecycle;
  disabled: boolean;
  onChange: (lifecycle: LocalEnvironmentLifecycle) => void;
}) {
  const t = useT();
  return (
    <div className="rounded-xl border border-surface-border/30 bg-surface-panel/55 p-4">
      <h3 className="text-sm font-semibold text-content-primary">{label}</h3>
      <p className="mb-3 mt-1 text-xs text-content-muted">
        {t(
          "Runs once after Halv creates the worktree. Platform scripts override the default when non-empty.",
        )}
      </p>
      <LifecycleFields disabled={disabled} lifecycle={lifecycle} onChange={onChange} />
    </div>
  );
}

function LifecycleFields({
  lifecycle,
  disabled,
  onChange,
}: {
  lifecycle: LocalEnvironmentLifecycle;
  disabled: boolean;
  onChange: (lifecycle: LocalEnvironmentLifecycle) => void;
}) {
  const t = useT();
  const update = (
    platform: "script" | "darwin" | "linux" | "win32",
    script: string,
  ) => {
    if (platform === "script") {
      onChange({ ...lifecycle, script });
      return;
    }
    onChange({
      ...lifecycle,
      [platform]: script ? { script } : null,
    });
  };

  return (
    <div className="grid gap-3 sm:grid-cols-2">
      <label className="text-xs text-content-muted sm:col-span-2">
        {t("Default script")}
        <textarea
          className={scriptClass}
          disabled={disabled}
          onChange={(event) => update("script", event.target.value)}
          spellCheck={false}
          value={lifecycle.script}
        />
      </label>
      {(["darwin", "linux", "win32"] as const).map((platform) => (
        <label className="text-xs text-content-muted" key={platform}>
          {lifecycleOverrideLabel(platform, t)}
          <textarea
            className={`${scriptClass} min-h-20`}
            disabled={disabled}
            onChange={(event) => update(platform, event.target.value)}
            spellCheck={false}
            value={lifecycle[platform]?.script ?? ""}
          />
        </label>
      ))}
    </div>
  );
}

function actionIconLabel(icon: LocalEnvironmentAction["icon"], t: Translate) {
  switch (icon) {
    case "tool":
      return t("tool");
    case "run":
      return t("run");
    case "debug":
      return t("debug");
    case "test":
      return t("test");
  }
}

function environmentScopeFileLabel(
  inherited: boolean,
  name: string,
  t: Translate,
) {
  return inherited
    ? t("inherited · {name}", { name })
    : t("project · {name}", { name });
}

function commandPlatformLabel(
  platform: LocalEnvironmentAction["platform"],
  t: Translate,
) {
  switch (platform) {
    case "darwin":
      return t("Command · macOS");
    case "win32":
      return t("Command · Windows");
    case "linux":
      return t("Command · Linux");
    default:
      return t("Command · All platforms");
  }
}

function lifecycleOverrideLabel(
  platform: "darwin" | "linux" | "win32",
  t: Translate,
) {
  switch (platform) {
    case "darwin":
      return t("macOS override");
    case "linux":
      return t("Linux override");
    case "win32":
      return t("Windows override");
  }
}
