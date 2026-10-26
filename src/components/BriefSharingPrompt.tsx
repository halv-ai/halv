import {
  useEffect,
  useRef,
  useSyncExternalStore,
  type KeyboardEvent,
} from "react";
import { BRIEF_SHARING_LEARN_MORE_URL } from "../briefSharing.ts";
import { briefSharingGate } from "../briefSharingGate.ts";
import { openExternalLink } from "../externalLinks.ts";
import { useT, type Translate } from "../i18n/runtime";
import { AppDialogFrame } from "./AppDialogHost";

// ADR 0023: the task brief question. The prompt holds a launch that the user
// started. The card asks after a launch without a user action.

const secondaryButtonClass =
  "rounded-md border border-surface-border/35 px-3 py-2 text-sm text-content-secondary transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-60";
const primaryButtonClass =
  "rounded-md bg-accent px-3 py-2 text-sm font-medium text-accent-contrast transition-opacity hover:opacity-90 focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-60";

/** The facts of the question. Settings shows the same list. */
export function BriefSharingFacts({ t }: { t: Translate }) {
  return (
    <ul className="list-disc space-y-1.5 pl-5 text-sm leading-5 text-content-secondary">
      <li>
        {t(
          "To route a task, your agent writes a task brief. A brief can contain code, file paths, and secrets from the task.",
        )}
      </li>
      <li>
        {t(
          "Halv sends the brief to halv.ai. halv.ai sends it to OpenRouter, where the TypeSafe JEV model picks a model tier.",
        )}
      </li>
      <li>{t("Halv does not store or log the brief.")}</li>
    </ul>
  );
}

export function BriefSharingLearnMore({ t }: { t: Translate }) {
  return (
    <a
      className="text-sm text-accent underline-offset-2 hover:underline focus:outline-none focus:ring-2 focus:ring-accent/30"
      href={BRIEF_SHARING_LEARN_MORE_URL}
      onClick={(event) => {
        event.preventDefault();
        void openExternalLink(BRIEF_SHARING_LEARN_MORE_URL).catch(() => undefined);
      }}
    >
      {t("Learn more")}
    </a>
  );
}

/** Shows the prompt or the card when the gate asks. */
export function BriefSharingPrompt() {
  const view = useSyncExternalStore(
    briefSharingGate.subscribe,
    briefSharingGate.getSnapshot,
    briefSharingGate.getSnapshot,
  );
  // Only a mounted view lets a launch wait for the prompt.
  useEffect(() => briefSharingGate.attachHost(), []);
  if (view.surface === "prompt") {
    return <BriefSharingDialog error={view.error} pending={view.pending} />;
  }
  if (view.surface === "card") {
    return <BriefSharingCard error={view.error} pending={view.pending} />;
  }
  return null;
}

function BriefSharingAnswers({
  pending,
  autoFocusAllow,
}: {
  pending: boolean;
  autoFocusAllow: boolean;
}) {
  const t = useT();
  return (
    <>
      <button
        className={secondaryButtonClass}
        data-testid="brief-sharing-decline"
        disabled={pending}
        onClick={() => void briefSharingGate.answer("declined")}
        type="button"
      >
        {t("Don't send")}
      </button>
      <button
        autoFocus={autoFocusAllow}
        className={primaryButtonClass}
        data-testid="brief-sharing-allow"
        disabled={pending}
        onClick={() => void briefSharingGate.answer("allowed")}
        type="button"
      >
        {pending ? t("Saving...") : t("Allow")}
      </button>
    </>
  );
}

function BriefSharingDialog({
  pending,
  error,
}: {
  pending: boolean;
  error: string | null;
}) {
  const t = useT();
  const panelRef = useRef<HTMLDivElement>(null);

  // Escape closes the question without an answer. The launch then starts
  // without routing.
  function handleKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      briefSharingGate.dismiss();
      return;
    }
    if (event.key !== "Tab") return;
    const focusable = Array.from(
      panelRef.current?.querySelectorAll<HTMLElement>(
        "a[href], button:not([disabled])",
      ) ?? [],
    );
    if (focusable.length === 0) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  return (
    <AppDialogFrame
      labelledBy="brief-sharing-title"
      onDismiss={() => undefined}
      restoreFocus
    >
      <div
        className="w-full max-w-md overflow-hidden rounded-xl border border-surface-border/35 bg-surface-glass/95 shadow-2xl shadow-black/30"
        data-testid="brief-sharing-prompt"
        onKeyDown={handleKeyDown}
        ref={panelRef}
      >
        <div className="space-y-3 px-5 py-4">
          <h2
            className="text-sm font-semibold text-content-primary"
            id="brief-sharing-title"
          >
            {t("Send task briefs to Halv?")}
          </h2>
          <BriefSharingFacts t={t} />
          <p className="text-xs leading-5 text-content-muted">
            {t(
              "Without your yes, Halv sends nothing and the agent does the task itself. You can change this in Settings, JEV.",
            )}
          </p>
          <BriefSharingLearnMore t={t} />
          {error ? (
            <p className="text-xs text-danger" role="alert">
              {t("Could not save your answer: {error}", { error })}
            </p>
          ) : null}
        </div>
        <div className="flex justify-end gap-2 border-t border-surface-border/25 bg-surface-raised/30 px-5 py-3">
          <BriefSharingAnswers autoFocusAllow pending={pending} />
        </div>
      </div>
    </AppDialogFrame>
  );
}

function BriefSharingCard({
  pending,
  error,
}: {
  pending: boolean;
  error: string | null;
}) {
  const t = useT();
  return (
    <section
      aria-labelledby="brief-sharing-card-title"
      className="fixed bottom-12 right-4 z-[70] w-[min(26rem,calc(100vw-2rem))] space-y-3 rounded-xl border border-surface-border/35 bg-surface-glass/95 p-4 shadow-2xl shadow-black/30"
      data-testid="brief-sharing-card"
      role="region"
    >
      <div className="flex items-start gap-3">
        <h2
          className="min-w-0 flex-1 text-sm font-semibold text-content-primary"
          id="brief-sharing-card-title"
        >
          {t("Send task briefs to Halv?")}
        </h2>
        <button
          aria-label={t("Close")}
          className="rounded-md px-1.5 text-lg leading-none text-content-muted hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
          disabled={pending}
          onClick={() => briefSharingGate.dismiss()}
          type="button"
        >
          ×
        </button>
      </div>
      <p className="text-sm leading-5 text-content-secondary">
        {t(
          "Halv started an agent session without task routing. Task routing needs your yes before a brief leaves this computer.",
        )}
      </p>
      <BriefSharingFacts t={t} />
      <BriefSharingLearnMore t={t} />
      {error ? (
        <p className="text-xs text-danger" role="alert">
          {t("Could not save your answer: {error}", { error })}
        </p>
      ) : null}
      <div className="flex justify-end gap-2">
        <BriefSharingAnswers autoFocusAllow={false} pending={pending} />
      </div>
    </section>
  );
}
