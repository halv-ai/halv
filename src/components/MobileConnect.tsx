// The quick way to connect a phone (ADR 0009). The status bar button and the
// "Connect phone" command open one dialog. The dialog turns Mobile on, shows
// the pairing QR code at once, and lists the paired phones.
// Everything here shows only while the mobileRemote feature flag is on.
import {
  useEffect,
  useId,
  useRef,
  useState,
  useSyncExternalStore,
  type KeyboardEvent,
} from "react";
import { copyText } from "../clipboard";
import { confirmDialog } from "../dialogs";
import { featureFlagStore } from "../featureFlags";
import { getLocaleDefinition } from "../i18n/config";
import { useLocale, useT } from "../i18n/runtime";
import type { RemoteDevice } from "../ipc";
import { mobileConnectStore } from "../mobileConnect";
import {
  formatCountdown,
  pairingExpired,
  remoteErrorText,
  remoteStore,
} from "../remoteStore";
import {
  ConnectionChip,
  DeviceRow,
  PhoneIcon,
  RemotePairingApproval,
  useNow,
  useRemoteSnapshot,
} from "./SettingsMobile";

// The gateway removes an expired offer within 2 s.
const EXPIRY_SLACK_MS = 3_000;
const COPIED_RESET_MS = 2_000;
const COUNTDOWN_TICK_MS = 1_000;
const LAST_SEEN_TICK_MS = 30_000;

function useMobileRemoteFlag() {
  return useSyncExternalStore(
    featureFlagStore.subscribe,
    () => featureFlagStore.getSnapshot().mobileRemote,
  );
}

/**
 * Mounts the Mobile parts of the app. The flag activates the gateway in Rust:
 * with the flag off, Halv opens no relay connection, even with a stored preference.
 */
export function MobileRemoteHost() {
  const enabled = useMobileRemoteFlag();
  const activated = useRef(false);
  useEffect(() => {
    if (enabled) {
      activated.current = true;
      void remoteStore.setActive(true).catch(() => undefined);
    } else if (activated.current) {
      activated.current = false;
      mobileConnectStore.close();
      void remoteStore.setActive(false).catch(() => undefined);
    }
  }, [enabled]);
  if (!enabled) return null;
  return (
    <>
      <MobileConnectDialog />
      <RemotePairingApproval />
    </>
  );
}

const HINT_STORAGE_KEY = "halv.mobile-hint.v1";

function hintDismissed() {
  try {
    return window.localStorage.getItem(HINT_STORAGE_KEY) === "dismissed";
  } catch {
    return false;
  }
}

function dismissHint() {
  try {
    window.localStorage.setItem(HINT_STORAGE_KEY, "dismissed");
  } catch {
    // The hint shows again in the next session. That is acceptable.
  }
}

/**
 * The labeled phone button of the top bar, next to Commands. It is the main way to
 * connect a phone. A one-time hint points to it until a phone is paired.
 */
export function MobileHeaderButton() {
  const enabled = useMobileRemoteFlag();
  if (!enabled) return null;
  return <MobileHeaderButtonInner />;
}

function MobileHeaderButtonInner() {
  const t = useT();
  const { status } = useRemoteSnapshot(true);
  const [hintOpen, setHintOpen] = useState(false);
  const devices = status?.enabled ? status.devices : [];
  const connected = devices.some((device) => device.connected);
  const pairedCount = status?.devices.length ?? 0;

  useEffect(() => {
    if (status && pairedCount === 0 && !hintDismissed()) setHintOpen(true);
    if (pairedCount > 0) setHintOpen(false);
  }, [status, pairedCount]);

  function closeHint() {
    dismissHint();
    setHintOpen(false);
  }

  const label = connected
    ? t("Phone connected")
    : pairedCount > 0
      ? t("Phone")
      : t("Connect phone");
  return (
    <div className="agent-session-pro-toggle-anchor hidden shrink-0 sm:flex">
      <button
        aria-haspopup="dialog"
        className={`flex items-center gap-2 rounded-lg border px-3 py-2 text-sm transition-colors focus:outline-none focus:ring-2 focus:ring-accent/30 ${
          pairedCount === 0
            ? "border-accent/40 bg-accent/10 text-accent hover:bg-accent/15"
            : "border-surface-border/25 text-content-secondary hover:bg-surface-raised hover:text-content-primary"
        }`}
        onClick={() => {
          closeHint();
          mobileConnectStore.open();
        }}
        title={t("Connect your phone")}
        type="button"
      >
        <span className="relative">
          <PhoneIcon className="h-4 w-4" />
          {connected ? (
            <span
              aria-hidden="true"
              className="absolute -right-0.5 -top-0.5 h-2 w-2 rounded-full bg-accent ring-2 ring-surface-base"
            />
          ) : null}
        </span>
        {label}
      </button>
      {hintOpen ? (
        <div className="agent-session-pro-hint" role="status">
          <span>{t("New: follow and control your agents from your phone.")}</span>
          <button aria-label={t("Dismiss")} onClick={closeHint} type="button">
            ×
          </button>
        </div>
      ) : null}
    </div>
  );
}

/** The phone button of the status bar. It shows the state of paired phones only. */
export function MobileStatusButton() {
  const enabled = useMobileRemoteFlag();
  if (!enabled) return null;
  return <MobileStatusButtonInner />;
}

function MobileStatusButtonInner() {
  const t = useT();
  const { status } = useRemoteSnapshot(true);
  const devices = status?.enabled ? status.devices : [];
  const connected = devices.some((device) => device.connected);
  // The top bar has the button that connects the first phone.
  if (devices.length === 0) return null;
  const label = connected ? t("Phone connected") : t("Phone");
  return (
    <button
      aria-haspopup="dialog"
      className={`inline-flex h-5 shrink-0 items-center gap-1.5 rounded px-1.5 transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40 ${
        connected ? "text-accent" : "text-content-secondary"
      }`}
      onClick={() => mobileConnectStore.open()}
      title={t("Connect your phone")}
      type="button"
    >
      <span className="relative">
        <PhoneIcon className="h-3.5 w-3.5" />
        {connected ? (
          <span
            aria-hidden="true"
            className="absolute -right-0.5 -top-0.5 h-1.5 w-1.5 rounded-full bg-accent ring-2 ring-surface-sunken"
          />
        ) : null}
      </span>
      <span>{label}</span>
    </button>
  );
}

function MobileConnectDialog() {
  const open = useSyncExternalStore(
    mobileConnectStore.subscribe,
    mobileConnectStore.getSnapshot,
  );
  if (!open) return null;
  return <ConnectDialog onClose={() => mobileConnectStore.close()} />;
}

type DialogAction = "enable" | "pair" | "off" | `disconnect:${string}`;

function ConnectDialog({ onClose }: { onClose: () => void }) {
  const t = useT();
  const intlLocale = getLocaleDefinition(useLocale()).intlLocale;
  const { status, loadError, approval } = useRemoteSnapshot(true);
  const [busy, setBusy] = useState<DialogAction | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [expired, setExpired] = useState(false);
  const copiedTimer = useRef<number | undefined>(undefined);
  const autoEnabled = useRef(false);
  const autoPaired = useRef(false);
  const panelRef = useRef<HTMLDivElement>(null);
  const titleId = useId();

  const pairing = status?.pairing ?? null;
  const devices = status?.devices ?? [];
  const now = useNow(
    pairing !== null ? COUNTDOWN_TICK_MS : devices.length > 0 ? LAST_SEEN_TICK_MS : null,
  );
  const enabled = status?.enabled ?? false;
  const connected = enabled && status?.connection === "connected";

  async function run(action: DialogAction, task: () => Promise<string | null>) {
    if (busy) return;
    setBusy(action);
    setError(null);
    setNotice(null);
    try {
      setNotice(await task());
    } catch (cause) {
      setError(remoteErrorText(cause));
    } finally {
      setBusy(null);
    }
  }

  const startPairing = () =>
    run("pair", async () => {
      setCopied(false);
      setExpired(false);
      await remoteStore.startPairing();
      return null;
    });

  // Opening the dialog is the request to connect a phone: Mobile turns on by itself.
  useEffect(() => {
    if (!status || status.enabled || autoEnabled.current) return;
    autoEnabled.current = true;
    void run("enable", async () => {
      await remoteStore.setEnabled(true);
      return null;
    });
  });

  // Without a paired phone, the QR code shows as soon as the relay is connected.
  useEffect(() => {
    if (!connected || autoPaired.current || busy) return;
    if (pairing || approval || devices.length > 0) return;
    autoPaired.current = true;
    void startPairing();
  });

  // An offer that ends at its expiry stays on the screen as "Expired".
  const pairingExpiresAt = pairing?.expiresAt ?? null;
  const previousExpiresAt = useRef<number | null>(null);
  useEffect(() => {
    const previous = previousExpiresAt.current;
    previousExpiresAt.current = pairingExpiresAt;
    if (pairingExpiresAt !== null) setExpired(false);
    else if (previous !== null && Date.now() >= previous - EXPIRY_SLACK_MS) setExpired(true);
  }, [pairingExpiresAt]);

  // Closing the dialog ends an offer that no phone used.
  const latest = useRef({ pairing, approval });
  latest.current = { pairing, approval };
  useEffect(
    () => () => {
      window.clearTimeout(copiedTimer.current);
      if (latest.current.pairing && !latest.current.approval) {
        void remoteStore.cancelPairing().catch(() => undefined);
      }
    },
    [],
  );

  // Focus goes into the dialog, and back to the opener on close.
  useEffect(() => {
    const previousFocus =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    panelRef.current?.focus();
    return () => {
      window.requestAnimationFrame(() => previousFocus?.focus());
    };
  }, []);

  function handleKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      onClose();
      return;
    }
    if (event.key !== "Tab") return;
    const focusable = Array.from(
      panelRef.current?.querySelectorAll<HTMLElement>("button:not([disabled])") ?? [],
    );
    if (focusable.length === 0) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  async function copyPairingCode() {
    if (!pairing) return;
    setError(null);
    if (await copyText(pairing.uri)) {
      setCopied(true);
      window.clearTimeout(copiedTimer.current);
      copiedTimer.current = window.setTimeout(() => setCopied(false), COPIED_RESET_MS);
    } else {
      setError(t("Halv could not copy the code. Scan the QR code instead."));
    }
  }

  async function disconnect(device: RemoteDevice) {
    const confirmed = await confirmDialog({
      title: t("Disconnect {name}?", { name: device.name }),
      message: t(
        "This phone loses access to Halv at once. To use it again, pair it again.",
      ),
      confirmLabel: t("Disconnect"),
      danger: true,
    });
    if (!confirmed) return;
    await run(`disconnect:${device.id}`, async () => {
      await remoteStore.revoke(device.id);
      return t("{name} can no longer connect.", { name: device.name });
    });
  }

  const turnOff = () =>
    run("off", async () => {
      await remoteStore.setEnabled(false);
      onClose();
      return null;
    });

  const showExpired = expired || (pairing !== null && pairingExpired(pairing, now));
  const showQr = pairing !== null && !showExpired;
  const relayError = enabled ? status?.error ?? null : null;

  return (
    <div
      className="fixed inset-0 z-[90] flex items-center justify-center bg-black/40 p-4 backdrop-blur-sm"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        aria-labelledby={titleId}
        aria-modal="true"
        className="flex max-h-full w-full max-w-lg flex-col overflow-hidden rounded-xl border border-surface-border/35 bg-surface-glass/95 shadow-2xl shadow-black/30 outline-none"
        onKeyDown={handleKeyDown}
        ref={panelRef}
        role="dialog"
        tabIndex={-1}
      >
        <div className="flex items-center gap-3 border-b border-surface-border/25 px-5 py-4">
          <span className="grid h-9 w-9 shrink-0 place-items-center rounded-xl bg-accent/10 text-accent">
            <PhoneIcon className="h-5 w-5" />
          </span>
          <div className="flex min-w-0 flex-1 flex-wrap items-center gap-2">
            <h2 className="text-sm font-semibold text-content-primary" id={titleId}>
              {t("Connect your phone")}
            </h2>
            {status ? <ConnectionChip status={status} /> : null}
          </div>
          <button
            aria-label={t("Close")}
            className="grid h-7 w-7 shrink-0 place-items-center rounded-md text-content-muted transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40"
            onClick={onClose}
            type="button"
          >
            <svg aria-hidden="true" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeLinecap="round" strokeWidth="1.8" viewBox="0 0 12 12">
              <path d="m3 3 6 6M9 3 3 9" />
            </svg>
          </button>
        </div>

        <div className="space-y-5 overflow-y-auto px-5 py-5">
          {!status ? (
            loadError ? (
              <div className="space-y-3">
                <p className="text-xs text-danger" role="alert">
                  {loadError}
                </p>
                <button
                  className="settings-secondary-action"
                  onClick={() => void remoteStore.refresh().catch(() => undefined)}
                  type="button"
                >
                  {t("Try again")}
                </button>
              </div>
            ) : (
              <p className="text-sm text-content-muted" role="status">
                {t("Loading Mobile settings…")}
              </p>
            )
          ) : (
            <>
              {relayError ? (
                <p
                  className="rounded-md border border-danger/40 bg-danger-surface/55 px-3 py-2 text-xs leading-5 text-danger"
                  role="alert"
                >
                  {relayError}
                </p>
              ) : null}

              {showQr || showExpired || devices.length === 0 ? (
                <div className="flex flex-col gap-5 sm:flex-row sm:items-center">
                  {showQr ? (
                    <div className="w-fit shrink-0 self-center rounded-2xl bg-white p-3 shadow-sm ring-1 ring-black/10">
                      <img
                        alt={t("Pairing QR code")}
                        className="block h-48 w-48"
                        draggable={false}
                        src={pairing.qrSvgDataUri}
                      />
                    </div>
                  ) : (
                    <div
                      aria-hidden="true"
                      className="grid h-[13.5rem] w-[13.5rem] shrink-0 place-items-center self-center rounded-2xl border border-dashed border-surface-border/50 bg-surface-base/55 text-content-muted"
                    >
                      <PhoneIcon
                        className={`h-10 w-10 opacity-60 ${!showExpired && !relayError ? "animate-pulse" : ""}`}
                      />
                    </div>
                  )}
                  <div className="min-w-0 flex-1 space-y-3">
                    <ol className="list-decimal space-y-1.5 pl-4 text-sm leading-5 text-content-primary marker:text-content-muted">
                      <li>{t("Open Halv on your iPhone.")}</li>
                      <li>{t("Tap Pair a desktop, then Scan QR code.")}</li>
                      <li>{t("Check that both screens show the same code, then allow the phone.")}</li>
                    </ol>
                    {showQr ? (
                      <>
                        <p className="text-xs tabular-nums text-content-muted" role="timer">
                          {t("Expires in {time}", {
                            time: formatCountdown(pairing.expiresAt, now),
                          })}
                        </p>
                        <button
                          className="settings-secondary-action"
                          onClick={() => void copyPairingCode()}
                          type="button"
                        >
                          {copied ? t("Code copied") : t("Copy code")}
                        </button>
                      </>
                    ) : showExpired ? (
                      <div className="space-y-2">
                        <p className="text-xs leading-5 text-content-muted">
                          {t("This code expired. Make a new code to pair a phone.")}
                        </p>
                        <button
                          className="settings-secondary-action"
                          disabled={busy !== null || !connected}
                          onClick={() => void startPairing()}
                          type="button"
                        >
                          {busy === "pair" ? t("Starting…") : t("New code")}
                        </button>
                      </div>
                    ) : relayError ? (
                      <button
                        className="settings-secondary-action"
                        disabled={busy !== null}
                        onClick={() =>
                          void run("enable", async () => {
                            await remoteStore.setEnabled(true);
                            return null;
                          })
                        }
                        type="button"
                      >
                        {t("Try again")}
                      </button>
                    ) : (
                      <p className="text-xs text-content-muted" role="status">
                        {connected ? t("Starting…") : t("Connecting to the relay…")}
                      </p>
                    )}
                  </div>
                </div>
              ) : null}

              {devices.length > 0 ? (
                <section aria-label={t("Paired phones")} className="space-y-2">
                  <div className="flex items-center justify-between gap-3">
                    <h3 className="text-xs font-medium text-content-secondary">
                      {t("Paired phones")}
                    </h3>
                    {!showQr && !showExpired ? (
                      <button
                        className="settings-secondary-action"
                        disabled={busy !== null || !connected}
                        onClick={() => void startPairing()}
                        type="button"
                      >
                        {busy === "pair" ? t("Starting…") : t("Pair another phone")}
                      </button>
                    ) : null}
                  </div>
                  <ul className="space-y-2">
                    {devices.map((device) => (
                      <DeviceRow
                        action="disconnect"
                        busy={busy !== null}
                        device={device}
                        intlLocale={intlLocale}
                        key={device.id}
                        now={now}
                        onRevoke={() => void disconnect(device)}
                        revoking={busy === `disconnect:${device.id}`}
                        t={t}
                      />
                    ))}
                  </ul>
                </section>
              ) : null}

              {error ? (
                <p className="text-xs text-danger" role="alert">
                  {error}
                </p>
              ) : null}
              {notice ? (
                <p className="text-xs text-accent" role="status">
                  {notice}
                </p>
              ) : null}
            </>
          )}
        </div>

        <div className="flex items-center justify-between gap-2 border-t border-surface-border/25 bg-surface-raised/30 px-5 py-3">
          {enabled ? (
            <button
              className="rounded-md px-2 py-1.5 text-xs text-content-muted transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/30 disabled:opacity-50"
              disabled={busy !== null}
              onClick={() => void turnOff()}
              type="button"
            >
              {t("Turn off Mobile")}
            </button>
          ) : (
            <span />
          )}
          <button
            className="rounded-md border border-accent bg-accent px-3 py-1.5 text-sm font-medium text-accent-contrast transition-colors hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40"
            onClick={onClose}
            type="button"
          >
            {t("Done")}
          </button>
        </div>
      </div>
    </div>
  );
}
