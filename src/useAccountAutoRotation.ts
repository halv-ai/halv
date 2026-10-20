// Runs the opt-in automatic rotation of Claude accounts (ADR 0017). The
// decisions are in accountAutoRotation.ts. This hook reads the stores,
// switches through the normal select path, and keeps the result for the
// card above the status bar.
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import {
  autoRotationSetting,
  autoRotationTarget,
  claudePanesBusy,
  claudeStaleSessionIds,
  finishAutoRotation,
  INITIAL_AUTO_ROTATION_WATCH,
  nextAutoRotationStep,
  rememberRotatedAway,
  rotatedAwayIds,
  rotatedAwayUntil,
  type AutoRotationSettings,
  type RotatedAwayAccounts,
} from "./accountAutoRotation.ts";
import { accountRestartStore } from "./accountRestart.ts";
import type { AccountSwapReading } from "./accountSwap.ts";
import { activeWalletAccount, walletStore } from "./accountWallet.ts";
import { agentSessionStore } from "./agentSessions.ts";
import { ipc } from "./ipc.ts";
import type { ProviderPlanWindow } from "./providerPlanWindows.ts";
import { terminalActivityStore } from "./terminalActivityStore.ts";
import { errorText } from "./components/AccountWallet";

/** The result of the last automatic switch, for the card. */
export type AutoRotationNotice =
  | {
      kind: "switched";
      // The email of the account that Halv left.
      from: string;
      // The email of the new active account.
      to: string;
      usedPercent: number;
    }
  | { kind: "failed"; error: string };

export type AutoRotationState = {
  settings: AutoRotationSettings;
  // True while the rotation found no target in this crossing. The
  // assisted card then offers a switch.
  fallback: boolean;
  notice: AutoRotationNotice | null;
  dismiss: () => void;
};

// The accounts that Halv left in this app session. The memory outlives a
// remount of the status bar.
let rotatedAway: RotatedAwayAccounts = {};

function subscribeClaudePanes(listener: () => void) {
  const stopSessions = agentSessionStore.subscribe(listener);
  const stopActivity = terminalActivityStore.subscribe(listener);
  return () => {
    stopSessions();
    stopActivity();
  };
}

function claudePanesBusyNow() {
  return claudePanesBusy(agentSessionStore.getSnapshot().entries, (sessionId) =>
    terminalActivityStore.paneIsBusy(sessionId),
  );
}

export function useAccountAutoRotation({
  reading,
  windows,
  loaded,
  nowMs,
}: {
  // The Claude reading of the assisted card: only usage that belongs to
  // the active account.
  reading: AccountSwapReading;
  // The plan windows of the active Claude account.
  windows: readonly ProviderPlanWindow[];
  loaded: boolean;
  nowMs: number;
}): AutoRotationState {
  const settings = useSyncExternalStore(
    autoRotationSetting.subscribe,
    autoRotationSetting.getSnapshot,
    autoRotationSetting.getSnapshot,
  );
  // A change of activity runs the effect again, so a waiting rotation
  // starts when the turn ends.
  const busy = useSyncExternalStore(
    subscribeClaudePanes,
    claudePanesBusyNow,
    claudePanesBusyNow,
  );
  const watchRef = useRef(INITIAL_AUTO_ROTATION_WATCH);
  // True after the user closes a failure card in this crossing. Later
  // failures then stay quiet.
  const quietRef = useRef(false);
  const inputRef = useRef({ reading, windows });
  inputRef.current = { reading, windows };
  const [fallback, setFallback] = useState(false);
  const [notice, setNotice] = useState<AutoRotationNotice | null>(null);

  useEffect(() => {
    if (settings.enabled) return;
    watchRef.current = INITIAL_AUTO_ROTATION_WATCH;
    quietRef.current = false;
    setFallback(false);
    setNotice(null);
  }, [settings.enabled]);

  useEffect(() => {
    if (!loaded || !settings.enabled) return;
    const current = inputRef.current.reading;
    const step = nextAutoRotationStep(watchRef.current, {
      ...current,
      busy,
      nowMs: Date.now(),
      settings,
    });
    watchRef.current = step.watch;
    if (step.rearmed) {
      quietRef.current = false;
      setFallback(false);
      setNotice((shown) => (shown?.kind === "failed" ? null : shown));
    }
    if (step.rotate && current.usedPercent !== null) {
      void rotate(current.accountKey, current.usedPercent, settings.startPercent);
    }
  }, [busy, loaded, nowMs, reading.accountKey, reading.usedPercent, settings]);

  async function rotate(accountKey: string, usedPercent: number, startPercent: number) {
    const finish = (outcome: Parameters<typeof finishAutoRotation>[2]) => {
      watchRef.current = finishAutoRotation(watchRef.current, accountKey, outcome, Date.now());
    };
    try {
      const from = activeWalletAccount(walletStore.getSnapshot().state.claude);
      if (from?.id !== accountKey) return finish("cancelled");
      // Only Orca knows the usage of the accounts that are not active.
      const usage =
        walletStore.getSnapshot().state.store === "orca"
          ? await ipc.wallet.usage().catch(() => [])
          : [];
      // The active account can change while the usage loads.
      const state = walletStore.getSnapshot().state.claude;
      if (state.activeId !== accountKey) return finish("cancelled");
      const nowMs = Date.now();
      const target = autoRotationTarget({
        state,
        usage,
        activeUsedPercent: usedPercent,
        startPercent,
        rotatedAway: rotatedAwayIds(rotatedAway, nowMs),
        nowMs,
      });
      if (!target) {
        finish("noTarget");
        setFallback(true);
        return;
      }
      const until = rotatedAwayUntil(inputRef.current.windows, startPercent, nowMs);
      const result = await walletStore.select("claude", target.account.id);
      rotatedAway = rememberRotatedAway(rotatedAway, accountKey, until, Date.now());
      accountRestartStore.apply(result.staleSessions);
      // The user opted in to automatic switches, so the stale Claude panes
      // restart between turns without a question.
      accountRestartStore.queue(claudeStaleSessionIds(result.staleSessions));
      finish("switched");
      setNotice({ kind: "switched", from: from.email, to: target.account.email, usedPercent });
    } catch (cause) {
      finish("failed");
      if (!quietRef.current) setNotice({ kind: "failed", error: errorText(cause) });
    }
  }

  return {
    settings,
    fallback,
    notice,
    dismiss() {
      if (notice?.kind === "failed") quietRef.current = true;
      setNotice(null);
    },
  };
}
