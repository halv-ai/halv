import { useEffect, useRef, useState } from "react";
import {
  agentSessionAgentLabel,
  agentSessionAgentSubtitle,
  agentSessionTitle,
  respondAgentSessionPermission,
  listAgentSessionCheckpoints,
  previewAgentSessionCheckpoint,
  restoreAgentSessionCheckpoint,
  type AgentSessionCheckpoint,
  type AgentSessionCheckpointPreview,
  type AgentSessionCheckpointRestoreResult,
  type AgentSessionEntry,
  type AgentSessionTranscriptItem,
  sessionSavingsMeasurementNote,
  sessionSavingsUnmeasured,
} from "../agentSessions";
import { useLocale, useT, type Translate } from "../i18n/runtime";
import { MarkdownMessage } from "./MarkdownMessage";
import {
  SessionSavingsMeter,
  translateSavingsMeasurementNote,
} from "./SessionSavingsMeter";
import { laneIsPlan, routingSavingsFromTotals } from "../savingsMethod";
import { useRoutingSavingsReport } from "../useRoutingSavings";

type AgentSessionViewProps = {
  entry: AgentSessionEntry;
  onKill: (sessionId: string) => Promise<unknown>;
  onSend: (sessionId: string, text: string) => Promise<void>;
};

export function AgentSessionView({
  entry,
  onKill,
  onSend,
}: AgentSessionViewProps) {
  const t = useT();
  const routingReport = useRoutingSavingsReport(null, entry.session.id);
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const [killPending, setKillPending] = useState(false);
  const transcriptRef = useRef<HTMLDivElement>(null);
  const terminal =
    entry.session.status === "exited" || entry.session.status === "failed";
  const ghosttySession = entry.session.mode === "terminal_ghostty";
  const composerDisabled =
    sending || entry.session.status === "starting" || terminal;
  const lastTranscriptItem = entry.transcript.at(-1);

  useEffect(() => {
    const transcript = transcriptRef.current;
    if (!transcript) {
      return;
    }
    transcript.scrollTop = transcript.scrollHeight;
  }, [entry.transcript.length, lastTranscriptItem]);

  useEffect(() => {
    setDraft("");
    setSending(false);
    setKillPending(false);
  }, [entry.session.id]);

  async function send() {
    const text = draft.trim();
    if (!text || composerDisabled) {
      return;
    }
    setSending(true);
    try {
      await onSend(entry.session.id, text);
      setDraft("");
    } catch {
      // The session panel displays the shared store error.
    } finally {
      setSending(false);
    }
  }

  async function kill() {
    if (killPending || terminal) {
      return;
    }
    setKillPending(true);
    try {
      await onKill(entry.session.id);
    } catch {
      // The session panel displays the shared store error.
    } finally {
      setKillPending(false);
    }
  }

  return (
    <section className="agent-session-view">
      <header className="agent-session-view-header">
        <div className="agent-session-view-title">
          <div className="agent-session-view-title-line">
            <h3 title={agentSessionTitle(entry.session)}>
              {agentSessionTitle(entry.session)}
            </h3>
            <span className="agent-session-lane-badge">
              {laneLabel(entry.session.laneKind, t, entry.session.agentKind)}
            </span>
            <span className="agent-session-model-badge">
              {ghosttySession
                ? t("Ghostty window")
                : entry.model ?? t("model pending")}
            </span>
          </div>
          <code title={entry.session.cwd}>{entry.session.cwd}</code>
        </div>
        <div className="agent-session-view-actions">
          <span className="agent-session-status-label">
            <StatusIndicator status={entry.session.status} />
            {statusLabel(entry.session.status, t)}
          </span>
          <button
            className="agent-session-kill-button"
            disabled={killPending || terminal || ghosttySession}
            title={
              ghosttySession
                ? t("Close the Ghostty window to stop this session.")
                : undefined
            }
            onClick={() => void kill()}
            type="button"
          >
            {killPending
              ? t("Stopping…")
              : terminal
                ? t("Stopped")
                : ghosttySession
                  ? t("External")
                  : t("Kill")}
          </button>
        </div>
      </header>

      <SessionSavingsMeter
        measurementNote={translateSavingsMeasurementNote(
          sessionSavingsMeasurementNote(
            entry.session.laneKind,
            entry.session.savingsSources,
          ),
          t,
        )}
        planBilling={laneIsPlan(entry.session.laneKind)}
        ponytailMode={entry.session.ponytailMode}
        routing={routingSavingsFromTotals(routingReport?.totals)}
        turns={entry.turns}
        totals={entry.totals}
        unmeasured={sessionSavingsUnmeasured(
          entry.session.laneKind,
          entry.session.savingsSources,
        )}
      />

      <AgentSessionCheckpoints
        key={entry.session.id}
        sessionId={entry.session.id}
      />

      {ghosttySession ? (
        <div className="agent-session-external-terminal">
          <strong>{t("Agent running in Ghostty")}</strong>
          <span>
            {t(
              "Use the Ghostty window for input and output. Close that window to stop the agent.",
            )}
          </span>
        </div>
      ) : (
        <>
          <div
            aria-live="polite"
            className="agent-session-transcript"
            ref={transcriptRef}
          >
            {entry.transcript.length === 0 ? (
              <div className="agent-session-empty-transcript">
                <strong>{t("Session ready")}</strong>
                <span>
                  {t(
                    "Send a message to start. Halv keeps this transcript in memory.",
                  )}
                </span>
              </div>
            ) : (
              entry.transcript.map((item) => (
                <TranscriptItem
                  item={item}
                  key={item.id}
                  sessionId={entry.session.id}
                />
              ))
            )}
          </div>

          <form
            className="agent-session-composer"
            onSubmit={(event) => {
              event.preventDefault();
              void send();
            }}
          >
            <textarea
              aria-label={t("Message agent session")}
              disabled={composerDisabled}
              onChange={(event) => setDraft(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter" && !event.shiftKey) {
                  event.preventDefault();
                  void send();
                }
              }}
              placeholder={composerPlaceholder(entry.session.status, t)}
              rows={3}
              value={draft}
            />
            <div className="agent-session-composer-footer">
              <span>{t("Enter to send · Shift+Enter for a new line")}</span>
              <button
                disabled={composerDisabled || !draft.trim()}
                type="submit"
              >
                {sending ? t("Sending…") : t("Send")}
              </button>
            </div>
          </form>
        </>
      )}
    </section>
  );
}

function AgentSessionCheckpoints({ sessionId }: { sessionId: string }) {
  const t = useT();
  const locale = useLocale();
  const [checkpoints, setCheckpoints] = useState<
    AgentSessionCheckpoint[] | null
  >(null);
  const [loading, setLoading] = useState(false);
  const [actionId, setActionId] = useState<string | null>(null);
  const [preview, setPreview] =
    useState<AgentSessionCheckpointPreview | null>(null);
  const [restoreResult, setRestoreResult] =
    useState<AgentSessionCheckpointRestoreResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function load() {
    setLoading(true);
    setError(null);
    try {
      setCheckpoints(await listAgentSessionCheckpoints(sessionId));
    } catch (loadError) {
      setError(checkpointErrorMessage(loadError));
    } finally {
      setLoading(false);
    }
  }

  async function requestPreview(checkpointId: string) {
    setActionId(checkpointId);
    setError(null);
    try {
      setPreview(await previewAgentSessionCheckpoint(checkpointId));
    } catch (previewError) {
      setError(checkpointErrorMessage(previewError));
    } finally {
      setActionId(null);
    }
  }

  async function restore() {
    if (!preview) {
      return;
    }
    setActionId(preview.checkpointId);
    setError(null);
    try {
      const result = await restoreAgentSessionCheckpoint(
        preview.checkpointId,
      );
      setRestoreResult(result);
      setPreview(null);
      await load();
    } catch (restoreError) {
      setError(checkpointErrorMessage(restoreError));
    } finally {
      setActionId(null);
    }
  }

  return (
    <details
      className="agent-session-checkpoints"
      onToggle={(event) => {
        if (event.currentTarget.open && !loading) {
          void load();
        }
      }}
    >
      <summary>
        <span>{t("Checkpoints")}</span>
        {checkpoints ? (
          <span className="agent-session-checkpoint-count">
            {t("{count} saved", { count: checkpoints.length })}
          </span>
        ) : null}
      </summary>
      <div className="agent-session-checkpoint-panel">
        {loading && checkpoints === null ? (
          <span className="agent-session-checkpoint-muted">
            {t("Loading checkpoints…")}
          </span>
        ) : null}

        {!loading && checkpoints?.length === 0 ? (
          <div className="agent-session-checkpoint-empty">
            <strong>{t("No checkpoints available")}</strong>
            <span>
              {t(
                "Halv creates checkpoints for sessions that use a Git repository.",
              )}
            </span>
          </div>
        ) : null}

        {checkpoints && checkpoints.length > 0 ? (
          <div className="agent-session-checkpoint-list">
            {checkpoints.map((checkpoint) => (
              <div className="agent-session-checkpoint-row" key={checkpoint.id}>
                <div>
                  <strong>{checkpointLabel(checkpoint, t)}</strong>
                  <span>
                    {formatCheckpointRelativeTime(checkpoint.createdAt, locale)}
                  </span>
                </div>
                <button
                  disabled={actionId !== null}
                  onClick={() => void requestPreview(checkpoint.id)}
                  type="button"
                >
                  {actionId === checkpoint.id
                    ? t("Previewing…")
                    : t("Restore")}
                </button>
              </div>
            ))}
          </div>
        ) : null}

        {preview ? (
          <CheckpointConfirmation
            busy={actionId === preview.checkpointId}
            onCancel={() => setPreview(null)}
            onConfirm={() => void restore()}
            preview={preview}
          />
        ) : null}

        {restoreResult ? (
          <div className="agent-session-checkpoint-result" role="status">
            <div>
              <strong>{t("Checkpoint restored")}</strong>
              <span>{checkpointChangeSummary(restoreResult, t)}</span>
            </div>
            <button
              disabled={actionId !== null}
              onClick={() =>
                void requestPreview(restoreResult.safetyCheckpointId)
              }
              type="button"
            >
              {t("Undo restore")}
            </button>
          </div>
        ) : null}

        {error ? (
          <div className="agent-session-checkpoint-error" role="alert">
            <strong>{t("Checkpoint operation failed")}</strong>
            <span>{error}</span>
          </div>
        ) : null}
      </div>
    </details>
  );
}

function CheckpointConfirmation({
  busy,
  onCancel,
  onConfirm,
  preview,
}: {
  busy: boolean;
  onCancel: () => void;
  onConfirm: () => void;
  preview: AgentSessionCheckpointPreview;
}) {
  const t = useT();
  return (
    <div className="agent-session-checkpoint-confirmation">
      <div>
        <strong>{t("Restore this checkpoint?")}</strong>
        <span>{checkpointChangeSummary(preview, t)}</span>
      </div>
      <CheckpointFileGroup
        files={preview.modified}
        label={t("Modified ({count})", { count: preview.modified.length })}
      />
      <CheckpointFileGroup
        files={preview.created}
        label={t("Created ({count})", { count: preview.created.length })}
      />
      <CheckpointFileGroup
        files={preview.deleted}
        label={t("Deleted ({count})", { count: preview.deleted.length })}
      />
      <div className="agent-session-checkpoint-confirm-actions">
        <button disabled={busy} onClick={onCancel} type="button">
          {t("Cancel")}
        </button>
        <button disabled={busy} onClick={onConfirm} type="button">
          {busy ? t("Restoring…") : t("Confirm restore")}
        </button>
      </div>
    </div>
  );
}

function CheckpointFileGroup({ files, label }: { files: string[]; label: string }) {
  if (files.length === 0) {
    return null;
  }
  return (
    <div className="agent-session-checkpoint-file-group">
      <strong>{label}</strong>
      <ul>
        {files.map((file) => (
          <li key={file}>
            <code>{file}</code>
          </li>
        ))}
      </ul>
    </div>
  );
}

function checkpointLabel(
  checkpoint: AgentSessionCheckpoint,
  t: Translate,
): string {
  if (checkpoint.kind === "safety") {
    return t("Safety checkpoint");
  }
  if (checkpoint.turnId) {
    return t("Turn {turn}", { turn: checkpoint.turnId.slice(0, 8) });
  }
  return t("Turn checkpoint");
}

function checkpointChangeSummary(
  preview: AgentSessionCheckpointPreview,
  t: Translate,
): string {
  return t("{modified} modified · {created} created · {deleted} deleted", {
    modified: preview.modified.length,
    created: preview.created.length,
    deleted: preview.deleted.length,
  });
}

function formatCheckpointRelativeTime(value: string, locale: string): string {
  const timestamp = Date.parse(value);
  if (Number.isNaN(timestamp)) {
    return value;
  }
  const seconds = Math.round((timestamp - Date.now()) / 1000);
  const absolute = Math.abs(seconds);
  const units: Array<[Intl.RelativeTimeFormatUnit, number]> = [
    ["year", 60 * 60 * 24 * 365],
    ["month", 60 * 60 * 24 * 30],
    ["week", 60 * 60 * 24 * 7],
    ["day", 60 * 60 * 24],
    ["hour", 60 * 60],
    ["minute", 60],
  ];
  const formatter = new Intl.RelativeTimeFormat(locale, {
    numeric: "auto",
    style: "short",
  });
  for (const [unit, unitSeconds] of units) {
    if (absolute >= unitSeconds) {
      return formatter.format(Math.round(seconds / unitSeconds), unit);
    }
  }
  return formatter.format(seconds, "second");
}

function checkpointErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function TranscriptItem({
  item,
  sessionId,
}: {
  item: AgentSessionTranscriptItem;
  sessionId: string;
}) {
  const t = useT();
  switch (item.kind) {
    case "userText":
      return (
        <article className="agent-session-user-message">
          <MarkdownMessage className="markdown-body-user" content={item.text} />
        </article>
      );
    case "assistantText":
      return (
        <article className="agent-session-assistant-message">
          <span className="agent-session-message-role">{t("Assistant")}</span>
          <MarkdownMessage content={item.text} />
        </article>
      );
    case "toolCall":
      return item.permissionRequest ? (
        <PermissionRequest item={item} sessionId={sessionId} />
      ) : (
        <ToolCall item={item} />
      );
    case "error":
      return (
        <article className="agent-session-protocol-error">
          <strong>{t("Protocol error")}</strong>
          <span>{item.text}</span>
        </article>
      );
    case "system":
      return <div className="agent-session-system-message">{item.text}</div>;
  }
}

function ToolCall({
  item,
}: {
  item: Extract<AgentSessionTranscriptItem, { kind: "toolCall" }>;
}) {
  const t = useT();
  return (
    <details className="agent-session-tool-call">
      <summary>
        <span className="agent-session-tool-icon" aria-hidden>
          {item.completed ? "✓" : "…"}
        </span>
        <span>{item.name}</span>
        <span className="agent-session-tool-status">
          {toolCallStatusLabel(item.completed, t)}
        </span>
      </summary>
      {item.summaryJson ? <pre>{prettyJson(item.summaryJson)}</pre> : null}
    </details>
  );
}

function PermissionRequest({
  item,
  sessionId,
}: {
  item: Extract<AgentSessionTranscriptItem, { kind: "toolCall" }>;
  sessionId: string;
}) {
  const t = useT();
  const request = item.permissionRequest!;
  const [responding, setResponding] = useState(false);
  const [responseError, setResponseError] = useState<string | null>(null);

  const respond = async (approve: boolean) => {
    setResponding(true);
    setResponseError(null);
    try {
      await respondAgentSessionPermission(
        sessionId,
        request.requestId,
        approve,
      );
    } catch {
      setResponseError(t("Could not respond to the permission request."));
    } finally {
      setResponding(false);
    }
  };

  return (
    <article className="agent-session-permission-card">
      <div>
        <span className="agent-session-message-role">
          {t("Permission request")}
        </span>
        <strong>{request.description}</strong>
      </div>
      <details>
        <summary>{t("Protocol payload")}</summary>
        <pre>{prettyJson(request.payloadJson)}</pre>
      </details>
      {request.approved === undefined ? (
        <div className="agent-session-permission-actions">
          <button
            disabled={responding}
            onClick={() => void respond(false)}
            type="button"
          >
            {t("Deny")}
          </button>
          <button
            disabled={responding}
            onClick={() => void respond(true)}
            type="button"
          >
            {t("Approve")}
          </button>
        </div>
      ) : (
        <strong className="agent-session-permission-outcome">
          {request.approved ? t("Approved") : t("Denied")}
        </strong>
      )}
      {responseError ? (
        <span className="agent-session-permission-error" role="alert">
          {responseError}
        </span>
      ) : null}
    </article>
  );
}

export function StatusIndicator({
  status,
}: {
  status: AgentSessionEntry["session"]["status"];
}) {
  if (status === "starting") {
    return <span className="conversation-spinner agent-session-status-dot" />;
  }
  return (
    <span
      className={`agent-session-status-dot agent-session-status-dot-${status}`}
      aria-hidden
    />
  );
}

export function agentLabel(agentKind: AgentSessionEntry["session"]["agentKind"]) {
  return agentSessionAgentLabel(agentKind);
}

export function agentSubtitle(
  agentKind: AgentSessionEntry["session"]["agentKind"],
  t: Translate = identityTranslate,
) {
  const subtitle = agentSessionAgentSubtitle(agentKind);
  return subtitle === "Z.ai Coding Plan" ? t("Z.ai Coding Plan") : subtitle;
}

export function laneLabel(
  laneKind: AgentSessionEntry["session"]["laneKind"],
  t: Translate = identityTranslate,
  agentKind?: AgentSessionEntry["session"]["agentKind"],
) {
  if (laneKind === "custom" && agentKind === "gemini") return "Gemini CLI";
  if (laneKind === "custom" && agentKind === "antigravity") {
    return "Antigravity CLI";
  }
  switch (laneKind) {
    case "claudeSubscription":
    case "kimiSubscription":
      return t("subscription");
    case "claudeApiKey":
    case "kimiApiKey":
      return t("API key");
    case "codexChatGpt":
      return t("ChatGPT");
    case "zaiGlm":
      return t("GLM Coding Plan");
    case "grokApiKey":
      return t("Grok API key");
    case "custom":
      return t("custom");
  }
}

const identityTranslate: Translate = (source) => source;

function statusLabel(
  status: AgentSessionEntry["session"]["status"],
  t: Translate,
) {
  switch (status) {
    case "starting":
      return t("starting");
    case "running":
      return t("running");
    case "waiting_input":
      return t("waiting for input");
    case "exited":
      return t("exited");
    case "failed":
      return t("failed");
  }
}

function composerPlaceholder(
  status: AgentSessionEntry["session"]["status"],
  t: Translate,
) {
  if (status === "starting") return t("Session is starting…");
  if (status === "exited" || status === "failed") {
    return t("This session has stopped.");
  }
  return t("Message the agent…");
}

function toolCallStatusLabel(completed: boolean, t: Translate) {
  return completed ? t("completed") : t("running");
}

function prettyJson(value: string) {
  try {
    return JSON.stringify(JSON.parse(value), null, 2);
  } catch {
    return value;
  }
}
