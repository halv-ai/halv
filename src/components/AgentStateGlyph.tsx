import type { AnimationEvent, ReactNode } from "react";
import {
  agentGlyphTitle,
  agentPaneGlyph,
  agentPaneStateGlyph,
  agentPaneTitle,
  type AgentPaneGlyph,
  type AgentPaneView,
} from "../agentPaneStatus.ts";
import { useT } from "../i18n/runtime";

type AgentStateGlyphProps = {
  /** A pane view. It draws its news glyph, or its state glyph with `state`. */
  view?: (Pick<AgentPaneView, "status" | "unread"> & Partial<Pick<AgentPaneView, "source">>) | null;
  /** A glyph to draw, for a tree row, a tab or an aggregate. */
  glyph?: AgentPaneGlyph | null;
  /** Draw the state glyph of `view` (Orca's vocabulary), not its news glyph. */
  state?: boolean;
  /** A workspace or project summary, as Orca's worktree card: a finished
   *  turn is a green dot, and a pane that reports nothing is an idle dot. */
  summary?: boolean;
  /** Replaces the default state name in the tooltip and for screen readers. */
  label?: string;
  size?: "sm" | "md";
  className?: string;
};

const SPINNER_ANIMATION = "agent-spinner-rotate";

// Every spinner starts its turn at time 0, so all spinners on the screen
// turn in phase. The animation runs on the compositor, not in JavaScript.
function syncSpinnerPhase(event: AnimationEvent<HTMLSpanElement>) {
  if (event.animationName !== SPINNER_ANIMATION) return;
  const element = event.currentTarget;
  if (typeof element.getAnimations !== "function") return;
  const animation = element
    .getAnimations()
    .find(
      (candidate) =>
        "animationName" in candidate &&
        (candidate as CSSAnimation).animationName === SPINNER_ANIMATION,
    );
  if (animation) animation.startTime = 0;
}

// The icon paths come from Lucide (ISC license), as in Orca: CircleCheck,
// CircleDashed and MessageCircleQuestionMark.
function LucideIcon({ children }: { children: ReactNode }) {
  return (
    <svg
      aria-hidden="true"
      fill="none"
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      strokeWidth="2"
      viewBox="0 0 24 24"
    >
      {children}
    </svg>
  );
}

function GlyphShape({ glyph, summary }: { glyph: AgentPaneGlyph; summary: boolean }) {
  if (summary && (glyph === "done" || glyph === "unverifiable")) {
    return <span aria-hidden="true" className="agent-state-glyph-dot" />;
  }
  switch (glyph) {
    case "working":
      return (
        <span
          aria-hidden="true"
          className="agent-working-spinner"
          data-agent-spinner=""
          onAnimationStart={syncSpinnerPhase}
        />
      );
    case "attention":
      return (
        <LucideIcon>
          <path d="M2.992 16.342a2 2 0 0 1 .094 1.167l-1.065 3.29a1 1 0 0 0 1.236 1.168l3.413-.998a2 2 0 0 1 1.099.092 10 10 0 1 0-4.777-4.719" />
          <path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3" />
          <path d="M12 17h.01" />
        </LucideIcon>
      );
    case "done":
      return (
        <LucideIcon>
          <circle cx="12" cy="12" r="10" />
          <path d="m16 9-5.5 5.5L8 12" />
        </LucideIcon>
      );
    case "unverifiable":
      return (
        <LucideIcon>
          <path d="M10.1 2.182a10 10 0 0 1 3.8 0" />
          <path d="M13.9 21.818a10 10 0 0 1-3.8 0" />
          <path d="M17.609 3.721a10 10 0 0 1 2.69 2.7" />
          <path d="M2.182 13.9a10 10 0 0 1 0-3.8" />
          <path d="M20.279 17.609a10 10 0 0 1-2.7 2.69" />
          <path d="M21.818 10.1a10 10 0 0 1 0 3.8" />
          <path d="M3.721 6.391a10 10 0 0 1 2.7-2.69" />
          <path d="M6.391 20.279a10 10 0 0 1-2.69-2.7" />
        </LucideIcon>
      );
    case "failed":
    case "interrupted":
    case "idle":
      return <span aria-hidden="true" className="agent-state-glyph-dot" />;
  }
}

// Orca's state vocabulary: a yellow spinner while the agent works, an
// orange question bubble while it waits for the user, a green check for a
// finished turn, an amber dashed ring for a pane that reports nothing, a
// red dot for a failure, and a muted dot for a stop or an idle pane.
export function AgentStateGlyph({
  view = null,
  glyph: glyphProp,
  state = false,
  summary = false,
  label,
  size = "md",
  className,
}: AgentStateGlyphProps) {
  const t = useT();
  const glyph =
    glyphProp !== undefined
      ? glyphProp
      : state
        ? agentPaneStateGlyph(view)
        : agentPaneGlyph(view);
  if (!glyph) return null;
  const name =
    label ??
    (view && glyphProp === undefined && glyph !== "unverifiable"
      ? agentPaneTitle(view, t)
      : agentGlyphTitle(glyph, t));
  const shown = summary && glyph === "unverifiable" ? "idle" : glyph;
  const classes = [
    "agent-state-glyph",
    `agent-state-glyph-${shown}`,
    `agent-state-glyph-${size}`,
    className ?? "",
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <span
      aria-label={name}
      className={classes}
      data-agent-glyph={glyph}
      role="status"
      title={name}
    >
      <GlyphShape glyph={glyph} summary={summary} />
    </span>
  );
}
