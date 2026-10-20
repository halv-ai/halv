import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import type { KeyboardEvent, ReactNode } from "react";
import { useT } from "../i18n/runtime";
import {
  acceptActiveDialog,
  cancelActiveDialog,
  getActiveDialog,
  subscribeDialogs,
  type AppDialogRequest,
} from "../dialogs";

export function AppDialogHost() {
  const dialog = useSyncExternalStore(
    subscribeDialogs,
    getActiveDialog,
    getActiveDialog,
  );
  const previousFocusRef = useRef<HTMLElement | null>(null);
  const dialogOpenRef = useRef(false);

  if (dialog && !dialogOpenRef.current) {
    previousFocusRef.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    dialogOpenRef.current = true;
  }

  useEffect(() => {
    if (dialog || !dialogOpenRef.current) {
      return;
    }
    dialogOpenRef.current = false;
    const previousFocus = previousFocusRef.current;
    previousFocusRef.current = null;
    window.requestAnimationFrame(() => previousFocus?.focus());
  }, [dialog]);

  if (!dialog) {
    return null;
  }

  return <ActiveDialog dialog={dialog} key={dialog.id} />;
}

export function AppDialogFrame({
  children,
  labelledBy,
  onDismiss,
  restoreFocus = false,
}: {
  children: ReactNode;
  labelledBy: string;
  onDismiss: () => void;
  restoreFocus?: boolean;
}) {
  const previousFocusRef = useRef<HTMLElement | null>(
    document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null,
  );

  useEffect(() => {
    if (!restoreFocus) {
      return;
    }
    const previousFocus = previousFocusRef.current;
    return () => {
      window.requestAnimationFrame(() => previousFocus?.focus());
    };
  }, [restoreFocus]);

  return (
    <div
      aria-labelledby={labelledBy}
      aria-modal="true"
      className="fixed inset-0 z-[100] flex items-center justify-center bg-black/40 p-4 backdrop-blur-sm"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) {
          onDismiss();
        }
      }}
      role="dialog"
    >
      {children}
    </div>
  );
}

function ActiveDialog({ dialog }: { dialog: AppDialogRequest }) {
  const t = useT();
  const [promptValue, setPromptValue] = useState(
    dialog.kind === "prompt" ? (dialog.defaultValue ?? "") : "",
  );
  const panelRef = useRef<HTMLFormElement>(null);
  const heading =
    dialog.title ??
    dialog.message ??
    (dialog.kind === "confirm" ? t("Confirm") : t("Enter a value"));
  const hasMessageBody = Boolean(dialog.title && dialog.message);
  const details = dialog.kind === "confirm" ? (dialog.details ?? []) : [];
  // A review dialog focuses Cancel, so a stray Enter key does not approve it.
  const reviewDetails = details.length > 0;

  function handleKeyDown(event: KeyboardEvent<HTMLFormElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      cancelActiveDialog();
      return;
    }
    if (event.key !== "Tab") {
      return;
    }
    const focusable = Array.from(
      panelRef.current?.querySelectorAll<HTMLElement>(
        'button:not([disabled]), input:not([disabled]), [tabindex]:not([tabindex="-1"])',
      ) ?? [],
    );
    if (focusable.length === 0) {
      return;
    }
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
    <AppDialogFrame
      labelledBy="halv-app-dialog-title"
      onDismiss={cancelActiveDialog}
    >
      <form
        className={`w-full ${reviewDetails ? "max-w-2xl" : "max-w-md"} overflow-hidden rounded-xl border border-surface-border/35 bg-surface-glass/95 shadow-2xl shadow-black/30`}
        onKeyDown={handleKeyDown}
        onSubmit={(event) => {
          event.preventDefault();
          acceptActiveDialog(
            dialog.kind === "prompt" ? promptValue : undefined,
          );
        }}
        ref={panelRef}
      >
        <div className="space-y-2 px-5 py-4">
          <h2
            className="text-sm font-semibold text-content-primary"
            id="halv-app-dialog-title"
          >
            {heading}
          </h2>
          {hasMessageBody ? (
            <p className="text-sm leading-5 text-content-secondary">
              {dialog.message}
            </p>
          ) : null}
          {reviewDetails ? (
            <div
              className="max-h-[50vh] space-y-3 overflow-y-auto pt-1"
              tabIndex={0}
            >
              {details.map((detail, index) => (
                <div key={index}>
                  <p className="mb-1 font-mono text-[10px] uppercase tracking-[0.12em] text-content-muted">
                    {detail.label}
                  </p>
                  <pre className="whitespace-pre-wrap break-words rounded-lg border border-surface-border/30 bg-surface-base px-3 py-2 font-mono text-xs leading-relaxed text-content-primary">
                    {detail.text}
                  </pre>
                </div>
              ))}
            </div>
          ) : null}
          {dialog.kind === "prompt" ? (
            <input
              aria-label={dialog.message ?? dialog.title ?? t("Dialog input")}
              autoFocus
              className="w-full rounded-lg border border-surface-border/35 bg-surface-raised px-3 py-2 text-sm text-content-primary outline-none placeholder:text-content-muted focus:border-accent/60 focus:ring-2 focus:ring-accent/20"
              onChange={(event) => setPromptValue(event.target.value)}
              placeholder={dialog.placeholder}
              value={promptValue}
            />
          ) : null}
        </div>
        <div className="flex justify-end gap-2 border-t border-surface-border/25 bg-surface-raised/30 px-5 py-3">
          <button
            autoFocus={reviewDetails}
            className="rounded-md border border-surface-border/35 px-3 py-2 text-sm text-content-secondary transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
            onClick={cancelActiveDialog}
            type="button"
          >
            {dialog.kind === "confirm"
              ? (dialog.cancelLabel ?? t("Cancel"))
              : t("Cancel")}
          </button>
          <button
            autoFocus={dialog.kind === "confirm" && !reviewDetails}
            className={`rounded-md border px-3 py-2 text-sm font-medium transition-colors focus:outline-none focus:ring-2 ${
              dialog.kind === "confirm" && dialog.danger
                ? "border-danger/35 bg-danger/10 text-danger hover:bg-danger/15 focus:ring-danger/30"
                : "border-accent bg-accent text-accent-contrast hover:bg-accent-strong focus:ring-accent/40"
            }`}
            type="submit"
          >
            {dialog.confirmLabel ??
              (dialog.kind === "confirm" ? t("Confirm") : t("OK"))}
          </button>
        </div>
      </form>
    </AppDialogFrame>
  );
}
