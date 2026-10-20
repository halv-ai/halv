import { useMemo, useState } from "react";
import {
  readReasoningSummaryOpen,
  writeReasoningSummaryOpen,
} from "../reasoningSummaryState";
import { useT } from "../i18n/runtime";
import { MarkdownMessage } from "./MarkdownMessage";

type ReasoningSummaryProps = {
  text: string;
};

export function ReasoningSummary({ text }: ReasoningSummaryProps) {
  const t = useT();
  const [open, setOpen] = useState(() =>
    readReasoningSummaryOpen(reasoningSummaryStorage()),
  );
  const fragments = useMemo(() => reasoningFragments(text), [text]);

  if (fragments.length === 0) {
    return null;
  }

  return (
    <section className="rounded-lg border border-surface-border/25 bg-surface-panel/75">
      <button
        className="flex w-full items-center justify-between gap-3 px-4 py-3 text-left transition-colors duration-150 hover:bg-surface-raised/70 focus:outline-none focus:ring-2 focus:ring-inset focus:ring-accent/30"
        onClick={() => {
          const next = !open;
          setOpen(next);
          writeReasoningSummaryOpen(reasoningSummaryStorage(), next);
        }}
        type="button"
      >
        <span className="font-mono text-[10px] font-medium uppercase tracking-[0.16em] text-content-secondary">
          {t("Reasoning summary")}
        </span>
        <span className="font-mono text-[11px] text-content-muted">
          {open ? t("Hide") : t("Show")}
        </span>
      </button>
      {open ? (
        <div className="border-t border-surface-border/25 px-4 py-2.5">
          <div className="space-y-1.5">
            {fragments.map((fragment, index) => (
              <div
                className="flex items-start gap-2.5"
                key={`${index}-${fragment}`}
              >
                <span
                  aria-hidden="true"
                  className="mt-2 h-1 w-1 shrink-0 rounded-full bg-content-muted/60"
                />
                <MarkdownMessage
                  className="min-w-0 flex-1 !font-mono !text-xs !leading-5 !text-content-secondary [&>*]:!my-0"
                  content={fragment}
                />
              </div>
            ))}
          </div>
        </div>
      ) : null}
    </section>
  );
}

function reasoningSummaryStorage() {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

function reasoningFragments(text: string) {
  const sanitized = text
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(/<!--[\s\S]*$/, "")
    .replace(/\r\n?/g, "\n");
  const fragments: string[] = [];
  let currentLines: string[] = [];

  function flushFragment() {
    if (currentLines.length > 0) {
      fragments.push(currentLines.join("\n"));
      currentLines = [];
    }
  }

  for (const line of sanitized.split("\n")) {
    if (line.trim().length === 0) {
      flushFragment();
    } else {
      currentLines.push(line.trimEnd());
    }
  }

  flushFragment();
  return fragments;
}
