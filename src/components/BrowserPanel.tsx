import { useEffect, useRef, useState, type MouseEvent } from "react";
import { ipc } from "../ipc.ts";
import { confirmDialog } from "../dialogs";
import { useT, type Translate } from "../i18n/runtime";
import type {
  BrowserAnnotationTarget,
  BrowserSettings,
  BrowserSiteRule,
  BrowserStylePreview,
  BrowserUiState,
} from "../types";

type BrowserPanelProps = {
  open: boolean;
  openSettings: boolean;
  onClose: () => void;
  onSettingsOpened: () => void;
  onSendAnnotations: (message: string) => Promise<void>;
};

type SavedAnnotation = {
  target: BrowserAnnotationTarget;
  comment: string;
  style: BrowserStylePreview;
};

type BrowserPoint = {
  x: number;
  y: number;
  surfaceWidth: number;
  surfaceHeight: number;
};

type BrowserNavigation =
  | { kind: "navigate"; url: string; newTab: boolean }
  | {
      kind: "history";
      direction: "back" | "forward" | "reload";
      url: string;
    }
  | { kind: "click"; point: BrowserPoint; url: string };

type BrowserNavigationError = {
  message: string;
  request: BrowserNavigation;
  url: string;
};

const defaultSettings: BrowserSettings = {
  siteAccess: "always_ask",
  siteRules: [],
  fullCdpAccess: false,
};

const inputClass =
  "w-full rounded-lg border border-surface-border/35 bg-surface-base px-3 py-2 text-sm text-content-primary outline-none focus:border-accent/55 focus:ring-2 focus:ring-accent/15";

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function normalizedUrl(value: string) {
  const trimmed = value.trim();
  if (!trimmed) return "about:blank";
  if (/^https?:\/\//i.test(trimmed)) return trimmed;
  if (/^(localhost|127\.0\.0\.1|\[::1\])(?::\d+)?(?:\/|$)/i.test(trimmed)) {
    return `http://${trimmed}`;
  }
  return `https://${trimmed}`;
}

function styleSummary(style: BrowserStylePreview, t: Translate) {
  const values = {
    fontSize: style.fontSize ?? "",
    color: style.color ?? "",
    padding: style.padding ?? "",
    margin: style.margin ?? "",
  };
  const hasFontSize = style.fontSize != null;
  const hasColor = Boolean(style.color);
  const hasPadding = style.padding != null;
  const hasMargin = style.margin != null;

  if (hasFontSize && hasColor && hasPadding && hasMargin) {
    return t(
      "font-size {fontSize}px, color {color}, padding {padding}px, margin {margin}px",
      values,
    );
  }
  if (hasFontSize && hasColor && hasPadding) {
    return t("font-size {fontSize}px, color {color}, padding {padding}px", values);
  }
  if (hasFontSize && hasColor && hasMargin) {
    return t("font-size {fontSize}px, color {color}, margin {margin}px", values);
  }
  if (hasFontSize && hasPadding && hasMargin) {
    return t(
      "font-size {fontSize}px, padding {padding}px, margin {margin}px",
      values,
    );
  }
  if (hasColor && hasPadding && hasMargin) {
    return t("color {color}, padding {padding}px, margin {margin}px", values);
  }
  if (hasFontSize && hasColor) {
    return t("font-size {fontSize}px, color {color}", values);
  }
  if (hasFontSize && hasPadding) {
    return t("font-size {fontSize}px, padding {padding}px", values);
  }
  if (hasFontSize && hasMargin) {
    return t("font-size {fontSize}px, margin {margin}px", values);
  }
  if (hasColor && hasPadding) {
    return t("color {color}, padding {padding}px", values);
  }
  if (hasColor && hasMargin) {
    return t("color {color}, margin {margin}px", values);
  }
  if (hasPadding && hasMargin) {
    return t("padding {padding}px, margin {margin}px", values);
  }
  if (hasFontSize) {
    return t("font-size {fontSize}px", values);
  }
  if (hasColor) {
    return t("color {color}", values);
  }
  if (hasPadding) {
    return t("padding {padding}px", values);
  }
  if (hasMargin) {
    return t("margin {margin}px", values);
  }
  return "";
}

function rawStyleSummary(style: BrowserStylePreview) {
  const parts = [
    style.fontSize != null ? `font-size ${style.fontSize}px` : null,
    style.color ? `color ${style.color}` : null,
    style.padding != null ? `padding ${style.padding}px` : null,
    style.margin != null ? `margin ${style.margin}px` : null,
  ].filter(Boolean);
  return parts.join(", ");
}

export function BrowserPanel({
  open,
  openSettings,
  onClose,
  onSettingsOpened,
  onSendAnnotations,
}: BrowserPanelProps) {
  const t = useT();
  const [state, setState] = useState<BrowserUiState | null>(null);
  const [url, setUrl] = useState("http://localhost:3000");
  const [pageText, setPageText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [navigationInFlight, setNavigationInFlight] = useState(false);
  const [navigationError, setNavigationError] =
    useState<BrowserNavigationError | null>(null);
  const [annotationMode, setAnnotationMode] = useState(false);
  const [annotationTarget, setAnnotationTarget] =
    useState<BrowserAnnotationTarget | null>(null);
  const [annotationComment, setAnnotationComment] = useState("");
  const [annotationStyle, setAnnotationStyle] =
    useState<BrowserStylePreview>({});
  const [annotations, setAnnotations] = useState<SavedAnnotation[]>([]);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settings, setSettings] = useState<BrowserSettings>(defaultSettings);
  const [ruleHostname, setRuleHostname] = useState("");
  const [ruleDecision, setRuleDecision] =
    useState<BrowserSiteRule["decision"]>("allow");
  const [sending, setSending] = useState(false);
  const imageRef = useRef<HTMLImageElement>(null);
  const refreshingRef = useRef(false);
  const busyRef = useRef(false);
  const addressFocusedRef = useRef(false);
  const navigationRequestRef = useRef(0);

  busyRef.current = busy;

  async function refresh(silent = false) {
    if (refreshingRef.current || busyRef.current) return;
    refreshingRef.current = true;
    if (!silent) setBusy(true);
    if (!silent) setError(null);
    try {
      const next = await ipc.browser.uiState();
      setState(next);
      if (!addressFocusedRef.current) {
        setUrl(next.url === "about:blank" ? "" : next.url);
      }
    } catch (cause) {
      if (!silent) setError(errorMessage(cause));
    } finally {
      refreshingRef.current = false;
      if (!silent) setBusy(false);
    }
  }

  async function loadSettings() {
    try {
      setSettings(await ipc.browser.settingsGet());
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }

  useEffect(() => {
    if (!open) return;
    void refresh();
    void loadSettings();
    const interval = window.setInterval(() => {
      if (document.visibilityState === "visible") void refresh(true);
    }, 2_000);
    return () => window.clearInterval(interval);
  }, [open]);

  useEffect(() => {
    if (open) return;
    navigationRequestRef.current += 1;
    setBusy(false);
    busyRef.current = false;
    setNavigationInFlight(false);
    setNavigationError(null);
  }, [open]);

  useEffect(() => {
    if (!open || !openSettings) return;
    setSettingsOpen(true);
    setAnnotationMode(false);
    setAnnotationTarget(null);
    onSettingsOpened();
  }, [onSettingsOpened, open, openSettings]);

  async function mutate(action: () => Promise<BrowserUiState>) {
    setBusy(true);
    busyRef.current = true;
    setError(null);
    try {
      const next = await action();
      setState(next);
      setUrl(next.url === "about:blank" ? "" : next.url);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
      busyRef.current = false;
    }
  }

  async function runNavigation(request: BrowserNavigation) {
    const requestId = navigationRequestRef.current + 1;
    navigationRequestRef.current = requestId;
    setBusy(true);
    busyRef.current = true;
    setError(null);
    setNavigationError(null);
    setNavigationInFlight(true);
    try {
      const next =
        request.kind === "navigate"
          ? await ipc.browser.uiNavigate({
              url: request.url,
              newTab: request.newTab,
            })
          : request.kind === "history"
            ? await ipc.browser.uiHistory({ direction: request.direction })
            : await ipc.browser.uiClick(request.point);
      if (navigationRequestRef.current !== requestId) return;
      setState(next);
      setUrl(next.url === "about:blank" ? "" : next.url);
    } catch (cause) {
      if (navigationRequestRef.current !== requestId) return;
      setNavigationError({
        message: errorMessage(cause),
        request,
        url: request.url,
      });
    } finally {
      if (navigationRequestRef.current === requestId) {
        setBusy(false);
        busyRef.current = false;
        setNavigationInFlight(false);
      }
    }
  }

  function navigate(newTab = false) {
    const target = normalizedUrl(url);
    if (target === "about:blank") return;
    void runNavigation({ kind: "navigate", url: target, newTab });
  }

  function browserPoint(event: MouseEvent<HTMLImageElement>) {
    const rect = event.currentTarget.getBoundingClientRect();
    return {
      x: event.clientX - rect.left,
      y: event.clientY - rect.top,
      surfaceWidth: rect.width,
      surfaceHeight: rect.height,
    };
  }

  async function handleSurfaceClick(event: MouseEvent<HTMLImageElement>) {
    if (busy) return;
    const point = browserPoint(event);
    if (annotationMode) {
      setBusy(true);
      setError(null);
      try {
        setAnnotationTarget(
          await ipc.browser.uiAnnotationTarget(point),
        );
        setAnnotationComment("");
        setAnnotationStyle({});
        setSettingsOpen(false);
      } catch (cause) {
        setError(errorMessage(cause));
      } finally {
        setBusy(false);
      }
      return;
    }
    await runNavigation({
      kind: "click",
      point,
      url: state?.url ?? normalizedUrl(url),
    });
  }

  function saveAnnotation() {
    if (!annotationTarget) return;
    const comment = annotationComment.trim();
    if (!comment && !styleSummary(annotationStyle, t)) {
      setError(
        t("Add a comment or a style adjustment before saving the annotation."),
      );
      return;
    }
    setAnnotations((current) => [
      ...current,
      { target: annotationTarget, comment, style: annotationStyle },
    ]);
    setAnnotationTarget(null);
    setAnnotationComment("");
    setAnnotationStyle({});
  }

  async function previewStyle() {
    if (!annotationTarget) return;
    await mutate(() =>
      ipc.browser.uiPreviewStyle({
        annotationId: annotationTarget.annotationId,
        style: annotationStyle,
      }),
    );
  }

  async function sendAnnotations() {
    if (annotations.length === 0 || sending) return;
    setSending(true);
    setError(null);
    const lines = annotations.map(({ target, comment, style }, index) => {
      const adjustment = rawStyleSummary(style);
      return [
        `${index + 1}. ${target.selector} (${target.tag})`,
        target.text ? `Visible text: ${target.text}` : null,
        comment ? `Comment: ${comment}` : null,
        adjustment ? `Requested style adjustment: ${adjustment}` : null,
        `Viewport rect: x=${Math.round(target.rect.x)}, y=${Math.round(target.rect.y)}, width=${Math.round(target.rect.width)}, height=${Math.round(target.rect.height)}`,
      ]
        .filter(Boolean)
        .join("\n   ");
    });
    try {
      await onSendAnnotations(
        `Address these browser annotations on ${state?.url ?? annotations[0].target.url}. Keep the requested scope narrow.\n\n${lines.join("\n\n")}`,
      );
      setAnnotations([]);
      setAnnotationTarget(null);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setSending(false);
    }
  }

  async function saveSettings() {
    setBusy(true);
    setError(null);
    try {
      setSettings(
        await ipc.browser.settingsSet({ settings }),
      );
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  function addRule() {
    let hostname = ruleHostname.trim().toLowerCase();
    try {
      if (hostname.includes("://")) hostname = new URL(hostname).hostname;
    } catch {
      // The backend returns the canonical validation error on save.
    }
    if (!hostname) return;
    setSettings((current) => ({
      ...current,
      siteRules: [
        ...current.siteRules.filter((rule) => rule.hostname !== hostname),
        { hostname, decision: ruleDecision },
      ].sort((left, right) => left.hostname.localeCompare(right.hostname)),
    }));
    setRuleHostname("");
  }

  if (!open) return null;

  const renderedTargets = [
    ...annotations.map((annotation) => annotation.target),
    ...(annotationTarget ? [annotationTarget] : []),
  ];

  return (
    <section
      aria-label={t("Shared browser")}
      aria-busy={navigationInFlight}
      className="flex min-h-0 flex-1 flex-col bg-surface-sunken"
    >
      <div className="flex min-h-10 items-center gap-1 overflow-x-auto border-b border-surface-border/30 bg-surface-panel/70 px-2 py-1">
        {state?.tabs.map((tab) => (
          <div
            className={`flex min-w-32 max-w-56 items-center rounded-lg border ${
              tab.active
                ? "border-accent/45 bg-accent/10"
                : "border-surface-border/25 bg-surface-base/70"
            }`}
            key={tab.id}
          >
            <button
              className="min-w-0 flex-1 truncate px-2.5 py-1.5 text-left text-xs text-content-secondary"
              disabled={busy}
              onClick={() =>
                void mutate(() =>
                  ipc.browser.uiSelectTab({
                    targetId: tab.id,
                  }),
                )
              }
              title={
                tab.title
                  ? t("{title} · {url}", { title: tab.title, url: tab.url })
                  : t("New tab · {url}", { url: tab.url })
              }
              type="button"
            >
              {tab.title || t("New tab")}
            </button>
            <button
              aria-label={
                tab.title
                  ? t("Close {title}", { title: tab.title })
                  : t("Close tab")
              }
              className="px-2 text-content-muted hover:text-danger disabled:opacity-50"
              disabled={busy || (state?.tabs.length ?? 0) <= 1}
              onClick={() =>
                void mutate(() =>
                  ipc.browser.uiCloseTab({
                    targetId: tab.id,
                  }),
                )
              }
              type="button"
            >
              ×
            </button>
          </div>
        ))}
        <button
          aria-label={t("New browser tab")}
          className="rounded-md px-2 py-1 text-content-muted hover:bg-surface-raised hover:text-content-primary"
          disabled={busy}
          onClick={() => navigate(true)}
          type="button"
        >
          +
        </button>
      </div>

      <div className="flex items-center gap-2 border-b border-surface-border/30 bg-surface-base px-3 py-2">
        {(["back", "forward", "reload"] as const).map((direction) => (
          <button
            aria-label={browserHistoryDirectionLabel(direction, t)}
            className="rounded-md px-2 py-1 text-content-muted hover:bg-surface-raised hover:text-content-primary disabled:opacity-50"
            disabled={busy}
            key={direction}
            onClick={() =>
              void runNavigation({
                kind: "history",
                direction,
                url: state?.url ?? normalizedUrl(url),
              })
            }
            type="button"
          >
            {direction === "back" ? "←" : direction === "forward" ? "→" : "↻"}
          </button>
        ))}
        <form
          className="flex min-w-0 flex-1 gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            navigate(false);
          }}
        >
          <input
            aria-label={t("Browser address")}
            className={`${inputClass} min-w-0 flex-1 font-mono text-xs`}
            onChange={(event) => setUrl(event.target.value)}
            onBlur={() => {
              addressFocusedRef.current = false;
            }}
            onFocus={() => {
              addressFocusedRef.current = true;
            }}
            placeholder="https://example.com"
            value={url}
          />
          <button
            className="rounded-lg border border-surface-border/30 px-3 py-1.5 text-xs text-content-secondary hover:bg-surface-raised disabled:opacity-50"
            disabled={busy}
            type="submit"
          >
            {t("Go")}
          </button>
        </form>
        <button
          aria-pressed={annotationMode}
          className={`rounded-lg border px-3 py-1.5 text-xs ${
            annotationMode
              ? "border-accent/50 bg-accent/15 text-accent"
              : "border-surface-border/30 text-content-secondary hover:bg-surface-raised"
          }`}
          onClick={() => {
            setAnnotationMode((current) => !current);
            setSettingsOpen(false);
          }}
          type="button"
        >
          {annotations.length
            ? t("Annotate {count}", { count: annotations.length })
            : t("Annotate")}
        </button>
        <button
          aria-pressed={settingsOpen}
          className="rounded-lg border border-surface-border/30 px-3 py-1.5 text-xs text-content-secondary hover:bg-surface-raised"
          onClick={() => {
            setSettingsOpen((current) => !current);
            setAnnotationTarget(null);
          }}
          type="button"
        >
          {t("Browser settings")}
        </button>
        <button
          className="rounded-lg px-2 py-1 text-content-muted hover:bg-surface-raised hover:text-content-primary"
          onClick={onClose}
          type="button"
        >
          ×
        </button>
      </div>

      {navigationInFlight ? (
        <div
          aria-label={t("Loading page…")}
          aria-live="polite"
          className="h-0.5 overflow-hidden bg-surface-border/20"
          role="status"
        >
          <div className="h-full w-full animate-pulse bg-accent" />
        </div>
      ) : null}

      {navigationError ? (
        <div
          className="flex items-start gap-3 border-b border-danger/25 bg-danger/10 px-4 py-2 text-xs text-danger"
          role="alert"
        >
          <div className="min-w-0 flex-1">
            <p className="break-all font-medium">
              {t("Could not load {url}", { url: navigationError.url })}
            </p>
            <p className="mt-0.5 break-words text-[10px] opacity-80">
              {navigationError.message}
            </p>
          </div>
          <button
            className="shrink-0 rounded-md border border-danger/30 px-2 py-1 font-medium hover:bg-danger/10"
            onClick={() => void runNavigation(navigationError.request)}
            type="button"
          >
            {t("Retry")}
          </button>
        </div>
      ) : null}

      {error ? (
        <div
          className="border-b border-danger/25 bg-danger/10 px-4 py-2 text-xs text-danger"
          role="alert"
        >
          {error}
        </div>
      ) : null}

      <div className="grid min-h-0 flex-1 grid-cols-[minmax(0,1fr)_auto]">
        <div className="min-h-0 overflow-auto bg-[#1b1b1b] p-3">
          {state && state.url !== "about:blank" ? (
            <div className="relative mx-auto w-full max-w-[1400px] overflow-hidden rounded-lg bg-white shadow-2xl">
              <img
                alt={t("Browser preview of {page}", {
                  page: state.title || state.url,
                })}
                className={`block h-auto w-full select-none ${annotationMode ? "cursor-crosshair" : "cursor-pointer"}`}
                draggable={false}
                onClick={(event) => void handleSurfaceClick(event)}
                ref={imageRef}
                src={state.screenshotDataUrl}
              />
              <div className="pointer-events-none absolute inset-0">
                {renderedTargets.map((target, index) => (
                  <div
                    className={`absolute border-2 ${
                      target.annotationId === annotationTarget?.annotationId
                        ? "border-warning bg-warning/15"
                        : "border-accent bg-accent/10"
                    }`}
                    key={target.annotationId}
                    style={{
                      left: `${(target.rect.x / state.viewportWidth) * 100}%`,
                      top: `${(target.rect.y / state.viewportHeight) * 100}%`,
                      width: `${(target.rect.width / state.viewportWidth) * 100}%`,
                      height: `${(target.rect.height / state.viewportHeight) * 100}%`,
                    }}
                  >
                    <span className="absolute -left-1 -top-5 rounded bg-accent px-1.5 py-0.5 font-mono text-[9px] text-accent-contrast">
                      {index + 1}
                    </span>
                  </div>
                ))}
              </div>
              {busy && !navigationInFlight ? (
                <div className="absolute inset-0 flex items-center justify-center bg-black/20 text-sm text-white backdrop-blur-[1px]">
                  {t("Updating browser…")}
                </div>
              ) : null}
            </div>
          ) : (
            <div
              aria-live="polite"
              className="flex h-full min-h-80 items-center justify-center text-center text-sm text-content-muted"
            >
              {busy ? (
                t("Starting dedicated browser…")
              ) : state?.url === "about:blank" ? (
                <div>
                  <p className="font-medium text-content-secondary">
                    {t("No page loaded yet")}
                  </p>
                  <p className="mt-1 text-xs">
                    {t("Enter an address above to start browsing.")}
                  </p>
                </div>
              ) : (
                t("Browser preview unavailable")
              )}
            </div>
          )}
        </div>

        {annotationTarget ? (
          <aside className="w-80 overflow-y-auto border-l border-surface-border/30 bg-surface-base p-4">
            <h3 className="text-sm font-semibold text-content-primary">
              {t("Page annotation")}
            </h3>
            <p className="mt-1 break-all font-mono text-[10px] text-content-muted">
              {annotationTarget.selector}
            </p>
            {annotationTarget.text ? (
              <p className="mt-2 line-clamp-3 text-xs text-content-secondary">
                “{annotationTarget.text}”
              </p>
            ) : null}
            <label className="mt-4 block text-xs text-content-muted">
              {t("Comment")}
              <textarea
                autoFocus
                className={`${inputClass} mt-1 min-h-28 resize-y`}
                onChange={(event) => setAnnotationComment(event.target.value)}
                placeholder={t("What should change here?")}
                value={annotationComment}
              />
            </label>
            <details className="mt-4 rounded-lg border border-surface-border/30 bg-surface-panel/50 p-3">
              <summary className="cursor-pointer text-xs font-medium text-content-primary">
                {t("Adjust style")}
              </summary>
              <div className="mt-3 grid grid-cols-2 gap-2">
                <StyleNumber
                  label={t("Font px")}
                  onChange={(fontSize) =>
                    setAnnotationStyle((current) => ({ ...current, fontSize }))
                  }
                  value={annotationStyle.fontSize}
                />
                <label className="text-[10px] text-content-muted">
                  {t("Color")}
                  <input
                    className={`${inputClass} mt-1 h-9 p-1`}
                    onChange={(event) =>
                      setAnnotationStyle((current) => ({
                        ...current,
                        color: event.target.value,
                      }))
                    }
                    type="color"
                    value={annotationStyle.color ?? "#111111"}
                  />
                </label>
                <StyleNumber
                  label={t("Padding px")}
                  onChange={(padding) =>
                    setAnnotationStyle((current) => ({ ...current, padding }))
                  }
                  value={annotationStyle.padding}
                />
                <StyleNumber
                  label={t("Margin px")}
                  onChange={(margin) =>
                    setAnnotationStyle((current) => ({ ...current, margin }))
                  }
                  value={annotationStyle.margin}
                />
              </div>
              <button
                className="mt-3 w-full rounded-lg border border-accent/35 px-3 py-2 text-xs text-accent hover:bg-accent/10 disabled:opacity-50"
                disabled={busy}
                onClick={() => void previewStyle()}
                type="button"
              >
                {t("Preview on page")}
              </button>
            </details>
            <div className="mt-4 flex gap-2">
              <button
                className="flex-1 rounded-lg bg-accent px-3 py-2 text-xs font-semibold text-accent-contrast hover:bg-accent-strong"
                onClick={saveAnnotation}
                type="button"
              >
                {t("Save annotation")}
              </button>
              <button
                className="rounded-lg px-3 py-2 text-xs text-content-muted hover:bg-surface-raised"
                onClick={() => setAnnotationTarget(null)}
                type="button"
              >
                {t("Cancel")}
              </button>
            </div>
          </aside>
        ) : settingsOpen ? (
          <aside className="w-96 overflow-y-auto border-l border-surface-border/30 bg-surface-base p-4">
            <h3 className="text-sm font-semibold text-content-primary">
              {t("Browser settings")}
            </h3>
            <p className="mt-1 text-xs text-content-muted">
              {t(
                "Site access never makes page content trustworthy. Consequential actions still ask separately.",
              )}
            </p>
            <label className="mt-4 block text-xs text-content-muted">
              {t("Default site access")}
              <select
                className={`${inputClass} mt-1`}
                onChange={(event) =>
                  setSettings((current) => ({
                    ...current,
                    siteAccess: event.target.value as BrowserSettings["siteAccess"],
                  }))
                }
                value={settings.siteAccess}
              >
                {(["always_ask", "auto_approve", "always_allow"] as const).map(
                  (siteAccess) => (
                    <option key={siteAccess} value={siteAccess}>
                      {browserSiteAccessLabel(siteAccess, t)}
                    </option>
                  ),
                )}
              </select>
            </label>
            <div className="mt-5 rounded-xl border border-surface-border/30 bg-surface-panel/50 p-3">
              <p className="text-xs font-medium text-content-primary">
                {t("Site rules")}
              </p>
              <div className="mt-2 flex gap-2">
                <input
                  className={`${inputClass} min-w-0 flex-1 font-mono text-xs`}
                  onChange={(event) => setRuleHostname(event.target.value)}
                  placeholder="example.com"
                  value={ruleHostname}
                />
                <select
                  className="rounded-lg border border-surface-border/35 bg-surface-base px-2 text-xs text-content-primary"
                  onChange={(event) =>
                    setRuleDecision(event.target.value as BrowserSiteRule["decision"])
                  }
                  value={ruleDecision}
                >
                  {(["allow", "block"] as const).map((decision) => (
                    <option key={decision} value={decision}>
                      {browserSiteRuleDecisionLabel(decision, t)}
                    </option>
                  ))}
                </select>
                <button
                  className="rounded-lg border border-surface-border/30 px-2.5 text-xs text-content-secondary hover:bg-surface-raised"
                  onClick={addRule}
                  type="button"
                >
                  {t("Add")}
                </button>
              </div>
              <div className="mt-3 space-y-1">
                {settings.siteRules.map((rule) => (
                  <div
                    className="flex items-center gap-2 rounded-lg bg-surface-base/70 px-2.5 py-2"
                    key={rule.hostname}
                  >
                    <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-content-secondary">
                      {rule.hostname}
                    </span>
                    <span className={rule.decision === "block" ? "text-[10px] text-danger" : "text-[10px] text-accent"}>
                      {browserSiteRuleDecisionStatusLabel(rule.decision, t)}
                    </span>
                    <button
                      aria-label={t("Remove {hostname} rule", {
                        hostname: rule.hostname,
                      })}
                      className="text-content-muted hover:text-danger"
                      onClick={() =>
                        setSettings((current) => ({
                          ...current,
                          siteRules: current.siteRules.filter(
                            (item) => item.hostname !== rule.hostname,
                          ),
                        }))
                      }
                      type="button"
                    >
                      ×
                    </button>
                  </div>
                ))}
              </div>
            </div>
            <label className="mt-5 flex items-start gap-3 rounded-xl border border-danger/30 bg-danger/5 p-3">
              <input
                checked={settings.fullCdpAccess}
                className="mt-0.5"
                onChange={(event) =>
                  setSettings((current) => ({
                    ...current,
                    fullCdpAccess: event.target.checked,
                  }))
                }
                type="checkbox"
              />
              <span>
                <span className="block text-xs font-semibold text-content-primary">
                  {t("Enable full CDP access")}
                </span>
                <span className="mt-1 block text-[10px] text-content-muted">
                  {t(
                    "Developer mode exposes sensitive browser internals. Every raw CDP call still requires explicit approval.",
                  )}
                </span>
              </span>
            </label>
            <button
              className="mt-4 w-full rounded-lg bg-accent px-3 py-2 text-xs font-semibold text-accent-contrast hover:bg-accent-strong disabled:opacity-50"
              disabled={busy}
              onClick={() => void saveSettings()}
              type="button"
            >
              {t("Save browser settings")}
            </button>
            <button
              className="mt-2 w-full rounded-lg px-3 py-2 text-xs text-content-muted hover:bg-danger/10 hover:text-danger disabled:opacity-50"
              disabled={busy}
              onClick={async () => {
                if (
                  !(await confirmDialog({
                    danger: true,
                    message: t(
                      "Clear Halv browser cookies, sessions, and site data?",
                    ),
                  }))
                ) {
                  return;
                }
                setBusy(true);
                void ipc.browser.clearData()
                  .then(() => ipc.browser.uiState())
                  .then((next) => {
                    setAnnotations([]);
                    setState(next);
                    setUrl(next.url === "about:blank" ? "" : next.url);
                  })
                  .catch((cause) => setError(errorMessage(cause)))
                  .finally(() => setBusy(false));
              }}
              type="button"
            >
              {t("Clear browser data")}
            </button>
          </aside>
        ) : annotations.length > 0 ? (
          <aside className="w-80 overflow-y-auto border-l border-surface-border/30 bg-surface-base p-4">
            <h3 className="text-sm font-semibold text-content-primary">
              {t("Saved annotations · {count}", { count: annotations.length })}
            </h3>
            <div className="mt-3 space-y-2">
              {annotations.map((annotation, index) => (
                <div
                  className="rounded-lg border border-surface-border/25 bg-surface-panel/50 p-3"
                  key={annotation.target.annotationId}
                >
                  <div className="flex items-start gap-2">
                    <span className="font-mono text-[10px] text-accent">{index + 1}</span>
                    <p className="min-w-0 flex-1 break-all font-mono text-[9px] text-content-muted">
                      {annotation.target.selector}
                    </p>
                    <button
                      className="text-content-muted hover:text-danger"
                      onClick={() =>
                        setAnnotations((current) =>
                          current.filter((_, itemIndex) => itemIndex !== index),
                        )
                      }
                      type="button"
                    >
                      ×
                    </button>
                  </div>
                  {annotation.comment ? (
                    <p className="mt-2 text-xs text-content-secondary">{annotation.comment}</p>
                  ) : null}
                  {styleSummary(annotation.style, t) ? (
                    <p className="mt-2 text-[10px] text-accent">
                      {styleSummary(annotation.style, t)}
                    </p>
                  ) : null}
                </div>
              ))}
            </div>
            <button
              className="mt-4 w-full rounded-lg bg-accent px-3 py-2 text-xs font-semibold text-accent-contrast hover:bg-accent-strong disabled:opacity-50"
              disabled={sending}
              onClick={() => void sendAnnotations()}
              type="button"
            >
              {sending ? t("Sending…") : t("Send annotations to chat")}
            </button>
          </aside>
        ) : null}
      </div>

      <div className="flex items-center gap-2 border-t border-surface-border/30 bg-surface-base px-3 py-2">
        <span className="shrink-0 text-[10px] text-content-muted">
          {t("Click a text field above, then type here")}
        </span>
        <input
          className={`${inputClass} h-8 min-w-0 flex-1 py-1 text-xs`}
          onChange={(event) => setPageText(event.target.value)}
          placeholder={t("Text for the focused page field")}
          value={pageText}
        />
        <button
          className="rounded-lg border border-surface-border/30 px-3 py-1.5 text-xs text-content-secondary hover:bg-surface-raised disabled:opacity-50"
          disabled={busy || !pageText}
          onClick={() =>
            void mutate(() =>
              ipc.browser.uiTypeFocused({
                text: pageText,
                submit: false,
              }),
            ).then(() => setPageText(""))
          }
          type="button"
        >
          {t("Type")}
        </button>
        <button
          className="rounded-lg border border-warning/35 px-3 py-1.5 text-xs text-warning hover:bg-warning/10 disabled:opacity-50"
          disabled={busy || !pageText}
          onClick={async () => {
            if (
              !(await confirmDialog({
                message: t("Type this text and submit the page form?"),
              }))
            ) {
              return;
            }
            void mutate(() =>
              ipc.browser.uiTypeFocused({
                text: pageText,
                submit: true,
              }),
            ).then(() => setPageText(""));
          }}
          type="button"
        >
          {t("Type & submit")}
        </button>
      </div>
    </section>
  );
}

function browserHistoryDirectionLabel(
  direction: "back" | "forward" | "reload",
  t: Translate,
) {
  switch (direction) {
    case "back":
      return t("back");
    case "forward":
      return t("forward");
    case "reload":
      return t("reload");
  }
}

function browserSiteAccessLabel(
  siteAccess: BrowserSettings["siteAccess"],
  t: Translate,
) {
  switch (siteAccess) {
    case "always_ask":
      return t("Always ask");
    case "auto_approve":
      return t("Auto approve after risk checks");
    case "always_allow":
      return t("Always allow site access");
  }
}

function browserSiteRuleDecisionLabel(
  decision: BrowserSiteRule["decision"],
  t: Translate,
) {
  switch (decision) {
    case "allow":
      return t("Allow");
    case "block":
      return t("Block");
  }
}

function browserSiteRuleDecisionStatusLabel(
  decision: BrowserSiteRule["decision"],
  t: Translate,
) {
  switch (decision) {
    case "allow":
      return t("allow");
    case "block":
      return t("block");
  }
}

function StyleNumber({
  label,
  value,
  onChange,
}: {
  label: string;
  value: number | null | undefined;
  onChange: (value: number | null) => void;
}) {
  return (
    <label className="text-[10px] text-content-muted">
      {label}
      <input
        className={`${inputClass} mt-1 h-9 text-xs`}
        min={0}
        onChange={(event) =>
          onChange(event.target.value === "" ? null : Number(event.target.value))
        }
        type="number"
        value={value ?? ""}
      />
    </label>
  );
}
