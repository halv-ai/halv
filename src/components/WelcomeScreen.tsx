import { useT } from "../i18n/runtime";
import { AnonymousUsagePreference } from "./AnonymousUsagePreference";

type WelcomeScreenProps = {
  onGetStarted: () => void;
  onSkip: () => void;
};

export function WelcomeScreen({ onGetStarted, onSkip }: WelcomeScreenProps) {
  const t = useT();
  return (
    <main className="flex min-h-screen w-full items-center justify-center overflow-y-auto bg-surface-base px-6 py-10 text-content-primary">
      <section aria-labelledby="welcome-title" className="w-full max-w-[920px]">
        <div className="mb-10 flex items-center gap-3">
          <span
            aria-hidden="true"
            className="flex h-10 w-10 items-center justify-center rounded-xl bg-accent text-xl font-bold text-accent-contrast"
          >
            h
          </span>
          <span className="text-xl font-semibold tracking-tight">
            {t("Halv")}
          </span>
        </div>
        <div className="grid items-center gap-10 md:grid-cols-[1.1fr_1fr] md:gap-14">
          <div>
            <p className="mb-4 text-xs font-semibold uppercase tracking-[0.16em] text-accent">
              {t("Your agents. Your project.")}
            </p>
            <h1
              id="welcome-title"
              className="text-[38px] font-semibold leading-[1.1] tracking-[-0.04em] sm:text-[46px]"
            >
              {t("Less setup.")}
              <br />
              <span className="text-accent">{t("More getting it done.")}</span>
            </h1>
            <p className="mt-5 max-w-sm text-[15px] leading-6 text-content-secondary">
              {t(
                "Bring your AI account. Pick a project. Get a useful answer, with less wasted context along the way.",
              )}
            </p>
            <button
              className="mt-7 flex w-full items-center justify-center gap-3 rounded-xl bg-accent px-5 py-3.5 text-sm font-semibold text-accent-contrast transition-colors hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40 focus:ring-offset-2 focus:ring-offset-surface-base sm:w-auto"
              onClick={onGetStarted}
              type="button"
            >
              {t("Let's try it on my project")}
              <span aria-hidden="true">→</span>
            </button>
            <p className="mt-3 text-xs text-content-muted">
              {t("Try your first task before creating a Halv account.")}
            </p>
            <ul className="mt-5 max-w-sm space-y-1.5 text-xs leading-5 text-content-muted">
              <li className="flex gap-2">
                <span aria-hidden="true" className="text-accent">·</span>
                {t("You need a ChatGPT or Claude plan, or an API key. Halv does not sell model access.")}
              </li>
              <li className="flex gap-2">
                <span aria-hidden="true" className="text-accent">·</span>
                {t("A Halv account comes after your first result. The 7-day trial asks for a card.")}
              </li>
            </ul>
            <button
              className="mt-5 rounded text-xs text-content-secondary underline underline-offset-4 hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/40"
              onClick={onSkip}
              type="button"
            >
              {t("I know my way around — skip the guide")}
            </button>
          </div>
          <div className="rounded-2xl border border-surface-border/30 bg-surface-panel p-6 shadow-xl shadow-black/10 sm:p-7">
            <div className="mb-6 flex items-center justify-between gap-3">
              <p className="text-xs font-semibold uppercase tracking-[0.12em] text-content-muted">
                {t("Your first useful result")}
              </p>
              <span aria-hidden="true" className="text-xl text-accent">
                ↗
              </span>
            </div>
            <div className="rounded-xl border border-accent/20 bg-accent/5 p-4">
              <p className="text-[15px] font-medium leading-6 text-content-primary">
                {t("“Where should I start with this project?”")}
              </p>
            </div>
            <ol className="mt-6 space-y-5">
              {[
                [
                  t("A map of your code"),
                  t("The entry points and files that matter."),
                ],
                [
                  t("One worthwhile next step"),
                  t("A focused suggestion you can act on."),
                ],
                [
                  t("Savings you can inspect"),
                  t("See measured context savings when available."),
                ],
              ].map(([title, detail], index) => (
                <li key={title} className="flex gap-3">
                  <span
                    aria-hidden="true"
                    className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-accent/10 font-mono text-[11px] text-accent"
                  >
                    0{index + 1}
                  </span>
                  <div>
                    <p className="text-sm font-semibold">{title}</p>
                    <p className="mt-1 text-xs leading-5 text-content-muted">
                      {detail}
                    </p>
                  </div>
                </li>
              ))}
            </ol>
            <p className="mt-6 border-t border-surface-border/20 pt-4 text-xs leading-5 text-content-muted">
              {t("You review the first prompt before it runs.")}
            </p>
          </div>
        </div>
        <div className="mt-10 max-w-[580px] border-t border-surface-border/20 pt-5">
          <AnonymousUsagePreference />
        </div>
      </section>
    </main>
  );
}
