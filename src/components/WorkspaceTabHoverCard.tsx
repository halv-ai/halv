import { useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { agentGlyphTitle, type AgentPaneGlyph } from "../agentPaneStatus.ts";
import { useT } from "../i18n/runtime";
import { hoverCardPosition } from "../workspaceTabStrip.ts";
import { AgentGlyph } from "./AgentGlyph";
import { AgentStateGlyph } from "./AgentStateGlyph";

// The hover card of a strip tab (ADR 0010, stage S7), after Orca's
// TabHoverCard: the title, the program line with its icon, and Halv's
// state line and pane count. It opens below the tab, aligned to its left
// edge. The strip decides when it shows.

export type WorkspaceTabHoverInfo = {
  title: string;
  identity: string | null;
  program: string;
  glyph: AgentPaneGlyph | null;
  paneCount: number;
};

export function WorkspaceTabHoverCard({
  info,
  anchor,
}: {
  info: WorkspaceTabHoverInfo;
  anchor: { left: number; bottom: number };
}) {
  const t = useT();
  const cardRef = useRef<HTMLDivElement>(null);
  const [place, setPlace] = useState<{ left: number; top: number } | null>(null);

  useLayoutEffect(() => {
    const card = cardRef.current;
    if (!card) return;
    setPlace(
      hoverCardPosition(anchor, { width: card.getBoundingClientRect().width }, window.innerWidth),
    );
  }, [anchor.left, anchor.bottom]);

  return createPortal(
    <div
      className="workspace-tab-hover-card"
      data-workspace-tab-hover-card
      ref={cardRef}
      role="tooltip"
      style={{
        left: place?.left ?? anchor.left,
        top: place?.top ?? anchor.bottom,
        visibility: place ? undefined : "hidden",
      }}
    >
      <div className="workspace-tab-hover-title" data-tab-hover-card-title>
        {info.title}
      </div>
      <div className="workspace-tab-hover-line" data-tab-hover-card-program>
        <span aria-hidden className="workspace-tab-hover-icon">
          <AgentGlyph kind={info.identity} size={14} />
        </span>
        <span>{info.program}</span>
      </div>
      {info.glyph ? (
        <div className="workspace-tab-hover-line" data-tab-hover-card-state>
          <span aria-hidden className="workspace-tab-hover-icon">
            <AgentStateGlyph glyph={info.glyph} size="sm" />
          </span>
          <span>{agentGlyphTitle(info.glyph, t)}</span>
        </div>
      ) : null}
      {info.paneCount > 0 ? (
        <div className="workspace-tab-hover-line" data-tab-hover-card-panes>
          <span aria-hidden className="workspace-tab-hover-icon">
            <PanesIcon />
          </span>
          <span>
            {info.paneCount === 1
              ? t("1 pane")
              : t("{count} panes", { count: info.paneCount })}
          </span>
        </div>
      ) : null}
    </div>,
    document.body,
  );
}

function PanesIcon() {
  return (
    <svg aria-hidden fill="none" height="13" stroke="currentColor" strokeWidth="1.6" viewBox="0 0 16 16" width="13">
      <rect height="12" rx="2" width="13" x="1.5" y="2" />
      <path d="M8 2v12" />
    </svg>
  );
}
