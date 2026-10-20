// Brings one terminal pane to the front, for example after a click on its
// notification. The terminal panel that holds the pane activates its leaf.
// The panel can mount after the request, so a request waits a short time
// for a panel that knows the session.

export const FOCUS_REQUEST_LIFETIME_MS = 10_000;
const PANE_FLASH_MS = 900;
// About three seconds of frames: a conversation load can take a moment.
const PANE_FOCUS_ATTEMPTS = 180;

type FocusHandler = (sessionId: string) => boolean;

type FocusRequest = { sessionId: string; at: number };

export function createTerminalFocusBridge(now: () => number = Date.now) {
  const handlers = new Set<FocusHandler>();
  let pending: FocusRequest | null = null;

  const dispatch = () => {
    if (!pending) return;
    if (now() - pending.at > FOCUS_REQUEST_LIFETIME_MS) {
      pending = null;
      return;
    }
    const { sessionId } = pending;
    for (const handler of handlers) {
      if (handler(sessionId)) {
        pending = null;
        return;
      }
    }
  };

  return {
    request(sessionId: string) {
      pending = { sessionId, at: now() };
      dispatch();
    },
    /** Registers a panel. The handler returns true when it holds the pane. */
    subscribe(handler: FocusHandler) {
      handlers.add(handler);
      dispatch();
      return () => {
        handlers.delete(handler);
      };
    },
    /** Offers the open request again after a panel changes its panes. */
    retry: dispatch,
    pendingSessionId: () => pending?.sessionId ?? null,
  };
}

export const terminalFocusBridge = createTerminalFocusBridge();

function paneElement(sessionId: string) {
  return [
    ...document.querySelectorAll<HTMLElement>(".terminal-pane[data-terminal-session-id]"),
  ].find(
    (element) =>
      element.dataset.terminalSessionId === sessionId &&
      element.getClientRects().length > 0 &&
      // An inactive conversation surface keeps its layout but hides it. A
      // pane there cannot take the focus, so wait for the surface to show.
      getComputedStyle(element).visibility !== "hidden",
  );
}

/**
 * Focuses the input of a visible pane and flashes its border once. The
 * conversation can take a few frames to show the pane.
 */
export function focusTerminalPaneElement(
  sessionId: string,
  options: { flash?: boolean } = {},
) {
  let attempts = 0;
  const attempt = () => {
    const pane = paneElement(sessionId);
    if (!pane) {
      attempts += 1;
      if (attempts < PANE_FOCUS_ATTEMPTS) window.requestAnimationFrame(attempt);
      return;
    }
    pane.querySelector<HTMLElement>(".xterm-helper-textarea")?.focus();
    if (options.flash === false) return;
    pane.dataset.attentionFlash = "true";
    window.setTimeout(() => {
      delete pane.dataset.attentionFlash;
    }, PANE_FLASH_MS);
  };
  window.requestAnimationFrame(attempt);
}
