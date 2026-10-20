import { memo, useEffect, useRef, useState, type ReactNode } from "react";
import { imageAttachmentDataUrl } from "../attachments";
import { copyText } from "../clipboard";
import { formatCompactNumber, formatRelativeTime } from "../format";
import { userMessageForkSequence } from "../forkConversation";
import { isProxyRecoveryError } from "../messageRecovery";
import { replySavings, type ReplySavings } from "../messageSavings";
import {
  formatDateTime,
  formatMoney,
  formatNumber,
  useT,
  type Translate,
} from "../i18n/runtime";
import type { TokenUsage, UiMessage } from "../types";
import { MarkdownMessage } from "./MarkdownMessage";

type MessageBubbleProps = {
  actionsDisabled?: boolean;
  canRegenerate?: boolean;
  laterMessageCount?: number;
  message: UiMessage;
  onEdit?: (message: UiMessage) => void;
  onFork?: (message: UiMessage) => void;
  onOpenSettings?: () => void;
  onRegenerate?: (message: UiMessage) => void;
  onRestartProxy?: () => void;
  restartPending?: boolean;
  sessionBacked?: boolean;
};

export const MessageBubble = memo(function MessageBubble({
  actionsDisabled = false,
  canRegenerate = false,
  laterMessageCount = 0,
  message,
  onEdit,
  onFork,
  onOpenSettings,
  onRegenerate,
  onRestartProxy,
  restartPending = false,
  sessionBacked = false,
}: MessageBubbleProps) {
  const t = useT();
  const [copied, setCopied] = useState(false);
  const copyResetTimer = useRef<number | undefined>(undefined);
  const [editConfirmationOpen, setEditConfirmationOpen] = useState(false);
  const usage = usageForMessage(message);
  const roleLabel = roleLabelForMessage(message, t);
  const isUser = message.role === "user";
  const forkSequence = userMessageForkSequence(message);
  const relativeTime = relativeTimeLabel(message.createdAt, t);
  const timestampTitle = formatDateTime(new Date(message.createdAt));
  const savingsLine = translatedReplySavingsLine(
    replySavings(message, usage, sessionBacked),
    t,
  );

  useEffect(() => {
    // Clear the reset timer on unmount.
    return () => window.clearTimeout(copyResetTimer.current);
  }, []);

  async function copyMessage() {
    if (await copyText(message.content)) {
      setCopied(true);
      window.clearTimeout(copyResetTimer.current);
      copyResetTimer.current = window.setTimeout(() => setCopied(false), 1200);
    }
  }

  if (isUser) {
    return (
      <article
        className="group relative flex flex-col items-end"
        data-message-id={message.id}
      >
        <MessageMetadata
          action={
            onFork || onEdit ? (
              <>
                {onFork ? (
                  <button
                    className="rounded-full border border-surface-border/30 px-2 py-0.5 text-[11px] text-content-muted transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
                    disabled={actionsDisabled || forkSequence === null}
                    data-message-action="fork"
                    onClick={() => onFork(message)}
                    type="button"
                  >
                    {t("Fork from here")}
                  </button>
                ) : null}
                {onEdit ? (
                  <button
                    className="rounded-full border border-surface-border/30 px-2 py-0.5 text-[11px] text-content-muted transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
                    disabled={actionsDisabled}
                    data-message-action="edit"
                    onClick={() => setEditConfirmationOpen(true)}
                    type="button"
                  >
                    {t("Edit")}
                  </button>
                ) : null}
              </>
            ) : null
          }
          align="right"
          timestampTitle={timestampTitle}
          relativeTime={relativeTime}
          usage={usage}
          message={message}
        />
        <div className="max-w-[75%] rounded-2xl bg-accent/10 px-3 py-2 text-content-primary">
          {message.attachments?.length ? (
            <div className="flex flex-wrap justify-end gap-2">
              {message.attachments.map((attachment) => (
                <a
                  className="block max-w-full overflow-hidden rounded-xl border border-surface-border/30 bg-surface-sunken focus:outline-none focus:ring-2 focus:ring-accent/40"
                  href={imageAttachmentDataUrl(attachment)}
                  key={attachment.id}
                  rel="noreferrer"
                  target="_blank"
                  title={attachment.name}
                >
                  <img
                    alt={attachment.name}
                    className="max-h-72 max-w-full object-contain"
                    src={imageAttachmentDataUrl(attachment)}
                  />
                </a>
              ))}
            </div>
          ) : null}
          {message.content.trim() ? (
            <MarkdownMessage
              className={`markdown-body-user ${message.attachments?.length ? "mt-2 px-1" : "px-1"}`}
              content={message.content}
            />
          ) : null}
        </div>
        {editConfirmationOpen && onEdit ? (
          <div className="mt-2 flex max-w-[75%] flex-wrap items-center justify-end gap-2 rounded-md border border-danger/30 bg-danger/10 px-3 py-2 font-mono text-[11px] text-danger">
            <span>
              {laterMessageCount === 1
                ? t("Editing removes {count} later message \u2014", {
                    count: Math.max(0, laterMessageCount),
                  })
                : t("Editing removes {count} later messages \u2014", {
                    count: Math.max(0, laterMessageCount),
                  })}
            </span>
            <button
              className="rounded-md border border-danger/35 bg-danger/10 px-2 py-1 font-medium text-danger transition-colors duration-150 hover:bg-danger/15 focus:outline-none focus:ring-2 focus:ring-danger/30 disabled:cursor-not-allowed disabled:opacity-45"
              disabled={actionsDisabled}
              onClick={() => {
                setEditConfirmationOpen(false);
                onEdit(message);
              }}
              type="button"
            >
              {t("Edit")}
            </button>
            <button
              className="rounded-md border border-surface-border/30 px-2 py-1 text-content-muted transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
              onClick={() => setEditConfirmationOpen(false)}
              type="button"
            >
              {t("Cancel")}
            </button>
          </div>
        ) : null}
      </article>
    );
  }

  return (
    <article
      className={`group relative mr-auto max-w-[70ch] ${
        message.error ? "text-danger" : "text-content-primary"
      }`}
      data-message-id={message.id}
    >
      <MessageMetadata
        action={
          message.role === "assistant" ? (
            <>
              <button
                className="rounded-full border border-surface-border/30 px-2 py-0.5 text-[11px] text-content-muted transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
                onClick={() => void copyMessage()}
                type="button"
              >
                {copied ? t("Copied") : t("Copy")}
              </button>
              {!message.error && canRegenerate && onRegenerate ? (
                <button
                  className="rounded-full border border-surface-border/30 px-2 py-0.5 text-[11px] text-content-muted transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
                  disabled={actionsDisabled}
                  onClick={() => onRegenerate(message)}
                  type="button"
                >
                  {t("Regenerate")}
                </button>
              ) : null}
            </>
          ) : null
        }
        label={roleLabel}
        message={message}
        relativeTime={relativeTime}
        timestampTitle={timestampTitle}
        usage={usage}
      />

      <div className="relative">
        <MarkdownMessage content={message.content} />
        {message.streaming ? <span className="streaming-caret" /> : null}
      </div>

      {savingsLine ? (
        <div className="mt-1.5 font-mono text-[11px] text-accent">
          {savingsLine}
        </div>
      ) : null}


      {message.error ? (
        <div className="mt-3 rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">
          {t("This response ended with an error.")}
          <ErrorRecoveryActions
            error={message.content}
            onOpenSettings={onOpenSettings}
            onRetry={
              canRegenerate && onRegenerate
                ? () => onRegenerate(message)
                : undefined
            }
            onRestartProxy={onRestartProxy}
            retryDisabled={actionsDisabled}
            restartPending={restartPending}
          />
        </div>
      ) : null}
    </article>
  );
});

export function ErrorRecoveryActions({
  error,
  onOpenSettings,
  onRetry,
  onRunInNewWorktree,
  onRestartProxy,
  retryDisabled = false,
  restartPending = false,
}: {
  error: string;
  onOpenSettings?: () => void;
  onRetry?: () => void;
  onRunInNewWorktree?: () => Promise<void> | void;
  onRestartProxy?: () => void;
  retryDisabled?: boolean;
  restartPending?: boolean;
}) {
  const t = useT();
  const [runInNewWorktreePending, setRunInNewWorktreePending] = useState(false);
  const normalizedError = error.toLowerCase();
  const workspaceBusyError = normalizedError.includes(
    "workspace is busy in another conversation",
  );
  const usageLimitReached =
    normalizedError.includes("usage_limit_reached") ||
    normalizedError.includes("usage limit");
  const headroomProxyError = isProxyRecoveryError(
    normalizedError,
    usageLimitReached,
  );

  if (workspaceBusyError && onRunInNewWorktree) {
    return (
      <div className="mt-2 flex flex-wrap items-center gap-2 border-t border-danger/20 pt-2">
        <button
          className="rounded-md border border-accent bg-accent px-2.5 py-1 font-mono text-[11px] font-medium text-accent-contrast transition-colors duration-150 hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40 disabled:cursor-not-allowed disabled:opacity-45"
          data-message-action="run-in-new-worktree"
          disabled={runInNewWorktreePending}
          onClick={() => {
            setRunInNewWorktreePending(true);
            void Promise.resolve(onRunInNewWorktree()).finally(() => {
              setRunInNewWorktreePending(false);
            });
          }}
          type="button"
        >
          {runInNewWorktreePending
            ? t("Creating worktree...")
            : t("Run in a new worktree")}
        </button>
      </div>
    );
  }

  if (!onRetry && !usageLimitReached && !headroomProxyError) {
    return null;
  }

  return (
    <div className="mt-2 flex flex-wrap items-center gap-2 border-t border-danger/20 pt-2">
      {onRetry ? (
        <button
          className="rounded-md border border-accent bg-accent px-2.5 py-1 font-mono text-[11px] font-medium text-accent-contrast transition-colors duration-150 hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40 disabled:cursor-not-allowed disabled:opacity-45"
          data-message-action="retry"
          disabled={retryDisabled}
          onClick={onRetry}
          type="button"
        >
          {t("Retry")}
        </button>
      ) : null}
      {headroomProxyError && onRestartProxy ? (
        <button
          className="rounded-md border border-danger/35 bg-danger/10 px-2 py-1 font-mono text-[11px] font-medium text-danger transition-colors duration-150 hover:bg-danger/15 focus:outline-none focus:ring-2 focus:ring-danger/30 disabled:cursor-not-allowed disabled:opacity-45"
          disabled={restartPending}
          onClick={onRestartProxy}
          type="button"
        >
          {restartPending ? t("Restarting...") : t("Restart proxy")}
        </button>
      ) : null}
      {onOpenSettings ? (
        <button
          className="rounded-md border border-surface-border/30 px-2 py-1 font-mono text-[11px] text-content-muted transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
          onClick={onOpenSettings}
          type="button"
        >
          {t("Open Settings")}
        </button>
      ) : null}
      {usageLimitReached ? (
        <span className="font-mono text-[11px] text-content-muted">
          {t("Switch model with /model, or add an API key in Settings.")}
        </span>
      ) : null}
    </div>
  );
}

function MessageMetadata({
  action,
  align = "left",
  label,
  message,
  relativeTime,
  timestampTitle,
  usage,
}: {
  action?: ReactNode;
  align?: "left" | "right";
  label?: string;
  message: UiMessage;
  relativeTime: string;
  timestampTitle: string;
  usage: TokenUsage | null;
}) {
  const t = useT();
  return (
    <div
      className={`pointer-events-auto absolute -top-5 z-10 flex items-center gap-1.5 whitespace-nowrap rounded-md bg-surface-glass/95 px-1.5 py-0.5 font-mono text-[10px] text-content-muted opacity-0 shadow-sm transition-opacity group-hover:opacity-100 group-focus-within:opacity-100 ${
        align === "right" ? "right-0 justify-end" : "left-0"
      }`}
    >
      {label ? <span>{label}</span> : null}
      <span title={timestampTitle}>{relativeTime}</span>
      {message.streaming ? <span>{t("streaming")}</span> : null}
      {usage ? <TokenUsageChips usage={usage} /> : null}
      {action}
    </div>
  );
}

function TokenUsageChips({ usage }: { usage: TokenUsage }) {
  const t = useT();
  return (
    <>
      <span className="rounded-full border border-surface-border/25 bg-surface-raised/70 px-2 py-0.5 font-mono text-content-secondary">
        {t("{count} in", {
          count: formatCompactNumber(usage.inputTokens),
        })}
      </span>
      <span className="rounded-full border border-surface-border/25 bg-surface-raised/70 px-2 py-0.5 font-mono text-content-secondary">
        {t("{count} out", {
          count: formatCompactNumber(usage.outputTokens),
        })}
      </span>
      {usage.reasoningTokens > 0 ? (
        <span className="rounded-full border border-surface-border/25 bg-surface-raised/70 px-2 py-0.5 font-mono text-content-muted">
          {t("{count} reasoning", {
            count: formatCompactNumber(usage.reasoningTokens),
          })}
        </span>
      ) : null}
    </>
  );
}

function usageForMessage(message: UiMessage): TokenUsage | null {
  if (message.usage) {
    return message.usage;
  }
  if (message.inputTokens == null && message.outputTokens == null) {
    return null;
  }
  const inputTokens = message.inputTokens ?? 0;
  const outputTokens = message.outputTokens ?? 0;
  return {
    inputTokens,
    outputTokens,
    totalTokens: inputTokens + outputTokens,
    cachedTokens: message.cachedTokens ?? 0,
    reasoningTokens: 0,
  };
}

function roleLabelForMessage(message: UiMessage, t: Translate) {
  switch (message.role) {
    case "user":
      return t("You");
    case "assistant":
      return t("Assistant");
    case "developer":
      return t("Developer");
    case "system":
      return t("System");
  }
}

function relativeTimeLabel(value: string, t: Translate) {
  const relativeTime = formatRelativeTime(value);
  if (relativeTime === "unknown") return t("unknown");
  if (relativeTime === "just now") return t("just now");

  const past = /^(\d+)(y|mo|w|d|h|m) ago$/.exec(relativeTime);
  if (past) {
    const amount = past[1];
    switch (past[2]) {
      case "y":
        return t("{amount}y ago", { amount });
      case "mo":
        return t("{amount}mo ago", { amount });
      case "w":
        return t("{amount}w ago", { amount });
      case "d":
        return t("{amount}d ago", { amount });
      case "h":
        return t("{amount}h ago", { amount });
      case "m":
        return t("{amount}m ago", { amount });
    }
  }

  const future = /^in (\d+)(y|mo|w|d|h|m)$/.exec(relativeTime);
  if (future) {
    const amount = future[1];
    switch (future[2]) {
      case "y":
        return t("in {amount}y", { amount });
      case "mo":
        return t("in {amount}mo", { amount });
      case "w":
        return t("in {amount}w", { amount });
      case "d":
        return t("in {amount}d", { amount });
      case "h":
        return t("in {amount}h", { amount });
      case "m":
        return t("in {amount}m", { amount });
    }
  }

  return relativeTime;
}

function translatedReplySavingsLine(
  savings: ReplySavings | null,
  t: Translate,
) {
  if (!savings) return null;
  const compact = (value: number) =>
    formatNumber(value, { notation: "compact", maximumFractionDigits: 1 });
  switch (savings.kind) {
    case "dollars":
      return t("\u2193 {amount} saved \u00b7 {tokens} tokens kept out", {
        amount: formatMoney(savings.amountUsd),
        tokens: compact(savings.tokens),
      });
    case "compressed":
      return t(
        "\u2193 compressed {originalInputTokens} \u2192 {compressedInputTokens} \u00b7 -{percent}%",
        {
          originalInputTokens: formatNumber(savings.originalInputTokens),
          compressedInputTokens: formatNumber(savings.compressedInputTokens),
          percent: formatNumber(savings.percent),
        },
      );
    case "tokens":
      return savings.label === "saved"
        ? t("\u2193 saved {count} tokens", { count: compact(savings.tokens) })
        : t("\u2193 {source} saved {count} tokens", {
            source:
              savings.label === "compressed"
                ? t("compressed")
                : savings.label === "RTK"
                  ? "RTK"
                  : savings.label === "crux (est.)"
                    ? t("crux (est.)")
                    : t("RTK + crux (est.)"),
            count: compact(savings.tokens),
          });
    case "usage":
      return t("\u2193 {input} in \u00b7 {output} out \u00b7 {cached} cached", {
        input: compact(savings.input),
        output: compact(savings.output),
        cached: compact(savings.cached),
      });
  }
}
