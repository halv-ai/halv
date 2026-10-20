// The update segment in the status bar. It stays while the update
// downloads, is ready, installs, or failed, so a hidden card is never lost.
// Orca's UpdateStatusSegment (stablyai/orca, MIT) has the same rule.
import { appUpdateStatusSegment, isAppUpdateCardVisible } from "../appUpdateCard";
import { appUpdateStore } from "../appUpdateStore";
import { formatNumber, useT } from "../i18n/runtime";
import { useAppUpdateView } from "./AppUpdateCard";

export function AppUpdateSegment() {
  const t = useT();
  const { snapshot, ui } = useAppUpdateView();
  const segment = snapshot ? appUpdateStatusSegment(snapshot, { t, formatNumber }) : null;
  if (!snapshot || !segment) return null;
  const toneClass =
    segment.tone === "danger"
      ? "text-danger"
      : segment.tone === "accent"
        ? "text-accent"
        : "text-content-secondary";
  return (
    // The visible label is the accessible name, and the title adds detail.
    <button
      aria-expanded={isAppUpdateCardVisible(snapshot, ui)}
      className={`inline-flex h-5 shrink-0 items-center gap-1.5 rounded px-1 font-medium tabular-nums transition-colors hover:bg-surface-raised focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40 ${toneClass}`}
      data-app-update-segment={snapshot.status.state}
      onClick={() => appUpdateStore.toggleCard()}
      title={segment.title}
      type="button"
    >
      {segment.tone === "neutral" ? null : (
        <span
          aria-hidden
          className={`inline-block h-1.5 w-1.5 rounded-full ${
            segment.tone === "danger" ? "bg-danger" : "bg-accent"
          }`}
        />
      )}
      {segment.label}
    </button>
  );
}
