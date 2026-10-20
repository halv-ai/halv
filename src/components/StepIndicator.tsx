import { useT } from "../i18n/runtime";

type StepIndicatorProps = { step: 1 | 2 | 3 };

export function StepIndicator({ step }: StepIndicatorProps) {
  const t = useT();
  const labels = [t("Connect"), t("Your project"), t("First result")];
  return (
    <ol
      aria-label={t("Step {step} of 3", { step })}
      className="flex w-full items-center gap-2 text-[11px] sm:gap-3 sm:text-xs"
    >
      {labels.map((label, index) => (
        <li
          key={label}
          aria-current={index + 1 === step ? "step" : undefined}
          className={`flex items-center gap-2 ${index < 2 ? "flex-1" : ""} ${index + 1 <= step ? "text-accent" : "text-content-muted"}`}
        >
          <span
            aria-hidden="true"
            className={`flex h-6 w-6 shrink-0 items-center justify-center rounded-full border text-[10px] font-semibold ${index + 1 < step ? "border-accent/20 bg-accent/10" : index + 1 === step ? "border-accent/50 bg-accent/5" : "border-surface-border/30"}`}
          >
            {index + 1 < step ? "✓" : index + 1}
          </span>
          <span className="font-medium">{label}</span>
          {index < 2 ? (
            <span
              aria-hidden="true"
              className="ml-1 h-px min-w-2 flex-1 bg-surface-border/20"
            />
          ) : null}
        </li>
      ))}
    </ol>
  );
}
