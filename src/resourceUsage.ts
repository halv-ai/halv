// The resource segment in the status bar shows the memory of Halv and its
// terminals, like Orca's ResourceUsageStatusSegment (stablyai/orca, MIT).
import type { ResourceUsageSnapshot, TerminalSession } from "./types";

export const RESOURCE_USAGE_POLL_MS = 5_000;
export const RESOURCE_USAGE_OPEN_POLL_MS = 2_000;

const MEBIBYTE = 1024 * 1024;
const GIBIBYTE = MEBIBYTE * 1024;

/** A short memory size: "<1 MB", "412 MB", or "1.4 GB". */
export function formatMemoryBytes(bytes: number): string {
  const safe = Number.isFinite(bytes) && bytes > 0 ? bytes : 0;
  const mebibytes = safe / MEBIBYTE;
  if (mebibytes < 1) return "<1 MB";
  const roundedMebibytes = Math.round(mebibytes);
  if (roundedMebibytes < 1024) return `${roundedMebibytes} MB`;
  const gibibytes = safe / GIBIBYTE;
  const digits = gibibytes >= 10 ? 0 : 1;
  return `${gibibytes.toFixed(digits)} GB`;
}

export type ResourceUsageRow = {
  id: string;
  kind: "app" | "terminal";
  label: string;
  bytes: number | null;
};

export type ResourceUsageSession = Pick<
  TerminalSession,
  "id" | "title" | "projectName" | "agentKind" | "status"
>;

/** The terminals that hold a live process. */
export function runningResourceSessions<T extends ResourceUsageSession>(
  sessions: readonly T[],
): T[] {
  return sessions.filter((session) => session.status === "running");
}

/** The app row first, then the terminals with the largest memory first. */
export function resourceUsageRows(
  snapshot: ResourceUsageSnapshot | null,
  terminals: readonly ResourceUsageSession[],
  labels: { app: string },
): ResourceUsageRow[] {
  const bytesBySession = new Map(
    (snapshot?.terminals ?? []).map((terminal) => [terminal.sessionId, terminal.bytes]),
  );
  const rows: ResourceUsageRow[] = terminals.map((terminal) => ({
    id: terminal.id,
    kind: "terminal",
    label: terminalRowLabel(terminal),
    bytes: bytesBySession.get(terminal.id) ?? null,
  }));
  rows.sort((left, right) => (right.bytes ?? -1) - (left.bytes ?? -1));
  return [
    { id: "app", kind: "app", label: labels.app, bytes: snapshot?.appBytes ?? null },
    ...rows,
  ];
}

function terminalRowLabel(terminal: ResourceUsageSession): string {
  const title = terminal.title?.trim();
  if (title) return title;
  const agent = terminal.agentKind?.trim();
  if (agent) return `${agent} · ${terminal.projectName}`;
  return terminal.projectName;
}
