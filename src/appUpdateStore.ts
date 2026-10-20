// App update store. Rust owns the update state (ADR 0012) and sends a full
// snapshot on every change. This store mirrors the last snapshot and keeps
// only view state: whether the user hid or opened the card.
import {
  APP_UPDATE_CARD_UI_DEFAULT,
  appUpdateCardKey,
  isAppUpdateCardVisible,
  type AppUpdateCardUi,
} from "./appUpdateCard.ts";
import { ipc } from "./ipc.ts";
import type { AppUpdateBeforeRestart, AppUpdateSnapshot } from "./types";

export const APP_UPDATE_SNAPSHOT_EVENT = "app_update://snapshot";
export const APP_UPDATE_BEFORE_RESTART_EVENT = "app_update://before-restart";
// Rust waits at most 1 second for the acknowledgement.
const FLUSH_TIMEOUT_MS = 800;

export type AppUpdateAction =
  | "check"
  | "download"
  | "restart"
  | "restartWhenIdle"
  | "cancelRestart"
  | "defer"
  | "autoDownload"
  | "dismissPostUpdate";

export type AppUpdateView = {
  // Null until the first snapshot arrives.
  snapshot: AppUpdateSnapshot | null;
  ui: AppUpdateCardUi;
  pending: AppUpdateAction | null;
  // The rejection of the last action. The next action clears it.
  actionError: unknown;
};

type Listener = () => void;
type Flush = () => unknown;

export class AppUpdateStore {
  private view: AppUpdateView = {
    snapshot: null,
    ui: APP_UPDATE_CARD_UI_DEFAULT,
    pending: null,
    actionError: null,
  };
  private listeners = new Set<Listener>();
  private flushes = new Set<Flush>();
  private started: Promise<void> | null = null;
  // Each event increments it, so an older load result never wins.
  private eventCount = 0;

  subscribe = (listener: Listener) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getSnapshot = () => this.view;

  private set(next: Partial<AppUpdateView>) {
    this.view = { ...this.view, ...next };
    for (const listener of this.listeners) listener();
  }

  /** Takes a snapshot from Rust. Every snapshot is complete. */
  commit(snapshot: AppUpdateSnapshot) {
    const previous = this.view.snapshot;
    if (previous && JSON.stringify(previous) === JSON.stringify(snapshot)) return;
    this.set({ snapshot });
  }

  /** Starts the listeners once, then loads the first snapshot. */
  ensureStarted() {
    this.started ??= this.start().catch(() => {
      this.started = null;
    });
    return this.started;
  }

  private async start() {
    await Promise.all([
      ipc.events.listen<AppUpdateSnapshot>(APP_UPDATE_SNAPSHOT_EVENT, (snapshot) => {
        this.eventCount += 1;
        this.commit(snapshot);
      }),
      ipc.events.listen<AppUpdateBeforeRestart>(
        APP_UPDATE_BEFORE_RESTART_EVENT,
        (payload) => void this.acknowledgeRestart(payload.requestId),
      ),
    ]);
    const eventsBefore = this.eventCount;
    const snapshot = await ipc.app.updateSnapshot();
    if (this.eventCount === eventsBefore) this.commit(snapshot);
  }

  /** Runs a save before Halv restarts for an update. */
  registerFlush(flush: Flush) {
    this.flushes.add(flush);
    return () => {
      this.flushes.delete(flush);
    };
  }

  async acknowledgeRestart(requestId: number) {
    const flushes = [...this.flushes].map((flush) =>
      Promise.resolve()
        .then(flush)
        .catch(() => undefined),
    );
    let timer: ReturnType<typeof setTimeout> | undefined;
    await Promise.race([
      Promise.all(flushes),
      new Promise((resolve) => {
        timer = setTimeout(resolve, FLUSH_TIMEOUT_MS);
      }),
    ]);
    clearTimeout(timer);
    await ipc.app.updateRestartAck({ requestId }).catch(() => undefined);
  }

  isCardVisible() {
    const { snapshot, ui } = this.view;
    return snapshot !== null && isAppUpdateCardVisible(snapshot, ui);
  }

  minimize() {
    const key = this.view.snapshot ? appUpdateCardKey(this.view.snapshot) : null;
    this.set({ ui: { minimizedKey: key, revealedKey: null }, actionError: null });
  }

  reveal() {
    const key = this.view.snapshot ? appUpdateCardKey(this.view.snapshot) : null;
    this.set({ ui: { minimizedKey: null, revealedKey: key } });
  }

  /** The status bar segment opens and hides the card. */
  toggleCard() {
    if (this.isCardVisible()) this.minimize();
    else this.reveal();
  }

  // Rust sends the new state as an event, so an action only records errors.
  private async run(action: AppUpdateAction, call: () => Promise<unknown>) {
    this.set({ pending: action, actionError: null });
    try {
      return await call();
    } catch (error) {
      this.set({ actionError: error });
      return undefined;
    } finally {
      if (this.view.pending === action) this.set({ pending: null });
    }
  }

  /** A user check. It shows the card again, also after "Later". */
  async check() {
    this.set({ ui: APP_UPDATE_CARD_UI_DEFAULT });
    await this.run("check", () => ipc.app.updateCheck({ userInitiated: true }));
  }

  async download() {
    await this.run("download", () => ipc.app.updateDownload());
  }

  /** Returns the blockers when work would stop, or null. */
  async restart(force = false) {
    const blockers = await this.run("restart", () => ipc.app.updateRestart({ force }));
    return blockers ?? null;
  }

  async restartWhenIdle() {
    await this.run("restartWhenIdle", () => ipc.app.updateRestartWhenIdle());
  }

  async cancelRestart() {
    await this.run("cancelRestart", () => ipc.app.updateCancelRestart());
  }

  /** "Later": hides the card for this version until the next start. */
  async later(version: string) {
    this.set({ ui: APP_UPDATE_CARD_UI_DEFAULT });
    await this.run("defer", () => ipc.app.updateDefer({ version }));
  }

  async setAutoDownload(enabled: boolean) {
    await this.run("autoDownload", () => ipc.app.updateSetAutoDownload({ enabled }));
  }

  async dismissPostUpdate() {
    await this.run("dismissPostUpdate", () => ipc.app.updateDismissPostUpdate());
  }
}

export const appUpdateStore = new AppUpdateStore();
