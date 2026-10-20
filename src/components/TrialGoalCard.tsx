import type { TrialGoalProgress } from "../activation";
import { useT } from "../i18n/runtime";

type TrialGoalCardProps = {
  progress: TrialGoalProgress;
  onOpenPlans: () => void;
};

// The trial has an active goal (ADR 0018): a task on five of the seven
// days. Each completed day raises the value of the habit before billing.
export function TrialGoalCard({ progress, onOpenPlans }: TrialGoalCardProps) {
  const t = useT();
  const ratio = Math.min(1, progress.activeDays / progress.targetDays);
  const headline = progress.reached
    ? t("Goal reached: you ran tasks on {target} days of your trial.", {
        target: progress.targetDays,
      })
    : t("Trial goal: run a task on {target} of {total} days.", {
        target: progress.targetDays,
        total: progress.totalDays,
      });
  const detail = progress.ended
    ? t("Your trial has ended.")
    : progress.daysLeft <= 2
      ? t("Day {day} of {total} · ends in {days} days", {
          day: progress.day,
          total: progress.totalDays,
          days: progress.daysLeft,
        })
      : t("Day {day} of {total} · {active} of {target} days done", {
          day: progress.day,
          total: progress.totalDays,
          active: progress.activeDays,
          target: progress.targetDays,
        });
  return (
    <section
      aria-label={t("Trial goal")}
      className="flex flex-wrap items-center gap-x-4 gap-y-2 border-t border-surface-border/20 bg-surface-panel px-4 py-2 text-xs text-content-secondary"
    >
      <div className="min-w-0 flex-1">
        <p className="font-medium text-content-primary">{headline}</p>
        <p className="mt-0.5 text-content-muted">{detail}</p>
      </div>
      <div
        aria-label={t("{active} of {target} days", {
          active: progress.activeDays,
          target: progress.targetDays,
        })}
        aria-valuemax={progress.targetDays}
        aria-valuemin={0}
        aria-valuenow={progress.activeDays}
        className="h-1.5 w-28 overflow-hidden rounded-full bg-surface-sunken"
        role="progressbar"
      >
        <div
          className="h-full rounded-full bg-accent transition-[width]"
          style={{ width: `${Math.round(ratio * 100)}%` }}
        />
      </div>
      <button
        className="rounded text-xs text-content-secondary underline underline-offset-4 hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/40"
        onClick={onOpenPlans}
        type="button"
      >
        {t("Plans")}
      </button>
    </section>
  );
}
