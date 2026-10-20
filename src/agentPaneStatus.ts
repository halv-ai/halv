import type { TerminalActivityStatus } from "./terminalActivity.ts";
import type { Translate } from "./i18n/runtime";

// The status of one agent terminal pane. Rust reads the agent signals (the
// Claude presence file and hooks, the Codex rollout and terminal
// notifications, and the terminal title of each agent) and sends one entry
// per pane. The title is the Rust fallback: it drives a pane without a
// fresh signal, such as an old pane without hooks. The byte-volume tracker
// is the fallback for panes without any signal.

export type AgentPaneStatus =
  | "working"
  | "done"
  | "needsInput"
  | "needsApproval"
  | "idle"
  | "exited"
  | "failed"
  | "interrupted";

export type AgentStatusSource =
  | "none"
  | "presence"
  | "hook"
  | "rollout"
  | "terminal"
  | "title"
  | "process";

export type AgentPromptDetail =
  | {
      kind: "approval";
      toolName: string;
      command?: string;
      filePath?: string;
      message?: string;
    }
  | { kind: "question"; text: string; options?: string[] };

/** One pane in `agent_status_snapshot` and in the `agent-status` event. */
export type AgentStatusEntry = {
  sessionId: string;
  status: AgentPaneStatus;
  reason?: string;
  updatedAt: number;
  unread: boolean;
  waitSeq: number;
  turnSeq: number;
  source: AgentStatusSource;
  prompt?: AgentPromptDetail;
};

/** What the UI shows for one running agent pane. */
export type AgentPaneView = {
  status: AgentPaneStatus;
  reason: string | null;
  unread: boolean;
  waitSeq: number;
  turnSeq: number;
  prompt: AgentPromptDetail | null;
  source: AgentStatusSource | "heuristic";
  updatedAt: number;
};

/** The byte-volume fallback state of one pane. */
export type HeuristicPane = {
  status: "idle" | "working" | "done";
  /** The status before the current working spell. */
  restStatus: "idle" | "done";
  /** The current working spell had sustained output. */
  responded: boolean;
  turnSeq: number;
  settledTurnSeq: number;
  viewedTurnSeq: number;
  updatedAt: number;
};

export function startHeuristicPane(now: number): HeuristicPane {
  return {
    status: "idle",
    restStatus: "idle",
    responded: false,
    turnSeq: 0,
    settledTurnSeq: 0,
    viewedTurnSeq: 0,
    updatedAt: now,
  };
}

// The tracker reports "running" for any live output and "responding" for
// sustained output in a turn. Only a spell with sustained output ends as a
// finished turn. A short burst, such as a redraw, returns to the rest state.
export function nextHeuristicPane(
  pane: HeuristicPane,
  activity: TerminalActivityStatus | null,
  now: number,
  viewed: boolean,
): HeuristicPane {
  if (activity === "running" || activity === "responding") {
    const working = pane.status === "working";
    const responded = activity === "responding" || (working && pane.responded);
    if (working && pane.responded === responded) return pane;
    return {
      ...pane,
      status: "working",
      restStatus: working ? pane.restStatus : pane.status === "done" ? "done" : "idle",
      responded,
      updatedAt: working ? pane.updatedAt : now,
    };
  }
  if (pane.status !== "working") return pane;
  if (activity === "waiting_input" && pane.responded) {
    const turnSeq = pane.turnSeq + 1;
    return {
      ...pane,
      status: "done",
      restStatus: "done",
      responded: false,
      turnSeq,
      settledTurnSeq: turnSeq,
      viewedTurnSeq: viewed ? turnSeq : pane.viewedTurnSeq,
      updatedAt: now,
    };
  }
  return { ...pane, status: pane.restStatus, responded: false, updatedAt: now };
}

/** Reads every finished turn. A turn that still runs stays unread later. */
export function viewHeuristicPane(pane: HeuristicPane): HeuristicPane {
  return pane.viewedTurnSeq === pane.settledTurnSeq
    ? pane
    : { ...pane, viewedTurnSeq: pane.settledTurnSeq };
}

export function heuristicPaneUnread(pane: HeuristicPane) {
  return pane.status === "done" && pane.viewedTurnSeq < pane.settledTurnSeq;
}

/** True when an agent signal drives the pane. */
export function agentStatusIsAuthoritative(
  entry: AgentStatusEntry | undefined,
): entry is AgentStatusEntry {
  return entry !== undefined && entry.source !== "none" && entry.status !== "exited";
}

// The agent signal wins, also a terminal title. The heuristic applies only
// to a pane without one. The terminal list owns process exit, so an exited
// entry falls back too.
export function resolveAgentPaneView(
  entry: AgentStatusEntry | undefined,
  heuristic: HeuristicPane | undefined,
): AgentPaneView | null {
  if (agentStatusIsAuthoritative(entry)) {
    return {
      status: entry.status,
      reason: entry.reason ?? null,
      unread: entry.unread,
      waitSeq: entry.waitSeq,
      turnSeq: entry.turnSeq,
      prompt: entry.prompt ?? null,
      source: entry.source,
      updatedAt: entry.updatedAt,
    };
  }
  if (!heuristic) return null;
  return {
    status: heuristic.status,
    reason: null,
    unread: heuristicPaneUnread(heuristic),
    waitSeq: 0,
    turnSeq: heuristic.turnSeq,
    prompt: null,
    source: "heuristic",
    updatedAt: heuristic.updatedAt,
  };
}

export function agentPaneViewsEqual(
  left: AgentPaneView | null,
  right: AgentPaneView | null,
) {
  if (left === right) return true;
  if (!left || !right) return false;
  return (
    left.status === right.status &&
    left.reason === right.reason &&
    left.unread === right.unread &&
    left.waitSeq === right.waitSeq &&
    left.turnSeq === right.turnSeq &&
    left.source === right.source &&
    left.updatedAt === right.updatedAt &&
    JSON.stringify(left.prompt) === JSON.stringify(right.prompt)
  );
}

export function agentPaneNeedsUser(view: Pick<AgentPaneView, "status"> | null) {
  return view?.status === "needsInput" || view?.status === "needsApproval";
}

/** The pane state that blocks a pasted draft. */
export function agentPaneBusy(view: Pick<AgentPaneView, "status"> | null) {
  return view?.status === "working" || agentPaneNeedsUser(view);
}

export type AgentPaneGlyph =
  | "attention"
  | "failed"
  | "working"
  | "interrupted"
  | "done"
  | "unverifiable"
  | "idle";

// The news glyph. A read finished turn, an idle pane, and a stopped turn
// show no glyph. The pinned rail and the Recent list use it.
export function agentPaneGlyph(
  view: Pick<AgentPaneView, "status" | "unread"> | null,
): AgentPaneGlyph | null {
  if (!view) return null;
  if (agentPaneNeedsUser(view)) return "attention";
  if (view.status === "working") return "working";
  if (view.unread && view.status === "failed") return "failed";
  if (view.unread && view.status === "done") return "done";
  return null;
}

/**
 * The state glyph, with Orca's vocabulary. Every live agent pane has one.
 * A finished turn keeps its check until the next turn starts. An idle pane
 * without an agent signal is "unverifiable": the PTY lives, but the agent
 * reports nothing.
 */
export function agentPaneStateGlyph(
  view: (Pick<AgentPaneView, "status"> & Partial<Pick<AgentPaneView, "source">>) | null,
): AgentPaneGlyph | null {
  if (!view) return null;
  if (agentPaneNeedsUser(view)) return "attention";
  switch (view.status) {
    case "working":
    case "failed":
    case "interrupted":
    case "done":
      return view.status;
    case "idle":
      return view.source === "none" || view.source === "heuristic"
        ? "unverifiable"
        : "idle";
    default:
      return null;
  }
}

/** The view of a live agent pane that sent no signal yet. */
export function unreportedAgentPaneView(updatedAt: number): AgentPaneView {
  return {
    status: "idle",
    reason: null,
    unread: false,
    waitSeq: 0,
    turnSeq: 0,
    prompt: null,
    source: "none",
    updatedAt,
  };
}

// Orca's order: a question first, then a failure, then live work. A stop
// is not news but is not a finish. A pane that reports nothing comes last
// before true idle.
export const GLYPH_RANK: Readonly<Record<AgentPaneGlyph, number>> = {
  attention: 0,
  failed: 1,
  working: 2,
  interrupted: 3,
  done: 4,
  unverifiable: 5,
  idle: 6,
};

/** The strongest glyph of a list, or null when the list has none. */
export function strongestGlyph(
  glyphs: readonly (AgentPaneGlyph | null | undefined)[],
): AgentPaneGlyph | null {
  let strongest: AgentPaneGlyph | null = null;
  for (const glyph of glyphs) {
    if (glyph && (!strongest || GLYPH_RANK[glyph] < GLYPH_RANK[strongest])) {
      strongest = glyph;
    }
  }
  return strongest;
}

// A question outranks work, so a busy split cannot hide a pane that waits
// for the user. Equal glyphs prefer the most recent change. The news
// glyph ranks by default; the tree and the tabs rank by the state glyph.
export function strongestAgentPaneView<T extends AgentPaneView>(
  views: readonly (T | null | undefined)[],
  glyphOf: (view: T) => AgentPaneGlyph | null = agentPaneGlyph,
): T | null {
  let strongest: T | null = null;
  let strongestRank = Number.POSITIVE_INFINITY;
  for (const view of views) {
    const glyph = view ? glyphOf(view) : null;
    if (!view || !glyph) continue;
    const rank = GLYPH_RANK[glyph];
    if (
      rank < strongestRank ||
      (rank === strongestRank && strongest && view.updatedAt > strongest.updatedAt)
    ) {
      strongest = view;
      strongestRank = rank;
    }
  }
  return strongest;
}

/** The tooltip and screen reader name of a glyph without a pane view. */
export function agentGlyphTitle(glyph: AgentPaneGlyph, t: Translate) {
  switch (glyph) {
    case "attention":
      return t("Needs input");
    case "failed":
      return t("Failed");
    case "working":
      return t("Working");
    case "interrupted":
      return t("Stopped");
    case "done":
      return t("Done");
    case "unverifiable":
      return t("Not reporting status");
    case "idle":
      return t("Idle");
  }
}

/** The short lowercase label of the terminal header. */
export function agentPaneLabel(view: Pick<AgentPaneView, "status">, t: Translate) {
  switch (view.status) {
    case "working":
      return t("working");
    case "needsApproval":
      return t("needs approval");
    case "needsInput":
      return t("needs input");
    case "done":
      return t("done");
    case "failed":
      return t("failed");
    case "interrupted":
      return t("stopped");
    case "exited":
      return t("exited");
    case "idle":
      return t("idle");
  }
}

/** The capitalized name of the state for tooltips and screen readers. */
export function agentPaneTitle(view: Pick<AgentPaneView, "status">, t: Translate) {
  switch (view.status) {
    case "working":
      return t("Working");
    case "needsApproval":
      return t("Needs approval");
    case "needsInput":
      return t("Needs input");
    case "done":
      return t("Done");
    case "failed":
      return t("Failed");
    case "interrupted":
      return t("Stopped");
    case "exited":
      return t("Finished");
    case "idle":
      return t("Idle");
  }
}

/** The prompt detail as one line, or null when the agent gave none. */
export function agentPromptSummary(
  prompt: AgentPromptDetail | null,
  agent: string,
  t: Translate,
) {
  if (!prompt) return null;
  if (prompt.kind === "question") return prompt.text.trim() || null;
  if (prompt.command?.trim()) {
    return t("{agent} wants to run: {command}", {
      agent,
      command: prompt.command.trim(),
    });
  }
  if (prompt.filePath?.trim()) {
    return t("{agent} wants to edit {file}", {
      agent,
      file: prompt.filePath.trim(),
    });
  }
  return prompt.message?.trim() || null;
}

export type AgentPaneNotificationText = { title: string; body: string };

/** The OS notification text for a pane state, or null for no notification. */
export function agentPaneNotificationText(
  view: AgentPaneView,
  context: { agent: string; title: string },
  t: Translate,
): AgentPaneNotificationText | null {
  const { agent } = context;
  let heading: string;
  switch (view.status) {
    case "needsApproval":
      heading = t("{agent} needs your approval", { agent });
      break;
    case "needsInput":
      heading =
        view.reason === "question"
          ? t("{agent} has a question", { agent })
          : t("{agent} needs your input", { agent });
      break;
    case "done":
      heading = t("{agent} finished", { agent });
      break;
    case "failed":
      heading = t("{agent} failed", { agent });
      break;
    default:
      return null;
  }
  const detail = agentPromptSummary(view.prompt, agent, t);
  return {
    title: heading,
    // The separator is the same in every language.
    body: detail ? `${context.title} · ${detail}` : context.title,
  };
}
