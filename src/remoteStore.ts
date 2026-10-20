// Settings > Mobile: the desktop remote gateway (ADR 0009).
// Rust owns the gateway; this store mirrors the last status it reported.
// The status holds the pairing URI, which holds the pairing secret.
// Never log the status, the URI, or the QR code.
// Every webview can read events, so the "remote-changed" event never holds the
// pairing offer. The main window reads the offer with the remote_status command.
import {
  ipc,
  type RemotePairing,
  type RemotePairingRequest,
  type RemoteStatus,
} from "./ipc.ts";

export const REMOTE_CHANGED_EVENT = "remote-changed";
export const REMOTE_PAIRING_REQUEST_EVENT = "remote-pairing-request";

export type RemoteSnapshot = {
  // Null until the first status arrives.
  status: RemoteStatus | null;
  loadError: string | null;
  // The pairing request that waits for the user. The "remote-pairing-request"
  // event opens it, and each status replaces it with its pendingApproval.
  approval: RemotePairingRequest | null;
};

type RemoteListener = () => void;

export const EMPTY_REMOTE_SNAPSHOT: RemoteSnapshot = {
  status: null,
  loadError: null,
  approval: null,
};

/**
 * Commit-based store, like the wallet store. An event increments the request
 * counter, so a status that was in flight during an event never replaces it.
 */
export class RemoteStore {
  private snapshot: RemoteSnapshot = EMPTY_REMOTE_SNAPSHOT;
  private listeners = new Set<RemoteListener>();
  private listening = false;
  private request = 0;

  subscribe = (listener: RemoteListener) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getSnapshot = () => this.snapshot;

  private publish(next: RemoteSnapshot) {
    this.snapshot = next;
    for (const listener of this.listeners) listener();
  }

  commit(status: RemoteStatus) {
    this.publish({
      status,
      loadError: null,
      approval: status.pendingApproval,
    });
  }

  /** Starts the event listeners once. It makes no IPC call. */
  listen() {
    if (this.listening) return;
    this.listening = true;
    void Promise.all([
      ipc.events.listen<RemoteStatus>(REMOTE_CHANGED_EVENT, (status) => {
        this.request += 1;
        // Keep the known offer until the command result tells if it still exists.
        this.commit({ ...status, pairing: this.snapshot.status?.pairing ?? null });
        void this.refresh().catch(() => undefined);
      }),
      ipc.events.listen<RemotePairingRequest>(
        REMOTE_PAIRING_REQUEST_EVENT,
        (approval) => {
          this.request += 1;
          this.publish({ ...this.snapshot, approval });
        },
      ),
    ]).catch(() => {
      this.listening = false;
    });
  }

  /** Starts the listeners and loads the status when no status is known. */
  ensureLoaded() {
    this.listen();
    if (!this.snapshot.status) void this.refresh().catch(() => undefined);
  }

  async refresh() {
    const request = ++this.request;
    try {
      const status = await ipc.remote.status();
      if (request === this.request) this.commit(status);
      return status;
    } catch (cause) {
      if (request === this.request) {
        this.publish({ ...this.snapshot, loadError: remoteErrorText(cause) });
      }
      throw cause;
    }
  }

  // A mutation result is newer than the snapshot, unless an event arrived
  // while the command ran. Then the event is at least as new.
  private async mutate(run: () => Promise<RemoteStatus>) {
    const request = ++this.request;
    const status = await run();
    if (request === this.request) this.commit(status);
    return status;
  }

  /** Activates the gateway while the `mobileRemote` flag is on. */
  setActive(active: boolean) {
    return this.mutate(() => ipc.remote.setActive({ active }));
  }

  setEnabled(enabled: boolean) {
    return this.mutate(() => ipc.remote.setEnabled({ enabled }));
  }

  setRelayUrl(relayUrl: string | null) {
    return this.mutate(() => ipc.remote.setRelayUrl({ relayUrl }));
  }

  async startPairing(): Promise<RemotePairing> {
    ++this.request;
    const pairing = await ipc.remote.pairingStart();
    // This call made the offer, so no event is newer: events never hold the offer.
    const status = this.snapshot.status;
    if (status) this.publish({ ...this.snapshot, status: { ...status, pairing } });
    return pairing;
  }

  cancelPairing() {
    return this.mutate(() => ipc.remote.pairingCancel());
  }

  respond(requestId: string, allow: boolean) {
    return this.mutate(() => ipc.remote.pairingRespond({ requestId, allow }));
  }

  revoke(deviceId: string) {
    return this.mutate(() => ipc.remote.deviceRevoke({ deviceId }));
  }

  resetIdentity() {
    return this.mutate(() => ipc.remote.identityReset());
  }
}

export const remoteStore = new RemoteStore();

export function remoteErrorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause);
}

/** Shows the 6-digit confirmation code as two groups, for example "028 809". */
export function formatConfirmationCode(code: string) {
  return /^\d{6}$/.test(code) ? `${code.slice(0, 3)} ${code.slice(3)}` : code;
}

/** Shows the time until `expiresAt` as m:ss. It rounds up and never goes below 0:00. */
export function formatCountdown(expiresAt: number, now: number) {
  const seconds = Math.max(0, Math.ceil((expiresAt - now) / 1000));
  const minutes = Math.floor(seconds / 60);
  return `${minutes}:${String(seconds % 60).padStart(2, "0")}`;
}

export function pairingExpired(pairing: RemotePairing, now: number) {
  return now >= pairing.expiresAt;
}

const PLATFORM_NAMES: Record<string, string> = {
  ios: "iOS",
  android: "Android",
};

/** The platform and the model of a phone, for example "iOS · iPhone 17". */
export function remotePlatformLabel(platform: string, model: string | null) {
  const name = PLATFORM_NAMES[platform.toLowerCase()] ?? platform;
  const detail = model?.trim();
  return detail ? `${name} · ${detail}` : name;
}

const RELATIVE_UNITS: ReadonlyArray<[Intl.RelativeTimeFormatUnit, number]> = [
  ["year", 365 * 24 * 60 * 60_000],
  ["month", 30 * 24 * 60 * 60_000],
  ["week", 7 * 24 * 60 * 60_000],
  ["day", 24 * 60 * 60_000],
  ["hour", 60 * 60_000],
  ["minute", 60_000],
];

/** Shows a past time relative to `now`, for example "5 minutes ago". */
export function formatTimeAgo(at: number, now: number, intlLocale: string) {
  const formatter = new Intl.RelativeTimeFormat(intlLocale, { numeric: "auto" });
  const elapsed = Math.max(0, now - at);
  for (const [unit, size] of RELATIVE_UNITS) {
    if (elapsed >= size) return formatter.format(-Math.floor(elapsed / size), unit);
  }
  return formatter.format(0, "second");
}
