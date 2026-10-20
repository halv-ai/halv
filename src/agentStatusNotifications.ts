import { agentPaneNeedsUser, type AgentPaneView } from "./agentPaneStatus.ts";
import type { NotificationKind } from "./notifications.ts";

/** Auto-approved prompts can flash. A wait must last this long. */
export const NEEDS_INPUT_NOTIFY_DELAY_MS = 1_000;
/** A user Stop hook can restart a finished turn at once. */
export const COMPLETION_NOTIFY_DELAY_MS = 1_500;

export type PaneNotification = {
  kind: NotificationKind;
  /** One banner for each key and pane. */
  key: string;
  delayMs: number;
};

function waitKey(view: AgentPaneView) {
  return `wait:${view.source}:${view.waitSeq}`;
}

function turnKey(view: AgentPaneView) {
  return `turn:${view.source}:${view.turnSeq}:${view.status}`;
}

function finishedUnread(view: AgentPaneView) {
  return view.unread && (view.status === "done" || view.status === "failed");
}

/**
 * The banner that a pane change can earn, or null for none. The byte-volume
 * fallback can report a false finish, so only an agent signal earns one.
 */
export function paneNotificationFor(
  previous: AgentPaneView | null,
  next: AgentPaneView | null,
): PaneNotification | null {
  if (!next || next.source === "heuristic") return null;
  if (agentPaneNeedsUser(next)) {
    if (previous && agentPaneNeedsUser(previous) && waitKey(previous) === waitKey(next)) {
      return null;
    }
    return {
      kind: next.status === "needsApproval" ? "permission" : "question",
      key: waitKey(next),
      delayMs: NEEDS_INPUT_NOTIFY_DELAY_MS,
    };
  }
  if (finishedUnread(next)) {
    if (previous && finishedUnread(previous) && turnKey(previous) === turnKey(next)) {
      return null;
    }
    return {
      kind: "completion",
      key: turnKey(next),
      delayMs: COMPLETION_NOTIFY_DELAY_MS,
    };
  }
  return null;
}

/** True while the pane still shows the state that the banner names. */
export function paneNotificationStillDue(
  notification: PaneNotification,
  current: AgentPaneView | null,
) {
  if (!current) return false;
  if (notification.kind === "completion") {
    return finishedUnread(current) && turnKey(current) === notification.key;
  }
  return agentPaneNeedsUser(current) && waitKey(current) === notification.key;
}

type Schedule = (callback: () => void, delayMs: number) => () => void;

const scheduleWithTimer: Schedule = (callback, delayMs) => {
  const timer = setTimeout(callback, delayMs);
  return () => clearTimeout(timer);
};

export type AgentStatusNotifierDependencies = {
  /** The current state of the pane when the delay ends. */
  current(sessionId: string): AgentPaneView | null;
  /** True when the user looks at the pane in a focused window. */
  paneViewed(sessionId: string): boolean;
  deliver(sessionId: string, kind: NotificationKind, view: AgentPaneView): void;
  schedule?: Schedule;
};

// Waits a moment after each change that can earn a banner, then sends it
// only if the pane still shows that state and the user does not see it.
export function createAgentStatusNotifier(
  dependencies: AgentStatusNotifierDependencies,
) {
  const schedule = dependencies.schedule ?? scheduleWithTimer;
  const pending = new Map<string, () => void>();
  const handled = new Map<string, Set<string>>();

  const cancel = (sessionId: string) => {
    pending.get(sessionId)?.();
    pending.delete(sessionId);
  };

  return {
    update(
      sessionId: string,
      view: AgentPaneView | null,
      previous: AgentPaneView | null,
    ) {
      const notification = paneNotificationFor(previous, view);
      if (!notification || handled.get(sessionId)?.has(notification.key)) {
        return;
      }
      cancel(sessionId);
      const cancelTimer = schedule(() => {
        pending.delete(sessionId);
        const current = dependencies.current(sessionId);
        if (!current || !paneNotificationStillDue(notification, current)) {
          return;
        }
        let keys = handled.get(sessionId);
        if (!keys) handled.set(sessionId, (keys = new Set()));
        keys.add(notification.key);
        if (dependencies.paneViewed(sessionId)) return;
        dependencies.deliver(sessionId, notification.kind, current);
      }, notification.delayMs);
      pending.set(sessionId, cancelTimer);
    },
    forget(sessionId: string) {
      cancel(sessionId);
      handled.delete(sessionId);
    },
    dispose() {
      for (const sessionId of [...pending.keys()]) cancel(sessionId);
      handled.clear();
    },
  };
}
