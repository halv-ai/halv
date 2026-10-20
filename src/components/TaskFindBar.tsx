import { useEffect, useMemo, useRef, useState } from "react";
import { useT } from "../i18n/runtime";
import type { UiMessage } from "../types";

type TaskFindBarProps = {
  open: boolean;
  messages: UiMessage[];
  onClose: () => void;
};

export function TaskFindBar({ open, messages, onClose }: TaskFindBarProps) {
  const t = useT();
  const [query, setQuery] = useState("");
  const [selectedIndex, setSelectedIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const normalizedQuery = query.trim().toLowerCase();
  const matchIds = useMemo(
    () =>
      normalizedQuery
        ? messages
            .filter((message) =>
              message.content.toLowerCase().includes(normalizedQuery),
            )
            .map((message) => message.id)
        : [],
    [messages, normalizedQuery],
  );

  useEffect(() => {
    if (!open) return;
    window.requestAnimationFrame(() => inputRef.current?.focus());
  }, [open]);

  useEffect(() => {
    setSelectedIndex((current) =>
      matchIds.length === 0 ? 0 : Math.min(current, matchIds.length - 1),
    );
  }, [matchIds.length]);

  useEffect(() => {
    const nodes = document.querySelectorAll<HTMLElement>("[data-message-id]");
    nodes.forEach((node) => {
      delete node.dataset.taskFindMatch;
      delete node.dataset.taskFindCurrent;
    });
    if (!open || matchIds.length === 0) return;
    for (const id of matchIds) {
      const node = document.querySelector<HTMLElement>(
        `[data-message-id="${CSS.escape(id)}"]`,
      );
      if (node) node.dataset.taskFindMatch = "true";
    }
    const selectedId = matchIds[selectedIndex];
    const selected = document.querySelector<HTMLElement>(
      `[data-message-id="${CSS.escape(selectedId)}"]`,
    );
    if (selected) {
      selected.dataset.taskFindCurrent = "true";
      selected.scrollIntoView({ behavior: "smooth", block: "center" });
    }
    return () => {
      nodes.forEach((node) => {
        delete node.dataset.taskFindMatch;
        delete node.dataset.taskFindCurrent;
      });
    };
  }, [matchIds, open, selectedIndex]);

  if (!open) return null;

  function move(delta: number) {
    if (matchIds.length === 0) return;
    setSelectedIndex(
      (current) => (current + delta + matchIds.length) % matchIds.length,
    );
  }

  return (
    <form
      className="flex items-center gap-2 border-b border-surface-border/25 bg-surface-panel px-4 py-2"
      onSubmit={(event) => {
        event.preventDefault();
        move(1);
      }}
    >
      <span className="font-mono text-[10px] uppercase tracking-[0.14em] text-content-muted">
        {t("Find")}
      </span>
      <input
        aria-label={t("Find in task")}
        className="min-w-0 flex-1 rounded-md border border-surface-border/30 bg-surface-base px-3 py-1.5 text-sm text-content-primary outline-none placeholder:text-content-muted focus:border-accent/55 focus:ring-2 focus:ring-accent/15"
        onChange={(event) => {
          setQuery(event.target.value);
          setSelectedIndex(0);
        }}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            onClose();
          } else if (event.key === "Enter" && event.shiftKey) {
            event.preventDefault();
            move(-1);
          }
        }}
        placeholder={t("Text in this task")}
        ref={inputRef}
        value={query}
      />
      <span className="w-16 text-center font-mono text-[10px] text-content-muted">
        {matchIds.length ? `${selectedIndex + 1}/${matchIds.length}` : "0/0"}
      </span>
      <button
        aria-label={t("Previous match")}
        className="rounded px-2 py-1 text-content-muted hover:bg-surface-raised hover:text-content-primary disabled:opacity-40"
        disabled={matchIds.length === 0}
        onClick={() => move(-1)}
        type="button"
      >
        ↑
      </button>
      <button
        aria-label={t("Next match")}
        className="rounded px-2 py-1 text-content-muted hover:bg-surface-raised hover:text-content-primary disabled:opacity-40"
        disabled={matchIds.length === 0}
        onClick={() => move(1)}
        type="button"
      >
        ↓
      </button>
      <button
        aria-label={t("Close find in task")}
        className="rounded px-2 py-1 text-content-muted hover:bg-surface-raised hover:text-content-primary"
        onClick={onClose}
        type="button"
      >
        ×
      </button>
    </form>
  );
}
