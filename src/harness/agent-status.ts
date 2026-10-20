import type { AgentStatusEntry } from "../agentPaneStatus.ts";
import type { TerminalSession } from "../types.ts";
import {
  MARKETING_PRO_SPLIT_PRO_MODE_HINT_SEEN_STORAGE_KEY,
  MARKETING_PRO_SPLIT_SESSION_IDS,
  seedMarketingProSplitLayout,
} from "./marketing-pro-split.ts";
import { FIXED_VISUAL_NOW } from "./states.ts";

// One pane in each agent state. The split conversation of the marketing app
// holds three panes. Three more panes sit in other conversations and in the
// sidebar terminal list.

// The account restart and account rotation states reuse these panes.
export function isAgentStatusStateName(stateName: string) {
  return /^(agent-status|account-restart|account-rotation-busy)-(dark|light)$/.test(stateName);
}

export const AGENT_STATUS_SESSION_IDS = {
  approval: MARKETING_PRO_SPLIT_SESSION_IDS.plan,
  working: MARKETING_PRO_SPLIT_SESSION_IDS.bypass,
  done: MARKETING_PRO_SPLIT_SESSION_IDS.codex,
  question: "agent-status-claude-question",
  finished: "agent-status-codex-finished",
  failed: "agent-status-grok-failed",
} as const;

const NOW = Date.parse(FIXED_VISUAL_NOW);

function extraTerminal(
  id: string,
  title: string,
  agentKind: TerminalSession["agentKind"],
  conversationId: string | null,
  pid: number,
): TerminalSession {
  return {
    id,
    title,
    projectId: 3,
    conversationId,
    projectName: "halv",
    cwd: "/Users/demo/programming/halv",
    shell: "/bin/zsh",
    pid,
    executionProfile: "full_access",
    agentKind,
    status: "running",
    cols: 100,
    rows: 30,
    startedAt: "2026-08-13T14:30:00.000Z",
  };
}

export const AGENT_STATUS_EXTRA_TERMINALS: TerminalSession[] = [
  extraTerminal(
    AGENT_STATUS_SESSION_IDS.question,
    "Claude 6",
    "claude",
    "audit-agent-refactor",
    4321,
  ),
  extraTerminal(
    AGENT_STATUS_SESSION_IDS.finished,
    "Codex 2",
    "codex",
    "audit-release",
    4322,
  ),
  extraTerminal(AGENT_STATUS_SESSION_IDS.failed, "Grok 1", "grok", null, 4323),
];

export const AGENT_STATUS_ENTRIES: AgentStatusEntry[] = [
  {
    sessionId: AGENT_STATUS_SESSION_IDS.approval,
    status: "needsApproval",
    reason: "permission",
    updatedAt: NOW - 45_000,
    unread: false,
    waitSeq: 2,
    turnSeq: 3,
    source: "presence",
    prompt: {
      kind: "approval",
      toolName: "Bash",
      command: "pnpm test --filter auth",
    },
  },
  {
    sessionId: AGENT_STATUS_SESSION_IDS.working,
    status: "working",
    updatedAt: NOW - 30_000,
    unread: false,
    waitSeq: 0,
    turnSeq: 2,
    source: "presence",
  },
  {
    sessionId: AGENT_STATUS_SESSION_IDS.done,
    status: "done",
    updatedAt: NOW - 90_000,
    unread: true,
    waitSeq: 0,
    turnSeq: 4,
    source: "rollout",
  },
  {
    sessionId: AGENT_STATUS_SESSION_IDS.question,
    status: "needsInput",
    reason: "question",
    updatedAt: NOW - 120_000,
    unread: false,
    waitSeq: 1,
    turnSeq: 1,
    source: "presence",
    prompt: {
      kind: "question",
      text: "Which session store should the middleware use?",
      options: ["Redis", "Postgres"],
    },
  },
  {
    sessionId: AGENT_STATUS_SESSION_IDS.finished,
    status: "done",
    updatedAt: NOW - 300_000,
    unread: true,
    waitSeq: 0,
    turnSeq: 2,
    source: "rollout",
  },
  {
    sessionId: AGENT_STATUS_SESSION_IDS.failed,
    status: "failed",
    updatedAt: NOW - 600_000,
    unread: true,
    waitSeq: 0,
    turnSeq: 1,
    source: "hook",
  },
];

export function prepareAgentStatusStorage(
  stateName: string,
  storage: Pick<Storage, "setItem">,
) {
  if (!isAgentStatusStateName(stateName)) return;
  storage.setItem(MARKETING_PRO_SPLIT_PRO_MODE_HINT_SEEN_STORAGE_KEY, "true");
}

export function prepareAgentStatusHarness(stateName: string) {
  if (!isAgentStatusStateName(stateName)) return;
  seedMarketingProSplitLayout();
}

const NOTIFICATION_CARD_ID = "harness-notification-card";

type NotificationCardWindow = Window & {
  /** The pane whose banners the card shows. A spec sets it. */
  __harnessNotificationCardFor?: string;
};

/**
 * Draws a terminal banner as a card in the top right corner. The OS banner
 * is outside the webview, so the harness shows a stand-in that the visual
 * specs can capture. The card shows only the banners of the pane that a
 * spec names, so other screenshots never include it.
 */
export function showHarnessNotificationCard(notification: {
  sessionId: string;
  title: string;
  body: string;
}) {
  const wanted = (window as NotificationCardWindow).__harnessNotificationCardFor;
  if (wanted !== notification.sessionId) return;
  let card = document.getElementById(NOTIFICATION_CARD_ID);
  if (!card) {
    card = document.createElement("div");
    card.id = NOTIFICATION_CARD_ID;
    card.setAttribute("role", "status");
    card.dataset.harnessNotificationCard = "";
    Object.assign(card.style, {
      position: "fixed",
      top: "12px",
      right: "12px",
      zIndex: "2147483647",
      width: "340px",
      padding: "10px 12px",
      borderRadius: "12px",
      border: "1px solid rgb(var(--surface-border) / 0.25)",
      background: "rgb(var(--surface-raised))",
      color: "rgb(var(--content-primary))",
      boxShadow: "0 8px 24px rgb(var(--shadow-color) / 0.25)",
      font: "12px/1.4 ui-sans-serif, system-ui, -apple-system, sans-serif",
      pointerEvents: "none",
    });
    document.body.append(card);
  }
  const title = document.createElement("div");
  title.dataset.harnessNotificationTitle = "";
  title.style.fontWeight = "600";
  title.textContent = notification.title;
  const body = document.createElement("div");
  body.dataset.harnessNotificationBody = "";
  body.style.color = "rgb(var(--content-secondary))";
  body.textContent = notification.body;
  card.replaceChildren(title, body);
}
