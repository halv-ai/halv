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
import { getLocaleDefinition } from "../i18n/config";
import { formatDateTime, useLocale, useT, type Translate } from "../i18n/runtime";
import type { RemoteDevice, RemotePairingRequest, RemoteStatus } from "../ipc";
import {
  formatConfirmationCode,
  formatCountdown,
  formatTimeAgo,
  pairingExpired,
  remoteErrorText,
  remotePlatformLabel,
  remoteStore,
} from "../remoteStore";

type RemoteAction =
  | "toggle"
  | "pair"
  | "cancel"
  | "relay"
  | "reset"
  | `revoke:${string}`;

// The monitor of the gateway removes an expired offer within 2 s.
const EXPIRY_SLACK_MS = 3_000;
const COPIED_RESET_MS = 2_000;
const COUNTDOWN_TICK_MS = 1_000;
// "Last seen" shows minutes at best, so a slow clock is enough.
const LAST_SEEN_TICK_MS = 30_000;

/** The remote snapshot. The listeners start once; `load` also fetches the status. */
export function useRemoteSnapshot(load: boolean) {
  const snapshot = useSyncExternalStore(
    remoteStore.subscribe,
    remoteStore.getSnapshot,
    remoteStore.getSnapshot,
  );
  useEffect(() => {
    if (load) remoteStore.ensureLoaded();
    else remoteStore.listen();
  }, [load]);
  return snapshot;
}

/** The current time. It updates every `intervalMs`, or never when the interval is null. */
export function useNow(intervalMs: number | null) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (intervalMs === null) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(timer);
  }, [intervalMs]);
  return now;
}

export function PhoneIcon({ className = "h-[18px] w-[18px]" }: { className?: string }) {
  return (
    <svg
      aria-hidden="true"
      className={className}
      fill="none"
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      strokeWidth="1.6"
      viewBox="0 0 24 24"
    >
      <path d="M8 2.5h8a1.5 1.5 0 0 1 1.5 1.5v16a1.5 1.5 0 0 1-1.5 1.5H8A1.5 1.5 0 0 1 6.5 20V4A1.5 1.5 0 0 1 8 2.5ZM11 18.5h2" />
    </svg>
  );
}

const NEUTRAL_CHIP = "border-surface-border/40 bg-surface-raised/60 text-content-muted";

/** The relay chip shows each connection state: online, connecting, offline, or error. */
export function ConnectionChip({ status }: { status: RemoteStatus }) {
  const t = useT();
  // The gateway can report "off" for a short time after the switch turns on.
  const connection = status.enabled ? status.connection : "off";
  const tone =
    connection === "connected"
      ? { label: t("Online"), className: "border-accent/35 bg-accent/10 text-accent" }
      : connection === "error"
        ? { label: t("Error"), className: "border-danger/35 bg-danger/10 text-danger" }
        : connection === "connecting"
          ? { label: t("Connecting…"), className: NEUTRAL_CHIP }
          : { label: t("Offline"), className: NEUTRAL_CHIP };
  return (
    <span
      className={`inline-flex shrink-0 items-center gap-1.5 rounded-full border px-2 py-0.5 text-[11px] font-medium ${tone.className}`}
      role="status"
    >
      <span
        aria-hidden="true"
        className={`h-1.5 w-1.5 rounded-full bg-current ${connection === "connecting" ? "animate-pulse" : ""}`}
      />
      {tone.label}
    </span>
  );
}

export function SettingsMobile() {
  const t = useT();
  const intlLocale = getLocaleDefinition(useLocale()).intlLocale;
  const { status, loadError } = useRemoteSnapshot(true);
  const [busy, setBusy] = useState<RemoteAction | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const copiedTimer = useRef<number | undefined>(undefined);
  const [expired, setExpired] = useState(false);
  const [relayDraft, setRelayDraft] = useState("");
  const [relayDirty, setRelayDirty] = useState(false);
  const [relayError, setRelayError] = useState<string | null>(null);
  const [relayNotice, setRelayNotice] = useState<string | null>(null);
  const descriptionId = useId();
  const pairTitleId = useId();
  const relayInputId = useId();
  const relayHintId = useId();

  const pairing = status?.pairing ?? null;
  const devices = status?.devices ?? [];
  const now = useNow(
    pairing !== null
      ? COUNTDOWN_TICK_MS
      : devices.length > 0
        ? LAST_SEEN_TICK_MS
        : null,
  );

  useEffect(() => () => window.clearTimeout(copiedTimer.current), []);

  // An offer that ends at its expiry stays on the screen as "Expired".
  // An offer that ends early (allow, deny, cancel, or turn off) goes away.
  const pairingExpiresAt = pairing?.expiresAt ?? null;
  const previousExpiresAt = useRef<number | null>(null);
  useEffect(() => {
    const previous = previousExpiresAt.current;
    previousExpiresAt.current = pairingExpiresAt;
    if (pairingExpiresAt !== null) {
      setExpired(false);
    } else if (previous !== null && Date.now() >= previous - EXPIRY_SLACK_MS) {
      setExpired(true);
    }
  }, [pairingExpiresAt]);

  // The field shows a custom relay URL. It stays empty for the default.
  const savedRelayUrl = status && !status.relayUrlIsDefault ? status.relayUrl : "";
  useEffect(() => {
    if (!relayDirty) setRelayDraft(savedRelayUrl);
  }, [relayDirty, savedRelayUrl]);

  async function run(action: RemoteAction, task: () => Promise<string | null>) {
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

  if (!status) {
    return (
      <div className="space-y-3">
        {loadError ? (
          <>
            <p className="text-xs text-danger" role="alert">
              {loadError}
            </p>
            <button
              className="settings-secondary-action shrink-0 whitespace-nowrap"
              onClick={() => void remoteStore.refresh().catch(() => undefined)}
              type="button"
            >
              {t("Try again")}
            </button>
          </>
        ) : (
          <p className="text-sm text-content-muted" role="status">
            {t("Loading Mobile settings…")}
          </p>
        )}
      </div>
    );
  }

  const enabled = status.enabled;
  const connected = status.connection === "connected";
  const showExpired =
    expired || (pairing !== null && pairingExpired(pairing, now));

  const toggle = () =>
    run("toggle", async () => {
      await remoteStore.setEnabled(!enabled);
      return null;
    });

  const startPairing = () =>
    run("pair", async () => {
      setCopied(false);
      await remoteStore.startPairing();
      setExpired(false);
      return null;
    });

  const cancelPairing = () =>
    run("cancel", async () => {
      setExpired(false);
      if (status.pairing) await remoteStore.cancelPairing();
      return null;
    });

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

  async function revoke(device: RemoteDevice) {
    const confirmed = await confirmDialog({
      title: t("Revoke {name}?", { name: device.name }),
      message: t(
        "This phone loses access to Halv at once. To use it again, pair it again.",
      ),
      confirmLabel: t("Revoke"),
      danger: true,
    });
    if (!confirmed) return;
    await run(`revoke:${device.id}`, async () => {
      await remoteStore.revoke(device.id);
      return t("{name} can no longer connect.", { name: device.name });
    });
  }

  async function resetIdentity() {
    const confirmed = await confirmDialog({
      title: t("Reset remote identity?"),
      message: t(
        "Halv revokes every paired phone and makes a new remote identity for this computer. Each phone must scan a new code to connect again. You cannot undo this.",
      ),
      confirmLabel: t("Reset identity"),
      danger: true,
    });
    if (!confirmed) return;
    await run("reset", async () => {
      await remoteStore.resetIdentity();
      return t("Halv made a new remote identity. Pair your phones again.");
    });
  }

  async function saveRelayUrl(value: string | null) {
    if (busy) return;
    setBusy("relay");
    setRelayError(null);
    setRelayNotice(null);
    try {
      await remoteStore.setRelayUrl(value);
      setRelayDirty(false);
      setRelayNotice(t("Relay URL saved."));
    } catch (cause) {
      setRelayError(remoteErrorText(cause));
    } finally {
      setBusy(null);
    }
  }

  const relayDraftValue = relayDraft.trim() || null;
  const relayUnchanged = relayDraftValue === (savedRelayUrl || null);

  return (
    <div className="space-y-6">
      <div className="space-y-3">
        <div className="flex items-start justify-between gap-4">
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <h4 className="text-sm font-medium text-content-primary">
                {t("Mobile remote")}
              </h4>
              <ConnectionChip status={status} />
            </div>
            <p className="mt-1 text-xs leading-5 text-content-muted" id={descriptionId}>
              {t(
                "Watch your terminals and send input from your phone. Halv connects through an end-to-end encrypted relay that cannot read your terminals. When this is off, Halv makes no connection to the relay.",
              )}
            </p>
          </div>
          <button
            aria-checked={enabled}
            aria-describedby={descriptionId}
            aria-label={t("Mobile remote")}
            className={`relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors duration-150 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40 disabled:cursor-not-allowed disabled:opacity-60 ${
              enabled ? "bg-accent" : "bg-surface-border/50"
            }`}
            disabled={busy !== null}
            onClick={() => void toggle()}
            role="switch"
            type="button"
          >
            <span
              aria-hidden="true"
              className={`inline-block h-5 w-5 rounded-full bg-white shadow-sm transition-transform duration-150 ${
                enabled ? "translate-x-[22px]" : "translate-x-0.5"
              }`}
            />
          </button>
        </div>
        {enabled && status.error ? (
          <p
            className="rounded-md border border-danger/40 bg-danger-surface/55 px-3 py-2 text-xs leading-5 text-danger"
            role="alert"
          >
            {status.error}
          </p>
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
      </div>

      {enabled ? (
        <section
          aria-labelledby={pairTitleId}
          className="space-y-4 border-t border-surface-border/25 pt-5"
        >
          {pairing && !showExpired ? (
            <div className="flex flex-col gap-5 sm:flex-row sm:items-center">
              <div className="w-fit shrink-0 rounded-2xl bg-white p-3 shadow-sm ring-1 ring-black/10">
                <img
                  alt={t("Pairing QR code")}
                  className="block h-48 w-48"
                  draggable={false}
                  src={pairing.qrSvgDataUri}
                />
              </div>
              <div className="min-w-0 space-y-3">
                <h4
                  className="text-sm font-medium text-content-primary"
                  id={pairTitleId}
                >
                  {t("Scan with the Halv app")}
                </h4>
                <p className="text-xs leading-5 text-content-muted">
                  {t(
                    "Open Halv on your phone, start a pairing, and scan this code. Then Halv asks you to allow the phone.",
                  )}
                </p>
                <p
                  className="text-xs tabular-nums text-content-secondary"
                  role="timer"
                >
                  {t("Expires in {time}", {
                    time: formatCountdown(pairing.expiresAt, now),
                  })}
                </p>
                <div className="flex flex-wrap gap-2">
                  <button
                    aria-label={t("Copy pairing code")}
                    className="settings-secondary-action shrink-0 whitespace-nowrap"
                    onClick={() => void copyPairingCode()}
                    type="button"
                  >
                    {copied ? t("Code copied") : t("Copy code")}
                  </button>
                  <button
                    aria-label={t("Cancel pairing")}
                    className="settings-secondary-action shrink-0 whitespace-nowrap"
                    disabled={busy !== null}
                    onClick={() => void cancelPairing()}
                    type="button"
                  >
                    {t("Cancel")}
                  </button>
                </div>
                <p className="text-xs leading-5 text-content-muted">
                  {t(
                    "The code works for one phone. Use it only on your own phone.",
                  )}
                </p>
              </div>
            </div>
          ) : showExpired ? (
            <div className="flex flex-col gap-5 sm:flex-row sm:items-center">
              <div
                aria-hidden="true"
                className="grid h-[13.5rem] w-[13.5rem] shrink-0 place-items-center rounded-2xl border border-dashed border-surface-border/50 bg-surface-base/55 text-content-muted"
              >
                <PhoneIcon className="h-10 w-10 opacity-60" />
              </div>
              <div className="min-w-0 space-y-3">
                <h4
                  className="text-sm font-medium text-content-primary"
                  id={pairTitleId}
                >
                  {t("Expired")}
                </h4>
                <p className="text-xs leading-5 text-content-muted">
                  {t("This code expired. Make a new code to pair a phone.")}
                </p>
                <div className="flex flex-wrap gap-2">
                  <button
                    aria-label={t("Make a new pairing code")}
                    className="settings-secondary-action shrink-0 whitespace-nowrap"
                    disabled={busy !== null || !connected}
                    onClick={() => void startPairing()}
                    type="button"
                  >
                    {busy === "pair" ? t("Starting…") : t("New code")}
                  </button>
                  <button
                    aria-label={t("Cancel pairing")}
                    className="settings-secondary-action shrink-0 whitespace-nowrap"
                    disabled={busy !== null}
                    onClick={() => void cancelPairing()}
                    type="button"
                  >
                    {t("Cancel")}
                  </button>
                </div>
              </div>
            </div>
          ) : (
            <div className="flex flex-col items-start gap-3 sm:flex-row sm:items-center sm:justify-between sm:gap-5">
              <div className="min-w-0">
                <h4
                  className="text-sm font-medium text-content-primary"
                  id={pairTitleId}
                >
                  {t("Pair a phone")}
                </h4>
                <p className="mt-1 text-xs leading-5 text-content-muted">
                  {connected
                    ? t(
                        "Show a code for the Halv app to scan. Each code works for one phone and expires after 5 minutes.",
                      )
                    : t("Halv must connect to the relay before you can pair a phone.")}
                </p>
              </div>
              <button
                className="settings-secondary-action shrink-0 whitespace-nowrap"
                disabled={busy !== null || !connected}
                onClick={() => void startPairing()}
                type="button"
              >
                {busy === "pair" ? t("Starting…") : t("Pair a phone")}
              </button>
            </div>
          )}
          <p className="text-xs leading-5 text-content-muted">
            {t("Your phone shows this computer as {name}.", {
              name: status.hostName,
            })}
          </p>
        </section>
      ) : null}

      {enabled || devices.length > 0 ? (
        <section
          aria-label={t("Paired phones")}
          className="space-y-3 border-t border-surface-border/25 pt-5"
        >
          <h4 className="text-sm font-medium text-content-primary">
            {t("Paired phones")}
          </h4>
          {devices.length === 0 ? (
            <p className="text-xs text-content-muted">{t("No phones are paired.")}</p>
          ) : (
            <ul className="space-y-2">
              {devices.map((device) => (
                <DeviceRow
                  busy={busy !== null}
                  device={device}
                  intlLocale={intlLocale}
                  key={device.id}
                  now={now}
                  onRevoke={() => void revoke(device)}
                  revoking={busy === `revoke:${device.id}`}
                  t={t}
                />
              ))}
            </ul>
          )}
        </section>
      ) : null}

      <details className="group border-t border-surface-border/25 pt-5 text-xs text-content-muted">
        <summary className="flex w-fit cursor-pointer list-none items-center gap-1.5 font-medium text-content-secondary hover:text-content-primary">
          <svg
            aria-hidden="true"
            className="h-3 w-3 transition-transform duration-150 group-open:rotate-90"
            fill="none"
            stroke="currentColor"
            strokeLinecap="round"
            strokeLinejoin="round"
            strokeWidth="2"
            viewBox="0 0 12 12"
          >
            <path d="m4.5 2.5 3.5 3.5-3.5 3.5" />
          </svg>
          {t("Developer options")}
        </summary>
        <form
          className="mt-3 space-y-2"
          onSubmit={(event) => {
            event.preventDefault();
            void saveRelayUrl(relayDraftValue);
          }}
        >
          <label
            className="block font-medium text-content-secondary"
            htmlFor={relayInputId}
          >
            {t("Relay URL")}
          </label>
          <div className="flex flex-wrap gap-2">
            <input
              aria-describedby={relayHintId}
              autoComplete="off"
              className="min-w-0 flex-1 rounded-md border border-surface-border/30 bg-surface-raised px-3 py-2 font-mono text-xs text-content-primary outline-none placeholder:text-content-muted focus:border-accent focus:ring-2 focus:ring-accent/30"
              id={relayInputId}
              onChange={(event) => {
                setRelayDraft(event.target.value);
                setRelayDirty(true);
                setRelayError(null);
                setRelayNotice(null);
              }}
              placeholder={status.relayUrlIsDefault ? status.relayUrl : undefined}
              spellCheck={false}
              type="text"
              value={relayDraft}
            />
            <button
              aria-label={t("Save relay URL")}
              className="settings-secondary-action shrink-0 whitespace-nowrap"
              disabled={busy !== null || relayUnchanged}
              type="submit"
            >
              {t("Save")}
            </button>
            <button
              aria-label={t("Use the default relay")}
              className="settings-secondary-action shrink-0 whitespace-nowrap"
              disabled={busy !== null || status.relayUrlIsDefault}
              onClick={() => {
                setRelayDraft("");
                void saveRelayUrl(null);
              }}
              type="button"
            >
              {t("Use default")}
            </button>
          </div>
          <p className="leading-5" id={relayHintId}>
            {t("Leave this empty to use the default relay. Change it only for development.")}
          </p>
          {relayError ? (
            <p className="text-danger" role="alert">
              {relayError}
            </p>
          ) : null}
          {relayNotice ? (
            <p className="text-accent" role="status">
              {relayNotice}
            </p>
          ) : null}
        </form>
      </details>

      {enabled || devices.length > 0 ? (
        <section
          aria-label={t("Reset remote identity")}
          className="flex flex-wrap items-start justify-between gap-3 rounded-xl border border-danger/30 bg-danger-surface/25 px-4 py-3"
        >
          <div className="min-w-0 flex-1">
            <h4 className="text-sm font-medium text-danger">
              {t("Reset remote identity")}
            </h4>
            <p className="mt-1 text-xs leading-5 text-content-muted">
              {t(
                "Revokes every paired phone and makes a new identity for this computer. Use this if you think someone copied a pairing code.",
              )}
            </p>
          </div>
          <button
            aria-label={t("Reset remote identity")}
            className="shrink-0 whitespace-nowrap rounded-md border border-danger/40 px-3 py-1.5 text-xs font-medium text-danger transition-colors hover:bg-danger/10 focus:outline-none focus-visible:ring-2 focus-visible:ring-danger/30 disabled:cursor-not-allowed disabled:opacity-50"
            disabled={busy !== null}
            onClick={() => void resetIdentity()}
            type="button"
          >
            {busy === "reset" ? t("Resetting…") : t("Reset identity")}
          </button>
        </section>
      ) : null}
    </div>
  );
}

/** One paired phone. The "Connect phone" dialog names the action "Disconnect". */
export function DeviceRow({
  action = "revoke",
  busy,
  device,
  intlLocale,
  now,
  onRevoke,
  revoking,
  t,
}: {
  action?: "revoke" | "disconnect";
  busy: boolean;
  device: RemoteDevice;
  intlLocale: string;
  now: number;
  onRevoke: () => void;
  revoking: boolean;
  t: Translate;
}) {
  const paired = formatDateTime(device.pairedAt, { dateStyle: "medium" });
  return (
    <li className="flex items-center gap-3 rounded-xl border border-surface-border/40 bg-surface-base/55 px-4 py-3">
      <span className="grid h-9 w-9 shrink-0 place-items-center rounded-lg bg-accent/10 text-accent">
        <PhoneIcon />
      </span>
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm font-medium text-content-primary">{device.name}</p>
        <p className="mt-0.5 truncate text-xs text-content-muted">
          {remotePlatformLabel(device.platform, device.model)}
          {" · "}
          {t("Paired {date}", { date: paired })}
        </p>
        <p className="mt-0.5 text-xs">
          {device.connected ? (
            <span className="inline-flex items-center gap-1.5 text-accent">
              <span aria-hidden="true" className="h-1.5 w-1.5 rounded-full bg-current" />
              {t("Connected now")}
            </span>
          ) : device.lastSeenAt !== null ? (
            <span className="text-content-muted">
              {t("Last seen {time}", {
                time: formatTimeAgo(device.lastSeenAt, now, intlLocale),
              })}
            </span>
          ) : (
            <span className="text-content-muted">{t("Never connected")}</span>
          )}
        </p>
      </div>
      <button
        aria-label={
          action === "disconnect"
            ? t("Disconnect {name}", { name: device.name })
            : t("Revoke {name}", { name: device.name })
        }
        className="shrink-0 rounded-md border border-danger/30 px-2.5 py-1.5 text-xs text-danger transition-colors hover:bg-danger/10 focus:outline-none focus-visible:ring-2 focus-visible:ring-danger/30 disabled:cursor-not-allowed disabled:opacity-50"
        disabled={busy}
        onClick={onRevoke}
        type="button"
      >
        {action === "disconnect"
          ? revoking
            ? t("Disconnecting…")
            : t("Disconnect")
          : revoking
            ? t("Revoking…")
            : t("Revoke")}
      </button>
    </li>
  );
}

/**
 * The approval dialog for a phone that asks to pair (protocol section 7.3, step 8).
 * The app mounts it once, so it opens over any view.
 */
export function RemotePairingApproval() {
  const { approval } = useRemoteSnapshot(false);
  const now = useNow(approval !== null ? COUNTDOWN_TICK_MS : null);
  if (!approval || now >= approval.expiresAt) return null;
  return <ApprovalDialog key={approval.requestId} now={now} request={approval} />;
}

function ApprovalDialog({
  now,
  request,
}: {
  now: number;
  request: RemotePairingRequest;
}) {
  const t = useT();
  const titleId = useId();
  const descriptionId = useId();
  const panelRef = useRef<HTMLDivElement>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const previousFocus =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    return () => {
      window.requestAnimationFrame(() => previousFocus?.focus());
    };
  }, []);

  async function respond(allow: boolean) {
    if (pending) return;
    setPending(true);
    setError(null);
    try {
      await remoteStore.respond(request.requestId, allow);
    } catch (cause) {
      setError(remoteErrorText(cause));
      // The request can end in Rust first. A new status then closes the dialog.
      void remoteStore.refresh().catch(() => undefined);
    } finally {
      setPending(false);
    }
  }

  // Escape answers "deny", the safe answer. Tab stays inside the dialog.
  function handleKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      void respond(false);
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

  return (
    <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/40 p-4 backdrop-blur-sm">
      <div
        aria-describedby={descriptionId}
        aria-labelledby={titleId}
        aria-modal="true"
        className="w-full max-w-sm overflow-hidden rounded-xl border border-surface-border/35 bg-surface-glass/95 shadow-2xl shadow-black/30"
        onKeyDown={handleKeyDown}
        ref={panelRef}
        role="alertdialog"
      >
        <div className="space-y-4 px-5 py-5">
          <div className="flex items-start gap-3">
            <span className="grid h-10 w-10 shrink-0 place-items-center rounded-xl bg-accent/10 text-accent">
              <PhoneIcon className="h-5 w-5" />
            </span>
            <div className="min-w-0">
              <h2 className="text-sm font-semibold text-content-primary" id={titleId}>
                {t("Pair this phone?")}
              </h2>
              <p className="mt-0.5 break-words text-sm text-content-secondary">
                {request.name}
              </p>
              <p className="text-xs text-content-muted">
                {remotePlatformLabel(request.platform, request.model)}
              </p>
            </div>
          </div>
          <div className="rounded-lg border border-surface-border/35 bg-surface-raised/60 px-4 py-3 text-center">
            <p className="text-xs font-medium text-content-muted">
              {t("Confirmation code")}
            </p>
            <p className="mt-1 text-3xl font-semibold tabular-nums tracking-[0.12em] text-content-primary">
              {formatConfirmationCode(request.code)}
            </p>
          </div>
          <div className="space-y-1" id={descriptionId}>
            <p className="text-sm leading-5 text-content-primary">
              {t("Check that your phone shows the same code.")}
            </p>
            <p className="text-xs leading-5 text-content-muted">
              {t(
                "If the codes differ, or if you did not start this pairing, deny it.",
              )}
            </p>
          </div>
          <p className="text-xs tabular-nums text-content-muted" role="timer">
            {t("Halv denies the request in {time}.", {
              time: formatCountdown(request.expiresAt, now),
            })}
          </p>
          {error ? (
            <p className="text-xs text-danger" role="alert">
              {error}
            </p>
          ) : null}
        </div>
        <div className="flex justify-end gap-2 border-t border-surface-border/25 bg-surface-raised/30 px-5 py-3">
          <button
            autoFocus
            className="rounded-md border border-surface-border/35 px-3 py-2 text-sm text-content-secondary transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:opacity-50"
            disabled={pending}
            onClick={() => void respond(false)}
            type="button"
          >
            {t("Deny pairing")}
          </button>
          <button
            className="rounded-md border border-accent bg-accent px-3 py-2 text-sm font-medium text-accent-contrast transition-colors hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40 disabled:opacity-50"
            disabled={pending}
            onClick={() => void respond(true)}
            type="button"
          >
            {t("Allow pairing")}
          </button>
        </div>
      </div>
    </div>
  );
}
