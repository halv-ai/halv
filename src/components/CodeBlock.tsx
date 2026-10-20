import { useEffect, useMemo, useRef, useState } from "react";
import hljs from "highlight.js/lib/common";
import { copyText } from "../clipboard";
import { useT } from "../i18n/runtime";

type CodeBlockProps = {
  code: string;
  language: string;
};

export function CodeBlock({ code, language }: CodeBlockProps) {
  const t = useT();
  const [copied, setCopied] = useState(false);
  const copyResetTimer = useRef<number | undefined>(undefined);
  const highlighted = useMemo(() => {
    try {
      if (language && hljs.getLanguage(language)) {
        return hljs.highlight(code, { language }).value;
      }
      return hljs.highlightAuto(code).value;
    } catch {
      return escapeHtml(code);
    }
  }, [code, language]);

  useEffect(() => {
    // Clear the reset timer on unmount.
    return () => window.clearTimeout(copyResetTimer.current);
  }, []);

  async function handleCopy() {
    if (await copyText(code)) {
      setCopied(true);
      window.clearTimeout(copyResetTimer.current);
      copyResetTimer.current = window.setTimeout(() => setCopied(false), 1200);
    }
  }

  return (
    <div className="group/code my-3 overflow-hidden rounded-md border border-surface-border/30 bg-code-bg">
      <div className="flex items-center justify-between border-b border-surface-border/25 bg-code-header px-3 py-2">
        <span className="font-mono text-[10px] uppercase tracking-[0.16em] text-content-muted">
          {language || "plain text"}
        </span>
        <button
          className="rounded border border-surface-border/30 px-2 py-1 font-mono text-[11px] text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
          onClick={() => void handleCopy()}
          type="button"
        >
          {copied ? t("Copied") : t("Copy")}
        </button>
      </div>
      <pre className="overflow-x-auto p-3 font-mono text-sm leading-6">
        <code className="hljs block bg-transparent" dangerouslySetInnerHTML={{ __html: highlighted }} />
      </pre>
    </div>
  );
}

function escapeHtml(value: string) {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}
