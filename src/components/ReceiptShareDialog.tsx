import { save } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState } from "react";
import { copyText } from "../clipboard";
import { useT } from "../i18n/runtime";
import { ipc, type AccountPublishReceiptResult } from "../ipc.ts";
import {
  drawReceiptCard,
  type ReceiptData,
  type ReceiptTheme,
} from "../receiptCard";
import type { AccountAuthStatus } from "../types";
import { AppDialogFrame } from "./AppDialogHost";

type ReceiptShareDialogProps = {
  account: AccountAuthStatus | null;
  data: ReceiptData;
  onClose: () => void;
  onSignIn: () => void;
};

export function ReceiptShareDialog({
  account,
  data,
  onClose,
  onSignIn,
}: ReceiptShareDialogProps) {
  const t = useT();
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const copiedTimerRef = useRef<number | null>(null);
  const [receipt, setReceipt] = useState(data);
  const [published, setPublished] =
    useState<AccountPublishReceiptResult | null>(null);
  const [pending, setPending] = useState<"save" | "publish" | null>(null);
  const [linkCopied, setLinkCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (canvas) {
      drawReceiptCard(canvas, receipt, receiptThemeFromDocument());
    }
  }, [receipt]);

  useEffect(
    () => () => {
      if (copiedTimerRef.current !== null) {
        window.clearTimeout(copiedTimerRef.current);
      }
    },
    [],
  );

  async function saveImage() {
    setPending("save");
    setError(null);
    try {
      const path = await save({
        defaultPath: `halv-receipt-${receipt.periodEnd}.png`,
        filters: [{ name: "PNG", extensions: ["png"] }],
      });
      if (!path) {
        return;
      }
      const bytes = await canvasPngBytes(requireCanvas(canvasRef.current));
      await ipc.receipt.writePng({ path, bytes: Array.from(bytes) });
    } catch (saveError) {
      setError(errorText(saveError));
    } finally {
      setPending(null);
    }
  }

  async function copyLink() {
    if (!account?.signedIn) {
      onClose();
      onSignIn();
      return;
    }

    setPending("publish");
    setError(null);
    try {
      const result =
        published ??
        (await ipc.account.publishReceipt({
          payload: {
            periodStart: receipt.periodStart,
            periodEnd: receipt.periodEnd,
            tokensSaved: receipt.tokensSaved,
            savingsUsd: receipt.savingsUsd ?? 0,
            percentSaved: receipt.percentSaved,
            agents: receipt.agents,
            pngBase64: canvasPngBase64(requireCanvas(canvasRef.current)),
          },
        }));
      if (!published) {
        setPublished(result);
        setReceipt((current) => ({ ...current, receiptId: result.id }));
      }
      if (!(await copyText(result.url))) {
        throw new Error(t("Could not copy link."));
      }
      setLinkCopied(true);
      if (copiedTimerRef.current !== null) {
        window.clearTimeout(copiedTimerRef.current);
      }
      copiedTimerRef.current = window.setTimeout(() => {
        setLinkCopied(false);
        copiedTimerRef.current = null;
      }, 2_000);
    } catch (publishError) {
      setError(errorText(publishError));
    } finally {
      setPending(null);
    }
  }

  return (
    <AppDialogFrame
      labelledBy="receipt-share-title"
      onDismiss={onClose}
      restoreFocus
    >
      <section
        className="w-full max-w-4xl overflow-hidden rounded-xl border border-surface-border/35 bg-surface-glass/95 shadow-2xl shadow-black/30"
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.stopPropagation();
            onClose();
          }
        }}
      >
        <header className="flex items-center justify-between gap-3 border-b border-surface-border/25 px-5 py-4">
          <h2
            className="text-sm font-semibold text-content-primary"
            id="receipt-share-title"
          >
            {t("Share savings")}
          </h2>
          <button
            aria-label={t("Close")}
            autoFocus
            className="rounded-md px-2 py-1 text-content-muted hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
            onClick={onClose}
            type="button"
          >
            {"\u00d7"}
          </button>
        </header>
        <div className="space-y-3 px-5 py-4">
          <div className="overflow-hidden rounded-lg border border-surface-border/30 bg-surface-base">
            <canvas
              aria-label={t("Savings receipt preview")}
              className="block h-auto max-w-full"
              ref={canvasRef}
              role="img"
            />
          </div>
          <p className="truncate text-xs text-content-muted">
            {t(
              "The link shows this card and your invite code. No email, no prompts.",
            )}
          </p>
          {error ? <p className="text-xs text-danger">{error}</p> : null}
        </div>
        <footer className="flex flex-wrap items-center justify-end gap-2 border-t border-surface-border/25 bg-surface-raised/30 px-5 py-3">
          {linkCopied ? (
            <span className="mr-auto text-xs text-accent">
              {t("Link copied")}
            </span>
          ) : null}
          <button
            className="rounded-md border border-surface-border/35 px-3 py-2 text-sm text-content-secondary transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:opacity-50"
            disabled={pending !== null}
            onClick={() => void saveImage()}
            type="button"
          >
            {pending === "save" ? t("Saving\u2026") : t("Save image\u2026")}
          </button>
          <button
            className="rounded-md border border-accent bg-accent px-3 py-2 text-sm font-medium text-accent-contrast transition-colors hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40 disabled:opacity-50"
            disabled={pending !== null}
            onClick={() => void copyLink()}
            type="button"
          >
            {account?.signedIn
              ? pending === "publish"
                ? t("Publishing\u2026")
                : t("Copy link")
              : t("Sign in to get a link")}
          </button>
          <button
            className="rounded-md border border-surface-border/35 px-3 py-2 text-sm text-content-secondary transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
            onClick={onClose}
            type="button"
          >
            {t("Close")}
          </button>
        </footer>
      </section>
    </AppDialogFrame>
  );
}

export function receiptThemeFromDocument(): ReceiptTheme {
  const styles = window.getComputedStyle(document.documentElement);
  const dark = document.documentElement.classList.contains("dark");
  return {
    accent: cssRgb(styles.getPropertyValue("--accent")),
    accentContrast: cssRgb(styles.getPropertyValue("--accent-contrast")),
    bg: cssRgb(
      styles.getPropertyValue(dark ? "--surface-panel" : "--surface-sunken"),
    ),
    border: cssRgb(styles.getPropertyValue("--surface-border")),
    fg: cssRgb(styles.getPropertyValue("--content-primary")),
    muted: cssRgb(styles.getPropertyValue("--content-muted")),
    secondary: cssRgb(styles.getPropertyValue("--content-secondary")),
  };
}

function cssRgb(value: string) {
  const channels = value.trim().replace(/,/g, " ").split(/\s+/).join(" ");
  return `rgb(${channels})`;
}

function requireCanvas(canvas: HTMLCanvasElement | null) {
  if (!canvas) {
    throw new Error("Receipt preview is not ready.");
  }
  return canvas;
}

function canvasPngBase64(canvas: HTMLCanvasElement) {
  return canvas.toDataURL("image/png").split(",", 2)[1] ?? "";
}

async function canvasPngBytes(canvas: HTMLCanvasElement) {
  const blob = await new Promise<Blob>((resolve, reject) => {
    canvas.toBlob((result) => {
      if (result) {
        resolve(result);
      } else {
        reject(new Error("Could not encode the receipt image."));
      }
    }, "image/png");
  });
  return new Uint8Array(await blob.arrayBuffer());
}

function errorText(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}
