import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { copyText } from "../clipboard";
import { useT, type Translate } from "../i18n/runtime";
import type {
  ConversationCompaction,
  McpElicitationField,
  McpElicitationValue,
  TaskPlan,
  PatchFileChange,
  SubagentThread,
  ThreadNotice,
  ToolCallUi,
  TurnDiffSummary,
  UiMessage,
} from "../types";
import { isCompactionBoundaryMessage } from "../compaction";
import { ErrorRecoveryActions, MessageBubble } from "./MessageBubble";
import { SubagentActivityPanel } from "./SubagentActivityPanel";
import { isSubagentActivityToolCall } from "../subagents";
import {
  artifactBelongsToUserTurn,
  shouldShowTaskPlanForUserTurn,
} from "../turnArtifacts";
import {
  readTranscriptScrollState,
  writeTranscriptScrollState,
} from "../conversationWorkspace";

type EmptyStateContent = {
  title: string;
  description: string;
  showConnectChatGpt?: boolean;
  showSuggestions: boolean;
  showRestart: boolean;
};

type MessageListProps = {
  conversationId: string | null;
  compaction: ConversationCompaction | null;
  messages: UiMessage[];
  sessionBacked?: boolean;
  notices?: ThreadNotice[];
  toolCalls: ToolCallUi[];
  subagents: SubagentThread[];
  taskPlan: TaskPlan | null;
  pendingApproval: ToolCallUi | null;
  approvalDecisionPending: boolean;
  turnDiffs: TurnDiffSummary[];
  actionsDisabled: boolean;
  onRevertTurnDiff: (turnId: string) => void;
  onEditMessage: (message: UiMessage) => void;
  onForkMessage: (message: UiMessage) => void;
  onConnectChatGpt: () => void;
  onOpenSettings: () => void;
  onRegenerateMessage: (message: UiMessage) => void;
  onRunInNewWorktree?: () => Promise<void> | void;
  onRestartProxy: () => void;
  onApprovalDecision: (
    callId: string,
    decision: "approved" | "approvedForSession" | "denied",
    paths?: string[],
    elicitationContent?: Record<string, McpElicitationValue>,
  ) => void;
  onSuggestion: (text: string) => void;
  onInterruptSubagent: (agentId: string) => void;
  onInterruptAllSubagents: () => void;
  onSendSubagentMessage: (agentId: string, message: string) => Promise<void>;
  chatError: string | null;
  connectPending: boolean;
  restartPending: boolean;
  scrollToBottomSignal: number;
  streaming: boolean;
  turnStartedAt: number | null;
  emptyState: EmptyStateContent;
};

const EMPTY_TOOL_CALLS: ToolCallUi[] = [];
const EMPTY_SUBAGENTS: SubagentThread[] = [];
const EMPTY_TURN_DIFFS: TurnDiffSummary[] = [];

export function MessageList({
  conversationId,
  compaction,
  messages,
  sessionBacked = false,
  notices = [],
  toolCalls,
  subagents,
  taskPlan,
  pendingApproval,
  approvalDecisionPending,
  turnDiffs,
  actionsDisabled,
  onRevertTurnDiff,
  onEditMessage,
  onForkMessage,
  onConnectChatGpt,
  onOpenSettings,
  onRegenerateMessage,
  onRunInNewWorktree,
  onRestartProxy,
  onApprovalDecision,
  onSuggestion,
  onInterruptSubagent,
  onInterruptAllSubagents,
  onSendSubagentMessage,
  chatError,
  connectPending,
  restartPending,
  scrollToBottomSignal,
  streaming,
  turnStartedAt,
  emptyState,
}: MessageListProps) {
  const t = useT();
  const handleRevertTurnDiff = useStableCallback(onRevertTurnDiff);
  const handleEditMessage = useStableCallback(onEditMessage);
  const handleForkMessage = useStableCallback(onForkMessage);
  const handleOpenSettings = useStableCallback(onOpenSettings);
  const handleRegenerateMessage = useStableCallback(onRegenerateMessage);
  const handleRestartProxy = useStableCallback(onRestartProxy);
  const handleApprovalDecision = useStableCallback(onApprovalDecision);
  const handleInterruptSubagent = useStableCallback(onInterruptSubagent);
  const handleInterruptAllSubagents = useStableCallback(
    onInterruptAllSubagents,
  );
  const handleSendSubagentMessage = useStableCallback(onSendSubagentMessage);
  const scrollerRef = useRef<HTMLDivElement>(null);
  const pinnedToBottom = useRef(true);
  const persistFrameRef = useRef<number | null>(null);
  const scrollStateRef = useRef({ top: 0, pinnedToBottom: true });
  const [showJump, setShowJump] = useState(false);
  const stableToolCalls = toolCalls.length === 0 ? EMPTY_TOOL_CALLS : toolCalls;
  const stableSubagents = subagents.length === 0 ? EMPTY_SUBAGENTS : subagents;
  const stableTurnDiffs = turnDiffs.length === 0 ? EMPTY_TURN_DIFFS : turnDiffs;
  const showEmptyState =
    messages.length === 0 && notices.length === 0 && pendingApproval === null;
  const hasVisibleStreamingAssistant = messages.some(
    (message) =>
      message.role === "assistant" &&
      message.streaming &&
      message.content.trim().length > 0,
  );
  const latestToolCall = stableToolCalls[stableToolCalls.length - 1] ?? null;
  const latestToolRunning = latestToolCall?.status === "running";
  const showThinkingIndicator =
    streaming &&
    turnStartedAt !== null &&
    !chatError &&
    !hasVisibleStreamingAssistant &&
    !latestToolRunning;
  const lastMessage = messages[messages.length - 1] ?? null;
  const regenerableAssistantId =
    lastMessage?.role === "assistant" && !lastMessage.streaming && !streaming
      ? lastMessage.id
      : null;
  const laterMessageCountById = useMemo(
    () =>
      new Map(
        messages.map((message, index) => [
          message.id,
          Math.max(0, messages.length - index - 1),
        ]),
      ),
    [messages],
  );
  const groupedTurnDiffs = useMemo(
    () => groupTurnDiffs(stableTurnDiffs),
    [stableTurnDiffs],
  );
  const streamItems = useMemo(
    () =>
      [
        ...messages.map((message) => ({
          kind: "message" as const,
          createdAt: message.createdAt,
          message,
          order: 1,
        })),
        ...notices.map((notice) => ({
          kind: "notice" as const,
          createdAt: notice.createdAt,
          notice,
          order: 0,
        })),
      ].sort((left, right) => {
        const timeDelta =
          Date.parse(left.createdAt) - Date.parse(right.createdAt);
        return timeDelta === 0 ? left.order - right.order : timeDelta;
      }),
    [messages, notices],
  );
  const userMessageIdList = messages
    .filter((message) => message.role === "user")
    .map((message) => message.id);
  const userMessageIdSignature = JSON.stringify(userMessageIdList);
  const userMessageIds = useMemo(
    () => new Set<string>(JSON.parse(userMessageIdSignature)),
    [userMessageIdSignature],
  );
  const latestUserMessageId = useMemo(() => {
    for (let index = streamItems.length - 1; index >= 0; index -= 1) {
      const item = streamItems[index];
      if (item.kind === "message" && item.message.role === "user") {
        return item.message.id;
      }
    }
    return null;
  }, [streamItems]);

  function scrollToBottom(behavior: ScrollBehavior = "smooth") {
    const scroller = scrollerRef.current;
    if (!scroller) {
      return;
    }
    scroller.scrollTo({
      top: scroller.scrollHeight,
      behavior,
    });
  }

  function updatePinnedState() {
    const scroller = scrollerRef.current;
    if (!scroller) {
      return;
    }
    const distanceFromBottom =
      scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
    const nextPinned = distanceFromBottom < 96;
    pinnedToBottom.current = nextPinned;
    scrollStateRef.current = {
      top: scroller.scrollTop,
      pinnedToBottom: nextPinned,
    };
    setShowJump(!nextPinned && streaming);
    if (conversationId && persistFrameRef.current === null) {
      persistFrameRef.current = window.requestAnimationFrame(() => {
        persistFrameRef.current = null;
        const currentScroller = scrollerRef.current;
        if (!currentScroller) return;
        writeTranscriptScrollState(
          conversationId,
          scrollStateRef.current,
          window.localStorage,
        );
      });
    }
  }

  useEffect(() => {
    const saved = conversationId
      ? readTranscriptScrollState(conversationId, window.localStorage)
      : null;
    pinnedToBottom.current = saved?.pinnedToBottom ?? true;
    scrollStateRef.current = saved ?? { top: 0, pinnedToBottom: true };
    setShowJump(false);
    const restoreFrame = window.requestAnimationFrame(() => {
      const scroller = scrollerRef.current;
      if (!scroller) return;
      scroller.scrollTo({
        top: saved?.top ?? scroller.scrollHeight,
        behavior: "auto",
      });
      updatePinnedState();
    });
    return () => {
      window.cancelAnimationFrame(restoreFrame);
      if (persistFrameRef.current !== null) {
        window.cancelAnimationFrame(persistFrameRef.current);
        persistFrameRef.current = null;
      }
      if (!conversationId) return;
      writeTranscriptScrollState(
        conversationId,
        scrollStateRef.current,
        window.localStorage,
      );
    };
  }, [conversationId]);

  useEffect(() => {
    if (pinnedToBottom.current) {
      window.requestAnimationFrame(() => scrollToBottom("smooth"));
    }
  }, [
    streamItems,
    taskPlan,
    chatError,
    turnDiffs,
    pendingApproval?.callId,
    showThinkingIndicator,
  ]);

  useEffect(() => {
    setShowJump(streaming && !pinnedToBottom.current);
  }, [streaming]);

  useEffect(() => {
    if (scrollToBottomSignal === 0) {
      return;
    }
    pinnedToBottom.current = true;
    setShowJump(false);
    window.requestAnimationFrame(() => scrollToBottom("smooth"));
  }, [scrollToBottomSignal]);

  return (
    <div
      className="relative min-h-0 flex-1 overflow-y-auto px-6 py-5"
      onScroll={updatePinnedState}
      ref={scrollerRef}
    >
      <div
        className={`mx-auto flex max-w-4xl flex-col gap-2.5 ${
          showEmptyState ? "min-h-full justify-center" : ""
        }`}
      >
        {showEmptyState ? (
          <EmptyThreadState
            connectPending={connectPending}
            emptyState={emptyState}
            onConnectChatGpt={onConnectChatGpt}
            onRestart={onRestartProxy}
            onSuggestion={onSuggestion}
            restartPending={restartPending}
          />
        ) : (
          streamItems.map((item) =>
            item.kind === "message" ? (
              <MessageRow
                actionsDisabled={actionsDisabled}
                approvalDecisionPending={approvalDecisionPending}
                canRegenerate={item.message.id === regenerableAssistantId}
                compaction={compaction}
                groupedTurnDiffs={groupedTurnDiffs}
                key={`stream-message-${item.message.id}`}
                laterMessageCount={
                  laterMessageCountById.get(item.message.id) ?? 0
                }
                latestUserMessageId={latestUserMessageId}
                message={item.message}
                onApprovalDecision={handleApprovalDecision}
                onEditMessage={handleEditMessage}
                onForkMessage={handleForkMessage}
                onInterruptAllSubagents={handleInterruptAllSubagents}
                onInterruptSubagent={handleInterruptSubagent}
                onOpenSettings={handleOpenSettings}
                onRegenerateMessage={handleRegenerateMessage}
                onRestartProxy={handleRestartProxy}
                onRevertTurnDiff={handleRevertTurnDiff}
                onSendSubagentMessage={handleSendSubagentMessage}
                pendingApproval={pendingApproval}
                restartPending={restartPending}
                sessionBacked={sessionBacked}
                showThinkingIndicator={showThinkingIndicator}
                streaming={streaming}
                subagents={stableSubagents}
                taskPlan={taskPlan}
                toolCalls={stableToolCalls}
                turnStartedAt={turnStartedAt}
                userMessageIds={userMessageIds}
              />
            ) : (
              <ThreadNoticePill
                key={`stream-notice-${item.notice.id}`}
                notice={item.notice}
              />
            ),
          )
        )}

        {chatError ? (
          <div className="rounded-lg border border-danger/40 bg-danger-surface/55 p-4 text-sm text-danger">
            {chatError}
            <ErrorRecoveryActions
              error={chatError}
              onOpenSettings={onOpenSettings}
              onRunInNewWorktree={onRunInNewWorktree}
              onRestartProxy={onRestartProxy}
              restartPending={restartPending}
            />
          </div>
        ) : null}
      </div>

      {showJump ? (
        <button
          className="sticky bottom-4 left-1/2 mt-4 -translate-x-1/2 rounded-full border border-surface-border/30 bg-surface-raised/95 px-3 py-2 text-sm text-content-primary shadow-lg shadow-[rgb(var(--shadow-color)/0.18)] backdrop-blur-xl transition-colors duration-150 hover:border-accent/60 hover:bg-surface-panel focus:outline-none focus:ring-2 focus:ring-accent/30"
          onClick={() => {
            pinnedToBottom.current = true;
            setShowJump(false);
            scrollToBottom();
          }}
          type="button"
        >
          {t("Scroll to bottom")}
        </button>
      ) : null}
    </div>
  );
}

type MessageRowProps = {
  actionsDisabled: boolean;
  approvalDecisionPending: boolean;
  canRegenerate: boolean;
  compaction: ConversationCompaction | null;
  groupedTurnDiffs: GroupedTurnDiff[];
  laterMessageCount: number;
  latestUserMessageId: string | null;
  message: UiMessage;
  onApprovalDecision: MessageListProps["onApprovalDecision"];
  onEditMessage: MessageListProps["onEditMessage"];
  onForkMessage: MessageListProps["onForkMessage"];
  onInterruptAllSubagents: MessageListProps["onInterruptAllSubagents"];
  onInterruptSubagent: MessageListProps["onInterruptSubagent"];
  onOpenSettings: MessageListProps["onOpenSettings"];
  onRegenerateMessage: MessageListProps["onRegenerateMessage"];
  onRestartProxy: MessageListProps["onRestartProxy"];
  onRevertTurnDiff: MessageListProps["onRevertTurnDiff"];
  onSendSubagentMessage: MessageListProps["onSendSubagentMessage"];
  pendingApproval: ToolCallUi | null;
  restartPending: boolean;
  sessionBacked: boolean;
  showThinkingIndicator: boolean;
  streaming: boolean;
  subagents: SubagentThread[];
  taskPlan: TaskPlan | null;
  toolCalls: ToolCallUi[];
  turnStartedAt: number | null;
  userMessageIds: Set<string>;
};

const MessageRow = memo(function MessageRow({
  actionsDisabled,
  approvalDecisionPending,
  canRegenerate,
  compaction,
  groupedTurnDiffs,
  laterMessageCount,
  latestUserMessageId,
  message,
  onApprovalDecision,
  onEditMessage,
  onForkMessage,
  onInterruptAllSubagents,
  onInterruptSubagent,
  onOpenSettings,
  onRegenerateMessage,
  onRestartProxy,
  onRevertTurnDiff,
  onSendSubagentMessage,
  pendingApproval,
  restartPending,
  sessionBacked,
  showThinkingIndicator,
  streaming,
  subagents,
  taskPlan,
  toolCalls,
  turnStartedAt,
  userMessageIds,
}: MessageRowProps) {
  const isUser = message.role === "user";
  const isCurrentTurn = message.id === latestUserMessageId;
  const diffsForTurn = useMemo(
    () =>
      isUser
        ? groupedTurnDiffs.filter((turnDiff) =>
            artifactBelongsToUserTurn(
              turnDiff.turnId,
              turnDiff.createdAt,
              message.id,
              message.createdAt,
              latestUserMessageId,
              streaming,
              userMessageIds,
            ),
          )
        : [],
    [
      groupedTurnDiffs,
      isUser,
      latestUserMessageId,
      message.createdAt,
      message.id,
      streaming,
      userMessageIds,
    ],
  );
  const subagentsForTurn = useMemo(
    () =>
      isUser
        ? subagents.filter((thread) =>
            artifactBelongsToUserTurn(
              thread.turnId,
              thread.createdAt,
              message.id,
              message.createdAt,
              latestUserMessageId,
              streaming,
              userMessageIds,
            ),
          )
        : [],
    [
      isUser,
      latestUserMessageId,
      message.createdAt,
      message.id,
      streaming,
      subagents,
      userMessageIds,
    ],
  );
  const showTaskPlan =
    isUser &&
    shouldShowTaskPlanForUserTurn(
      message.id,
      latestUserMessageId,
      streaming,
      taskPlan !== null,
    );

  return (
    <>
      <MessageBubble
        actionsDisabled={actionsDisabled}
        canRegenerate={canRegenerate}
        laterMessageCount={laterMessageCount}
        message={message}
        sessionBacked={sessionBacked}
        onEdit={onEditMessage}
        onFork={onForkMessage}
        onOpenSettings={onOpenSettings}
        onRegenerate={onRegenerateMessage}
        onRestartProxy={onRestartProxy}
        restartPending={restartPending}
      />

      {isUser ? (
        <>
          {showTaskPlan && taskPlan ? (
            <TaskPlanCard active taskPlan={taskPlan} />
          ) : null}

          <SubagentActivityPanel
            onInterrupt={onInterruptSubagent}
            onSendMessage={onSendSubagentMessage}
            onStopAll={onInterruptAllSubagents}
            threads={subagentsForTurn}
            toolCalls={isCurrentTurn ? toolCalls : EMPTY_TOOL_CALLS}
          />

          {isCurrentTurn ? (
            <ToolCallGroup streaming={streaming} toolCalls={toolCalls} />
          ) : null}

          {isCurrentTurn && showThinkingIndicator ? (
            <ThinkingIndicator
              label="Thinking"
              turnStartedAt={turnStartedAt}
            />
          ) : null}

          {diffsForTurn.map((turnDiff) => (
            <TurnDiffGroup
              disabled={actionsDisabled}
              key={turnDiff.turnId}
              onRevert={() => onRevertTurnDiff(turnDiff.turnId)}
              turnDiff={turnDiff}
            />
          ))}

          {isCurrentTurn && pendingApproval ? (
            <ApprovalCard
              approval={pendingApproval}
              decisionPending={approvalDecisionPending}
              onDecision={onApprovalDecision}
            />
          ) : null}
        </>
      ) : null}

      {isCompactionBoundaryMessage(message, compaction) ? (
        <CompactionDivider />
      ) : null}
    </>
  );
});

function useStableCallback<Arguments extends unknown[], Result>(
  callback: (...args: Arguments) => Result,
) {
  const callbackRef = useRef(callback);
  useLayoutEffect(() => {
    callbackRef.current = callback;
  }, [callback]);
  return useCallback(
    (...args: Arguments) => callbackRef.current(...args),
    [],
  );
}

function CompactionDivider() {
  const t = useT();
  return (
    <div
      aria-label={t("Context compacted")}
      className="my-1 flex items-center gap-3 text-content-muted"
      role="separator"
    >
      <span className="h-px flex-1 bg-surface-border/30" />
      <span className="font-mono text-[10px]">
        {t("Context compacted, older messages summarized for the model")}
      </span>
      <span className="h-px flex-1 bg-surface-border/30" />
    </div>
  );
}

function TaskPlanCard({
  active,
  taskPlan,
}: {
  active: boolean;
  taskPlan: TaskPlan;
}) {
  const t = useT();
  const completed = taskPlan.plan.filter(
    (item) => item.status === "completed",
  ).length;
  return (
    <div
      className="sticky top-0 z-10 -mx-1 bg-surface-base/95 px-1 py-1 backdrop-blur-xl"
      data-task-plan-active={active ? "true" : "false"}
    >
      <section
        aria-label={t("Task progress")}
        className="overflow-hidden rounded-lg border border-surface-border/25 bg-surface-panel/45"
      >
        <div className="flex items-center justify-between gap-3 border-b border-surface-border/20 px-3 py-2">
          <span className="text-xs font-medium text-content-secondary">
            {t("Tasks")}
          </span>
          <span className="font-mono text-[10px] text-content-muted">
            {t("{completed}/{total}", {
              completed,
              total: taskPlan.plan.length,
            })}
          </span>
        </div>
        {taskPlan.explanation ? (
          <p className="px-3 pt-2 text-xs text-content-muted">
            {taskPlan.explanation}
          </p>
        ) : null}
        <ol className="space-y-1 px-3 py-2.5">
          {taskPlan.plan.map((item, index) => (
            <li className="flex items-start gap-2 text-xs" key={`${index}-${item.step}`}>
              <span
                aria-hidden
                className={`mt-0.5 w-3.5 shrink-0 text-center font-mono ${
                  item.status === "completed"
                    ? "text-accent"
                    : item.status === "in_progress"
                      ? "text-warning"
                      : "text-content-muted"
                }`}
              >
                {item.status === "completed"
                  ? "✓"
                  : item.status === "in_progress"
                    ? "•"
                    : "○"}
              </span>
              <span
                className={
                  item.status === "completed"
                    ? "text-content-muted line-through"
                    : "text-content-secondary"
                }
              >
                {item.step}
              </span>
            </li>
          ))}
        </ol>
      </section>
    </div>
  );
}

function EmptyThreadState({
  connectPending,
  emptyState,
  onConnectChatGpt,
  onRestart,
  onSuggestion,
  restartPending,
}: {
  connectPending: boolean;
  emptyState: EmptyStateContent;
  onConnectChatGpt: () => void;
  onRestart: () => void;
  onSuggestion: (text: string) => void;
  restartPending: boolean;
}) {
  const t = useT();
  const suggestions = [
    { label: t("Explain this repo"), value: "Explain this repo" },
    {
      label: t("Ship the auth refactor"),
      value: "/goal ship the auth refactor",
    },
    { label: t("Fix the failing test"), value: "Fix the failing test" },
  ];

  return (
    <div className="flex flex-col items-center text-center">
      <div className="text-[56px] font-semibold leading-none text-accent opacity-90">
        {"\u00bd"}
      </div>
      <h2 className="mt-5 text-lg font-semibold text-content-primary">
        {emptyState.title}
      </h2>
      <p className="mt-2 max-w-md text-sm leading-6 text-content-secondary">
        {emptyState.description}
      </p>
      {emptyState.showConnectChatGpt ? (
        <button
          className="mt-4 rounded-md bg-accent px-3 py-2 text-sm font-semibold text-accent-contrast transition-colors duration-150 hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40 disabled:cursor-not-allowed disabled:opacity-60"
          disabled={connectPending}
          onClick={onConnectChatGpt}
          type="button"
        >
          {connectPending
            ? t("Waiting for ChatGPT...")
            : t("Connect ChatGPT")}
        </button>
      ) : null}
      {emptyState.showRestart ? (
        <button
          className="mt-4 rounded-md border border-danger/35 bg-danger/10 px-3 py-2 text-sm font-medium text-danger transition-colors duration-150 hover:bg-danger/15 focus:outline-none focus:ring-2 focus:ring-danger/30 disabled:cursor-not-allowed disabled:opacity-45"
          disabled={restartPending}
          onClick={onRestart}
          type="button"
        >
          {restartPending ? t("Restarting...") : t("Restart")}
        </button>
      ) : null}
      {emptyState.showSuggestions ? (
        <div className="mt-5 flex flex-wrap justify-center gap-2">
          {suggestions.map((suggestion) => (
            <button
              className="rounded-full border border-surface-border/30 px-4 py-2 text-sm text-content-secondary transition-colors hover:border-accent/45 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
              key={suggestion.label}
              onClick={() => onSuggestion(suggestion.value)}
              type="button"
            >
              {suggestion.label}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

const ThreadNoticePill = memo(function ThreadNoticePill({
  notice,
}: {
  notice: ThreadNotice;
}) {
  const t = useT();
  const [copied, setCopied] = useState(false);
  const copyResetTimer = useRef<number | undefined>(undefined);

  useEffect(() => {
    // Clear the reset timer on unmount.
    return () => window.clearTimeout(copyResetTimer.current);
  }, []);

  async function copyNotice() {
    if (await copyText(notice.text)) {
      setCopied(true);
      window.clearTimeout(copyResetTimer.current);
      copyResetTimer.current = window.setTimeout(() => setCopied(false), 1200);
    }
  }

  if (notice.copyable) {
    return (
      <div className="flex justify-center">
        <div className="w-full max-w-2xl rounded-lg border border-accent/30 bg-accent/5 px-3 py-2 font-mono text-[11px] text-accent">
          <div className="flex items-start gap-3">
            <pre className="min-w-0 flex-1 whitespace-pre-wrap break-words font-mono">
              {"\u2691"} {notice.text}
            </pre>
            <button
              className="shrink-0 rounded-full border border-surface-border/30 px-2 py-0.5 text-[11px] text-content-muted transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
              onClick={() => void copyNotice()}
              type="button"
            >
              {copied ? t("Copied") : t("Copy")}
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="flex justify-center">
      <span className="rounded-full border border-accent/30 bg-accent/5 px-3 py-1 font-mono text-[11px] text-accent">
        {"\u2691"} {notice.text}
      </span>
    </div>
  );
});

function ThinkingIndicator({
  label,
  turnStartedAt,
}: {
  label: "Thinking" | "Reasoning";
  turnStartedAt: number | null;
}) {
  const t = useT();
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    setNow(Date.now());
    const interval = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(interval);
  }, [turnStartedAt]);

  const elapsedSeconds =
    turnStartedAt === null
      ? 0
      : Math.max(0, Math.floor((now - turnStartedAt) / 1000));

  return (
    <div className="flex justify-start">
      <div className="inline-flex items-center gap-2">
        <span className="thinking-dot" aria-hidden />
        <span className="font-mono text-[11px] text-content-muted">
          {label === "Thinking"
            ? t("Thinking · {seconds}s", { seconds: elapsedSeconds })
            : t("Reasoning · {seconds}s", { seconds: elapsedSeconds })}
        </span>
      </div>
    </div>
  );
}

function ApprovalCard({
  approval,
  decisionPending,
  onDecision,
}: {
  approval: ToolCallUi;
  decisionPending: boolean;
  onDecision: (
    callId: string,
    decision: "approved" | "approvedForSession" | "denied",
    paths?: string[],
    elicitationContent?: Record<string, McpElicitationValue>,
  ) => void;
}) {
  const t = useT();
  const [selectedPaths, setSelectedPaths] = useState<string[]>([]);
  const [approveForSession, setApproveForSession] = useState(false);
  const [elicitationValues, setElicitationValues] = useState<
    Record<string, McpElicitationValue>
  >({});
  const isPatch = approval.toolKind === "apply_patch";
  const isMcp = approval.toolKind === "mcp";
  const isElicitation = approval.toolKind === "mcp_elicitation";
  const isBrowser = approval.toolKind === "browser";
  const requiredChanges = (approval.fileChanges ?? []).filter(
    (change) => change.requiresApproval && !change.approved,
  );
  const approveDisabled = isPatch
    ? requiredChanges.length > 0 && selectedPaths.length < requiredChanges.length
    : isElicitation && approval.elicitationMode === "form"
      ? !elicitationFormIsComplete(approval, elicitationValues)
      : false;

  useEffect(() => {
    setApproveForSession(false);
    setSelectedPaths(
      (approval.fileChanges ?? [])
        .filter((change) => change.requiresApproval && !change.approved)
        .map((change) => change.path),
    );
    setElicitationValues(initialElicitationValues(approval));
  }, [approval.callId, approval.fileChanges, approval.elicitationSchema]);

  return (
    <section
      className="rounded-xl border border-warning/35 bg-warning-surface/35 px-4 py-3"
      data-approval-card="current"
    >
      <div className="flex flex-wrap items-start gap-3">
        <div className="min-w-0 flex-1">
          <h3 className="text-sm font-semibold text-content-primary">
            {isPatch
              ? t("Wants to edit files")
              : isElicitation
                ? approval.elicitationMode === "url"
                  ? t("Needs to open an external page")
                  : t("Needs information to continue")
              : isMcp
                ? t("Wants to call an external tool")
                : isBrowser
                  ? t("Wants to control the browser")
                  : t("Wants to run a command")}
          </h3>
          <code className="mt-1 block truncate font-mono text-xs text-content-secondary" title={approval.command}>
            {approval.command}
          </code>
          {approval.cwd ? (
            <span className="mt-0.5 block truncate font-mono text-[10px] text-content-muted">
              {approval.cwd}
            </span>
          ) : null}
        </div>
        {!isMcp && !isElicitation ? (
          <label className="flex shrink-0 items-center gap-1.5 self-center text-xs text-content-muted">
            <input
              checked={approveForSession}
              onChange={(event) => setApproveForSession(event.target.checked)}
              type="checkbox"
            />
            {t("This session")}
          </label>
        ) : null}
        <button
          className="shrink-0 rounded-lg border border-surface-border/30 px-4 py-2 text-sm text-content-secondary transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:opacity-50"
          data-approval-action="deny"
          disabled={decisionPending}
          onClick={() =>
            onDecision(
              approval.callId,
              "denied",
              undefined,
              isElicitation ? elicitationValues : undefined,
            )
          }
          type="button"
        >
          {t("Deny")}
        </button>
        <button
          className="shrink-0 rounded-lg bg-accent px-4 py-2 text-sm font-semibold text-accent-contrast transition-colors hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40 disabled:opacity-60"
          data-approval-action="approve"
          disabled={decisionPending || approveDisabled}
          onClick={() =>
            onDecision(
              approval.callId,
              approveForSession && !isMcp && !isElicitation
                ? "approvedForSession"
                : "approved",
              selectedPaths,
              isElicitation
                ? elicitationSubmissionValues(approval, elicitationValues)
                : undefined,
            )
          }
          type="button"
        >
          {isElicitation && approval.elicitationMode === "url"
            ? t("Open & continue")
            : isElicitation
              ? t("Submit")
              : t("Allow")}
        </button>
      </div>

      <div className="mt-3 space-y-2 text-sm text-content-secondary">
        {isElicitation && approval.elicitationMode === "form" ? (
          <McpElicitationForm
            approval={approval}
            values={elicitationValues}
            onChange={(name, value) =>
              setElicitationValues((current) => ({ ...current, [name]: value }))
            }
          />
        ) : null}

        {isElicitation && approval.elicitationMode === "url" && approval.elicitationUrl ? (
          <div className="rounded-lg border border-warning/30 bg-surface-raised/45 px-3 py-2">
            <span className="mb-1 block text-xs text-content-muted">
              {t("Review the complete destination before opening it:")}
            </span>
            <code className="block break-all font-mono text-xs text-content-primary">
              {approval.elicitationUrl}
            </code>
          </div>
        ) : null}

        {isPatch ? (
          <div className="max-h-48 space-y-1 overflow-auto rounded-lg border border-surface-border/20 bg-surface-raised/45 p-2">
            {(approval.fileChanges ?? []).map((change) => (
              <label
                className="flex items-start gap-2 rounded-md px-2 py-1.5 text-xs text-content-secondary"
                key={`${change.changeType}-${change.path}`}
              >
                <input
                  checked={
                    !change.requiresApproval ||
                    change.approved ||
                    selectedPaths.includes(change.path)
                  }
                  className="mt-0.5"
                  disabled={!change.requiresApproval || change.approved}
                  onChange={(event) => {
                    setSelectedPaths((current) =>
                      event.target.checked
                        ? [...new Set([...current, change.path])]
                        : current.filter((path) => path !== change.path),
                    );
                  }}
                  type="checkbox"
                />
                <span className="min-w-0 flex-1">
                  <span className="mr-2 rounded-full border border-surface-border/25 px-1.5 py-0.5 font-mono text-[10px]">
                    {fileChangeTypeLabel(change.changeType, t)}
                  </span>
                  <code className="break-all font-mono">{change.path}</code>
                </span>
              </label>
            ))}
          </div>
        ) : null}

        {approval.justification ? (
          <div className="rounded-lg border border-surface-border/20 bg-surface-raised/45 px-3 py-2 text-content-secondary">
            {approval.justification}
          </div>
        ) : null}

        {approval.warning ? (
          <div className="rounded-lg border border-warning/30 bg-warning-surface/45 px-3 py-2 text-warning">
            {approval.warning}
          </div>
        ) : null}
      </div>

      <div className="mt-2 text-right font-mono text-[10px] text-content-muted">
        {t("\u23ce allow \u00b7 \u232b deny - when not typing")}
      </div>
    </section>
  );
}

function initialElicitationValues(approval: ToolCallUi) {
  return Object.fromEntries(
    Object.entries(approval.elicitationSchema?.properties ?? {}).map(
      ([name, field]) => [name, initialElicitationValue(field)],
    ),
  ) as Record<string, McpElicitationValue>;
}

function initialElicitationValue(field: McpElicitationField): McpElicitationValue {
  if (field.default !== undefined) {
    return field.default;
  }
  if (field.type === "boolean") {
    return false;
  }
  if (field.type === "array") {
    return [];
  }
  return "";
}

function elicitationFormIsComplete(
  approval: ToolCallUi,
  values: Record<string, McpElicitationValue>,
) {
  return (approval.elicitationSchema?.required ?? []).every((name) => {
    const field = approval.elicitationSchema?.properties[name];
    const value = values[name];
    if (!field || value === undefined) {
      return false;
    }
    if (field.type === "string") {
      return typeof value === "string" && value.trim().length > 0;
    }
    if (field.type === "number" || field.type === "integer") {
      return typeof value === "number" && Number.isFinite(value);
    }
    if (field.type === "array") {
      return Array.isArray(value) && value.length > 0;
    }
    return typeof value === "boolean";
  });
}

function elicitationSubmissionValues(
  approval: ToolCallUi,
  values: Record<string, McpElicitationValue>,
) {
  const output: Record<string, McpElicitationValue> = {};
  for (const [name, field] of Object.entries(
    approval.elicitationSchema?.properties ?? {},
  )) {
    const value = values[name];
    if (value === undefined) {
      continue;
    }
    if (
      (field.type === "number" || field.type === "integer") &&
      typeof value !== "number"
    ) {
      continue;
    }
    output[name] = value;
  }
  return output;
}

function McpElicitationForm({
  approval,
  values,
  onChange,
}: {
  approval: ToolCallUi;
  values: Record<string, McpElicitationValue>;
  onChange: (name: string, value: McpElicitationValue) => void;
}) {
  const t = useT();
  const required = new Set(approval.elicitationSchema?.required ?? []);
  const fields = Object.entries(approval.elicitationSchema?.properties ?? {});
  if (fields.length === 0) {
    return (
      <div className="rounded-lg border border-surface-border/20 bg-surface-raised/45 px-3 py-2 text-xs text-content-secondary">
        {t(
          "This server only needs your confirmation; no information will be sent.",
        )}
      </div>
    );
  }
  return (
    <div className="max-h-72 space-y-3 overflow-auto rounded-lg border border-surface-border/20 bg-surface-raised/45 p-3">
      {fields.map(([name, field]) => (
        <McpElicitationFieldInput
          field={field}
          key={name}
          name={name}
          onChange={(value) => onChange(name, value)}
          required={required.has(name)}
          value={values[name] ?? initialElicitationValue(field)}
        />
      ))}
    </div>
  );
}

function McpElicitationFieldInput({
  field,
  name,
  onChange,
  required,
  value,
}: {
  field: McpElicitationField;
  name: string;
  onChange: (value: McpElicitationValue) => void;
  required: boolean;
  value: McpElicitationValue;
}) {
  const t = useT();
  const label = field.title?.trim() || name;
  const inputClass =
    "mt-1 w-full rounded-md border border-surface-border/30 bg-surface-base px-2.5 py-2 text-sm text-content-primary outline-none focus:border-accent focus:ring-2 focus:ring-accent/25";
  return (
    <label className="block text-xs text-content-secondary">
      <span className="font-medium text-content-primary">
        {label}
        {required ? <span className="ml-1 text-warning">*</span> : null}
      </span>
      {field.description ? (
        <span className="mt-0.5 block text-[11px] text-content-muted">
          {field.description}
        </span>
      ) : null}
      {field.type === "boolean" ? (
        <span className="mt-2 flex items-center gap-2">
          <input
            checked={value === true}
            onChange={(event) => onChange(event.target.checked)}
            type="checkbox"
          />
          <span>{value === true ? t("Yes") : t("No")}</span>
        </span>
      ) : field.type === "array" && field.items?.enum ? (
        <span className="mt-2 grid gap-1.5">
          {field.items.enum.map((option) => {
            const selected = Array.isArray(value) && value.includes(option);
            return (
              <span className="flex items-center gap-2" key={option}>
                <input
                  checked={selected}
                  onChange={(event) => {
                    const current = Array.isArray(value) ? value : [];
                    onChange(
                      event.target.checked
                        ? [...new Set([...current, option])]
                        : current.filter((item) => item !== option),
                    );
                  }}
                  type="checkbox"
                />
                {option}
              </span>
            );
          })}
        </span>
      ) : field.enum?.length ? (
        <select
          className={inputClass}
          onChange={(event) => onChange(JSON.parse(event.target.value))}
          required={required}
          value={JSON.stringify(value)}
        >
          <option value={JSON.stringify("")}>{t("Select…")}</option>
          {field.enum.map((option) => (
            <option key={JSON.stringify(option)} value={JSON.stringify(option)}>
              {String(option)}
            </option>
          ))}
        </select>
      ) : field.type === "number" || field.type === "integer" ? (
        <input
          className={inputClass}
          max={field.maximum}
          min={field.minimum}
          onChange={(event) =>
            onChange(event.target.value === "" ? "" : Number(event.target.value))
          }
          required={required}
          step={field.type === "integer" ? 1 : "any"}
          type="number"
          value={typeof value === "number" ? value : ""}
        />
      ) : field.type === "array" ? (
        <input
          className={inputClass}
          onChange={(event) =>
            onChange(
              event.target.value
                .split(",")
                .map((item) => item.trim())
                .filter(Boolean),
            )
          }
          placeholder={t("Separate values with commas")}
          required={required}
          type="text"
          value={Array.isArray(value) ? value.join(", ") : ""}
        />
      ) : (
        <input
          className={inputClass}
          maxLength={field.maxLength}
          minLength={field.minLength}
          onChange={(event) => onChange(event.target.value)}
          required={required}
          type={field.format === "email" ? "email" : field.format === "uri" ? "url" : "text"}
          value={typeof value === "string" ? value : ""}
        />
      )}
    </label>
  );
}

function ToolCallGroup({
  streaming: _streaming,
  toolCalls,
}: {
  streaming: boolean;
  toolCalls: ToolCallUi[];
}) {
  const visibleToolCalls = toolCalls.filter(
    (toolCall) => !isSubagentActivityToolCall(toolCall),
  );
  if (visibleToolCalls.length === 0) {
    return null;
  }
  return (
    <div className="space-y-2">
      {visibleToolCalls.map((toolCall) => (
        <ToolCallCard key={toolCall.callId} toolCall={toolCall} />
      ))}
    </div>
  );
}

function fileChangeTypeLabel(
  changeType: PatchFileChange["changeType"],
  t: Translate,
) {
  switch (changeType) {
    case "add":
      return t("add");
    case "delete":
      return t("delete");
    case "update":
      return t("update");
    case "move_source":
      return t("move_source");
    case "move_target":
      return t("move_target");
  }
}

function toolCallFailed(toolCall: ToolCallUi) {
  const status = toolCall.status.toLowerCase();
  const failedStatus = ["failed", "error", "denied", "cancelled", "interrupted"].some((value) =>
    status.includes(value),
  );
  return failedStatus || (toolCall.exitCode != null && toolCall.exitCode !== 0);
}

function ToolCallCard({ toolCall }: { toolCall: ToolCallUi }) {
  const t = useT();
  const failed = toolCallFailed(toolCall);
  const status = toolCall.status.toLowerCase();
  const running =
    status === "running" || status === "pending" || status.includes("awaiting");
  const wallTime =
    toolCall.wallTimeMs == null
      ? null
      : t("{duration}s", {
          duration: (toolCall.wallTimeMs / 1000).toFixed(2),
        });
  return (
    <details className="group/tool overflow-hidden rounded-lg border border-surface-border/20 bg-surface-panel/35 text-sm">
      <summary className="flex cursor-pointer list-none items-center gap-2 px-3 py-2 text-content-secondary transition-colors hover:bg-surface-raised/55">
        <span
          className={`w-4 shrink-0 text-center font-mono text-sm ${
            failed ? "text-danger" : running ? "text-warning" : "text-accent"
          }`}
          aria-hidden
        >
          {failed ? "×" : running ? "•" : "✓"}
        </span>
        <code className="min-w-0 flex-1 truncate font-mono text-xs text-content-secondary">
          {toolReceiptLabel(toolCall, t)}
        </code>
        {toolCall.toolKind === "shell" && toolCall.rtkRoute ? (
          <span
            className="shrink-0 rounded-full border border-accent/25 bg-accent/5 px-1.5 py-0.5 font-mono text-[9px] uppercase tracking-wide text-accent"
            title={
              toolCall.rtkRoute === "rewritten"
                ? t(
                    "RTK replaced this command with a token-filtering equivalent.",
                  )
                : t(
                    "RTK tracked this command, but no safe output rewrite was available.",
                  )
            }
          >
            {toolCall.rtkRoute === "rewritten"
              ? t("RTK rewrite")
              : t("RTK tracked")}
          </span>
        ) : null}
        {wallTime ? (
          <span className="shrink-0 font-mono text-[10px] text-content-muted">
            {wallTime}
          </span>
        ) : null}
        {toolCall.truncatedOutput ? (
          <span className="shrink-0 font-mono text-[10px] text-warning">
            {t("truncated")}
          </span>
        ) : null}
        <span className="shrink-0 text-content-muted transition-transform group-open/tool:rotate-90" aria-hidden>
          {"\u203a"}
        </span>
      </summary>
      <div className="space-y-2 px-3 py-3">
        <div className="grid gap-1 text-xs text-content-muted sm:grid-cols-[4rem_1fr]">
          {toolCall.cwd ? <><span>{t("Cwd")}</span><code className="min-w-0 truncate font-mono text-content-secondary">{toolCall.cwd}</code></> : null}
          {toolCall.justification ? (
            <>
              <span>{t("Reason")}</span>
              <span className="text-content-secondary">
                {toolCall.justification}
              </span>
            </>
          ) : null}
          {toolCall.warning ? (
            <>
              <span>{t("Warning")}</span>
              <span className="text-warning">{toolCall.warning}</span>
            </>
          ) : null}
        </div>
        {toolCall.fileChanges?.length ? (
          <div className="grid gap-1">
            {toolCall.fileChanges.map((change) => (
              <div
                className="flex min-w-0 items-center gap-2 rounded-md border border-surface-border/20 bg-surface-raised px-2 py-1 text-xs text-content-secondary"
                key={`${change.changeType}-${change.path}`}
              >
                <span className="rounded-full border border-surface-border/25 px-1.5 py-0.5 font-mono text-[10px]">
                  {fileChangeTypeLabel(change.changeType, t)}
                </span>
                <code className="min-w-0 flex-1 truncate font-mono">
                  {change.path}
                </code>
                {change.requiresApproval ? (
                  <span className="text-warning">
                    {change.approved ? t("approved") : t("approval")}
                  </span>
                ) : null}
              </div>
            ))}
          </div>
        ) : (
          <pre className="max-h-64 overflow-auto rounded-md border border-surface-border/20 bg-code-bg p-3 font-mono text-xs leading-5 text-content-primary">
            {toolCall.tail || t("No output yet")}
          </pre>
        )}
      </div>
    </details>
  );
}

function toolReceiptLabel(toolCall: ToolCallUi, t: Translate) {
  if (toolCall.toolKind === "apply_patch" && toolCall.fileChanges?.length) {
    const first = toolCall.fileChanges[0];
    const remainder = toolCall.fileChanges.length - 1;
    switch (first.changeType) {
      case "add":
        return remainder > 0
          ? t("add {path} +{count} more", {
              path: first.path,
              count: remainder,
            })
          : t("add {path}", { path: first.path });
      case "delete":
        return remainder > 0
          ? t("delete {path} +{count} more", {
              path: first.path,
              count: remainder,
            })
          : t("delete {path}", { path: first.path });
      case "move_source":
      case "move_target":
        return remainder > 0
          ? t("move {path} +{count} more", {
              path: first.path,
              count: remainder,
            })
          : t("move {path}", { path: first.path });
      case "update":
        return remainder > 0
          ? t("edit {path} +{count} more", {
              path: first.path,
              count: remainder,
            })
          : t("edit {path}", { path: first.path });
    }
  }
  return toolCall.toolKind === "mcp"
    ? t("mcp {command}", { command: toolCall.command })
    : toolCall.toolKind === "mcp_elicitation"
      ? t("mcp prompt {command}", { command: toolCall.command })
    : toolCall.toolKind === "subagent"
      ? t("agent {command}", { command: toolCall.command })
      : toolCall.toolKind === "browser"
        ? t("browser {command}", { command: toolCall.command })
        : toolCall.toolKind === "web"
          ? t("web {command}", { command: toolCall.command })
          : toolCall.command;
}

type GroupedTurnDiff = {
  turnId: string;
  cwd: string;
  createdAt: string;
  revertible: boolean;
  diffs: string[];
};

type FileDiffCard = {
  id: string;
  label: string;
  diff: string;
};

function groupTurnDiffs(turnDiffs: TurnDiffSummary[]) {
  const grouped: GroupedTurnDiff[] = [];
  const byTurnId = new Map<string, GroupedTurnDiff>();
  for (const turnDiff of turnDiffs) {
    const existing = byTurnId.get(turnDiff.turnId);
    if (existing) {
      existing.diffs.push(turnDiff.diff);
      existing.revertible = existing.revertible && turnDiff.revertible;
      continue;
    }
    const next: GroupedTurnDiff = {
      turnId: turnDiff.turnId,
      cwd: turnDiff.cwd,
      createdAt: turnDiff.createdAt,
      revertible: turnDiff.revertible,
      diffs: [turnDiff.diff],
    };
    byTurnId.set(turnDiff.turnId, next);
    grouped.push(next);
  }
  return grouped;
}

function TurnDiffGroup({
  disabled,
  onRevert,
  turnDiff,
}: {
  disabled: boolean;
  onRevert: () => void;
  turnDiff: GroupedTurnDiff;
}) {
  const t = useT();
  const fileDiffs = turnDiff.diffs.flatMap((diff, index) =>
    splitDiffPayload(diff, index, t),
  );
  const nonRevertibleReason = turnDiff.revertible
    ? null
    : findNonRevertibleReason(turnDiff.diffs, t);

  return (
    <details
      className="group/turn-diff overflow-hidden rounded-md border border-accent/25 bg-surface-panel/70 text-sm shadow-sm shadow-[rgb(var(--shadow-color)/0.08)]"
      open={turnDiff.revertible}
    >
      <summary className="flex cursor-pointer list-none flex-wrap items-center justify-between gap-3 border-b border-surface-border/20 px-3 py-2 text-content-secondary transition-colors duration-150 hover:bg-surface-raised/45">
        <span className="font-medium text-content-primary">
          {t("Turn Diff")}
        </span>
        {turnDiff.revertible ? (
          <button
            className="rounded-md border border-danger/35 bg-danger/10 px-2 py-1 text-xs font-medium text-danger transition-colors duration-150 hover:bg-danger/15 focus:outline-none focus:ring-2 focus:ring-danger/30 disabled:cursor-not-allowed disabled:opacity-45"
            disabled={disabled}
            onClick={(event) => {
              event.preventDefault();
              event.stopPropagation();
              onRevert();
            }}
            type="button"
          >
            {t("Revert this turn")}
          </button>
        ) : (
          <span className="max-w-2xl truncate text-right font-mono text-[10px] leading-4 text-content-muted">
            {nonRevertibleReason}
          </span>
        )}
      </summary>
      <div className="space-y-2 p-2">
        {fileDiffs.map((fileDiff) => (
          <TurnFileDiffCard fileDiff={fileDiff} key={fileDiff.id} />
        ))}
      </div>
    </details>
  );
}

function TurnFileDiffCard({ fileDiff }: { fileDiff: FileDiffCard }) {
  return (
    <details className="group/file-diff overflow-hidden rounded-md border border-surface-border/25 bg-surface-raised/45" open>
      <summary className="cursor-pointer list-none border-b border-surface-border/20 px-3 py-2 font-mono text-[11px] text-content-secondary transition-colors duration-150 hover:bg-surface-raised">
        {fileDiff.label}
      </summary>
      <pre className="max-h-80 overflow-auto bg-code-bg p-3 font-mono text-xs leading-5 text-content-primary">
        {fileDiff.diff}
      </pre>
    </details>
  );
}

function splitDiffPayload(
  diff: string,
  payloadIndex: number,
  t: Translate,
): FileDiffCard[] {
  const gitMatches = [...diff.matchAll(/^diff --git .+$/gm)];
  if (gitMatches.length > 0) {
    return cardsFromDiffMarkers(
      diff,
      payloadIndex,
      gitMatches.map((match, index) => ({
        index: match.index ?? 0,
        label: diffCardLabel(match[0], index, t),
      })),
      t,
    );
  }

  const simpleMatches = [
    ...diff.matchAll(
      /^--- (a\/[^\r\n]+|\/dev\/null)\r?\n\+\+\+ (b\/[^\r\n]+|\/dev\/null)$/gm,
    ),
  ];
  if (simpleMatches.length > 0) {
    return cardsFromDiffMarkers(
      diff,
      payloadIndex,
      simpleMatches.map((match, index) => ({
        index: match.index ?? 0,
        label: simpleDiffCardLabel(match[1], match[2], index, t),
      })),
      t,
    );
  }

  return [
    {
      id: `${payloadIndex}-details`,
      label: t("Details"),
      diff,
    },
  ];
}

function cardsFromDiffMarkers(
  diff: string,
  payloadIndex: number,
  markers: Array<{ index: number; label: string }>,
  t: Translate,
) {
  if (markers.length === 0) {
    return [
      {
        id: `${payloadIndex}-details`,
        label: t("Details"),
        diff,
      },
    ];
  }

  const cards: FileDiffCard[] = [];
  const firstIndex = markers[0].index;
  const preface = diff.slice(0, firstIndex).trim();
  if (preface) {
    cards.push({
      id: `${payloadIndex}-details`,
      label: t("Details"),
      diff: preface,
    });
  }
  for (let index = 0; index < markers.length; index += 1) {
    const marker = markers[index];
    const start = marker.index;
    const end = markers[index + 1]?.index ?? diff.length;
    cards.push({
      id: `${payloadIndex}-${start}`,
      label: marker.label,
      diff: diff.slice(start, end).trimEnd(),
    });
  }
  return cards;
}

function diffCardLabel(header: string, index: number, t: Translate) {
  const match = header.match(
    /^diff --git (?:"?a\/)(.+?)"? (?:"?b\/)(.+?)"?$/,
  );
  return match?.[2] ?? t("File {number}", { number: index + 1 });
}

function simpleDiffCardLabel(
  before: string,
  after: string,
  index: number,
  t: Translate,
) {
  if (after.startsWith("b/")) {
    return after.slice(2);
  }
  if (before.startsWith("a/")) {
    return before.slice(2);
  }
  return t("File {number}", { number: index + 1 });
}

function findNonRevertibleReason(diffs: string[], t: Translate) {
  const lines = diffs.flatMap((diff) => diff.split(/\r?\n/));
  return (
    lines.find((line) =>
      /cannot be reverted|not revertible|could not safely capture|snapshot safety/i.test(
        line,
      ),
    )?.trim() ??
    t(
      "This turn is not revertible because Halv could not safely capture its preimage.",
    )
  );
}
