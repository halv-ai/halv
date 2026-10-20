import { useEffect, useMemo, useRef, useState } from "react";
import { matchCommands } from "../commandPalette";
import {
  commandCategoryLabel,
  commandDescriptionLabel,
  commandErrorLabel,
  commandUnavailableReasonLabel,
  type CommandRegistryEntry,
} from "../commands";
import { useT, type Translate } from "../i18n/runtime";

type CommandPaletteDialogProps = {
  open: boolean;
  commands: CommandRegistryEntry[];
  getUnavailableReason: (command: CommandRegistryEntry) => string | null;
  getShortcut: (command: CommandRegistryEntry) => string | null;
  onClose: () => void;
  onCreateWorktree: (() => Promise<void>) | null;
  onPrefill: (command: CommandRegistryEntry) => void;
  onRun: (command: CommandRegistryEntry) => Promise<void>;
};

export function CommandPaletteDialog({
  open,
  commands,
  getUnavailableReason,
  getShortcut,
  onClose,
  onCreateWorktree,
  onPrefill,
  onRun,
}: CommandPaletteDialogProps) {
  const t = useT();
  const [query, setQuery] = useState("");
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [runningId, setRunningId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);
  const paletteCommands = useMemo<CommandRegistryEntry[]>(
    () =>
      onCreateWorktree
        ? [
            {
              id: "workspace.new-worktree",
              name: "New worktree",
              argsHint: "",
              description: "Create a separate worktree for the selected project.",
              category: "Workspace",
              keywords: ["create", "branch"],
              handler: onCreateWorktree,
            },
            ...commands,
          ]
        : commands,
    [commands, onCreateWorktree],
  );
  const matches = useMemo(
    () => matchCommands(paletteCommands, query),
    [paletteCommands, query],
  );
  const matchStates = matches.map(({ command }) => ({
    command,
    unavailable: getUnavailableReason(command),
  }));
  const selectableIndexes = matchStates.flatMap(({ unavailable }, index) =>
    unavailable ? [] : [index],
  );
  const selectableKey = selectableIndexes.join(",");
  const activeIndex = selectableIndexes.includes(selectedIndex)
    ? selectedIndex
    : (selectableIndexes[0] ?? -1);
  const activeOptionId =
    activeIndex >= 0
      ? `halv-command-option-${matches[activeIndex]?.command.id}`
      : undefined;
  const runningCommand = paletteCommands.find(
    (command) => command.id === runningId,
  );

  useEffect(() => {
    setSelectedIndex(selectableIndexes[0] ?? -1);
  }, [query, selectableKey]);

  useEffect(() => {
    if (!open) {
      return;
    }
    previousFocusRef.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    setQuery("");
    setSelectedIndex(0);
    setError(null);
    window.requestAnimationFrame(() => inputRef.current?.focus());
    return () => {
      window.requestAnimationFrame(() => previousFocusRef.current?.focus());
    };
  }, [open]);

  useEffect(() => {
    if (!open || !activeOptionId) {
      return;
    }
    document.getElementById(activeOptionId)?.scrollIntoView({ block: "nearest" });
  }, [activeOptionId, open]);

  if (!open) {
    return null;
  }

  async function choose(command: CommandRegistryEntry) {
    if (getUnavailableReason(command)) {
      return;
    }
    if (command.argsHint) {
      onPrefill(command);
      onClose();
      return;
    }
    setRunningId(command.id);
    setError(null);
    try {
      if (command.id === "workspace.new-worktree") {
        await onCreateWorktree?.();
      } else {
        await onRun(command);
      }
      onClose();
    } catch (cause) {
      setError(
        commandErrorLabel(
          cause instanceof Error ? cause.message : String(cause),
          t,
        ),
      );
    } finally {
      setRunningId(null);
    }
  }

  function handleKeyDown(event: React.KeyboardEvent<HTMLInputElement>) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      if (selectableIndexes.length > 0) {
        const currentPosition = selectableIndexes.indexOf(activeIndex);
        setSelectedIndex(
          selectableIndexes[(currentPosition + 1) % selectableIndexes.length],
        );
      }
      return;
    }
    if (event.key === "ArrowUp") {
      event.preventDefault();
      if (selectableIndexes.length > 0) {
        const currentPosition = selectableIndexes.indexOf(activeIndex);
        setSelectedIndex(
          selectableIndexes[
            (currentPosition - 1 + selectableIndexes.length) %
              selectableIndexes.length
          ],
        );
      }
      return;
    }
    if (event.key === "Home") {
      event.preventDefault();
      setSelectedIndex(selectableIndexes[0] ?? -1);
      return;
    }
    if (event.key === "End") {
      event.preventDefault();
      setSelectedIndex(selectableIndexes.at(-1) ?? -1);
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      const selected = matches[activeIndex]?.command;
      if (selected) {
        void choose(selected);
      }
    }
  }

  function handleDialogKeyDown(event: React.KeyboardEvent<HTMLDivElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      onClose();
      return;
    }
    if (event.key !== "Tab") {
      return;
    }
    const focusable = Array.from(
      panelRef.current?.querySelectorAll<HTMLElement>(
        'button:not([disabled]), input:not([disabled]), [tabindex]:not([tabindex="-1"])',
      ) ?? [],
    ).filter((element) => !element.hasAttribute("aria-hidden"));
    if (focusable.length === 0) {
      return;
    }
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
    <div
      aria-busy={runningId !== null}
      aria-describedby="halv-command-palette-status"
      aria-labelledby="halv-command-palette-title"
      aria-modal="true"
      className="fixed inset-0 z-[80] flex items-start justify-center bg-black/35 px-2 pt-3 backdrop-blur-sm sm:px-4 sm:pt-[12vh]"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) {
          onClose();
        }
      }}
      role="dialog"
    >
      <div
        className="flex max-h-[calc(100vh-1.5rem)] w-full max-w-2xl flex-col overflow-hidden rounded-xl border border-surface-border/35 bg-surface-glass/95 shadow-2xl shadow-black/30 sm:max-h-[76vh]"
        onKeyDown={handleDialogKeyDown}
        ref={panelRef}
      >
        <div className="flex items-center justify-between border-b border-surface-border/25 px-4 py-2.5">
          <div>
            <h2
              className="text-sm font-semibold text-content-primary"
              id="halv-command-palette-title"
            >
              {t("Command palette")}
            </h2>
            <p className="mt-0.5 text-[10px] text-content-muted">
              {t(
                "Search commands and actions across conversations, workspaces, and the app.",
              )}
            </p>
          </div>
          <button
            aria-label={t("Close command palette")}
            className="flex h-8 w-8 items-center justify-center rounded-lg text-content-muted transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
            onClick={onClose}
            type="button"
          >
            <span aria-hidden>×</span>
          </button>
        </div>
        <div className="border-b border-surface-border/30 p-3">
          <input
            aria-activedescendant={activeOptionId}
            aria-controls="halv-command-results"
            aria-expanded="true"
            aria-label={t("Search commands")}
            className="w-full bg-transparent px-2 py-1 text-base text-content-primary outline-none placeholder:text-content-muted"
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={handleKeyDown}
            placeholder={t("Type a command or action…")}
            ref={inputRef}
            role="combobox"
            value={query}
          />
        </div>
        {error ? (
          <div
            className="border-b border-danger/25 bg-danger/10 px-4 py-2 text-xs text-danger"
            role="alert"
          >
            {error}
          </div>
        ) : null}
        <div className="flex items-center justify-between px-4 pb-1 pt-2 text-[10px] text-content-muted">
          <span>{query ? t("Search results") : t("All commands")}</span>
          <span>
            {selectableIndexes.length === matches.length
              ? t("{count} available", { count: matches.length })
              : t("{available} available · {shown} shown", {
                  available: selectableIndexes.length,
                  shown: matches.length,
                })}
          </span>
        </div>
        <div
          className="min-h-0 flex-1 overflow-y-auto p-1.5"
          id="halv-command-results"
          role="listbox"
        >
          {matches.length === 0 ? (
            <div className="px-3 py-8 text-center text-sm text-content-muted">
              {t("No commands match “{query}”", { query })}
            </div>
          ) : (
            matchStates.map(({ command, unavailable }, index) => {
              const shortcut = getShortcut(command);
              const selected = index === activeIndex;
              return (
                <button
                  aria-disabled={Boolean(unavailable)}
                  aria-selected={selected}
                  className={`flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left transition-colors ${
                    selected ? "bg-accent/10" : "hover:bg-surface-raised/70"
                  } ${unavailable ? "opacity-50" : ""}`}
                  disabled={runningId !== null || Boolean(unavailable)}
                  id={`halv-command-option-${command.id}`}
                  key={command.id}
                  onClick={() => void choose(command)}
                  onMouseEnter={() => {
                    if (!unavailable) {
                      setSelectedIndex(index);
                    }
                  }}
                  role="option"
                  type="button"
                >
                  <span className="min-w-0 flex-1">
                    <span className="flex items-center gap-2">
                      <span className="truncate text-sm font-medium text-content-primary">
                        {runningId === command.id
                          ? t("Running…")
                          : commandNameLabel(command, t)}
                      </span>
                      <span className="font-mono text-[9px] uppercase tracking-[0.14em] text-accent">
                        {commandCategoryLabel(command.category, t)}
                      </span>
                    </span>
                    <span className="mt-0.5 block truncate text-xs text-content-muted">
                      {unavailable
                        ? commandUnavailableReasonLabel(unavailable, t)
                        : commandDescriptionLabel(command, t)}
                    </span>
                  </span>
                  {command.argsHint ? (
                    <span className="shrink-0 font-mono text-[10px] text-content-muted">
                      {command.argsHint}
                    </span>
                  ) : shortcut ? (
                    <kbd className="shrink-0 rounded border border-surface-border/40 bg-surface-sunken px-2 py-0.5 font-mono text-[10px] text-content-muted">
                      {shortcut}
                    </kbd>
                  ) : null}
                </button>
              );
            })
          )}
        </div>
        <div
          aria-live="polite"
          className="sr-only"
          id="halv-command-palette-status"
          role="status"
        >
          {runningCommand
            ? t("Running {name}", {
                name: commandNameLabel(runningCommand, t),
              })
            : error
              ? t("Command failed: {error}", { error })
              : matches.length === 1
                ? t("{count} command found", { count: matches.length })
                : t("{count} commands found", { count: matches.length })}
        </div>
        <div className="flex items-center gap-3 border-t border-surface-border/25 px-4 py-2 font-mono text-[9px] uppercase tracking-[0.12em] text-content-muted">
          <span>{t("↑↓ navigate")}</span>
          <span>{t("↵ run")}</span>
          <span>{t("esc close")}</span>
          <span className="ml-auto hidden sm:inline">
            {t("{count} commands", { count: matches.length })}
          </span>
        </div>
      </div>
    </div>
  );
}

function commandNameLabel(command: CommandRegistryEntry, t: Translate) {
  return command.id === "workspace.new-worktree"
    ? t("New worktree")
    : command.name;
}
