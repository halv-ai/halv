import { useEffect, useRef, useState } from "react";
import { useT, type Translate } from "../i18n/runtime";
import type { LocalEnvironmentDescriptor, WorkspaceStatus } from "../types";

type WorkspaceSwitcherProps = {
  workspaces: WorkspaceStatus[];
  selectedWorkspaceId: string | null;
  conversationBound: boolean;
  busy: boolean;
  error: string | null;
  baseRefs: string[];
  selectedBaseRef: string;
  environments: LocalEnvironmentDescriptor[];
  selectedEnvironmentConfigPath: string | null;
  onCreate: () => void;
  onConfigureEnvironments: () => void;
  onPin: (workspace: WorkspaceStatus, pinned: boolean) => void;
  onRemove: (workspace: WorkspaceStatus) => void;
  onSelect: (workspace: WorkspaceStatus) => void;
  onSelectBaseRef: (baseRef: string) => void;
  onSelectEnvironment: (configPath: string | null) => void;
};

export function WorkspaceSwitcher({
  workspaces,
  selectedWorkspaceId,
  conversationBound,
  busy,
  error,
  baseRefs,
  selectedBaseRef,
  environments,
  selectedEnvironmentConfigPath,
  onCreate,
  onConfigureEnvironments,
  onPin,
  onRemove,
  onSelect,
  onSelectBaseRef,
  onSelectEnvironment,
}: WorkspaceSwitcherProps) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const [creationOptionsOpen, setCreationOptionsOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const selected =
    workspaces.find((workspace) => workspace.id === selectedWorkspaceId) ??
    workspaces[0] ??
    null;
  const selectedLabel =
    selected?.currentBranch ?? selected?.branch ?? selected?.name ?? t("Default");

  useEffect(() => {
    if (!open) {
      return;
    }
    const close = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") {
        return;
      }
      event.preventDefault();
      setOpen(false);
      window.requestAnimationFrame(() => triggerRef.current?.focus());
    };
    window.addEventListener("pointerdown", close);
    window.addEventListener("keydown", closeOnEscape);
    return () => {
      window.removeEventListener("pointerdown", close);
      window.removeEventListener("keydown", closeOnEscape);
    };
  }, [open]);

  function closeAndRestoreFocus() {
    setOpen(false);
    window.requestAnimationFrame(() => triggerRef.current?.focus());
  }

  return (
    <div className="relative" ref={rootRef}>
      <button
        aria-controls="halv-workspace-switcher"
        aria-expanded={open}
        aria-haspopup="dialog"
        aria-label={t("Workspace: {workspace}", { workspace: selectedLabel })}
        className="flex max-w-[min(14rem,42vw)] items-center gap-2 rounded-lg px-2 py-1.5 text-left transition-colors hover:bg-surface-raised focus:outline-none focus:ring-2 focus:ring-accent/30"
        onClick={() => setOpen((current) => !current)}
        ref={triggerRef}
        title={t("Workspace: {workspace} - switch or create worktrees", {
          workspace: selectedLabel,
        })}
        type="button"
      >
        <span className="h-2 w-2 shrink-0 rounded-full bg-accent" aria-hidden />
        <span className="min-w-0 truncate font-mono text-xs text-content-secondary">
          {selectedLabel}
        </span>
        {selected?.dirty ? (
          <span className="shrink-0 rounded-full bg-warning-surface/70 px-2 py-0.5 font-mono text-[10px] text-warning">
            {t("dirty")}
          </span>
        ) : null}
        <span
          aria-hidden
          className={`shrink-0 text-[9px] text-content-muted transition-transform ${open ? "rotate-180" : ""}`}
        >
          ▾
        </span>
      </button>

      {open ? (
        <div
          aria-labelledby="halv-workspace-switcher-title"
          className="absolute right-0 top-[calc(100%+0.5rem)] z-50 w-[min(24rem,calc(100vw-1rem))] overflow-hidden rounded-xl border border-surface-border/35 bg-surface-glass/95 shadow-2xl shadow-black/25 backdrop-blur-xl"
          id="halv-workspace-switcher"
          role="dialog"
        >
          <div className="flex items-center justify-between border-b border-surface-border/25 px-3 py-2">
            <div className="min-w-0">
              <p
                className="text-xs font-semibold text-content-primary"
                id="halv-workspace-switcher-title"
              >
                {t("Workspaces")}
              </p>
              <p className="mt-0.5 text-[10px] text-content-muted">
                {t("Separate worktrees can run independently.")}
              </p>
            </div>
            <div className="ml-3 flex shrink-0 items-center gap-1.5">
              <button
                className="rounded-md border border-accent/40 px-2.5 py-1 text-xs text-accent transition-colors hover:bg-accent/10 disabled:opacity-50"
                disabled={busy}
                onClick={onCreate}
                type="button"
              >
                {busy ? t("Working…") : t("+ Worktree")}
              </button>
              <button
                aria-label={t("Close workspace switcher")}
                className="flex h-7 w-7 items-center justify-center rounded-md text-content-muted transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
                onClick={closeAndRestoreFocus}
                type="button"
              >
                <span aria-hidden>×</span>
              </button>
            </div>
          </div>
          {error ? (
            <div
              className="border-b border-danger/25 bg-danger/10 px-3 py-2 text-xs text-danger"
              role="alert"
            >
              {error}
            </div>
          ) : null}
          <div className="border-b border-surface-border/25 px-3 py-2">
            <button
              aria-controls="halv-worktree-creation-options"
              aria-expanded={creationOptionsOpen}
              className="flex w-full items-center justify-between rounded-lg px-2 py-1.5 text-left text-xs text-content-secondary transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
              onClick={() => setCreationOptionsOpen((current) => !current)}
              type="button"
            >
              <span>
                {t("Creation options")}
                <span className="ml-2 text-[10px] text-content-muted">
                  {creationOptionsSummary(
                    selectedBaseRef,
                    Boolean(selectedEnvironmentConfigPath),
                    t,
                  )}
                </span>
              </span>
              <span aria-hidden>{creationOptionsOpen ? "−" : "+"}</span>
            </button>
            {creationOptionsOpen ? (
              <div
                className="mt-2 grid gap-2 sm:grid-cols-2"
                id="halv-worktree-creation-options"
              >
                <label className="min-w-0 text-[10px] text-content-muted">
                  {t("Start from")}
                  <select
                    className="mt-1 w-full rounded-lg border border-surface-border/30 bg-surface-base px-2.5 py-1.5 text-xs text-content-primary outline-none focus:border-accent/50"
                    disabled={busy || baseRefs.length === 0}
                    onChange={(event) => onSelectBaseRef(event.target.value)}
                    value={selectedBaseRef}
                  >
                    {baseRefs.length === 0 ? (
                      <option value="">{t("Detecting refs…")}</option>
                    ) : null}
                    {baseRefs.map((baseRef) => (
                      <option key={baseRef} value={baseRef}>
                        {baseRef}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="min-w-0 text-[10px] text-content-muted">
                  {t("Environment")}
                  <select
                    className="mt-1 w-full rounded-lg border border-surface-border/30 bg-surface-base px-2.5 py-1.5 text-xs text-content-primary outline-none focus:border-accent/50"
                    disabled={busy}
                    onChange={(event) => onSelectEnvironment(event.target.value || null)}
                    value={selectedEnvironmentConfigPath ?? ""}
                  >
                    <option value="">{t("Default (no setup)")}</option>
                    {environments
                      .filter((environment) => environment.config && !environment.error)
                      .map((environment) => (
                        <option key={environment.configPath} value={environment.configPath}>
                          {environmentOptionLabel(environment, t)}
                        </option>
                      ))}
                  </select>
                </label>
                <button
                  className="rounded-lg border border-surface-border/30 px-2.5 py-1.5 text-xs text-content-muted hover:bg-surface-raised hover:text-content-primary sm:col-span-2"
                  onClick={onConfigureEnvironments}
                  type="button"
                >
                  {t("Configure environments")}
                </button>
              </div>
            ) : null}
          </div>
          <div
            aria-label={t("Available workspaces")}
            className="max-h-[min(20rem,48vh)] space-y-1 overflow-y-auto p-1.5"
          >
            {workspaces.length === 0 ? (
              <div className="px-3 py-6 text-center text-xs text-content-muted" role="status">
                {busy ? t("Loading workspaces…") : t("No workspaces available")}
              </div>
            ) : workspaces.map((workspace) => {
              const active = workspace.id === selectedWorkspaceId;
              const removable =
                workspace.kind === "worktree" &&
                workspace.managed &&
                !workspace.pinned &&
                !workspace.dirty &&
                workspace.conversationCount === 0;
              return (
                <div
                  className={`rounded-lg border p-2.5 ${
                    active
                      ? "border-accent/45 bg-accent/10"
                      : "border-transparent hover:bg-surface-raised/70"
                  }`}
                  key={workspace.id}
                >
                  <button
                    className="w-full text-left"
                    disabled={!workspace.available || busy}
                    onClick={() => {
                      onSelect(workspace);
                      closeAndRestoreFocus();
                    }}
                    type="button"
                  >
                    <span className="flex items-center gap-2">
                      <span className="min-w-0 flex-1 truncate text-xs font-medium text-content-primary">
                        {workspace.currentBranch ?? workspace.branch ?? workspace.name}
                      </span>
                      {workspace.dirty ? (
                        <span className="font-mono text-[9px] uppercase text-warning">
                          {t("dirty")}
                        </span>
                      ) : null}
                      {workspace.pinned ? (
                        <span className="font-mono text-[9px] text-accent">
                          {t("pinned")}
                        </span>
                      ) : null}
                    </span>
                    <span className="mt-1 block truncate font-mono text-[10px] text-content-muted">
                      {workspaceLocationLabel(workspace, t)}
                    </span>
                    {workspace.localEnvironmentConfigPath ? (
                      <span className="mt-1 block truncate text-[10px] text-accent">
                        {workspaceEnvironmentLabel(workspace, environments, t)}
                      </span>
                    ) : null}
                    {conversationBound && !active ? (
                      <span className="mt-1 block text-[10px] text-content-muted">
                        {t("Opens a new chat in this workspace")}
                      </span>
                    ) : null}
                  </button>
                  {workspace.kind === "worktree" ? (
                    <div className="mt-2 flex items-center gap-2 border-t border-surface-border/20 pt-2">
                      <button
                        className="text-[10px] text-content-muted hover:text-content-primary disabled:opacity-50"
                        disabled={busy}
                        onClick={() => onPin(workspace, !workspace.pinned)}
                        type="button"
                      >
                        {workspace.pinned ? t("Unpin") : t("Pin")}
                      </button>
                      <button
                        className="text-[10px] text-content-muted hover:text-danger disabled:cursor-not-allowed disabled:opacity-35"
                        disabled={busy || !removable}
                        onClick={() => onRemove(workspace)}
                        title={
                          removable
                            ? t("Remove managed worktree")
                            : t(
                                "Worktrees must be unpinned, clean, and unused before removal",
                              )
                        }
                        type="button"
                      >
                        {t("Remove")}
                      </button>
                      {workspace.conversationCount > 0 ? (
                        <span className="ml-auto text-[10px] text-content-muted">
                          {workspace.conversationCount === 1
                            ? t("{count} chat", {
                                count: workspace.conversationCount,
                              })
                            : t("{count} chats", {
                                count: workspace.conversationCount,
                              })}
                        </span>
                      ) : null}
                    </div>
                  ) : null}
                </div>
              );
            })}
          </div>
        </div>
      ) : null}
    </div>
  );
}

function creationOptionsSummary(
  baseRef: string,
  customEnvironment: boolean,
  t: Translate,
) {
  if (baseRef) {
    return customEnvironment
      ? t("{baseRef} · custom environment", { baseRef })
      : t("{baseRef} · no setup", { baseRef });
  }
  return customEnvironment
    ? t("default ref · custom environment")
    : t("default ref · no setup");
}

function environmentOptionLabel(
  environment: LocalEnvironmentDescriptor,
  t: Translate,
) {
  const name = environment.config?.name ?? environment.configPath;
  return environment.inherited
    ? t("{name} · inherited", { name })
    : name;
}

function workspaceLocationLabel(workspace: WorkspaceStatus, t: Translate) {
  return workspace.kind === "primary"
    ? t("primary · {rootPath}", { rootPath: workspace.rootPath })
    : t("managed · {rootPath}", { rootPath: workspace.rootPath });
}

function workspaceEnvironmentLabel(
  workspace: WorkspaceStatus,
  environments: LocalEnvironmentDescriptor[],
  t: Translate,
) {
  const configPath = workspace.localEnvironmentConfigPath;
  if (!configPath) {
    return "";
  }
  const environment =
    environments.find((item) => item.configPath === configPath)?.config?.name ??
    configPath.split(/[\\/]/).pop() ??
    configPath;
  return t("env · {environment}", { environment });
}
