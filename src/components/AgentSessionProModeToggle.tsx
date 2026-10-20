import { useEffect, useState, useSyncExternalStore } from "react";
import {
  agentSessionProModeStore,
  claimAgentSessionProModeHint,
  type GhosttyProbe,
} from "../agentSessions";
import { useT, type Translate } from "../i18n/runtime";

type AgentSessionProModeToggleProps = {
  ghostty: GhosttyProbe | null | undefined;
  variant?: "compact" | "panel";
  showIntroHint?: boolean;
};

export function AgentSessionProModeToggle({
  ghostty,
  variant = "compact",
  showIntroHint = true,
}: AgentSessionProModeToggleProps) {
  const t = useT();
  const enabled = useSyncExternalStore(
    agentSessionProModeStore.subscribe,
    agentSessionProModeStore.getSnapshot,
  );
  const caption = proModeCaptionLabel(ghostty, t);
  const proModeTooltip = t(
    "Pro mode opens conversations in a real terminal. Turning it on also switches the current conversation.",
  );
  const title = `${proModeTooltip} ${caption}.`;
  const [hintOpen, setHintOpen] = useState(false);

  // Pro is on by default; the one-time hint doubles as onboarding for
  // users who never touched the toggle.
  useEffect(() => {
    if (enabled && claimAgentSessionProModeHint()) {
      setHintOpen(showIntroHint);
    }
  }, [enabled, showIntroHint]);

  function toggleProMode() {
    const turningOn = !enabled;
    agentSessionProModeStore.toggle();
    if (turningOn && claimAgentSessionProModeHint()) {
      setHintOpen(showIntroHint);
    }
  }

  const hint = hintOpen && showIntroHint ? (
    <div className="agent-session-pro-hint" role="status">
      <span>
        {t(
          "Pro mode is on: conversations open in a real terminal running the agent CLI, with live token savings. Toggle it off anytime for the classic chat.",
        )}
      </span>
      <button
        aria-label={t("Dismiss Pro mode hint")}
        onClick={() => setHintOpen(false)}
        type="button"
      >
        ×
      </button>
    </div>
  ) : null;

  if (variant === "panel") {
    return (
      <div className="agent-session-header-pro-mode agent-session-pro-toggle-anchor">
        <button
          aria-checked={enabled}
          className="agent-session-pro-toggle"
          onClick={toggleProMode}
          role="switch"
          title={proModeTooltip}
          type="button"
        >
          <span className="agent-session-pro-toggle-copy">
            <strong>{t("New conversation mode")}</strong>
            <small>{proModeStateLabel(enabled, t)}</small>
          </span>
          <span className="agent-session-toggle-track" aria-hidden>
            <span />
          </span>
        </button>
        <p className="agent-session-pro-caption">{caption}</p>
        {hint}
      </div>
    );
  }

  return (
    <div
      className="agent-session-pro-toggle-anchor flex shrink-0 items-center gap-1.5"
      data-composer-pro-mode
      title={title}
    >
      <button
        aria-checked={enabled}
        aria-label={t("New conversations open in Pro mode")}
        className="composer-pro-toggle"
        onClick={toggleProMode}
        role="switch"
        type="button"
      >
        <span>{t("Pro")}</span>
        <span className="agent-session-toggle-track" aria-hidden>
          <span />
        </span>
      </button>
      {hint}
    </div>
  );
}

function proModeCaptionLabel(
  ghostty: GhosttyProbe | null | undefined,
  t: Translate,
) {
  return ghostty?.found && ghostty.applescriptReady
    ? t("New conversations open in Ghostty")
    : t("New conversations open in the built-in terminal");
}

function proModeStateLabel(enabled: boolean, t: Translate) {
  return enabled ? t("On") : t("Off");
}
