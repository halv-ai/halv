import { useEffect, useId, useRef, useState, type FormEvent, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { useT } from "../i18n/runtime";
import type { WorkspaceStatus } from "../types";
import { AppDialogFrame } from "./AppDialogHost";

// The confirmation dialogs of the Projects tree (ADR 0010, stage S7). They
// copy Orca's flows: a removal says what goes and what stays, checks the
// worktree first, and offers no button while a rule blocks the removal.

function TreeDialog({
  title,
  children,
  confirmLabel,
  confirmDisabled,
  danger,
  onConfirm,
  onCancel,
}: {
  title: string;
  children: ReactNode;
  confirmLabel: string;
  confirmDisabled?: boolean;
  danger?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const t = useT();
  const titleId = useId();
  const formRef = useRef<HTMLFormElement>(null);

  useEffect(() => {
    const field = formRef.current?.querySelector<HTMLElement>("input, select");
    (field ?? formRef.current?.querySelector<HTMLElement>("button[type=button]"))?.focus();
  }, []);

  function submit(event: FormEvent) {
    event.preventDefault();
    if (!confirmDisabled) onConfirm();
  }

  return createPortal(
    <AppDialogFrame labelledBy={titleId} onDismiss={onCancel} restoreFocus>
      <form
        className="w-full max-w-md overflow-hidden rounded-xl border border-surface-border/35 bg-surface-glass/95 shadow-2xl shadow-black/30"
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            onCancel();
          }
        }}
        onSubmit={submit}
        ref={formRef}
      >
        <div className="space-y-3 px-5 py-4">
          <h2 className="text-sm font-semibold text-content-primary" id={titleId}>
            {title}
          </h2>
          {children}
        </div>
        <div className="flex justify-end gap-2 border-t border-surface-border/25 bg-surface-raised/30 px-5 py-3">
          <button
            className="rounded-md border border-surface-border/35 px-3 py-2 text-sm text-content-secondary transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
            onClick={onCancel}
            type="button"
          >
            {t("Cancel")}
          </button>
          <button
            className={`rounded-md border px-3 py-2 text-sm font-medium transition-colors focus:outline-none focus:ring-2 disabled:cursor-not-allowed disabled:opacity-50 ${
              danger
                ? "border-danger/35 bg-danger/10 text-danger hover:bg-danger/15 focus:ring-danger/30"
                : "border-accent bg-accent text-accent-contrast hover:bg-accent-strong focus:ring-accent/40"
            }`}
            disabled={confirmDisabled}
            type="submit"
          >
            {confirmLabel}
          </button>
        </div>
      </form>
    </AppDialogFrame>,
    document.body,
  );
}

function DialogNote({
  tone = "muted",
  children,
}: {
  tone?: "muted" | "warning" | "danger" | "ok";
  children: ReactNode;
}) {
  const color =
    tone === "danger"
      ? "border-danger/30 bg-danger/10 text-danger"
      : tone === "warning"
        ? "border-warning/30 bg-warning/10 text-warning"
        : tone === "ok"
          ? "border-surface-border/30 bg-surface-raised/40 text-content-secondary"
          : "border-surface-border/30 bg-surface-raised/40 text-content-muted";
  return (
    <p className={`rounded-lg border px-3 py-2 text-[12px] leading-[17px] ${color}`} role="status">
      {children}
    </p>
  );
}

/**
 * "Remove worktree…": Orca's delete dialog on Halv's rules. Halv removes
 * only a managed, unpinned and clean worktree that no conversation uses,
 * so the dialog checks those facts first.
 */
export function RemoveWorktreeDialog({
  name,
  rootPath,
  openTerminalCount,
  inspect,
  onConfirm,
  onCancel,
}: {
  name: string;
  rootPath: string;
  openTerminalCount: number;
  /** The status of the worktree, with its changes and conversations. */
  inspect: () => Promise<WorkspaceStatus | null>;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const t = useT();
  const [status, setStatus] = useState<
    | { kind: "checking" }
    | { kind: "failed" }
    | { kind: "ready"; workspace: WorkspaceStatus }
  >({ kind: "checking" });

  useEffect(() => {
    let cancelled = false;
    void inspect()
      .then((workspace) => {
        if (cancelled) return;
        setStatus(workspace ? { kind: "ready", workspace } : { kind: "failed" });
      })
      .catch(() => {
        if (!cancelled) setStatus({ kind: "failed" });
      });
    return () => {
      cancelled = true;
    };
    // The dialog checks once, when it opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const workspace = status.kind === "ready" ? status.workspace : null;
  const blockers = workspace
    ? [
        workspace.dirty
          ? t("This worktree has uncommitted or untracked changes. Commit or stash them first.")
          : null,
        workspace.pinned ? t("This worktree is pinned. Unpin it first.") : null,
        workspace.conversationCount === 1
          ? t("1 conversation uses this worktree. Halv removes a worktree only when no conversation uses it.")
          : workspace.conversationCount > 1
            ? t(
                "{count} conversations use this worktree. Halv removes a worktree only when no conversation uses it.",
                { count: workspace.conversationCount },
              )
            : null,
      ].filter((line): line is string => Boolean(line))
    : [];

  return (
    <TreeDialog
      confirmDisabled={status.kind !== "ready" || blockers.length > 0}
      confirmLabel={t("Remove worktree")}
      danger
      onCancel={onCancel}
      onConfirm={onConfirm}
      title={t("Remove worktree")}
    >
      <p className="text-sm leading-5 text-content-secondary">
        {t("Remove {name} from git and delete its worktree folder.", { name })}
      </p>
      <p className="truncate font-mono text-[11px] text-content-muted" title={rootPath}>
        {rootPath}
      </p>
      {status.kind === "checking" ? (
        <DialogNote>{t("Checking for changes…")}</DialogNote>
      ) : status.kind === "failed" ? (
        <DialogNote tone="warning">{t("Changes could not be checked.")}</DialogNote>
      ) : blockers.length > 0 ? (
        blockers.map((line) => (
          <DialogNote key={line} tone="danger">
            {line}
          </DialogNote>
        ))
      ) : (
        <DialogNote tone="ok">{t("No uncommitted or untracked changes")}</DialogNote>
      )}
      {openTerminalCount > 0 ? (
        <DialogNote tone="warning">
          {openTerminalCount === 1
            ? t("1 open terminal in this worktree will be closed.")
            : t("{count} open terminals in this worktree will be closed.", {
                count: openTerminalCount,
              })}
        </DialogNote>
      ) : null}
    </TreeDialog>
  );
}

/** "Remove project from Halv…": Orca's Remove Project dialog. */
export function RemoveProjectDialog({
  name,
  worktreeCount,
  openTerminalCount,
  onConfirm,
  onCancel,
}: {
  name: string;
  /** Halv-managed worktrees. The project stays until they go. */
  worktreeCount: number;
  openTerminalCount: number;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const t = useT();
  return (
    <TreeDialog
      confirmDisabled={worktreeCount > 0}
      confirmLabel={t("Remove")}
      danger
      onCancel={onCancel}
      onConfirm={onConfirm}
      title={t("Remove project")}
    >
      <p className="text-sm leading-5 text-content-secondary">
        {t("This only removes {name} from Halv. It is still on your disk.", { name })}
      </p>
      {worktreeCount > 0 ? (
        <DialogNote tone="danger">
          {worktreeCount === 1
            ? t("This project has 1 worktree. Remove it first.")
            : t("This project has {count} worktrees. Remove them first.", {
                count: worktreeCount,
              })}
        </DialogNote>
      ) : null}
      {openTerminalCount > 0 ? (
        <DialogNote tone="warning">
          {openTerminalCount === 1
            ? t("1 open terminal in this project will be closed.")
            : t("{count} open terminals in this project will be closed.", {
                count: openTerminalCount,
              })}
        </DialogNote>
      ) : null}
    </TreeDialog>
  );
}

/**
 * "New worktree…": Orca's "Create worktree" composer, with Halv's two
 * inputs. An empty branch name gives a detached worktree.
 */
export function NewWorktreeDialog({
  projectName,
  loadBaseRefs,
  onCreate,
  onCancel,
}: {
  projectName: string;
  loadBaseRefs: () => Promise<string[]>;
  onCreate: (request: { branch: string | null; baseRef: string }) => void;
  onCancel: () => void;
}) {
  const t = useT();
  const branchId = useId();
  const baseId = useId();
  const [branch, setBranch] = useState("");
  const [baseRefs, setBaseRefs] = useState<string[] | null>(null);
  const [baseRef, setBaseRef] = useState("HEAD");

  useEffect(() => {
    let cancelled = false;
    void loadBaseRefs()
      .then((refs) => {
        if (!cancelled) setBaseRefs(refs.length > 0 ? refs : ["HEAD"]);
      })
      .catch(() => {
        if (!cancelled) setBaseRefs(["HEAD"]);
      });
    return () => {
      cancelled = true;
    };
    // The dialog reads the refs once, when it opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <TreeDialog
      confirmLabel={t("Create")}
      onCancel={onCancel}
      onConfirm={() => onCreate({ branch: branch.trim() || null, baseRef })}
      title={t("Create worktree")}
    >
      <p className="text-sm leading-5 text-content-secondary">
        {t("A new worktree of {project}, in its own folder.", { project: projectName })}
      </p>
      <label className="block space-y-1" htmlFor={branchId}>
        <span className="text-[12px] font-medium text-content-secondary">{t("Branch name")}</span>
        <input
          className="w-full rounded-lg border border-surface-border/35 bg-surface-raised px-3 py-2 font-mono text-sm text-content-primary outline-none placeholder:text-content-muted focus:border-accent/60 focus:ring-2 focus:ring-accent/20"
          id={branchId}
          onChange={(event) => setBranch(event.target.value)}
          placeholder="feature/my-branch"
          spellCheck={false}
          value={branch}
        />
        <span className="block text-[11px] text-content-muted">
          {t("Optional. Without a name, the worktree starts on a detached HEAD.")}
        </span>
      </label>
      <label className="block space-y-1" htmlFor={baseId}>
        <span className="text-[12px] font-medium text-content-secondary">{t("Create from")}</span>
        <select
          className="w-full rounded-lg border border-surface-border/35 bg-surface-raised px-3 py-2 font-mono text-sm text-content-primary outline-none focus:border-accent/60 focus:ring-2 focus:ring-accent/20"
          disabled={baseRefs === null}
          id={baseId}
          onChange={(event) => setBaseRef(event.target.value)}
          value={baseRef}
        >
          {(baseRefs ?? [baseRef]).map((ref) => (
            <option key={ref} value={ref}>
              {ref}
            </option>
          ))}
        </select>
      </label>
    </TreeDialog>
  );
}

/** Orca's "New Project Group" and "Rename Project Group" dialog. */
export function ProjectGroupNameDialog({
  mode,
  initialName,
  onSubmit,
  onCancel,
}: {
  mode: "create" | "rename";
  initialName: string;
  onSubmit: (name: string) => void;
  onCancel: () => void;
}) {
  const t = useT();
  const nameId = useId();
  const [name, setName] = useState(initialName);
  return (
    <TreeDialog
      confirmDisabled={!name.trim()}
      confirmLabel={mode === "create" ? t("Create") : t("Rename")}
      onCancel={onCancel}
      onConfirm={() => onSubmit(name.trim())}
      title={mode === "create" ? t("New project group") : t("Rename project group")}
    >
      <p className="text-sm leading-5 text-content-secondary">
        {mode === "create"
          ? t("Create a group and move this project into it.")
          : t("Change the group name that the sidebar shows.")}
      </p>
      <label className="block space-y-1" htmlFor={nameId}>
        <span className="text-[12px] font-medium text-content-secondary">{t("Group name")}</span>
        <input
          className="w-full rounded-lg border border-surface-border/35 bg-surface-raised px-3 py-2 text-sm text-content-primary outline-none placeholder:text-content-muted focus:border-accent/60 focus:ring-2 focus:ring-accent/20"
          id={nameId}
          onChange={(event) => setName(event.target.value)}
          onFocus={(event) => event.currentTarget.select()}
          value={name}
        />
      </label>
    </TreeDialog>
  );
}

/** Orca's "Delete Project Group": the group goes, its projects stay. */
export function DeleteProjectGroupDialog({
  name,
  projectNames,
  onConfirm,
  onCancel,
}: {
  name: string;
  projectNames: string[];
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const t = useT();
  const shown = projectNames.slice(0, 4);
  return (
    <TreeDialog
      confirmLabel={t("Delete group")}
      danger
      onCancel={onCancel}
      onConfirm={onConfirm}
      title={t("Delete project group")}
    >
      <p className="text-sm leading-5 text-content-secondary">
        {t("Delete {name}. Its projects stay in the sidebar, without a group.", { name })}
      </p>
      {shown.length > 0 ? (
        <div className="rounded-lg border border-surface-border/30 bg-surface-raised/40 px-3 py-2">
          <p className="pb-1 text-[11px] font-medium uppercase tracking-[0.08em] text-content-muted">
            {t("Projects in this group")}
          </p>
          <ul className="space-y-0.5 text-[12px] text-content-secondary">
            {shown.map((project) => (
              <li className="truncate" key={project}>
                {project}
              </li>
            ))}
            {projectNames.length > shown.length ? (
              <li className="text-content-muted">
                {t("+{count} more", { count: projectNames.length - shown.length })}
              </li>
            ) : null}
          </ul>
        </div>
      ) : null}
    </TreeDialog>
  );
}

export { TreeDialog, DialogNote };
