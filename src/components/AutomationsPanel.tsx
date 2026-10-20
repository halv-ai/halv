import { ipc } from "../ipc.ts";
import { useEffect, useMemo, useState } from "react";
import { formatDateTime, useT, type Translate } from "../i18n/runtime";
import type {
  Automation,
  AutomationConversationMode,
  AutomationDraft,
  AutomationRun,
  AutomationTarget,
  AutomationWorkspaceStrategy,
  ChatModel,
  ConversationSummary,
  Project,
} from "../types";

type AutomationsPanelProps = {
  open: boolean;
  projects: Project[];
  conversations: ConversationSummary[];
  models: ChatModel[];
  currentProjectId: number | null;
  currentWorkspaceId: string | null;
  currentConversationId: string | null;
  onClose: () => void;
  onOpenConversation: (conversationId: string) => void;
  onUnreadCountChange: (count: number) => void;
};

type InboxRun = {
  automationId: string;
  automationName: string;
  run: AutomationRun;
};

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function formatWhen(value: string | null | undefined, t: Translate) {
  if (!value) return t("never");
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : formatDateTime(date);
}

function scheduleLabel(automation: Automation, t: Translate) {
  return automation.rrule
    ? automation.rrule.replace(/^RRULE:/, "")
    : t("every {cadenceMinutes}m", {
        cadenceMinutes: automation.cadenceMinutes,
      });
}

function automationStatusLabel(status: string, t: Translate) {
  switch (status) {
    case "running":
      return t("running");
    case "completed":
      return t("completed");
    case "failed":
      return t("failed");
    default:
      return status;
  }
}

function automationTargetLabel(target: AutomationTarget, t: Translate) {
  switch (target) {
    case "local":
      return t("local");
    case "cloud":
      return t("cloud");
  }
}

function automationContextLabel(automation: Automation, t: Translate) {
  if (automation.workspaceStrategy === "new_worktree") {
    return automation.conversationMode === "existing"
      ? t("isolated worktree · continues conversation")
      : t("isolated worktree · new conversation each run");
  }
  return automation.conversationMode === "existing"
    ? t("project checkout · continues conversation")
    : t("project checkout · new conversation each run");
}

function lastRunLabel(automation: Automation, t: Translate) {
  if (!automation.lastStatus) return "";
  const status = automationStatusLabel(automation.lastStatus, t);
  if (automation.lastError && automation.cloudTaskId) {
    return t("last: {status} — {error} · task {taskId}", {
      status,
      error: automation.lastError,
      taskId: automation.cloudTaskId,
    });
  }
  if (automation.lastError) {
    return t("last: {status} — {error}", {
      status,
      error: automation.lastError,
    });
  }
  if (automation.cloudTaskId) {
    return t("last: {status} · task {taskId}", {
      status,
      taskId: automation.cloudTaskId,
    });
  }
  return t("last: {status}", { status });
}

function runTone(status: string) {
  if (status === "failed") return "text-danger";
  if (status === "running") return "text-warning";
  return "text-accent";
}

const inputClass =
  "mt-1 w-full rounded-lg border border-surface-border/35 bg-surface-base px-3 py-2 text-sm text-content-primary";

export function AutomationsPanel({
  open,
  projects,
  conversations,
  models,
  currentProjectId,
  currentWorkspaceId,
  currentConversationId,
  onClose,
  onOpenConversation,
  onUnreadCountChange,
}: AutomationsPanelProps) {
  const t = useT();
  const [automations, setAutomations] = useState<Automation[]>([]);
  const [view, setView] = useState<"scheduled" | "inbox">("scheduled");
  const [name, setName] = useState("");
  const [prompt, setPrompt] = useState("");
  const [projectId, setProjectId] = useState<number | null>(currentProjectId);
  const [model, setModel] = useState(models[0]?.id ?? "gpt-5.5");
  const [effort, setEffort] = useState("medium");
  const [target, setTarget] = useState<AutomationTarget>("local");
  const [cadenceMinutes, setCadenceMinutes] = useState(1_440);
  const [rrule, setRrule] = useState("");
  const [timezone, setTimezone] = useState(
    () => Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC",
  );
  const [workspaceStrategy, setWorkspaceStrategy] =
    useState<AutomationWorkspaceStrategy>("current");
  const [conversationMode, setConversationMode] =
    useState<AutomationConversationMode>("new");
  const [conversationId, setConversationId] = useState<string | null>(
    currentConversationId,
  );
  const [repositoryUrl, setRepositoryUrl] = useState("");
  const [baseRef, setBaseRef] = useState("");
  const [editingId, setEditingId] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [initialLoading, setInitialLoading] = useState(true);
  const [initialLoadError, setInitialLoadError] = useState<string | null>(null);

  const projectConversations = useMemo(
    () => conversations.filter((conversation) => conversation.projectId === projectId),
    [conversations, projectId],
  );
  const inboxRuns = useMemo<InboxRun[]>(
    () =>
      automations
        .flatMap((automation) =>
          automation.runs.map((run) => ({
            automationId: automation.id,
            automationName: automation.name,
            run,
          })),
        )
        .sort(
          (left, right) =>
            new Date(right.run.startedAt).getTime() -
            new Date(left.run.startedAt).getTime(),
        ),
    [automations],
  );
  const unreadCount = inboxRuns.filter(({ run }) => run.unread).length;

  async function refresh(initialLoad = false) {
    if (initialLoad) {
      setInitialLoading(true);
      setInitialLoadError(null);
    }
    setError(null);
    try {
      const rows = await ipc.automation.list();
      setAutomations(rows);
      setInitialLoadError(null);
      onUnreadCountChange(
        rows.reduce(
          (total, automation) =>
            total + automation.runs.filter((run) => run.unread).length,
          0,
        ),
      );
    } catch (cause) {
      if (initialLoad) {
        setInitialLoadError(errorMessage(cause));
      } else {
        setError(errorMessage(cause));
      }
    } finally {
      if (initialLoad) setInitialLoading(false);
    }
  }

  useEffect(() => {
    if (open) {
      void refresh(true);
    } else {
      setInitialLoading(true);
      setInitialLoadError(null);
    }
  }, [open]);

  useEffect(() => {
    if (currentProjectId !== null && editingId === null) {
      setProjectId(currentProjectId);
    }
  }, [currentProjectId, editingId]);

  useEffect(() => {
    if (models.some((item) => item.id === model)) return;
    if (models[0]) setModel(models[0].id);
  }, [models, model]);

  useEffect(() => {
    if (conversationMode !== "existing") return;
    if (projectConversations.some((conversation) => conversation.id === conversationId)) {
      return;
    }
    const current = projectConversations.find(
      (conversation) => conversation.id === currentConversationId,
    );
    setConversationId(current?.id ?? projectConversations[0]?.id ?? null);
  }, [conversationMode, conversationId, currentConversationId, projectConversations]);

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

  function resetEditor() {
    setEditingId(null);
    setName("");
    setPrompt("");
    setRrule("");
    setWorkspaceStrategy("current");
    setConversationMode("new");
    setConversationId(currentConversationId);
    setRepositoryUrl("");
    setBaseRef("");
  }

  function createAutomation() {
    if (projectId === null || !name.trim() || !prompt.trim()) {
      setError(t("Name, prompt, and project are required."));
      return;
    }
    if (conversationMode === "existing" && !conversationId) {
      setError(t("Choose the conversation this schedule should continue."));
      return;
    }
    const draft: AutomationDraft = {
      id: editingId,
      name: name.trim(),
      prompt: prompt.trim(),
      projectId,
      workspaceId:
        target === "local" &&
        workspaceStrategy === "current" &&
        projectId === currentProjectId
          ? currentWorkspaceId
          : null,
      model,
      effort,
      target,
      repositoryUrl: target === "cloud" ? repositoryUrl.trim() : null,
      baseRef: baseRef.trim() || null,
      cadenceMinutes,
      rrule: rrule.trim() || null,
      timezone,
      workspaceStrategy: target === "cloud" ? "current" : workspaceStrategy,
      conversationMode: target === "cloud" ? "new" : conversationMode,
      conversationId:
        target === "local" && conversationMode === "existing"
          ? conversationId
          : null,
      enabled:
        automations.find((automation) => automation.id === editingId)?.enabled ??
        true,
    };
    void mutate(async () => {
      await ipc.automation.upsert({ draft });
      resetEditor();
    });
  }

  function editAutomation(automation: Automation) {
    setEditingId(automation.id);
    setName(automation.name);
    setPrompt(automation.prompt);
    setProjectId(automation.projectId);
    setModel(automation.model);
    setEffort(automation.effort);
    setTarget(automation.target);
    setCadenceMinutes(automation.cadenceMinutes);
    setRrule(automation.rrule ?? "");
    setTimezone(automation.timezone || "UTC");
    setWorkspaceStrategy(automation.workspaceStrategy ?? "current");
    setConversationMode(automation.conversationMode ?? "new");
    setConversationId(automation.conversationId ?? null);
    setRepositoryUrl(automation.repositoryUrl ?? "");
    setBaseRef(automation.baseRef ?? "");
    setView("scheduled");
    setError(null);
  }

  function openRun(automationId: string, run: AutomationRun) {
    if (!run.conversationId) return;
    void mutate(async () => {
      if (run.unread) {
        await ipc.automation.markRunRead({
          automationId,
          runId: run.id,
        });
      }
      onOpenConversation(run.conversationId!);
    });
  }

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-[76] flex justify-end bg-black/30 backdrop-blur-sm">
      <section
        aria-labelledby="automations-title"
        aria-modal="true"
        className="flex h-full w-full max-w-3xl flex-col border-0 bg-surface-base shadow-2xl shadow-black/30 sm:border-l sm:border-surface-border/35"
        role="dialog"
      >
        <div className="flex flex-wrap items-center gap-3 border-b border-surface-border/30 px-4 py-4 sm:px-5">
          <div className="min-w-0 flex-1 basis-[14rem]">
            <h2
              className="text-base font-semibold text-content-primary"
              id="automations-title"
            >
              {t("Automations")}
            </h2>
            <p className="mt-1 text-xs text-content-muted">
              {t(
                "Local schedules run while Halv is open; results collect in the inbox.",
              )}
            </p>
          </div>
          <div className="order-3 flex w-full rounded-lg border border-surface-border/30 bg-surface-panel/60 p-1 text-xs sm:order-none sm:w-auto">
            <button
              aria-pressed={view === "scheduled"}
              className={`rounded-md px-3 py-1.5 ${view === "scheduled" ? "bg-surface-raised text-content-primary" : "text-content-muted"}`}
              onClick={() => setView("scheduled")}
              type="button"
            >
              {t("Scheduled {count}", { count: automations.length })}
            </button>
            <button
              aria-pressed={view === "inbox"}
              className={`rounded-md px-3 py-1.5 ${view === "inbox" ? "bg-surface-raised text-content-primary" : "text-content-muted"}`}
              onClick={() => setView("inbox")}
              type="button"
            >
              {unreadCount
                ? t("Inbox {count}", { count: unreadCount })
                : t("Inbox")}
            </button>
          </div>
          <button
            aria-label={t("Close automations")}
            className="rounded-md px-2 py-1 text-xl text-content-muted hover:bg-surface-raised"
            onClick={onClose}
            type="button"
          >
            ×
          </button>
        </div>

        {error ? (
          <div
            className="border-b border-danger/25 bg-danger/10 px-5 py-2 text-xs text-danger"
            role="alert"
          >
            {error}
          </div>
        ) : null}

        {initialLoadError ? (
          <div
            className="flex items-center gap-3 border-b border-danger/25 bg-danger/10 px-5 py-2 text-xs text-danger"
            role="alert"
          >
            <div className="min-w-0 flex-1">
              <p className="font-medium">{t("Could not load automations.")}</p>
              <p className="mt-0.5 break-words text-[10px] opacity-80">
                {initialLoadError}
              </p>
            </div>
            <button
              className="shrink-0 rounded-md border border-danger/30 px-2 py-1 font-medium hover:bg-danger/10"
              onClick={() => void refresh(true)}
              type="button"
            >
              {t("Retry")}
            </button>
          </div>
        ) : null}

        <div
          aria-busy={initialLoading}
          className="min-h-0 flex-1 overflow-y-auto p-5"
        >
          {initialLoading ? (
            <div
              aria-live="polite"
              className="flex items-center gap-3 rounded-xl border border-surface-border/30 bg-surface-panel/55 px-4 py-4 text-sm text-content-muted"
              role="status"
            >
              <span
                aria-hidden
                className="h-3 w-3 animate-pulse rounded-full bg-accent"
              />
              <span>{t("Loading automations…")}</span>
              <span
                aria-hidden
                className="ml-auto h-2 w-24 animate-pulse rounded bg-surface-border/40"
              />
            </div>
          ) : initialLoadError ? null : view === "inbox" ? (
            <div>
              <div className="mb-3 flex items-center justify-between">
                <p className="text-xs text-content-muted">
                  {t(
                    "Completed and failed runs stay here until you review them.",
                  )}
                </p>
                {unreadCount ? (
                  <button
                    className="text-xs text-accent hover:underline"
                    disabled={busy}
                    onClick={() => void mutate(() => ipc.automation.markAllRead())}
                    type="button"
                  >
                    {t("Mark all read")}
                  </button>
                ) : null}
              </div>
              <div className="space-y-2">
                {inboxRuns.length === 0 ? (
                  <div className="rounded-xl border border-dashed border-surface-border/30 px-4 py-10 text-center text-sm text-content-muted">
                    {t("No automation runs yet.")}
                  </div>
                ) : (
                  inboxRuns.map(({ automationId, automationName, run }) => (
                    <article
                      className={`rounded-xl border p-4 ${run.unread ? "border-accent/45 bg-accent/5" : "border-surface-border/30"}`}
                      key={run.id}
                    >
                      <div className="flex items-start gap-3">
                        <span
                          aria-label={run.unread ? t("Unread") : t("Read")}
                          className={`mt-1.5 h-2 w-2 shrink-0 rounded-full ${run.unread ? "bg-accent" : "bg-surface-border"}`}
                        />
                        <div className="min-w-0 flex-1">
                          <div className="flex flex-wrap items-center gap-2">
                            <p className="text-sm font-medium text-content-primary">
                              {automationName}
                            </p>
                            <span className={`font-mono text-[10px] ${runTone(run.status)}`}>
                              {automationStatusLabel(run.status, t)}
                            </span>
                          </div>
                          <p className="mt-1 font-mono text-[10px] text-content-muted">
                            {run.workspaceId
                              ? t("{startedAt} · worktree {workspaceId}", {
                                  startedAt: formatWhen(run.startedAt, t),
                                  workspaceId: run.workspaceId.slice(0, 8),
                                })
                              : formatWhen(run.startedAt, t)}
                          </p>
                          {run.error ? <p className="mt-2 text-xs text-danger">{run.error}</p> : null}
                        </div>
                        {run.conversationId ? (
                          <button
                            className="rounded-lg border border-surface-border/30 px-3 py-1.5 text-xs text-content-primary hover:bg-surface-raised"
                            disabled={busy}
                            onClick={() => openRun(automationId, run)}
                            type="button"
                          >
                            {t("Open result")}
                          </button>
                        ) : run.unread ? (
                          <button
                            className="text-xs text-content-muted hover:text-content-primary"
                            disabled={busy}
                            onClick={() =>
                              void mutate(() =>
                                ipc.automation.markRunRead({
                                  automationId,
                                  runId: run.id,
                                }),
                              )
                            }
                            type="button"
                          >
                            {t("Mark read")}
                          </button>
                        ) : null}
                      </div>
                    </article>
                  ))
                )}
              </div>
            </div>
          ) : (
            <>
              <div className="rounded-xl border border-surface-border/30 bg-surface-panel/55 p-4">
                <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                  <label className="text-xs text-content-muted">
                    {t("Name")}
                    <input className={inputClass} onChange={(event) => setName(event.target.value)} value={name} />
                  </label>
                  <label className="text-xs text-content-muted">
                    {t("Project")}
                    <select
                      className={inputClass}
                      onChange={(event) => {
                        setProjectId(Number(event.target.value) || null);
                        setConversationMode("new");
                        setConversationId(null);
                      }}
                      value={projectId ?? ""}
                    >
                      <option value="">{t("Select project")}</option>
                      {projects.map((project) => (
                        <option key={project.id} value={project.id}>{project.name}</option>
                      ))}
                    </select>
                  </label>
                  <label className="text-xs text-content-muted">
                    {t("Model")}
                    <select className={inputClass} onChange={(event) => setModel(event.target.value)} value={model}>
                      {models.map((item) => <option key={item.id} value={item.id}>{item.label}</option>)}
                    </select>
                  </label>
                  <label className="text-xs text-content-muted">
                    {t("Effort")}
                    <select className={inputClass} onChange={(event) => setEffort(event.target.value)} value={effort}>
                      <option value="low">{t("low")}</option>
                      <option value="medium">{t("medium")}</option>
                      <option value="high">{t("high")}</option>
                      <option value="xhigh">{t("xhigh")}</option>
                    </select>
                  </label>
                  <label className="text-xs text-content-muted">
                    {t("Target")}
                    <select
                      className={inputClass}
                      onChange={(event) => {
                        const next = event.target.value as AutomationTarget;
                        setTarget(next);
                        if (next === "cloud") {
                          setWorkspaceStrategy("current");
                          setConversationMode("new");
                          setConversationId(null);
                        }
                      }}
                      value={target}
                    >
                      <option value="local">{t("Local background")}</option>
                      <option value="cloud">{t("Cloud handoff")}</option>
                    </select>
                  </label>
                  <label className="text-xs text-content-muted">
                    {t("Every (minutes)")}
                    <input
                      className={inputClass}
                      min={1}
                      onChange={(event) => setCadenceMinutes(Number(event.target.value) || 1)}
                      type="number"
                      value={cadenceMinutes}
                    />
                  </label>
                  <label className="text-xs text-content-muted sm:col-span-2">
                    {t("Advanced RRULE (optional)")}
                    <input
                      className={`${inputClass} font-mono text-xs`}
                      onChange={(event) => setRrule(event.target.value)}
                      placeholder="RRULE:FREQ=WEEKLY;BYDAY=MO,FR;BYHOUR=9;BYMINUTE=0"
                      value={rrule}
                    />
                    <span className="mt-1 block text-[10px] text-content-subtle">
                      {t(
                        "Supports minutely, hourly, daily, weekly, and monthly rules. RRULE times use the IANA zone below.",
                      )}
                    </span>
                  </label>
                  <label className="text-xs text-content-muted sm:col-span-2">
                    {t("Time zone")}
                    <input className={inputClass} onChange={(event) => setTimezone(event.target.value)} value={timezone} />
                  </label>
                </div>

                {target === "local" ? (
                  <div className="mt-3 grid grid-cols-1 gap-3 sm:grid-cols-2">
                    <label className="text-xs text-content-muted">
                      {t("Run location")}
                      <select
                        className={inputClass}
                        disabled={conversationMode === "existing"}
                        onChange={(event) => setWorkspaceStrategy(event.target.value as AutomationWorkspaceStrategy)}
                        value={workspaceStrategy}
                      >
                        <option value="current">{t("Selected project checkout")}</option>
                        <option value="new_worktree">{t("New isolated worktree per run")}</option>
                      </select>
                    </label>
                    <label className="text-xs text-content-muted">
                      {t("Conversation")}
                      <select
                        className={inputClass}
                        onChange={(event) => {
                          const mode = event.target.value as AutomationConversationMode;
                          setConversationMode(mode);
                          if (mode === "existing") setWorkspaceStrategy("current");
                        }}
                        value={conversationMode}
                      >
                        <option value="new">{t("New conversation per run")}</option>
                        <option disabled={projectConversations.length === 0} value="existing">{t("Continue an existing conversation")}</option>
                      </select>
                    </label>
                    {conversationMode === "existing" ? (
                      <label className="text-xs text-content-muted sm:col-span-2">
                        {t("Continue conversation")}
                        <select className={inputClass} onChange={(event) => setConversationId(event.target.value || null)} value={conversationId ?? ""}>
                          <option value="">{t("Select conversation")}</option>
                          {projectConversations.map((conversation) => (
                            <option key={conversation.id} value={conversation.id}>{conversation.title}</option>
                          ))}
                        </select>
                      </label>
                    ) : null}
                    {workspaceStrategy === "new_worktree" ? (
                      <label className="text-xs text-content-muted sm:col-span-2">
                        {t("Worktree base ref (optional)")}
                        <input className={inputClass} onChange={(event) => setBaseRef(event.target.value)} placeholder="HEAD" value={baseRef} />
                      </label>
                    ) : null}
                  </div>
                ) : (
                  <div className="mt-3 grid grid-cols-1 gap-3 sm:grid-cols-2">
                    <label className="text-xs text-content-muted">
                      {t("GitHub repository URL")}
                      <input className={inputClass} onChange={(event) => setRepositoryUrl(event.target.value)} placeholder="https://github.com/owner/repo" value={repositoryUrl} />
                    </label>
                    <label className="text-xs text-content-muted">
                      {t("Base ref (optional)")}
                      <input className={inputClass} onChange={(event) => setBaseRef(event.target.value)} placeholder="main" value={baseRef} />
                    </label>
                  </div>
                )}

                <label className="mt-3 block text-xs text-content-muted">
                  {t("Prompt")}
                  <textarea className={`${inputClass} min-h-28`} onChange={(event) => setPrompt(event.target.value)} value={prompt} />
                </label>
                <div className="mt-3 flex items-center gap-2">
                  <button className="rounded-lg bg-accent px-4 py-2 text-sm text-accent-contrast disabled:opacity-50" disabled={busy} onClick={createAutomation} type="button">
                    {editingId ? t("Save automation") : t("Create automation")}
                  </button>
                  {editingId ? (
                    <button className="rounded-lg border border-surface-border/30 px-4 py-2 text-sm text-content-secondary" disabled={busy} onClick={resetEditor} type="button">
                      {t("Cancel")}
                    </button>
                  ) : null}
                </div>
              </div>

              <div className="mt-5 space-y-3">
                {automations.length === 0 ? (
                  <div className="rounded-xl border border-dashed border-surface-border/30 px-4 py-8 text-center text-sm text-content-muted">
                    {t("No automations yet.")}
                  </div>
                ) : (
                  automations.map((automation) => (
                    <article className="rounded-xl border border-surface-border/30 p-4" key={automation.id}>
                      <div className="flex items-start gap-3">
                        <div className="min-w-0 flex-1">
                          <div className="flex flex-wrap items-center gap-2">
                            <p className="text-sm font-medium text-content-primary">{automation.name}</p>
                            {!automation.enabled ? <span className="rounded bg-surface-raised px-1.5 py-0.5 text-[9px] uppercase text-content-muted">{t("paused")}</span> : null}
                            {automation.runs.some((run) => run.unread) ? <span className="rounded bg-accent/15 px-1.5 py-0.5 text-[9px] uppercase text-accent">{t("unread")}</span> : null}
                          </div>
                          <p className="mt-1 line-clamp-2 text-xs text-content-muted">{automation.prompt}</p>
                          <p className="mt-2 font-mono text-[10px] text-accent">
                            {t("{target} · {schedule} · {timezone} · next {nextRun}", {
                              target: automationTargetLabel(automation.target, t),
                              schedule: scheduleLabel(automation, t),
                              timezone: automation.timezone,
                              nextRun: formatWhen(automation.nextRunAt, t),
                            })}
                          </p>
                          <p className="mt-1 font-mono text-[10px] text-content-muted">
                            {automationContextLabel(automation, t)}
                          </p>
                          {automation.lastStatus ? (
                            <p className={`mt-1 text-[10px] ${automation.lastStatus === "failed" ? "text-danger" : "text-content-muted"}`}>
                              {lastRunLabel(automation, t)}
                            </p>
                          ) : null}
                        </div>
                        <button className="text-xs text-accent hover:underline" disabled={busy} onClick={() => void mutate(() => ipc.automation.runNow({ automationId: automation.id }))} type="button">{t("Run now")}</button>
                        <button className="text-xs text-content-muted hover:text-content-primary" disabled={busy} onClick={() => editAutomation(automation)} type="button">{t("Edit")}</button>
                        <button className="text-xs text-content-muted hover:text-content-primary" disabled={busy} onClick={() => void mutate(() => ipc.automation.setEnabled({ automationId: automation.id, enabled: !automation.enabled }))} type="button">{automation.enabled ? t("Pause") : t("Enable")}</button>
                        <button className="text-xs text-content-muted hover:text-danger" disabled={busy} onClick={() => void mutate(() => ipc.automation.delete({ automationId: automation.id }))} type="button">{t("Delete")}</button>
                      </div>
                    </article>
                  ))
                )}
              </div>
            </>
          )}
        </div>
      </section>
    </div>
  );
}
