import {
  useEffect,
  useId,
  useSyncExternalStore,
  type ReactNode,
} from "react";
import type { AnalyticsEvent } from "../analytics.ts";
import { ipc } from "../ipc.ts";
import { useT, type Translate } from "../i18n/runtime";
import type { ConsentAnswer } from "../onboarding.ts";

// ADR 0021: Halv sends no usage event until the user answers "on".
// Three surfaces share one saved answer:
// - `ConsentCard variant="ask"` asks on the welcome screen and the first-run card.
// - `ConsentAskBanner` asks an install that upgraded from the old default.
// - `ConsentCard variant="settings"` (also `AnonymousUsagePreference`) changes the answer.

type PreferenceState = {
  /** The saved answer. `null` while the preference loads. */
  consent: ConsentAnswer | null;
  /** `true` after "on", `false` after "off" or "unset", `null` while loading. */
  enabled: boolean | null;
  pending: boolean;
  error: string | null;
};

function preferenceState(
  consent: ConsentAnswer | null,
  pending: boolean,
  error: string | null,
): PreferenceState {
  return {
    consent,
    enabled: consent === null ? null : consent === "on",
    pending,
    error,
  };
}

function consentAnswer(value: unknown): ConsentAnswer {
  return value === "on" || value === "off" ? value : "unset";
}

// Every surface uses the same saved native preference and live UI state.
let state: PreferenceState = preferenceState(null, false, null);
const listeners = new Set<() => void>();
const getSnapshot = () => state;
const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
};

function publish(next: PreferenceState) {
  state = next;
  listeners.forEach((listener) => listener());
}

// Without an argument, the call loads the saved answer. With an argument, it saves one.
async function updatePreference(answer?: "on" | "off") {
  if (state.pending) return;
  publish({ ...state, pending: true, error: null });
  try {
    const preferences =
      answer === undefined
        ? await ipc.preferences.get()
        : await ipc.preferences.setAnalyticsConsent({ consent: answer });
    publish(
      preferenceState(consentAnswer(preferences.analyticsConsent), false, null),
    );
  } catch (error) {
    publish({
      ...state,
      pending: false,
      error: error instanceof Error ? error.message : String(error),
    });
  }
}

export function useAnonymousUsagePreference() {
  const preference = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
  useEffect(() => {
    if (state.consent === null) void updatePreference();
  }, []);
  return preference;
}

type ConsentGroup = {
  title: string;
  events: { id: AnalyticsEvent; label: string }[];
};

// The 13 journey events in plain words. Each group lists the event names
// that leave the device, so the user can check them against the network.
function consentEventGroups(t: Translate): ConsentGroup[] {
  return [
    {
      title: t("Setup"),
      events: [
        { id: "first_open", label: t("Halv opened for the first time") },
        { id: "account_connected", label: t("You connected an AI account") },
        { id: "project_selected", label: t("You picked a project") },
        {
          id: "setup_failed",
          label: t("A setup step failed (the category only)"),
        },
      ],
    },
    {
      title: t("First task"),
      events: [
        { id: "first_task_started", label: t("Your first task started") },
        { id: "first_task_completed", label: t("Your first task finished") },
        {
          id: "first_task_failed",
          label: t("Your first task failed (the category only)"),
        },
        { id: "context_saved", label: t("Halv measured its first saving") },
      ],
    },
    {
      title: t("Halv Cloud"),
      events: [
        { id: "trial_prompted", label: t("Halv showed the trial offer") },
        { id: "checkout_opened", label: t("You opened checkout") },
        { id: "trial_confirmed", label: t("Your trial started") },
        { id: "payment_confirmed", label: t("Your payment went through") },
      ],
    },
    {
      title: t("Activity"),
      events: [{ id: "active_day", label: t("You used Halv on a new day") }],
    },
  ];
}

/**
 * The "What is sent" disclosure: the 13 journey events, the facts that
 * travel with them, and what Halv never sends. `children` render at the
 * end, inside the disclosure (Settings puts the install ID there).
 */
export function ConsentDetails({
  showChangeNote = false,
  children,
}: {
  showChangeNote?: boolean;
  children?: ReactNode;
}) {
  const t = useT();
  const sentWithEvents = [
    t("A random install ID"),
    t("The app version"),
    t("Your operating system"),
    t("The time of the event"),
    t("The download ID, if you came through a link on the Halv site"),
  ];
  const neverSent = [
    t("Your prompts"),
    t("Your code"),
    t("File names"),
    t("Terminal output"),
    t("Account emails"),
    t("API keys"),
  ];
  return (
    <details className="consent-details text-xs text-content-muted">
      <summary className="w-fit cursor-pointer rounded font-medium text-content-secondary hover:text-content-primary focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40">
        {t("What is sent")}
      </summary>
      <div className="mt-3 space-y-4 leading-5">
        <div className="grid gap-x-8 gap-y-4 sm:grid-cols-2">
          {consentEventGroups(t).map((group) => (
            <div key={group.title}>
              <p className="font-semibold text-content-secondary">
                {group.title}
              </p>
              <ul className="mt-1.5 space-y-1.5">
                {group.events.map((event) => (
                  <li key={event.id}>
                    <code className="block font-mono text-[11px] text-content-secondary">
                      {event.id}
                    </code>
                    <span>{event.label}</span>
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </div>
        <div className="grid gap-x-8 gap-y-4 sm:grid-cols-2">
          <div>
            <p className="font-semibold text-content-secondary">
              {t("Sent with every event")}
            </p>
            <ul className="mt-1.5 list-disc space-y-1 pl-4">
              {sentWithEvents.map((item) => (
                <li key={item}>{item}</li>
              ))}
            </ul>
          </div>
          <div>
            <p className="font-semibold text-content-secondary">
              {t("Never sent")}
            </p>
            <ul className="mt-1.5 list-disc space-y-1 pl-4">
              {neverSent.map((item) => (
                <li key={item}>{item}</li>
              ))}
            </ul>
          </div>
        </div>
        <p>{t("Events expire after 90 days.")}</p>
        {showChangeNote ? (
          <p>{t("You can change this any time in Settings.")}</p>
        ) : null}
        {children}
      </div>
    </details>
  );
}

function ConsentError({
  preference,
  onRetry,
}: {
  preference: PreferenceState;
  onRetry: () => void;
}) {
  const t = useT();
  if (!preference.error) return null;
  return (
    <div className="space-y-1 text-xs">
      <p className="text-danger" role="alert">
        {preference.consent === null
          ? t("Could not load the anonymous usage preference: {error}", {
              error: preference.error,
            })
          : t("Could not save the anonymous usage preference: {error}", {
              error: preference.error,
            })}
      </p>
      {preference.consent === null ? (
        <button
          className="text-accent underline underline-offset-2"
          disabled={preference.pending}
          onClick={onRetry}
          type="button"
        >
          {t("Retry")}
        </button>
      ) : null}
    </div>
  );
}

function ConsentChoices({
  preference,
  onAnswer,
}: {
  preference: PreferenceState;
  onAnswer: (answer: "on" | "off") => void;
}) {
  const t = useT();
  const disabled = preference.consent === null || preference.pending;
  // Both choices look the same on purpose. The card must not push a yes.
  return (
    <div className="flex flex-wrap items-center gap-2">
      <button
        className="consent-choice"
        disabled={disabled}
        onClick={() => onAnswer("on")}
        type="button"
      >
        {t("Share usage events")}
      </button>
      <button
        className="consent-choice"
        disabled={disabled}
        onClick={() => onAnswer("off")}
        type="button"
      >
        {t("No thanks")}
      </button>
    </div>
  );
}

export type ConsentCardVariant = "ask" | "settings";

export type ConsentCardViewProps = {
  variant: ConsentCardVariant | "banner";
  preference: PreferenceState;
  /**
   * `false` hides the intro sentence and the disclosure of the settings
   * variant. SettingsPanel passes `false` and renders `ConsentDetails` itself.
   */
  description?: boolean;
  onAnswer: (answer: "on" | "off") => void;
  onRetry: () => void;
};

/**
 * The consent surfaces without the saved state. The connected components
 * below feed it; tests and previews pass the state in.
 */
export function ConsentCardView({
  variant,
  preference,
  description = true,
  onAnswer,
  onRetry,
}: ConsentCardViewProps) {
  const t = useT();
  const titleId = useId();
  const descriptionId = useId();

  if (variant === "banner") {
    if (preference.consent !== "unset") return null;
    return (
      <aside
        aria-label={t("Usage events")}
        className="consent-banner space-y-2 border-b border-surface-border/25 bg-surface-panel px-5 py-3 text-xs text-content-secondary"
      >
        <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
          <p className="text-[13px] font-medium text-content-primary">
            {t("Halv now asks before it sends usage events.")}
          </p>
          <ConsentChoices preference={preference} onAnswer={onAnswer} />
        </div>
        <ConsentDetails showChangeNote />
        <ConsentError preference={preference} onRetry={onRetry} />
      </aside>
    );
  }

  if (variant === "ask") {
    if (preference.consent === "on" || preference.consent === "off") {
      return (
        <p
          className="text-xs leading-5 text-content-muted"
          role="status"
        >
          {preference.consent === "on"
            ? t("Usage events are on. You can change this in Settings.")
            : t("Usage events are off. You can change this in Settings.")}
        </p>
      );
    }
    return (
      <section
        aria-labelledby={titleId}
        className="consent-card space-y-3 rounded-xl border border-surface-border/25 bg-surface-panel p-4 text-xs text-content-muted"
      >
        <div>
          <h2
            className="text-sm font-semibold text-content-primary"
            id={titleId}
          >
            {t("Help improve Halv")}
          </h2>
          <p className="mt-1 leading-5 text-content-secondary">
            {t(
              "Halv sends anonymous usage events only if you say yes, so we can fix the steps where people get stuck.",
            )}
          </p>
        </div>
        <ConsentDetails showChangeNote />
        <ConsentChoices preference={preference} onAnswer={onAnswer} />
        <ConsentError preference={preference} onRetry={onRetry} />
      </section>
    );
  }

  return (
    <div className="space-y-2 text-xs font-normal tracking-normal text-content-muted">
      <label className="flex items-center gap-2 text-[13px] font-medium text-content-secondary">
        <input
          aria-describedby={description ? descriptionId : undefined}
          checked={preference.consent === "on"}
          className="h-3.5 w-3.5 shrink-0 accent-accent"
          disabled={preference.consent === null || preference.pending}
          onChange={(event) => onAnswer(event.target.checked ? "on" : "off")}
          type="checkbox"
        />
        {t("Share anonymous usage events")}
      </label>
      {description ? (
        <p className="leading-5" id={descriptionId}>
          {t(
            "Anonymous usage events help Halv fix the steps where people get stuck.",
          )}
        </p>
      ) : null}
      {preference.consent === "unset" ? (
        <p className="leading-5">
          {t("No answer yet. Halv sends nothing until you turn this on.")}
        </p>
      ) : null}
      {description ? <ConsentDetails /> : null}
      <ConsentError preference={preference} onRetry={onRetry} />
    </div>
  );
}

function useConsentControls() {
  const preference = useAnonymousUsagePreference();
  return {
    preference,
    onAnswer: (answer: "on" | "off") => void updatePreference(answer),
    onRetry: () => void updatePreference(),
  };
}

/**
 * The consent card of ADR 0021.
 * - `ask`: title, one sentence, a "What is sent" disclosure, two equal buttons.
 *   After an answer it shows one status line.
 * - `settings`: one checkbox with the same disclosure (`description={false}` hides it).
 */
export function ConsentCard({
  variant,
  description,
}: {
  variant: ConsentCardVariant;
  description?: boolean;
}) {
  return (
    <ConsentCardView {...useConsentControls()} description={description} variant={variant} />
  );
}

/**
 * One line for installs that upgraded from the old default (events on,
 * never asked). It shows while the answer is "unset" and disappears once
 * the user answers. App places it at the top of the main column.
 */
export function ConsentAskBanner() {
  return <ConsentCardView {...useConsentControls()} variant="banner" />;
}

/** The settings toggle. Kept for the callers that render it by name. */
export function AnonymousUsagePreference({
  description = true,
}: {
  description?: boolean;
}) {
  return <ConsentCard description={description} variant="settings" />;
}
