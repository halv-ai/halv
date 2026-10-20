import { useEffect, useId, useSyncExternalStore } from "react";
import { ipc } from "../ipc.ts";
import { useT } from "../i18n/runtime";

type PreferenceState = {
  enabled: boolean | null;
  pending: boolean;
  error: string | null;
};

// Every surface uses the same saved native preference and live UI state.
let state: PreferenceState = { enabled: null, pending: false, error: null };
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

async function updatePreference(enabled?: boolean) {
  if (state.pending) return;
  publish({ ...state, pending: true, error: null });
  try {
    const preferences =
      enabled === undefined
        ? await ipc.preferences.get()
        : await ipc.preferences.setAnalyticsEnabled({
            analyticsEnabled: enabled,
          });
    publish({
      enabled: preferences.analyticsEnabled,
      pending: false,
      error: null,
    });
  } catch (error) {
    publish({
      ...state,
      pending: false,
      error: error instanceof Error ? error.message : String(error),
    });
  }
}

export function useAnonymousUsagePreference() {
  const preference = useSyncExternalStore(subscribe, getSnapshot);
  useEffect(() => {
    if (state.enabled === null) void updatePreference();
  }, []);
  return preference;
}

export function AnonymousUsagePreference({
  description = true,
}: {
  description?: boolean;
}) {
  const t = useT();
  const descriptionId = useId();
  const preference = useAnonymousUsagePreference();

  return (
    <div className="space-y-2 text-xs font-normal tracking-normal text-content-muted">
      <label className="flex items-center gap-2 text-[13px] font-medium text-content-secondary">
        <input
          aria-describedby={description ? descriptionId : undefined}
          checked={preference.enabled ?? false}
          className="h-3.5 w-3.5 shrink-0 accent-accent"
          disabled={preference.enabled === null || preference.pending}
          onChange={(event) => void updatePreference(event.target.checked)}
          type="checkbox"
        />
        {t("Share anonymous usage events")}
      </label>
      {description ? (
        <p className="leading-5" id={descriptionId}>
          {t(
            "Setup steps, activity days, failure categories, app version, OS, and a random install ID. Events expire after 90 days. No prompts, code, emails, or file paths.",
          )}
        </p>
      ) : null}
      {preference.error ? (
        <div className="space-y-1">
          <p className="text-danger" role="alert">
            {preference.enabled === null
              ? t("Could not load the anonymous usage preference: {error}", {
                  error: preference.error,
                })
              : t("Could not save the anonymous usage preference: {error}", {
                  error: preference.error,
                })}
          </p>
          {preference.enabled === null ? (
            <button
              className="text-accent underline underline-offset-2"
              disabled={preference.pending}
              onClick={() => void updatePreference()}
              type="button"
            >
              {t("Retry")}
            </button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
