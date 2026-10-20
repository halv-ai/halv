// The identity icon of an agent: the Claude and Codex marks, a letter
// badge for other agents, a chat bubble for a chat conversation, and ">_"
// for a terminal without an agent. The Claude and Codex paths copy the
// provider icons of StatusBar.tsx. Stage S7 makes StatusBar use this file.

export type AgentGlyphKind = string | "chat" | null;

const AGENT_LETTERS: Record<string, string> = {
  antigravity: "A",
  gemini: "Ge",
  grok: "Gr",
  kimi: "K",
  zai: "G",
};

type AgentGlyphProps = {
  kind: AgentGlyphKind;
  size?: number;
  className?: string;
};

export function AgentGlyph({ kind, size = 13, className }: AgentGlyphProps) {
  const classes = ["agent-glyph", "shrink-0", className ?? ""].filter(Boolean).join(" ");
  if (kind === "claude") {
    return (
      <svg
        aria-hidden
        className={`${classes} text-accent`}
        data-agent-kind="claude"
        fill="none"
        height={size}
        stroke="currentColor"
        strokeWidth="2"
        viewBox="0 0 24 24"
        width={size}
      >
        <path d="M12 1v22M1 12h22M4.2 4.2l15.6 15.6M4.2 19.8 19.8 4.2M7.8 1.8l8.4 20.4M1.8 7.8l20.4 8.4M1.8 16.2l20.4-8.4M7.8 22.2l8.4-20.4" />
      </svg>
    );
  }
  if (kind === "codex") {
    return (
      <svg
        aria-hidden
        className={`${classes} text-content-primary`}
        data-agent-kind="codex"
        fill="none"
        height={size}
        stroke="currentColor"
        strokeWidth="2"
        viewBox="0 0 24 24"
        width={size}
      >
        <path d="m8 6-6 6 6 6m8-12 6 6-6 6M14 3l-4 18" />
      </svg>
    );
  }
  if (kind === "chat") {
    return (
      <svg
        aria-hidden
        className={`${classes} text-content-muted`}
        data-agent-kind="chat"
        fill="none"
        height={size}
        stroke="currentColor"
        strokeLinejoin="round"
        strokeWidth="2"
        viewBox="0 0 24 24"
        width={size}
      >
        <path d="M4 5h16v11H9l-5 4V5Z" />
      </svg>
    );
  }
  const letter = kind ? AGENT_LETTERS[kind] : undefined;
  return (
    <span
      aria-hidden
      className={`${classes} inline-flex min-w-[13px] items-center justify-center rounded-sm bg-surface-raised px-px font-mono text-[9px] font-semibold leading-none text-content-secondary`}
      data-agent-kind={kind ?? "terminal"}
      style={{ height: size }}
    >
      {letter ?? ">_"}
    </span>
  );
}
