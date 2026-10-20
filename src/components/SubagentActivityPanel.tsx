import { useMemo, useState } from "react";
import { useT, type Translate } from "../i18n/runtime";
import {
  subagentActivities,
  type SubagentActivityStatus,
} from "../subagents";
import type { SubagentThread, ToolCallUi } from "../types";

type SubagentActivityPanelProps = {
  toolCalls: ToolCallUi[];
  threads?: SubagentThread[];
  onInterrupt?: (agentId: string) => void;
  onOpen?: (agentId: string) => void;
  onStopAll?: () => void;
  onSendMessage?: (agentId: string, message: string) => Promise<void>;
};

export function SubagentActivityPanel({
  toolCalls,
  threads = [],
  onInterrupt,
  onOpen,
  onStopAll,
  onSendMessage,
}: SubagentActivityPanelProps) {
  const t = useT();
  const agents = useMemo(
    () => subagentActivities(toolCalls, threads),
    [threads, toolCalls],
  );
  const [expandedAgentId, setExpandedAgentId] = useState<string | null>(null);
  const [messageDraft, setMessageDraft] = useState("");
  const [messagePending, setMessagePending] = useState(false);
  const [messageError, setMessageError] = useState<string | null>(null);
  const threadsById = useMemo(
    () => new Map(threads.map((thread) => [thread.agentId, thread])),
    [threads],
  );
  const activeCount = agents.filter((agent) => agent.active).length;

  if (agents.length === 0) {
    return null;
  }

  return (
    <section
      aria-label={t("Subagent activity")}
      className="overflow-hidden rounded-lg border border-surface-border/25 bg-surface-panel/55 shadow-sm"
      tabIndex={-1}
    >
      <div className="flex items-center justify-between gap-3 border-b border-surface-border/20 px-3 py-2">
        <div className="flex min-w-0 items-center gap-2">
          <span className="text-xs font-medium text-content-secondary">
            {t("Agents")}
          </span>
          <span className="rounded-full border border-surface-border/25 px-1.5 py-0.5 font-mono text-[10px] text-content-muted">
            {activeCount > 0
              ? t("{count} active", { count: activeCount })
              : t("{count} finished", { count: agents.length })}
          </span>
        </div>
        {activeCount > 0 && onStopAll ? (
          <button
            className="rounded-md border border-danger/35 px-2 py-1 text-[11px] text-danger transition-colors hover:bg-danger-surface/60"
            onClick={onStopAll}
            type="button"
          >
            {t("Stop all")}
          </button>
        ) : null}
      </div>
      <div className="divide-y divide-surface-border/15">
        {agents.map((agent) => {
          const expanded = expandedAgentId === agent.agentId;
          const thread = threadsById.get(agent.agentId);
          return (
          <article className="grid gap-2 px-3 py-2.5" key={agent.agentId}>
            <div className="flex min-w-0 items-center gap-2">
              <span
                aria-hidden
                className={`h-2 w-2 shrink-0 rounded-full ${
                  agent.active
                    ? "animate-pulse bg-warning"
                    : agent.failed
                      ? "bg-danger"
                      : "bg-accent"
                }`}
              />
              <code className="min-w-0 flex-1 truncate font-mono text-xs text-content-secondary">
                {agent.taskName}
              </code>
              <span className="shrink-0 font-mono text-[10px] text-content-muted">
                {subagentStatusLabel(agent.status, t)}
              </span>
            </div>
            {agent.task ? (
              <p className="line-clamp-2 text-xs leading-5 text-content-muted">{agent.task}</p>
            ) : null}
            {agent.output ? (
              <p className="line-clamp-2 whitespace-pre-wrap font-mono text-[11px] leading-4 text-content-muted">
                {agent.output}
              </p>
            ) : null}
            {onOpen || thread || (agent.active && onInterrupt) ? (
              <div className="flex items-center gap-2">
                {onOpen || thread ? (
                  <button
                    className="rounded-md border border-surface-border/30 px-2 py-1 text-[11px] text-content-secondary hover:bg-surface-raised"
                    onClick={() => {
                      onOpen?.(agent.agentId);
                      if (!onOpen) {
                        setExpandedAgentId(expanded ? null : agent.agentId);
                        setMessageDraft("");
                        setMessageError(null);
                      }
                    }}
                    type="button"
                  >
                    {expanded ? t("Close thread") : t("Open thread")}
                  </button>
                ) : null}
                {agent.active && onInterrupt ? (
                  <button
                    className="rounded-md border border-danger/35 px-2 py-1 text-[11px] text-danger hover:bg-danger-surface/60"
                    onClick={() => onInterrupt(agent.agentId)}
                    type="button"
                  >
                    {t("Stop")}
                  </button>
                ) : null}
              </div>
            ) : null}
            {expanded && thread ? (
              <div className="grid gap-2 rounded-md border border-surface-border/20 bg-surface-sunken/45 p-2.5">
                <div className="max-h-64 space-y-2 overflow-y-auto pr-1">
                  {thread.messages.map((message) => (
                    <div
                      className={`rounded-md px-2.5 py-2 text-xs leading-5 ${
                        message.role === "user"
                          ? "ml-6 bg-accent/10 text-content-secondary"
                          : message.role === "error"
                            ? "mr-6 bg-danger/10 text-danger"
                            : "mr-6 bg-surface-raised text-content-secondary"
                      }`}
                      key={message.id}
                    >
                      <div className="mb-1 font-mono text-[9px] uppercase tracking-[0.12em] text-content-muted">
                        {subagentMessageRunLabel(
                          message.role,
                          message.generation,
                          t,
                        )}
                      </div>
                      <div className="whitespace-pre-wrap break-words">{message.content}</div>
                    </div>
                  ))}
                </div>
                {agent.active && onSendMessage ? (
                  <form
                    className="flex gap-2"
                    onSubmit={(event) => {
                      event.preventDefault();
                      const message = messageDraft.trim();
                      if (!message || messagePending) {
                        return;
                      }
                      setMessagePending(true);
                      setMessageError(null);
                      void onSendMessage(agent.agentId, message)
                        .then(() => setMessageDraft(""))
                        .catch((error) =>
                          setMessageError(
                            error instanceof Error ? error.message : String(error),
                          ),
                        )
                        .finally(() => setMessagePending(false));
                    }}
                  >
                    <input
                      aria-label={t("Message {agent}", {
                        agent: agent.taskName,
                      })}
                      className="min-w-0 flex-1 rounded-md border border-surface-border/30 bg-surface-raised px-2.5 py-1.5 text-xs text-content-primary outline-none focus:border-accent"
                      disabled={messagePending}
                      onChange={(event) => setMessageDraft(event.target.value)}
                      placeholder={t("Send guidance…")}
                      value={messageDraft}
                    />
                    <button
                      className="rounded-md bg-accent px-2.5 py-1.5 text-[11px] font-semibold text-accent-contrast disabled:opacity-50"
                      disabled={messagePending || !messageDraft.trim()}
                      type="submit"
                    >
                      {t("Send")}
                    </button>
                  </form>
                ) : null}
                {messageError ? (
                  <p className="text-[11px] text-danger">{messageError}</p>
                ) : null}
              </div>
            ) : null}
          </article>
          );
        })}
      </div>
    </section>
  );
}

function subagentStatusLabel(
  status: SubagentActivityStatus,
  t: Translate,
) {
  switch (status) {
    case "pending":
      return t("Queued");
    case "running":
      return t("Running");
    case "completed":
      return t("Completed");
    case "errored":
      return t("Failed");
    case "interrupted":
      return t("Stopped");
    case "unknown":
      return t("Unknown");
  }
}

function subagentMessageRunLabel(
  role: SubagentThread["messages"][number]["role"],
  generation: number,
  t: Translate,
) {
  return role === "user"
    ? t("Coordinator · run {generation}", { generation })
    : t("Agent · run {generation}", { generation });
}
