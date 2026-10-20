// Runs the queued account restarts of the panes that one terminal panel
// shows. A pane restarts only while it rests between turns, and the panel
// restarts one pane at a time.
import { useEffect, useRef } from "react";
import { accountRestartStore, nextRestartablePane } from "./accountRestart.ts";
import { terminalActivityStore } from "./terminalActivityStore.ts";

// Another resume in the panel blocks a restart. The runner tries again
// after this delay.
const RETRY_LATER_MS = 1_000;

export function useAccountRestartRunner(
  sessionIds: readonly string[],
  restart: (sessionId: string) => Promise<"done" | "later" | "gone">,
) {
  const restartRef = useRef(restart);
  restartRef.current = restart;
  // The pane set changes while a restart runs, so the flag outlives one
  // effect run.
  const runningRef = useRef(false);
  // The tick of the current effect run. A restart that ends calls it.
  const tickRef = useRef<() => void>(() => undefined);
  const key = [...new Set(sessionIds)].sort().join("\n");

  useEffect(() => {
    const owned = new Set(key ? key.split("\n") : []);
    if (owned.size === 0) return;
    let disposed = false;
    let retryTimer: ReturnType<typeof setTimeout> | undefined;
    const tick = () => {
      if (runningRef.current || disposed) return;
      const sessionId = nextRestartablePane(
        accountRestartStore.getSnapshot().entries,
        owned,
        (id) => terminalActivityStore.paneIsBusy(id),
      );
      if (!sessionId || !accountRestartStore.begin(sessionId)) return;
      runningRef.current = true;
      let retryLater = false;
      void restartRef
        .current(sessionId)
        .then(
          (outcome) => {
            if (outcome !== "later") {
              accountRestartStore.finish(sessionId);
              return;
            }
            retryLater = true;
            accountRestartStore.requeue(sessionId);
          },
          (cause: unknown) =>
            accountRestartStore.fail(
              sessionId,
              cause instanceof Error ? cause.message : String(cause),
            ),
        )
        .finally(() => {
          runningRef.current = false;
          if (!retryLater) {
            tickRef.current();
            return;
          }
          clearTimeout(retryTimer);
          retryTimer = setTimeout(() => tickRef.current(), RETRY_LATER_MS);
        });
    };
    tickRef.current = tick;
    const stopRestarts = accountRestartStore.subscribe(tick);
    const stopActivity = terminalActivityStore.subscribe(tick);
    tick();
    return () => {
      disposed = true;
      if (tickRef.current === tick) tickRef.current = () => undefined;
      clearTimeout(retryTimer);
      stopRestarts();
      stopActivity();
    };
  }, [key]);
}
