import { ipc } from "../ipc.ts";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  defaultShortcut,
  effectiveShortcut,
  formatShortcut,
  recordedShortcutBinding,
  shortcutConflict,
  shortcutDefinitions,
  shortcutFromKeyboardEvent,
  shortcutSearchText,
  type ShortcutDefinition,
  type KeyboardShortcutOverrides,
} from "../shortcuts";
import { useT, type Translate } from "../i18n/runtime";
import type { AppPreferences } from "../types";

type KeyboardShortcutsPanelProps = {
  open: boolean;
  overrides: KeyboardShortcutOverrides;
  onChange: (overrides: KeyboardShortcutOverrides) => void;
  onClose: () => void;
};

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

export function KeyboardShortcutsPanel({
  open,
  overrides,
  onChange,
  onClose,
}: KeyboardShortcutsPanelProps) {
  const t = useT();
  const [draft, setDraft] = useState<KeyboardShortcutOverrides>(overrides);
  const [query, setQuery] = useState("");
  const [keystrokeSearch, setKeystrokeSearch] = useState(false);
  const [searchedBinding, setSearchedBinding] = useState<string | null>(null);
  const [recordingId, setRecordingId] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    setDraft(overrides);
    setQuery("");
    setSearchedBinding(null);
    setRecordingId(null);
    setError(null);
    setNotice(null);
    window.requestAnimationFrame(() => searchRef.current?.focus());
  }, [open, overrides]);

  useEffect(() => {
    if (!open) return;
    function closeOnEscape(event: KeyboardEvent) {
      if (event.key === "Escape" && recordingId === null) {
        event.preventDefault();
        onClose();
      }
    }
    document.addEventListener("keydown", closeOnEscape);
    return () => document.removeEventListener("keydown", closeOnEscape);
  }, [onClose, open, recordingId]);

  const filtered = useMemo(() => {
    const normalized = query.trim().toLowerCase();
    return shortcutDefinitions.filter((definition) => {
      if (keystrokeSearch && searchedBinding) {
        return effectiveShortcut(definition.id, draft) === searchedBinding;
      }
      const translatedSearchText = [
        shortcutLabel(definition, t),
        shortcutDescription(definition, t),
        shortcutCategoryLabel(definition.category, t),
        shortcutSearchText(definition),
      ]
        .join(" ")
        .toLowerCase();
      return !normalized || translatedSearchText.includes(normalized);
    });
  }, [draft, keystrokeSearch, query, searchedBinding, t]);

  function recordShortcut(id: string, event: React.KeyboardEvent) {
    event.preventDefault();
    event.stopPropagation();
    if (event.key === "Escape") {
      setRecordingId(null);
      return;
    }
    const recorded = shortcutFromKeyboardEvent(event.nativeEvent);
    if (!recorded) {
      setError(t("Use Command/Ctrl or Alt with another key."));
      return;
    }
    const binding = recordedShortcutBinding(id, recorded);
    const conflict = shortcutConflict(id, binding, draft);
    if (conflict) {
      setError(
        t("{binding} is already assigned to {command}.", {
          binding: formatShortcut(binding),
          command: shortcutLabel(conflict, t),
        }),
      );
      return;
    }
    const definition = shortcutDefinitions.find((item) => item.id === id);
    setDraft((current) => {
      const next = { ...current };
      if (definition && binding === defaultShortcut(definition)) delete next[id];
      else next[id] = binding;
      return next;
    });
    setRecordingId(null);
    setError(null);
    setNotice(null);
  }

  function captureSearch(event: React.KeyboardEvent<HTMLInputElement>) {
    if (!keystrokeSearch) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.key === "Escape" || event.key === "Backspace") {
      setSearchedBinding(null);
      setQuery("");
      return;
    }
    const binding = shortcutFromKeyboardEvent(event.nativeEvent);
    if (!binding) return;
    setSearchedBinding(binding);
    setQuery(formatShortcut(binding));
  }

  async function save() {
    setSaving(true);
    setError(null);
    setNotice(null);
    try {
      const preferences = await ipc.preferences.setKeyboardShortcuts({ shortcuts: draft });
      setDraft(preferences.keyboardShortcuts);
      onChange(preferences.keyboardShortcuts);
      setNotice(t("Keyboard shortcuts saved."));
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setSaving(false);
    }
  }

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-[70] flex items-center justify-center bg-surface-sunken/75 p-0 backdrop-blur-sm sm:px-4 sm:py-6">
      <section
        aria-labelledby="keyboard-shortcuts-title"
        aria-modal="true"
        className="flex h-full max-h-full w-full max-w-4xl flex-col overflow-hidden border-0 bg-surface-panel shadow-2xl sm:h-auto sm:rounded-xl sm:border sm:border-surface-border/35"
        role="dialog"
      >
        <header className="flex items-center gap-3 border-b border-surface-border/25 px-5 py-4">
          <div className="min-w-0 flex-1">
            <h2
              className="text-base font-semibold text-content-primary"
              id="keyboard-shortcuts-title"
            >
              {t("Keyboard shortcuts")}
            </h2>
            <p className="mt-1 text-xs text-content-muted">
              {t(
                "Click a binding and press a new shortcut. Changes apply after saving.",
              )}
            </p>
          </div>
          <button
            aria-label={t("Close keyboard shortcuts")}
            className="rounded-md border border-surface-border/30 px-3 py-2 text-sm text-content-secondary hover:bg-surface-raised hover:text-content-primary"
            onClick={onClose}
            type="button"
          >
            {t("Close")}
          </button>
        </header>

        <div className="flex flex-wrap items-center gap-2 border-b border-surface-border/25 px-5 py-3">
          <input
            className="min-w-64 flex-1 rounded-lg border border-surface-border/35 bg-surface-base px-3 py-2 text-sm text-content-primary outline-none placeholder:text-content-muted focus:border-accent/55 focus:ring-2 focus:ring-accent/15"
            onChange={(event) => {
              if (!keystrokeSearch) setQuery(event.target.value);
            }}
            onKeyDown={captureSearch}
            placeholder={
              keystrokeSearch
                ? t("Press a shortcut to find its command")
                : t("Search commands")
            }
            readOnly={keystrokeSearch}
            ref={searchRef}
            value={query}
          />
          <button
            aria-pressed={keystrokeSearch}
            className={`rounded-lg border px-3 py-2 text-xs transition-colors ${
              keystrokeSearch
                ? "border-accent/50 bg-accent/10 text-accent"
                : "border-surface-border/30 text-content-secondary hover:bg-surface-raised"
            }`}
            onClick={() => {
              setKeystrokeSearch((current) => !current);
              setQuery("");
              setSearchedBinding(null);
              window.requestAnimationFrame(() => searchRef.current?.focus());
            }}
            type="button"
          >
            {keystrokeSearch
              ? t("Keystroke search on")
              : t("Search by keystroke")}
          </button>
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto p-5">
          <div className="space-y-2">
            {filtered.map((definition) => {
              const binding = effectiveShortcut(definition.id, draft);
              const customized = Object.prototype.hasOwnProperty.call(
                draft,
                definition.id,
              );
              const recording = recordingId === definition.id;
              return (
                <div
                  className="flex flex-wrap items-center gap-3 rounded-lg border border-surface-border/25 bg-surface-base/50 px-3 py-3 sm:flex-nowrap sm:gap-4"
                  key={definition.id}
                >
                  <div className="min-w-0 flex-1 basis-full sm:basis-auto">
                    <div className="flex items-center gap-2">
                      <p className="text-sm font-medium text-content-primary">
                        {shortcutLabel(definition, t)}
                      </p>
                      <span className="font-mono text-[9px] uppercase tracking-[0.14em] text-content-muted">
                        {shortcutCategoryLabel(definition.category, t)}
                      </span>
                      {customized ? (
                        <span className="rounded-full bg-accent/10 px-1.5 py-0.5 font-mono text-[9px] text-accent">
                          {t("custom")}
                        </span>
                      ) : null}
                    </div>
                    <p className="mt-1 text-xs text-content-muted">
                      {shortcutDescription(definition, t)}
                    </p>
                  </div>
                  {customized ? (
                    <button
                      className="text-[10px] text-content-muted hover:text-content-primary"
                      onClick={() => {
                        setDraft((current) => {
                          const next = { ...current };
                          delete next[definition.id];
                          return next;
                        });
                        setError(null);
                      }}
                      type="button"
                    >
                      {t("Reset")}
                    </button>
                  ) : null}
                  <button
                    className={`min-w-28 rounded-md border px-3 py-2 font-mono text-xs transition-colors ${
                      recording
                        ? "border-accent bg-accent/10 text-accent ring-2 ring-accent/20"
                        : "border-surface-border/35 bg-surface-raised text-content-primary hover:border-accent/45"
                    }`}
                    onBlur={() => {
                      if (recording) setRecordingId(null);
                    }}
                    onClick={() => {
                      setRecordingId(definition.id);
                      setError(null);
                    }}
                    onKeyDown={(event) => {
                      if (recording) recordShortcut(definition.id, event);
                    }}
                    type="button"
                  >
                    {recording ? t("Press keys…") : formatShortcut(binding)}
                  </button>
                  <button
                    className="text-[10px] text-content-muted hover:text-danger"
                    onClick={() => {
                      setDraft((current) => ({
                        ...current,
                        [definition.id]: "disabled",
                      }));
                      setRecordingId(null);
                    }}
                    type="button"
                  >
                    {t("Unassign")}
                  </button>
                </div>
              );
            })}
            {filtered.length === 0 ? (
              <p className="py-10 text-center text-sm text-content-muted">
                {t("No shortcuts match this search.")}
              </p>
            ) : null}
          </div>
        </div>

        <footer className="flex flex-wrap items-center gap-3 border-t border-surface-border/25 px-5 py-4">
          {error ? (
            <p className="min-w-0 flex-1 text-xs text-danger" role="alert">
              {error}
            </p>
          ) : notice ? (
            <p className="min-w-0 flex-1 text-xs text-accent" role="status">
              {notice}
            </p>
          ) : (
            <span className="min-w-0 flex-1" />
          )}
          <button
            className="rounded-md px-3 py-2 text-sm text-content-muted hover:bg-surface-raised hover:text-content-primary"
            onClick={() => {
              setDraft({});
              setError(null);
              setNotice(t("Defaults restored locally. Save to apply."));
            }}
            type="button"
          >
            {t("Reset all")}
          </button>
          <button
            className="rounded-md bg-accent px-4 py-2 text-sm font-semibold text-accent-contrast hover:bg-accent-strong disabled:opacity-50"
            disabled={saving}
            onClick={() => void save()}
            type="button"
          >
            {saving ? t("Saving…") : t("Save shortcuts")}
          </button>
        </footer>
      </section>
    </div>
  );
}

function shortcutLabel(definition: ShortcutDefinition, t: Translate) {
  switch (definition.id) {
    case "application.command_menu":
      return t("Command menu");
    case "application.settings":
      return t("Settings");
    case "application.keyboard_shortcuts":
      return t("Keyboard shortcuts");
    case "workspace.open_folder":
      return t("Open folder");
    case "navigation.back":
      return t("Navigate back");
    case "navigation.forward":
      return t("Navigate forward");
    case "appearance.increase_font":
      return t("Increase font size");
    case "appearance.decrease_font":
      return t("Decrease font size");
    case "application.toggle_sidebar":
      return t("Toggle sidebar");
    case "workspace.open_review":
      return t("Open review tab");
    case "workspace.toggle_review":
      return t("Toggle review panel");
    case "workspace.open_browser":
      return t("Open shared browser");
    case "workspace.toggle_bottom_panel":
      return t("Toggle bottom panel");
    case "workspace.toggle_terminal":
      return t("Toggle terminal");
    case "workspace.clear_terminal":
      return t("Clear terminal");
    case "workspace.previous_terminal_tab":
      return t("Previous tab");
    case "workspace.next_terminal_tab":
      return t("Next tab");
    case "workspace.select_tab_by_number":
      return t("Select tab by number");
    case "workspace.new_agent_tab":
      return t("New agent tab");
    case "workspace.new_shell_tab":
      return t("New terminal tab");
    case "workspace.close_pane_or_tab":
      return t("Close pane or tab");
    case "workspace.reopen_closed_tab":
      return t("Reopen closed tab");
    case "workspace.rename_tab":
      return t("Rename tab");
    case "workspace.recent_tab":
      return t("Switch to a recent tab");
    case "workspace.previous_workspace":
      return t("Previous workspace");
    case "workspace.next_workspace":
      return t("Next workspace");
    case "task.quick_chat":
      return t("Quick Chat");
    case "task.new":
      return t("New task");
    case "task.search":
      return t("Search tasks");
    case "task.find":
      return t("Find in task");
    case "task.previous":
      return t("Previous task");
    case "task.next":
      return t("Next task");
    case "task.previous_pinned":
      return t("Previous pinned conversation");
    case "task.next_pinned":
      return t("Next pinned conversation");
    case "input.dictation":
      return t("Dictation");
    default:
      return definition.label;
  }
}

function shortcutDescription(definition: ShortcutDefinition, t: Translate) {
  switch (definition.id) {
    case "application.command_menu":
      return t("Open the command palette.");
    case "application.settings":
      return t("Open application settings.");
    case "application.keyboard_shortcuts":
      return t("Review or customize keyboard shortcuts.");
    case "workspace.open_folder":
      return t("Add a local project folder.");
    case "navigation.back":
      return t("Open the previous task in the list.");
    case "navigation.forward":
      return t("Open the next task in the list.");
    case "appearance.increase_font":
      return t("Increase the app UI scale.");
    case "appearance.decrease_font":
      return t("Decrease the app UI scale.");
    case "application.toggle_sidebar":
      return t("Show or hide the task sidebar.");
    case "workspace.open_review":
      return t("Open the in-app code review surface.");
    case "workspace.toggle_review":
      return t("Toggle the in-app code review surface.");
    case "workspace.open_browser":
      return t("Open the browser preview and annotation surface.");
    case "workspace.toggle_bottom_panel":
    case "workspace.toggle_terminal":
      return t("Show or hide the integrated terminal panel.");
    case "workspace.clear_terminal":
      return t("Clear the visible integrated terminal buffer.");
    case "workspace.previous_terminal_tab":
      return t("Show the previous tab of the workspace.");
    case "workspace.next_terminal_tab":
      return t("Show the next tab of the workspace.");
    case "workspace.select_tab_by_number":
      return t("Show tab 1 to 8 of the workspace. 9 shows the last tab.");
    case "workspace.new_agent_tab":
      return t("Start the default agent in a new tab of the workspace.");
    case "workspace.new_shell_tab":
      return t("Open a shell in a new tab of the workspace.");
    case "workspace.close_pane_or_tab":
      return t("Close the focused pane, else the tab. With nothing to close, hide the window.");
    case "workspace.reopen_closed_tab":
      return t("Open the last closed tab of the workspace again.");
    case "workspace.rename_tab":
      return t("Rename the visible tab of the workspace.");
    case "workspace.recent_tab":
      return t("Hold Ctrl and press Tab to pick a recent tab of the workspace. Shift goes back.");
    case "workspace.previous_workspace":
      return t("Show the workspace above in the sidebar.");
    case "workspace.next_workspace":
      return t("Show the workspace below in the sidebar.");
    case "task.quick_chat":
      return t("Start a new chat without a project.");
    case "task.new":
      return t("Start a task in the selected project.");
    case "task.search":
      return t("Focus task and message search.");
    case "task.find":
      return t("Find text inside the open task.");
    case "task.previous":
      return t("Open the previous task.");
    case "task.next":
      return t("Open the next task.");
    case "task.previous_pinned":
      return t("Cycle to the previous pinned conversation.");
    case "task.next_pinned":
      return t("Cycle to the next pinned conversation.");
    case "input.dictation":
      return t("Start or stop speech dictation in the composer.");
    default:
      return definition.description;
  }
}

function shortcutCategoryLabel(
  category: ShortcutDefinition["category"],
  t: Translate,
) {
  switch (category) {
    case "General":
      return t("General");
    case "Task":
      return t("Task");
    case "Input":
      return t("Input");
    case "Workspace":
      return t("Workspace");
  }
}
