import { useT } from "../i18n/runtime";

export function DotShortcut({ compact = false, active, onOpen }: {
  compact?: boolean; active: boolean; onOpen: () => void;
}) {
  const t = useT();
  return (
    <div className={compact ? "dot-shortcut dot-shortcut-compact" : "dot-shortcut"}>
      <button type="button" className="dot-shortcut-button"
        aria-label={t("Open Dot")} aria-current={active ? "page" : undefined}
        title={t("Open Dot")} onClick={onOpen}>
        <span className="dot-shortcut-mark" aria-hidden="true">
          <svg width="24" height="24" viewBox="0 0 24 24" fill="none">
            <circle cx="12" cy="12" r="9" fill="currentColor" opacity=".17" />
            <circle cx="12" cy="12" r="6" fill="currentColor" />
            <circle cx="10" cy="11.5" r=".85" fill="var(--dot-eye)" />
            <circle cx="14" cy="11.5" r=".85" fill="var(--dot-eye)" />
          </svg>
        </span>
        {!compact && <span className="min-w-0 flex-1 text-left">
          <span className="block text-sm font-medium text-content-primary">Dot</span>
        </span>}
      </button>
    </div>
  );
}
