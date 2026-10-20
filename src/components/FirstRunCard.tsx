import { useState } from "react";
import type { FirstRunVariant } from "../firstRun";
import { SavingsSparkline } from "./SavingsPanel";
import type { ConversationMode } from "../conversationMode";
import type { FirstTaskGoal } from "../onboarding";
import { useT, type Translate } from "../i18n/runtime";
import { StepIndicator } from "./StepIndicator";
import { AnonymousUsagePreference } from "./AnonymousUsagePreference";

type FirstRunCardProps = {
  variant: FirstRunVariant;
  goal: FirstTaskGoal;
  pending: boolean;
  onGoalChange: (goal: FirstTaskGoal) => void;
  onDismiss: () => void;
  onOpenPrivacySettings: () => void;
  onPickProject: () => void;
};

export function FirstRunCard({
  variant,
  goal,
  pending,
  onGoalChange,
  onDismiss,
  onOpenPrivacySettings,
  onPickProject,
}: FirstRunCardProps) {
  const t = useT();
  const connectedAgent = connectedAgentLabel(variant, t);
  const goals = [
    {
      id: "quick-win" as const,
      icon: "+",
      title: t("Find a quick win"),
      detail: t("Get one focused improvement worth making."),
    },
    {
      id: "tour" as const,
      icon: "↗",
      title: t("Understand this project"),
      detail: t("Find the entry points and your next step."),
    },
    {
      id: "tests" as const,
      icon: "✓",
      title: t("Find a test gap"),
      detail: t("See what to test next and why it matters."),
    },
  ];
  return (
    <section
      aria-labelledby="first-task-title"
      className="w-full max-w-[620px]"
    >
      <StepIndicator step={2} />
      <div className="mt-7 inline-flex items-center gap-2 text-xs font-medium text-accent">
        <span
          aria-hidden="true"
          className="flex h-5 w-5 items-center justify-center rounded-full bg-accent/15"
        >
          ✓
        </span>
        {connectedAgent
          ? t("Step 1 done · {agent} connected", { agent: connectedAgent })
          : t("Step 1 done · your account is connected")}
      </div>
      <h1
        id="first-task-title"
        className="mt-3 text-[30px] font-semibold leading-tight tracking-[-0.025em] text-content-primary"
      >
        {t("Make this useful for you.")}
      </h1>
      <p className="mt-3 text-sm leading-6 text-content-secondary">
        {t(
          "Choose a starting point. Halv will prepare a focused task for a project on your computer.",
        )}
      </p>
      <fieldset className="mt-6 space-y-2.5">
        <legend className="sr-only">{t("Choose your first task")}</legend>
        {goals.map((option) => (
          <label
            key={option.id}
            className={`flex cursor-pointer items-center gap-3 rounded-xl border px-4 py-3.5 transition-colors focus-within:ring-2 focus-within:ring-accent/40 ${goal === option.id ? "border-accent/55 bg-accent/5" : "border-surface-border/25 bg-surface-panel hover:bg-surface-raised"}`}
          >
            <input
              className="order-last h-4 w-4 shrink-0 accent-accent"
              name="first-task-goal"
              type="radio"
              value={option.id}
              checked={goal === option.id}
              disabled={pending}
              onChange={() => onGoalChange(option.id)}
            />
            <span
              aria-hidden="true"
              className={`flex h-9 w-9 shrink-0 items-center justify-center rounded-lg text-lg ${goal === option.id ? "bg-accent/15 text-accent" : "bg-surface-raised text-content-muted"}`}
            >
              {option.icon}
            </span>
            <span className="min-w-0 flex-1">
              <span className="block text-sm font-semibold text-content-primary">
                {option.title}
              </span>
              <span className="mt-1 block text-xs leading-5 text-content-muted">
                {option.detail}
              </span>
            </span>
          </label>
        ))}
      </fieldset>
      <div className="mt-6 flex flex-wrap items-center gap-x-5 gap-y-3">
        <button
          className="flex items-center justify-center gap-4 rounded-xl bg-accent px-5 py-3 text-sm font-semibold text-accent-contrast transition-colors hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40 disabled:cursor-wait disabled:opacity-60"
          disabled={pending}
          onClick={onPickProject}
          type="button"
        >
          {pending ? t("Opening project…") : t("Choose a project folder")}
          <span aria-hidden="true">→</span>
        </button>
        <button
          className="rounded py-2 text-xs text-content-secondary underline underline-offset-4 hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/40"
          disabled={pending}
          onClick={onDismiss}
          type="button"
        >
          {t("I'll start with my own question")}
        </button>
      </div>
      <p className="mt-3 text-xs leading-5 text-content-muted">
        {t("You review the first prompt before it runs.")}
      </p>
      <footer className="mt-6 space-y-2 border-t border-surface-border/20 pt-4">
        <AnonymousUsagePreference />
        <button
          className="rounded text-xs text-content-muted underline underline-offset-4 hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/40"
          onClick={onOpenPrivacySettings}
          type="button"
        >
          {t("Privacy settings")}
        </button>
      </footer>
    </section>
  );
}

export function GuidedTaskStatus({
  phase,
  payoff,
  error,
  terminalFollowUp,
  celebration,
  onDismiss,
  onContinue,
  onInspectSavings,
}: {
  phase: "waiting" | "success" | "failed";
  payoff: string;
  error?: string | null;
  terminalFollowUp?: string;
  /** The peak moment: the saving of today and its recent replies. */
  celebration?: { amount: string | null; recent: number[] } | null;
  onDismiss: () => void;
  onContinue: (nextTask?: string) => void | Promise<void>;
  onInspectSavings: () => void;
}) {
  const t = useT();
  const complete = phase === "success";
  const failed = phase === "failed";
  const [nextTask, setNextTask] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  async function continueTask() {
    if (complete && terminalFollowUp && nextTask === null) {
      setNextTask(terminalFollowUp);
      return;
    }
    setPending(true);
    setActionError(null);
    try { await onContinue(nextTask ?? undefined); }
    catch (error) { setActionError(error instanceof Error ? error.message : String(error)); }
    finally { setPending(false); }
  }
  return (
    <section
      aria-label={t("First task progress")}
      className={`mx-5 mt-3 flex flex-wrap items-center gap-3 rounded-xl border px-4 py-3 ${complete ? "border-accent/35 bg-accent/5" : failed ? "border-danger/30 bg-danger/5" : "border-surface-border/25 bg-surface-raised/55"}`}
    >
      <span
        aria-hidden="true"
        className={`flex h-8 w-8 shrink-0 items-center justify-center rounded-full text-sm font-semibold ${complete ? "bg-accent text-accent-contrast" : "bg-surface-raised text-content-secondary"}`}
      >
        {complete ? "✓" : failed ? "!" : "↗"}
      </span>
      <div className="min-w-0 flex-1" role="status">
        <p className="text-sm font-semibold text-content-primary">
          {complete
            ? t("Your first result. Your next move.")
            : failed
              ? t("Let's get this task working.")
              : t("Finding your first useful result…")}
        </p>
        <p className="mt-1 text-xs leading-5 text-content-secondary">
          {complete
            ? t("Read the findings below, then choose your next step.")
            : failed
              ? error ||
                t(
                  "The task stopped before a result arrived. You can try again.",
                )
              : t(
                  "Your agent is working on the task below. You can follow its progress or stop it at any time.",
                )}
        </p>
      </div>
      <div className="flex flex-wrap items-center gap-3">
        {complete || failed ? (
          <button
            className="rounded-lg bg-accent px-3 py-2 text-xs font-semibold text-accent-contrast hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40"
            disabled={pending || (nextTask !== null && !nextTask.trim())}
            onClick={() => void continueTask()}
            type="button"
          >
            {complete
              ? terminalFollowUp
                ? nextTask === null ? t("Prepare next task") : t("Prepare in terminal")
                : t("Plan my next step")
              : t("Try again")}
          </button>
        ) : null}
        {complete ? (
          <button
            className="rounded py-1 text-xs font-medium text-content-secondary underline underline-offset-4 focus:outline-none focus:ring-2 focus:ring-accent/40"
            onClick={onInspectSavings}
            type="button"
          >
            {t("View savings")}
          </button>
        ) : null}
        <button
          aria-label={t("Hide guide")}
          className="rounded px-2 py-1 text-content-muted hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/40"
          onClick={onDismiss}
          type="button"
        >
          ×
        </button>
      </div>
      {complete ? (
        <div className="flex w-full flex-wrap items-center gap-x-4 gap-y-2 rounded-lg border border-accent/20 bg-accent/5 px-3 py-2 text-xs text-content-secondary">
          {celebration?.amount ? (
            <span className="text-base font-semibold text-accent">
              {t("{amount} saved today", { amount: celebration.amount })}
            </span>
          ) : null}
          {celebration && celebration.recent.length > 0 ? (
            <SavingsSparkline values={celebration.recent} />
          ) : null}
          <span className="min-w-0 flex-1">{guidedTaskPayoffLabel(payoff, t)}</span>
        </div>
      ) : null}
      {nextTask !== null ? (
        <div className="w-full text-xs text-content-secondary">
          <label>
            {t("Next task")}
            <textarea autoFocus className="mt-2 min-h-[90px] w-full rounded-lg border border-surface-border/30 bg-surface-panel p-3 text-sm text-content-primary focus:ring-2 focus:ring-accent/40"
              value={nextTask} disabled={pending} onChange={event => setNextTask(event.target.value)} />
          </label>
          <p className="mt-2">{t("Review it in your terminal. Press Enter when you're ready to send.")}</p>
        </div>
      ) : null}
      {actionError ? <p className="w-full text-xs text-danger" role="alert">{actionError}</p> : null}

    </section>
  );
}

export function PreparedFirstTask({
  draft,
  error,
  mode,
  projectPath,
  engineReady,
  pending,
  onChangeFolder,
  onDismiss,
  onSend,
  onModeChange,
  onDraftChange,
  onRetryEngine,
}: {
  draft: string;
  error: string | null;
  mode: ConversationMode;
  projectPath: string;
  engineReady: boolean;
  pending: boolean;
  onChangeFolder: () => void;
  onDismiss: () => void;
  onSend: () => void;
  onModeChange: (mode: ConversationMode) => void;
  onDraftChange: (draft: string) => void;
  onRetryEngine: () => void;
}) {
  const t = useT();
  const projectName =
    projectPath.split(/[\\/]/).filter(Boolean).at(-1) || projectPath;
  return (
    <section className="mx-auto flex w-full max-w-[620px] flex-col gap-5">
      <StepIndicator step={2} />
      <header className="mt-2">
        <p className="text-xs font-semibold text-accent">
          {t("Ready when you are")}
        </p>
        <h1 className="mt-2 text-[30px] font-semibold leading-tight tracking-[-0.025em] text-content-primary">
          {t("Let's put {project} to work.", { project: projectName })}
        </h1>
        <p className="mt-3 text-sm leading-6 text-content-secondary">
          {t("Here's your first task. Make it your own, or send it as it is.")}
        </p>
      </header>
      <div className="flex items-center gap-3 rounded-xl border border-surface-border/25 bg-surface-panel px-4 py-3">
        <svg
          aria-hidden="true"
          className="h-5 w-5 shrink-0 text-accent"
          fill="none"
          viewBox="0 0 24 24"
        >
          <path
            d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"
            stroke="currentColor"
            strokeWidth="1.8"
          />
        </svg>
        <span
          className="min-w-0 flex-1 truncate font-mono text-xs text-content-secondary"
          title={projectPath}
        >
          {projectPath}
        </span>
        <button
          className="shrink-0 rounded text-xs text-accent underline underline-offset-4 focus:outline-none focus:ring-2 focus:ring-accent/40 disabled:opacity-50"
          disabled={pending}
          onClick={onChangeFolder}
          type="button"
        >
          {t("Change folder")}
        </button>
      </div>
      <div>
        <label
          htmlFor="first-task-prompt"
          className="mb-2 block text-xs font-semibold text-content-secondary"
        >
          {t("Your first task")}
        </label>
        <textarea
          id="first-task-prompt"
          className="min-h-[160px] w-full resize-y rounded-xl border border-surface-border/30 bg-surface-panel px-4 py-3 text-sm leading-6 text-content-primary outline-none transition-colors focus:border-accent/60 focus:ring-2 focus:ring-accent/15"
          value={draft}
          onChange={(event) => onDraftChange(event.target.value)}
          disabled={pending}
        />
        <p className="mt-2 text-xs leading-5 text-content-muted">
          {t("The suggested task asks for analysis, not file changes.")}
        </p>
      </div>
      <details className="rounded-lg border border-surface-border/20 px-3 py-2 text-xs text-content-secondary">
        <summary className="cursor-pointer rounded focus:outline-none focus:ring-2 focus:ring-accent/40">
          {t("Run in {mode} · change", {
            mode: mode === "chat" ? t("Chat mode") : t("Pro mode"),
          })}
        </summary>
        <div className="mt-3 flex gap-2 pb-1">
          {(["chat", "agent"] as const).map((option) => (
            <button
              key={option}
              aria-pressed={mode === option}
              className={`flex-1 rounded-lg border px-3 py-2 text-left disabled:opacity-50 ${mode === option ? "border-accent/50 bg-accent/5" : "border-surface-border/30"}`}
              disabled={pending}
              onClick={() => onModeChange(option)}
              type="button"
            >
              <span className="block font-semibold">
                {option === "chat" ? t("Chat mode") : t("Pro mode")}
              </span>
              <span className="mt-1 block text-content-muted">
                {option === "chat"
                  ? t("Replies in this conversation.")
                  : t("Runs your agent in a terminal.")}
              </span>
            </button>
          ))}
        </div>
      </details>
      {error ? (
        <p
          className="rounded-lg bg-danger/5 px-3 py-2 text-sm text-danger"
          role="alert"
        >
          {error}
        </p>
      ) : null}
      {!engineReady ? (
        <div
          className="flex flex-wrap items-center gap-3 text-xs text-content-secondary"
          role="status"
        >
          <span>
            {t("The local engine needs to be ready before we start.")}
          </span>
          <button
            className="rounded text-accent underline underline-offset-4 disabled:opacity-50"
            disabled={pending}
            onClick={onRetryEngine}
            type="button"
          >
            {t("Retry automatic setup")}
          </button>
        </div>
      ) : null}
      <div className="flex flex-wrap items-center gap-x-5 gap-y-3">
        <button
          className="flex items-center gap-4 rounded-xl bg-accent px-5 py-3 text-sm font-semibold text-accent-contrast hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40 disabled:cursor-not-allowed disabled:opacity-50"
          disabled={!draft.trim() || !engineReady || pending}
          onClick={onSend}
          type="button"
        >
          {t("Run my first task")}
          <span aria-hidden="true">→</span>
        </button>
        <button
          className="rounded py-2 text-xs text-content-secondary underline underline-offset-4 focus:outline-none focus:ring-2 focus:ring-accent/40"
          disabled={pending}
          onClick={onDismiss}
          type="button"
        >
          {t("Skip for now")}
        </button>
      </div>
    </section>
  );
}

function guidedTaskPayoffLabel(payoff: string, t: Translate) {
  if (
    payoff ===
    "This reply had little context to trim. Savings grow with longer tasks."
  ) {
    return t(
      "This reply had little context to trim. Savings grow with longer tasks.",
    );
  }
  const reducedContext =
    /^Halv reduced the input context for this reply by (\d+)%\.$/.exec(payoff);
  if (reducedContext) {
    return t("Halv reduced the input context for this reply by {percent}%.", {
      percent: reducedContext[1],
    });
  }
  const savedDollars = /^Halv has saved (.+) today at API prices\.$/.exec(payoff);
  if (savedDollars) {
    return t("Halv has saved {amount} today at API prices.", {
      amount: savedDollars[1],
    });
  }
  const savedToday = /^Halv has saved (.+) tokens today\.$/.exec(payoff);
  if (savedToday) {
    return t("Halv has saved {tokens} tokens today.", {
      tokens: savedToday[1],
    });
  }
  if (
    payoff ===
    "Halv is now tracking the context saved by each successful reply."
  ) {
    return t(
      "Halv is now tracking the context saved by each successful reply.",
    );
  }
  return payoff;
}

function connectedAgentLabel(variant: FirstRunVariant, t: Translate) {
  switch (variant) {
    case "codex":
      return t("Codex");
    case "claude":
      return t("Claude");
    case "kimi":
      return t("Kimi");
    case "zai":
      return t("GLM");
    case "grok":
      return t("Grok");
    case "fresh":
      return null;
  }
}
