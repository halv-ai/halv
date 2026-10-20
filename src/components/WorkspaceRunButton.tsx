import {
  useEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { useT } from "../i18n/runtime";
import type { LocalEnvironmentAction } from "../types";

// The Run button of the strip (ADR 0010, stage S5b): Orca's Quick Commands
// split button. The main part runs the first local environment action of
// the workspace in a new shell tab. The chevron lists every action. The
// button sits at the right end of the tab strip and shows only when the
// workspace has an action (ADR 0020). The "•••" menu of the header keeps
// the actions and the local environment settings.

// Rust finds an action by its index in the environment file, so each entry
// keeps that index. onRun gets the index, not the command text.
export type WorkspaceRunAction = {
  action: LocalEnvironmentAction;
  index: number;
};

type WorkspaceRunButtonProps = {
  actions: WorkspaceRunAction[];
  busy: boolean;
  onRun: (index: number) => void;
  onConfigure: () => void;
};

function actionMark(action: LocalEnvironmentAction) {
  if (action.icon === "debug") return "◆";
  if (action.icon === "test") return "✓";
  if (action.icon === "tool") return "⌘";
  return "▶";
}

export function WorkspaceRunButton({
  actions,
  busy,
  onRun,
  onConfigure,
}: WorkspaceRunButtonProps) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const moreRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const first = actions[0] ?? null;

  useEffect(() => {
    if (!open) return undefined;
    menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus();
    const closeOnPointer = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const closeOnBlur = () => setOpen(false);
    document.addEventListener("pointerdown", closeOnPointer);
    window.addEventListener("blur", closeOnBlur);
    return () => {
      document.removeEventListener("pointerdown", closeOnPointer);
      window.removeEventListener("blur", closeOnBlur);
    };
  }, [open]);

  function choose(action: () => void) {
    setOpen(false);
    action();
  }

  function onMenuKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    if (event.nativeEvent.isComposing) return;
    const entries = [
      ...(menuRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? []),
    ];
    const index = entries.indexOf(document.activeElement as HTMLButtonElement);
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      setOpen(false);
      moreRef.current?.focus();
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const offset = event.key === "ArrowDown" ? 1 : -1;
      entries[(index + offset + entries.length) % entries.length]?.focus();
    } else if (event.key === "Tab") {
      setOpen(false);
    }
  }

  if (!first) return null;

  return (
    <div className="workspace-run" data-workspace-run ref={rootRef}>
      <button
        aria-label={t("Run {name}", { name: first.action.name })}
        className="workspace-run-main"
        disabled={busy}
        onClick={() => onRun(first.index)}
        title={`${first.action.name}: ${first.action.command}`}
        type="button"
      >
        <span aria-hidden>{actionMark(first.action)}</span>
        <span className="workspace-run-label">{first.action.name}</span>
      </button>
      <button
        aria-expanded={open}
        aria-haspopup="menu"
        aria-label={t("More actions")}
        className="workspace-run-more"
        onClick={() => setOpen((current) => !current)}
        ref={moreRef}
        type="button"
      >
        <svg aria-hidden fill="none" height="10" stroke="currentColor" strokeLinecap="round" strokeWidth="1.8" viewBox="0 0 12 12" width="10">
          <path d="m3 4.5 3 3 3-3" />
        </svg>
      </button>
      {open ? (
        <div
          aria-label={t("Actions")}
          className="workspace-tab-menu workspace-run-menu"
          onKeyDown={onMenuKeyDown}
          ref={menuRef}
          role="menu"
        >
          {actions.map(({ action, index }) => (
            <button
              className="workspace-tab-menu-item"
              disabled={busy}
              key={`${index}-${action.name}`}
              onClick={() => choose(() => onRun(index))}
              role="menuitem"
              title={action.command}
              type="button"
            >
              <span aria-hidden>{actionMark(action)}</span>
              <span className="workspace-tab-menu-label">{action.name}</span>
            </button>
          ))}
          <div className="workspace-tab-menu-separator" role="separator" />
          <button
            className="workspace-tab-menu-item"
            onClick={() => choose(onConfigure)}
            role="menuitem"
            type="button"
          >
            <span className="workspace-tab-menu-label">{t("Local environments…")}</span>
          </button>
        </div>
      ) : null}
    </div>
  );
}
