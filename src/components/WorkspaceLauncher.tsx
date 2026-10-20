import { useEffect, useRef, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { useT } from "../i18n/runtime";
import {
  launcherFocusIndex,
  launcherItemKey,
  type LauncherItem,
} from "../workspaceLauncher";
import { AgentGlyph } from "./AgentGlyph";

// The launcher of an empty workspace in Pro mode (ADR 0020). It copies the
// visual language of Orca's Landing page and its quick launch menu: a quiet
// centered column, the place first, then one row per choice with its key
// hint. Each row opens one tab in this workspace.

type WorkspaceLauncherProps = {
  items: LauncherItem[];
  projectName: string | null;
  projectAvatar: { initial: string; color: string } | null;
  workspaceLabel: string;
  primary: boolean;
  agentLabel: (agentKind: string) => string;
  onAgent: (agentKind: string) => void;
  onTerminal: () => void;
  onText: () => void;
  /** A change moves the focus to the first row (a workspace click). */
  focusSignal: number;
};

export function WorkspaceLauncher({
  items,
  projectName,
  projectAvatar,
  workspaceLabel,
  primary,
  agentLabel,
  onAgent,
  onTerminal,
  onText,
  focusSignal,
}: WorkspaceLauncherProps) {
  const t = useT();
  const listRef = useRef<HTMLDivElement>(null);
  const lastSignalRef = useRef(focusSignal);

  useEffect(() => {
    if (focusSignal === lastSignalRef.current) return;
    lastSignalRef.current = focusSignal;
    listRef.current?.querySelector<HTMLButtonElement>("button")?.focus();
  }, [focusSignal]);

  function onListKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    if (event.altKey || event.metaKey || event.ctrlKey || event.nativeEvent.isComposing) return;
    const buttons = [...(listRef.current?.querySelectorAll<HTMLButtonElement>("button") ?? [])];
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
    const next = launcherFocusIndex(event.key, index, buttons.length);
    if (next === null) return;
    event.preventDefault();
    buttons[next]?.focus();
  }

  return (
    <div className="workspace-launcher" data-workspace-launcher>
      <div className="workspace-launcher-column">
        <div className="workspace-launcher-place">
          {projectAvatar ? (
            <span
              aria-hidden
              className="project-tree-avatar workspace-launcher-avatar"
              data-color={projectAvatar.color}
            >
              {projectAvatar.initial}
            </span>
          ) : null}
          {projectName ? <p className="workspace-launcher-project">{projectName}</p> : null}
          <h2 className="workspace-launcher-workspace">
            <span className="truncate">{workspaceLabel}</span>
            {primary ? (
              <span className="project-tree-primary-badge">{t("primary")}</span>
            ) : null}
          </h2>
          <p className="workspace-launcher-hint">
            {t("Start an agent, a terminal or a text conversation in this workspace.")}
          </p>
        </div>
        <div
          aria-label={t("Start in {workspace}", { workspace: workspaceLabel })}
          className="workspace-launcher-list"
          onKeyDown={onListKeyDown}
          ref={listRef}
          role="group"
        >
          {items.map((item) => {
            const label =
              item.kind === "agent"
                ? agentLabel(item.agentKind)
                : item.kind === "terminal"
                  ? t("Terminal")
                  : t("Text conversation");
            const detail =
              item.kind === "agent"
                ? t("Agent in a new tab")
                : item.kind === "terminal"
                  ? t("Empty shell in a new tab")
                  : t("Choose the agent and model as you write");
            const shortcut = item.kind === "text" ? null : item.shortcut;
            return (
              <button
                className="workspace-launcher-item"
                data-launcher-item={launcherItemKey(item)}
                key={launcherItemKey(item)}
                onClick={() => {
                  if (item.kind === "agent") onAgent(item.agentKind);
                  else if (item.kind === "terminal") onTerminal();
                  else onText();
                }}
                type="button"
              >
                <span aria-hidden className="workspace-launcher-icon">
                  <AgentGlyph
                    kind={item.kind === "agent" ? item.agentKind : item.kind === "text" ? "chat" : null}
                    size={15}
                  />
                </span>
                <span className="workspace-launcher-text">
                  <span className="workspace-launcher-label">{label}</span>
                  <span className="workspace-launcher-detail">{detail}</span>
                </span>
                {shortcut ? <kbd className="workspace-launcher-kbd">{shortcut}</kbd> : null}
              </button>
            );
          })}
        </div>
      </div>
    </div>
  );
}
