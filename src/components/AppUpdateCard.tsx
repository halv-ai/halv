// The update card in the bottom-right corner. It shows on every screen,
// also over the Connect gate. Rust owns the update state (ADR 0012).
import {
  Component,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type FocusEvent,
  type ReactNode,
  type RefObject,
} from "react";
import {
  APP_UPDATE_CARD_LAYOUT,
  appUpdateCardBottom,
  appUpdateCardModel,
  appUpdateCardYields,
  isAppUpdateCardVisible,
  type AppUpdateCardAction,
  type AppUpdateCardModel,
} from "../appUpdateCard";
import { appUpdateStore, type AppUpdateAction } from "../appUpdateStore";
import { ipc } from "../ipc.ts";
import { formatNumber, useT } from "../i18n/runtime";

// The exit fade lasts 150 ms (see .app-update-card in styles.css).
const EXIT_MS = 150;

export function useAppUpdateView() {
  const view = useSyncExternalStore(
    appUpdateStore.subscribe,
    appUpdateStore.getSnapshot,
    appUpdateStore.getSnapshot,
  );
  useEffect(() => {
    void appUpdateStore.ensureStarted();
  }, []);
  return view;
}

const PENDING_FOR_ACTION: Partial<Record<AppUpdateCardAction["id"], AppUpdateAction>> = {
  download: "download",
  later: "defer",
  restart: "restart",
  restartAnyway: "restart",
  restartWhenDone: "restartWhenIdle",
  cancelRestart: "cancelRestart",
};

export function AppUpdateCard() {
  const t = useT();
  const view = useAppUpdateView();
  const { snapshot, ui, pending, actionError } = view;
  const visible = snapshot !== null && isAppUpdateCardVisible(snapshot, ui);
  const model =
    visible && snapshot
      ? appUpdateCardModel(snapshot, {
          t,
          formatNumber,
          debug: import.meta.env.DEV,
          actionError,
        })
      : null;

  // The last card stays on screen while it fades out.
  const lastModel = useRef<AppUpdateCardModel | null>(null);
  if (model) lastModel.current = model;
  const [gone, setGone] = useState(true);
  const hidden = model === null;
  useEffect(() => {
    if (!hidden) {
      setGone(false);
      return;
    }
    const timer = window.setTimeout(() => setGone(true), EXIT_MS);
    return () => window.clearTimeout(timer);
  }, [hidden]);

  useAutoHide(model);

  const card = model ?? (gone ? null : lastModel.current);
  const leaving = hidden && card !== null;
  const sectionRef = useRef<HTMLElement>(null);
  const { bottom, yielded } = useCardPlacement(sectionRef, card !== null);
  const focus = useCardFocus(sectionRef, card, hidden);
  if (!card) return null;

  function runAction(action: AppUpdateCardAction) {
    if (!snapshot) return;
    const { status } = snapshot;
    switch (action.id) {
      case "download":
        void appUpdateStore.download();
        return;
      case "later":
        if ("version" in status && typeof status.version === "string") {
          void appUpdateStore.later(status.version);
        }
        return;
      case "hide":
      case "dismiss":
        appUpdateStore.minimize();
        return;
      case "restart":
        void appUpdateStore.restart(false);
        return;
      case "restartAnyway":
        void appUpdateStore.restart(true);
        return;
      case "restartWhenDone":
        void appUpdateStore.restartWhenIdle();
        return;
      case "cancelRestart":
        void appUpdateStore.cancelRestart();
        return;
      case "retry":
        if (status.state !== "error") return;
        if (status.operation === "check") void appUpdateStore.check();
        else if (status.operation === "download") void appUpdateStore.download();
        else void appUpdateStore.restart(false);
        return;
    }
  }

  function close() {
    if (card?.close === "dismissPostUpdate") void appUpdateStore.dismissPostUpdate();
    else appUpdateStore.minimize();
  }

  // Pixels, not rem: the UI font size setting scales rem.
  return (
    <div
      className="app-update-card-region pointer-events-none fixed right-[16px] z-40 flex w-[360px] max-w-[calc(100vw-32px)] flex-col items-end max-[480px]:left-[16px] max-[480px]:w-auto"
      data-yielded={yielded ? "true" : "false"}
      style={{ bottom }}
    >
      <section
        aria-label={t("Halv update")}
        className="app-update-card pointer-events-auto w-full overflow-hidden rounded-lg border border-surface-border/30 bg-surface-panel text-content-primary shadow-2xl shadow-black/25 focus:outline-none"
        data-app-update-card={card.kind}
        data-leaving={leaving ? "true" : undefined}
        ref={sectionRef}
        onBlur={focus.onBlur}
        onFocus={focus.onFocus}
        onKeyDown={(event) => {
          if (event.key !== "Escape" || !model) return;
          event.stopPropagation();
          close();
        }}
        role="region"
        tabIndex={-1}
      >
        <div className="flex items-start gap-2.5 px-3.5 pb-3 pt-3">
          <CardIcon busy={card.busy} tone={card.tone} />
          <div aria-live="polite" className="min-w-0 flex-1" role="status">
            {card.title ? (
              <p className="text-[13px] font-semibold leading-5 text-content-primary">
                {card.title}
              </p>
            ) : null}
            {card.body ? (
              <p
                className={`text-xs leading-5 ${
                  card.title ? "mt-0.5 text-content-secondary" : "text-content-primary"
                }`}
              >
                {card.body}
              </p>
            ) : null}
            {card.alert ? (
              <p className="mt-1 text-xs leading-5 text-danger">{card.alert}</p>
            ) : null}
          </div>
          {card.close ? (
            <button
              aria-label={
                card.close === "dismissPostUpdate" ? t("Dismiss") : t("Hide update")
              }
              className="-mr-1 -mt-0.5 flex h-6 w-6 flex-none items-center justify-center rounded text-content-muted transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40"
              onClick={close}
              title={card.close === "dismissPostUpdate" ? t("Dismiss") : t("Hide update")}
              type="button"
            >
              <CloseIcon />
            </button>
          ) : null}
        </div>

        {card.list.length > 0 ? (
          <ul className="-mt-1 space-y-0.5 px-3.5 pb-3 pl-[42px] text-xs leading-5 text-content-primary">
            {card.list.map((line) => (
              <li className="flex items-center gap-2" key={line}>
                <span aria-hidden className="h-1 w-1 flex-none rounded-full bg-warning" />
                {line}
              </li>
            ))}
          </ul>
        ) : null}

        {card.notes.length > 0 ? (
          <ul className="-mt-1 space-y-1 px-3.5 pb-3 pl-[42px] text-xs leading-[18px] text-content-secondary">
            {card.notes.map((note) => (
              <li className="app-update-card-note relative pl-3" key={note}>
                <span
                  aria-hidden
                  className="absolute left-0 top-[7px] h-1 w-1 rounded-full bg-content-muted"
                />
                {note}
              </li>
            ))}
          </ul>
        ) : null}

        {card.progress ? (
          <div className="-mt-1 px-3.5 pb-3 pl-[42px]">
            <div
              aria-label={t("Update download progress")}
              aria-valuemax={100}
              aria-valuemin={0}
              aria-valuenow={card.progress.percent ?? undefined}
              aria-valuetext={card.progress.label}
              className="h-1.5 overflow-hidden rounded-full bg-surface-border/30"
              role="progressbar"
            >
              <div
                className={`h-full rounded-full bg-accent transition-[width] duration-200 ${
                  card.progress.percent === null ? "app-update-card-indeterminate w-1/3" : ""
                }`}
                style={
                  card.progress.percent === null
                    ? undefined
                    : { width: `${card.progress.percent}%` }
                }
              />
            </div>
            <p className="mt-1.5 font-mono text-[11px] tabular-nums text-content-muted">
              {card.progress.label}
            </p>
          </div>
        ) : null}

        {card.actions.length > 0 || card.link ? (
          <div className="flex flex-wrap items-center gap-2 border-t border-surface-border/20 bg-surface-sunken/40 px-3.5 py-2.5">
            {card.actions.map((action) => (
              <button
                className={actionClass(action.emphasis)}
                data-app-update-action={action.id}
                disabled={
                  leaving ||
                  (pending !== null && pending === PENDING_FOR_ACTION[action.id])
                }
                key={action.id}
                onClick={() => runAction(action)}
                type="button"
              >
                {action.label}
              </button>
            ))}
            {card.link ? (
              <button
                className="ml-auto rounded px-1 text-xs font-medium text-accent underline-offset-4 transition-colors hover:text-accent-strong hover:underline focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40"
                onClick={() => {
                  const url = card.link?.url;
                  if (url) void ipc.app.openExternalUrl({ url }).catch(() => undefined);
                }}
                type="button"
              >
                {card.link.label}
              </button>
            ) : null}
          </div>
        ) : null}
      </section>
    </div>
  );
}

type CardPlacement = { bottom: number; yielded: boolean };

const CARD_SPACE_PROPERTY = "--app-update-card-space";

/**
 * Keeps the card clear of the chat composer (data-update-card-avoid), and
 * hides it under an app menu (data-update-card-yield). The composer can
 * mount later, grow, or go away, so observers measure again, at most once
 * a frame. --app-update-card-space gives a screen without a status bar,
 * such as the Connect gate, room to scroll its controls clear of the card.
 */
function useCardPlacement(
  sectionRef: RefObject<HTMLElement>,
  shown: boolean,
) {
  const [placement, setPlacement] = useState<CardPlacement>({
    bottom: APP_UPDATE_CARD_LAYOUT.bottom,
    yielded: false,
  });
  // A layout effect places the card before the first paint.
  useLayoutEffect(() => {
    if (!shown) return;
    let frame = 0;
    let observed: Element | null = null;
    const resizeObserver = new ResizeObserver(() => schedule());
    const measure = () => {
      frame = 0;
      const avoid = document.querySelector<HTMLElement>("[data-update-card-avoid]");
      if (avoid !== observed) {
        if (observed) resizeObserver.unobserve(observed);
        if (avoid) resizeObserver.observe(avoid);
        observed = avoid;
      }
      const section = sectionRef.current;
      const height = section?.offsetHeight ?? 0;
      const bottom = appUpdateCardBottom(
        { width: window.innerWidth, height: window.innerHeight },
        height,
        avoid?.getBoundingClientRect() ?? null,
      );
      const sectionRect = section?.getBoundingClientRect();
      const cardRect = sectionRect
        ? {
            left: sectionRect.left,
            right: sectionRect.right,
            top: window.innerHeight - bottom - height,
            bottom: window.innerHeight - bottom,
          }
        : null;
      const menus = Array.from(
        document.querySelectorAll<HTMLElement>("[data-update-card-yield]"),
        (menu) => menu.getBoundingClientRect(),
      );
      const yielded = appUpdateCardYields(cardRect, menus);
      setPlacement((current) =>
        current.bottom === bottom && current.yielded === yielded
          ? current
          : { bottom, yielded },
      );
      document.documentElement.style.setProperty(
        CARD_SPACE_PROPERTY,
        `${bottom + height + APP_UPDATE_CARD_LAYOUT.gap}px`,
      );
    };
    const schedule = () => {
      if (!frame) frame = window.requestAnimationFrame(measure);
    };
    const mutationObserver = new MutationObserver(schedule);
    mutationObserver.observe(document.body, { childList: true, subtree: true });
    if (sectionRef.current) resizeObserver.observe(sectionRef.current);
    window.addEventListener("resize", schedule);
    measure();
    return () => {
      window.cancelAnimationFrame(frame);
      mutationObserver.disconnect();
      resizeObserver.disconnect();
      window.removeEventListener("resize", schedule);
      document.documentElement.style.removeProperty(CARD_SPACE_PROPERTY);
    };
  }, [sectionRef, shown]);
  return placement;
}

/**
 * Keeps keyboard focus in the card when its actions change, for example
 * from "ready" to the blockers step. When the card hides, focus goes back
 * to where it came from. The card never takes focus on its own.
 */
function useCardFocus(
  sectionRef: RefObject<HTMLElement>,
  card: AppUpdateCardModel | null,
  hidden: boolean,
) {
  const inside = useRef(false);
  const origin = useRef<HTMLElement | null>(null);
  const actionsKey = card
    ? `${card.key}|${card.actions.map((action) => action.id).join(",")}`
    : null;
  useLayoutEffect(() => {
    const section = sectionRef.current;
    if (!inside.current || !section) return;
    const active = document.activeElement;
    const lost = !active || active === document.body;
    if (hidden) {
      if (!lost && !section.contains(active)) return;
      inside.current = false;
      const back =
        origin.current?.isConnected === true
          ? origin.current
          : document.querySelector<HTMLElement>("[data-app-update-segment]");
      back?.focus();
      return;
    }
    if (!lost) return;
    const next =
      section.querySelector<HTMLElement>("[data-app-update-action]:not(:disabled)") ??
      section;
    next.focus();
  }, [actionsKey, hidden, sectionRef]);
  return {
    onFocus(event: FocusEvent<HTMLElement>) {
      const from = event.relatedTarget;
      if (!inside.current && from instanceof HTMLElement && !event.currentTarget.contains(from)) {
        origin.current = from;
      }
      inside.current = true;
    },
    onBlur(event: FocusEvent<HTMLElement>) {
      // A removed button blurs with no target. Focus then still belongs to
      // the card, so the layout effect can move it.
      const to = event.relatedTarget;
      if (to instanceof Node && !event.currentTarget.contains(to)) {
        inside.current = false;
      }
    },
  };
}

function useAutoHide(model: AppUpdateCardModel | null) {
  const key = model?.autoHideMs ? model.key : null;
  const delay = model?.autoHideMs ?? null;
  const kind = model?.kind;
  const timer = useRef<number | null>(null);
  useEffect(() => {
    if (!key || delay === null) return;
    timer.current = window.setTimeout(() => {
      if (kind === "postUpdate") void appUpdateStore.dismissPostUpdate();
      else appUpdateStore.minimize();
    }, delay);
    return () => {
      if (timer.current !== null) window.clearTimeout(timer.current);
    };
  }, [delay, key, kind]);
}

function actionClass(emphasis: AppUpdateCardAction["emphasis"]) {
  const base =
    "rounded-md px-2.5 py-1 text-xs font-medium transition-colors duration-150 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40 disabled:cursor-not-allowed disabled:opacity-60";
  switch (emphasis) {
    case "primary":
      return `${base} bg-accent font-semibold text-accent-contrast hover:bg-accent-strong`;
    case "secondary":
      return `${base} border border-surface-border/40 text-content-primary hover:bg-surface-raised`;
    case "quiet":
      return `${base} text-content-secondary hover:bg-surface-raised hover:text-content-primary`;
  }
}

function CardIcon({ busy, tone }: { busy: boolean; tone: AppUpdateCardModel["tone"] }) {
  if (busy) {
    return (
      <span aria-hidden className="mt-0.5 flex h-5 w-5 flex-none items-center justify-center">
        <span className="conversation-spinner h-3.5 w-3.5" />
      </span>
    );
  }
  const color =
    tone === "danger"
      ? "bg-danger/15 text-danger"
      : tone === "warning"
        ? "bg-warning/15 text-warning"
        : "bg-accent/15 text-accent";
  return (
    <span
      aria-hidden
      className={`mt-0.5 flex h-5 w-5 flex-none items-center justify-center rounded-full ${color}`}
    >
      {tone === "danger" || tone === "warning" ? (
        <svg fill="none" height="12" viewBox="0 0 12 12" width="12">
          <path d="M6 3v3.5M6 8.6v.1" stroke="currentColor" strokeLinecap="round" strokeWidth="1.6" />
        </svg>
      ) : (
        <svg fill="none" height="12" viewBox="0 0 12 12" width="12">
          <path
            d="M6 2.5v5.5M3.5 5.75 6 8.25l2.5-2.5"
            stroke="currentColor"
            strokeLinecap="round"
            strokeLinejoin="round"
            strokeWidth="1.5"
          />
          <path d="M3 9.75h6" stroke="currentColor" strokeLinecap="round" strokeWidth="1.5" />
        </svg>
      )}
    </span>
  );
}

function CloseIcon() {
  return (
    <svg aria-hidden fill="none" height="12" viewBox="0 0 12 12" width="12">
      <path d="m3 3 6 6M9 3 3 9" stroke="currentColor" strokeLinecap="round" strokeWidth="1.4" />
    </svg>
  );
}

/** A card failure never takes down the app. */
export class AppUpdateCardBoundary extends Component<
  { children: ReactNode },
  { failed: boolean }
> {
  state = { failed: false };

  static getDerivedStateFromError() {
    return { failed: true };
  }

  componentDidCatch(error: unknown) {
    console.error("The update card failed:", error);
  }

  render() {
    return this.state.failed ? null : this.props.children;
  }
}
