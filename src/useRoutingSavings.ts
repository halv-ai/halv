import { useEffect, useState } from "react";
import { ipc } from "./ipc";
import type { RoutingSavingsReport } from "./types";

const REFRESH_INTERVAL_MS = 10_000;

/**
 * The JEV routing dollars of one conversation, else of one session
 * (ADR 0016). The backend settles finished chains on each read.
 */
export function useRoutingSavingsReport(
  conversationId: string | null | undefined,
  sessionId: string | null | undefined,
): RoutingSavingsReport | null {
  const [report, setReport] = useState<{
    key: string;
    value: RoutingSavingsReport;
  } | null>(null);
  const key = conversationId
    ? `conversation:${conversationId}`
    : sessionId
      ? `session:${sessionId}`
      : null;

  useEffect(() => {
    if (!key) return;
    let disposed = false;
    const load = async () => {
      try {
        const value = await ipc.storage.routingSavings(
          conversationId ? { conversationId } : { sessionId },
        );
        if (!disposed) setReport({ key, value });
      } catch {
        // Routing dollars are optional on the meter.
      }
    };
    void load();
    const interval = window.setInterval(() => void load(), REFRESH_INTERVAL_MS);
    return () => {
      disposed = true;
      window.clearInterval(interval);
    };
  }, [key, conversationId, sessionId]);

  return report && report.key === key ? report.value : null;
}
