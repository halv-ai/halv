import { createTerminalCompositionGuard } from "../terminalComposition";
import { terminalDraftBridge, TerminalDraftError } from "../terminalDraft";
import { terminalDropBridge } from "../terminalDrop";
import { retainNativeTerminalDrop } from "../terminalNativeDrop";
import {
  agentPaneViewFor,
  terminalActivityStore,
} from "../terminalActivityStore";
import { terminalSizeRegistry } from "../terminalSizeOwner";
import { createTerminalInputStream } from "../terminalInput";
import { createTerminalPasteFilter, sanitizeTerminalPaste } from "../terminalPaste";
import { ipc } from "../ipc.ts";
import { terminalLinkHandler } from "../externalLinks.ts";
import {
  type DragEvent,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { agentSessionStore, type AgentSessionStatus } from "../agentSessions";
import { refreshLanesWhenSplitMenuOpens } from "../terminalSplit";
import {
  TERMINAL_DETACHED_MODE_RESET,
  TERMINAL_EXIT_MODE_RESET,
  createTerminalReplayGuard,
  eventsAfterTerminalSnapshot,
  readTerminalPalette,
  resolveTerminalMinimumContrastRatio,
  terminalContinuationPrelude,
} from "../terminalSurface";
import {
  calculateTerminalFit,
  resolveTerminalCellSize,
  type TerminalPaneSplitDirection,
} from "../terminalLayout";
import type {
  TerminalAttach,
  TerminalEvent,
  TerminalSession,
  TerminalSupervisorReconnect,
} from "../types";
import {
  shortcutForAction,
  shortcutMatchesEvent,
  type KeyboardShortcutOverrides,
} from "../shortcuts";
import { useT } from "../i18n/runtime";
import { useExclusiveDetailsMenu } from "./detailsMenu";
import { AgentStateGlyph } from "./AgentStateGlyph";
import { AccountRestartChip } from "./AccountRestartChip";

type TerminalPaneProps = {
  active: boolean;
  closing: boolean;
  splitting: boolean;
  displayStatus: AgentSessionStatus | TerminalSession["status"];
  restoredScrollback?: string;
  resumeFailure: { exitCode: number | null } | null;
  resuming: boolean;
  retryingSupervisor: boolean;
  session: TerminalSession;
  splitChoices: TerminalPaneSplitChoice[];
  shortcutOverrides: KeyboardShortcutOverrides;
  title: string;
  onActivate: () => void;
  onAttached: (sessionId: string) => void;
  onAttachError: (sessionId: string, message: string) => void;
  onClose: () => void;
  onCycleTab: (offset: -1 | 1) => void;
  onError: (message: string) => void;
  onInput: (sessionId: string) => void;
  onOpenShell?: () => void;
  /** A fresh shell in the same folder and pane, for a failed plain shell. */
  onRestart?: () => void;
  /** The shell tab rules: a plain shell that exits with 0 closes its pane,
   *  so its exit card waits the grace time too. */
  closesOnCleanExit?: boolean;
  onPromptSubmitGate?: () => boolean;
  onPromptSubmitSignalHandled: (sessionId: string, signal: number) => void;
  onResume?: () => void;
  onRetrySupervisor?: () => void;
  onStartFresh?: () => void;
  onSwap?: () => void;
  onRotate?: () => void;
  onSplit: (
    direction: TerminalPaneSplitDirection,
    choiceId: "duplicate" | string,
  ) => void;
  onTitle: (title: string) => void;
  promptSubmitSignal: number;
  supervisorFailureMessage?: string;
  supervisorReconnect: TerminalSupervisorReconnect | null;
  // False while the pane is laid out but hidden, for example on an inactive
  // conversation surface. A hidden pane does not fit or resize its PTY.
  visible?: boolean;
};

export type TerminalPaneSplitChoice = {
  id: string;
  label: string;
  ready: boolean;
  readiness: string;
};

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function terminalIsAtBottom(terminal: Terminal) {
  return terminal.buffer.active.viewportY >= terminal.buffer.active.baseY;
}

// Strip terminal control sequences from saved scrollback and return the last
// few readable lines. Replaying the raw stream of a full-screen TUI produces
// garbage, so the detached pane shows this plain-text preview instead.
export function detachedScrollbackPreview(
  raw: string,
  maxLines = 14,
): string {
  const plain = raw
    // OSC sequences (title, colour queries) terminated by BEL or ST.
    .replace(/\x1b\][\s\S]*?(?:\x07|\x1b\\)/g, "")
    // CSI sequences (colours, cursor moves, private modes, queries).
    .replace(/\x1b\[[0-9;?>=! ]*[@-~]/g, "")
    // Other escapes (charset select, save/restore cursor, index, etc.).
    .replace(/\x1b[()#][0-9A-Za-z]?|\x1b[=>78McDEHM]/g, "")
    // Remaining control bytes except newline and tab.
    .replace(/[\x00-\x08\x0b-\x1f\x7f]/g, "");
  const lines = plain
    .split("\n")
    .map((line) => line.replace(/\s+$/, ""))
    .filter((line) => line.trim().length > 0);
  return lines.slice(-maxLines).join("\n");
}

function cssPixels(style: CSSStyleDeclaration, property: string) {
  const value = Number.parseFloat(style.getPropertyValue(property));
  return Number.isFinite(value) ? value : 0;
}

type TerminalWithRenderDimensions = Terminal & {
  _core?: {
    _renderService?: {
      dimensions?: unknown;
    };
  };
};

const MAX_TERMINAL_FIT_FOLLOW_UPS = 3;

export function TerminalPane({
  active,
  closing,
  splitting,
  displayStatus,
  restoredScrollback,
  resumeFailure,
  resuming,
  retryingSupervisor,
  session,
  splitChoices,
  shortcutOverrides,
  title,
  onActivate,
  onAttached,
  onAttachError,
  onClose,
  onCycleTab,
  onError,
  onInput,
  onOpenShell,
  onRestart,
  closesOnCleanExit = false,
  onPromptSubmitGate,
  onPromptSubmitSignalHandled,
  onResume,
  onRetrySupervisor,
  onStartFresh,
  onSwap,
  onRotate,
  onSplit,
  onTitle,
  promptSubmitSignal,
  supervisorFailureMessage,
  supervisorReconnect,
  visible = true,
}: TerminalPaneProps) {
  const t = useT();
  const activitySnapshot = useSyncExternalStore(
    terminalActivityStore.subscribe,
    terminalActivityStore.getSnapshot,
  );
  const paneView =
    session.agentKind && session.status === "running"
      ? agentPaneViewFor(activitySnapshot, session.id)
      : null;
  const activate = () => {
    terminalActivityStore.markViewed(session.id);
    onActivate();
  };
  const [hasNewOutput, setHasNewOutput] = useState(false);
  const [attaching, setAttaching] = useState(restoredScrollback === undefined);
  const [sceneComplete, setSceneComplete] = useState(false);
  const paneRef = useRef<HTMLElement>(null);
  const toolbarRef = useRef<HTMLDivElement>(null);
  const hostRef = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const sequenceRef = useRef(0);
  const replayingRef = useRef(false);
  // True while the ended agent's last screen replays above a continuation
  // shell. The agent titles in that screen do not name the shell.
  const preludeReplayingRef = useRef(false);
  const attachingRef = useRef(restoredScrollback === undefined);
  const pendingEventsRef = useRef<TerminalEvent[]>([]);
  const handleEventRef = useRef<(event: TerminalEvent) => void>(() => undefined);
  const listenerReadyRef = useRef<Promise<void>>(Promise.resolve());
  const atBottomRef = useRef(true);
  const callbacksRef = useRef({
    onActivate: activate,
    onAttached,
    onAttachError,
    onCycleTab,
    onError,
    onInput,
    onPromptSubmitGate,
    onPromptSubmitSignalHandled,
    onSplit,
    onTitle,
  });
  callbacksRef.current = {
    onActivate: activate,
    onAttached,
    onAttachError,
    onCycleTab,
    onError,
    onInput,
    onPromptSubmitGate,
    onPromptSubmitSignalHandled,
    onSplit,
    onTitle,
  };
  const processEnded = resumeFailure !== null || session.status !== "running";
  // Input and resize target the PTY; against an exited session they only
  // produce "terminal session is not running" errors in the panel.
  const processEndedRef = useRef(processEnded);
  processEndedRef.current = processEnded;
  // After a clean agent exit, a continuation shell normally replaces this
  // pane within a moment. With the shell tab rules, a clean shell exit
  // closes the pane. The exit card waits a short grace time, so it does
  // not flash before the shell arrives or the pane closes.
  const exitGrace =
    processEnded &&
    resumeFailure === null &&
    restoredScrollback === undefined &&
    (Boolean(session.agentKind) || closesOnCleanExit) &&
    session.status === "exited" &&
    session.exitCode === 0 &&
    !session.signal;
  const visibleRef = useRef(visible);
  visibleRef.current = visible;
  const applyVisibleRef = useRef<(visible: boolean) => void>(() => undefined);

  useEffect(() => {
    if (restoredScrollback !== undefined) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    attachingRef.current = true;
    pendingEventsRef.current = [];
    listenerReadyRef.current = ipc.events.listen<TerminalEvent>(
      "terminal_event",
      (payload) => {
        if (disposed || payload.sessionId !== session.id) return;
        // Stage live events until the snapshot replay completes.
        if (attachingRef.current) {
          pendingEventsRef.current.push(payload);
          return;
        }
        handleEventRef.current(payload);
      },
    ).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [restoredScrollback, session.id]);

  useEffect(() => {
    const host = hostRef.current;
    const pane = paneRef.current;
    const toolbar = toolbarRef.current;
    if (!host || !pane || !toolbar) return;
    const initialPalette = readTerminalPalette(
      window.getComputedStyle(document.documentElement),
    );
    const terminal = new Terminal({
      allowProposedApi: false,
      // Enable alpha compositing so a translucent selection tints the cells
      // instead of rendering as an opaque block that hides the text.
      allowTransparency: true,
      convertEol: false,
      cursorBlink: true,
      cursorInactiveStyle: "outline",
      cursorStyle: "block",
      customGlyphs: true,
      drawBoldTextInBrightColors: false,
      fontFamily:
        '"SF Mono", SFMono-Regular, ui-monospace, Menlo, Monaco, Consolas, "Liberation Mono", monospace',
      fontSize: 13,
      fontWeight: 500,
      fontWeightBold: 700,
      letterSpacing: 0,
      lineHeight: 1,
      // A Cmd-click (macOS) or Ctrl-click opens an OSC 8 hyperlink in the
      // system browser, only for http and https.
      linkHandler: terminalLinkHandler,
      minimumContrastRatio: resolveTerminalMinimumContrastRatio(
        initialPalette.background,
      ),
      scrollback: 10_000,
      theme: initialPalette,
    });
    let compositionGuard: ReturnType<typeof createTerminalCompositionGuard> | undefined;
    terminal.attachCustomKeyEventHandler((event) => {
      if (
        event.type === "keydown" &&
        event.metaKey &&
        !event.ctrlKey &&
        !event.altKey &&
        event.key.toLowerCase() === "d"
      ) {
        event.preventDefault();
        callbacksRef.current.onSplit(
          event.shiftKey ? "horizontal" : "vertical",
          "duplicate",
        );
        return false;
      }
      if (
        event.type === "keydown" &&
        shortcutMatchesEvent(
          event,
          shortcutForAction(
            "workspace.previous_terminal_tab",
            shortcutOverrides,
          ),
        )
      ) {
        event.preventDefault();
        callbacksRef.current.onCycleTab(-1);
        return false;
      }
      if (
        event.type === "keydown" &&
        shortcutMatchesEvent(
          event,
          shortcutForAction("workspace.next_terminal_tab", shortcutOverrides),
        )
      ) {
        event.preventDefault();
        callbacksRef.current.onCycleTab(1);
        return false;
      }
      if (
        event.type === "keydown" &&
        shortcutMatchesEvent(
          event,
          shortcutForAction("workspace.clear_terminal", shortcutOverrides),
        )
      ) {
        terminal.clear();
        return false;
      }
      return compositionGuard?.accept(event) ?? true;
    });
    terminal.open(host);
    compositionGuard = createTerminalCompositionGuard(
      terminal.textarea!,
      (data) => terminal.input(data, true),
      { fontFamily: terminal.options.fontFamily, fontSize: terminal.options.fontSize },
    );
    // Pasted text must not end a bracketed paste. The guard above sends a
    // pending edit before the filter pastes.
    const pasteFilter = createTerminalPasteFilter(
      terminal.element!,
      terminal.textarea!,
      (text) => {
        callbacksRef.current.onInput(session.id);
        terminal.paste(text);
      },
    );
    terminalRef.current = terminal;
    const keyDisposable = terminal.onKey(() =>
      callbacksRef.current.onInput(session.id),
    );
    setSceneComplete(false);
    setAttaching(true);
    sequenceRef.current = 0;
    attachingRef.current = restoredScrollback === undefined;
    pendingEventsRef.current = [];
    atBottomRef.current = true;
    setHasNewOutput(false);
    const activityTracker = session.agentKind
      ? terminalActivityStore.trackerForSession(session.id)
      : null;
    const promptSubmissionTracker = terminalActivityStore.promptSubmissionForSession(session.id);

    // Write callbacks change the guard before xterm parses the next chunk.
    const replayGuard = createTerminalReplayGuard((chunk, callback) => {
      terminal.write(chunk, callback);
    });
    const writeOutput = (chunk: string, replay = false) => {
      if (!chunk) return;
      if (!atBottomRef.current) setHasNewOutput(true);
      // Xterm preserves parser state and answers terminal queries itself.
      replayGuard.write(chunk, replay);
    };

    const handleEvent = (payload: TerminalEvent) => {
      if (payload.sequence <= sequenceRef.current) return;
      sequenceRef.current = payload.sequence;
      if (payload.kind === "data" && payload.data) {
        writeOutput(payload.data, payload.replay === true);
        return;
      }
      const message =
        payload.message ===
        "Terminal supervisor connection failed after 10 attempts. Close and reopen this terminal pane."
          ? t(
              "Terminal supervisor connection failed after 10 attempts. Close and reopen this terminal pane.",
            )
          : payload.message;
      if (message) callbacksRef.current.onError(message);
      activityTracker?.finish();
      terminal.write(TERMINAL_EXIT_MODE_RESET);
      if (payload.kind === "exit") {
        terminal.writeln(
          `\r\n\x1b[2m[process exited ${payload.exitCode ?? ""}${
            payload.signal ? ` · ${payload.signal}` : ""
          }]\x1b[0m`,
        );
      } else {
        terminal.writeln(
          `\r\n\x1b[31m[terminal error: ${message ?? "unknown error"}]\x1b[0m`,
        );
      }
    };
    handleEventRef.current = handleEvent;

    let lastInputWrite: Promise<void> = Promise.resolve();
    const writeInput = createTerminalInputStream(session.id);
    const dataDisposable = terminal.onData((data) => {
      if (restoredScrollback !== undefined || processEndedRef.current) return;
      // While replaying stored scrollback, xterm answers the historical
      // terminal queries it contains. Sending those stale answers to the live
      // PTY makes the CLI echo them back as garbage, so drop them during replay.
      if (replayingRef.current || replayGuard.replayDepth() > 0) return;
      // Only an agent prompt is a submission. A plain shell command never
      // reaches the trial gate.
      if (session.agentKind && promptSubmissionTracker.noteInput(data)) {
        if (callbacksRef.current.onPromptSubmitGate?.()) {
          promptSubmissionTracker.rejectSubmission();
          return;
        }
        activityTracker?.notePromptSubmitted();
      }
      lastInputWrite = writeInput(data);
      void lastInputWrite.catch(
        (cause) => callbacksRef.current.onError(errorMessage(cause)),
      );
    });
    // A native file drop pastes the quoted paths, like a terminal app does.
    // Tauri reports the drop to the webview, not to the DOM, so the bridge
    // finds the pane from the drop point.
    const unregisterDrop = terminalDropBridge.register(session.id, {
      bounds: () => {
        if (!visibleRef.current) return null;
        const rect = pane.getBoundingClientRect();
        if (rect.width <= 0 || rect.height <= 0) return null;
        return { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom };
      },
      shell: session.shell,
      paste: (text) => {
        if (processEndedRef.current || attachingRef.current) return;
        callbacksRef.current.onActivate();
        callbacksRef.current.onInput(session.id);
        terminal.paste(sanitizeTerminalPaste(text));
        terminal.focus();
      },
      setHover: (hover) => {
        pane.toggleAttribute("data-drop-hover", hover);
      },
    });
    const releaseNativeDrop = retainNativeTerminalDrop();
    const unregisterDraft = terminalDraftBridge.register(session.id, async (text) => {
      if (restoredScrollback !== undefined || processEndedRef.current || replayingRef.current || attachingRef.current) {
        throw new TerminalDraftError("unavailable");
      }
      if (promptSubmissionTracker.hasDraft()) throw new TerminalDraftError("occupied");
      // A draft would answer a waiting dialog or interrupt the work.
      if (terminalActivityStore.paneIsBusy(session.id)) throw new TerminalDraftError("unavailable");
      callbacksRef.current.onActivate();
      terminal.paste(sanitizeTerminalPaste(text));
      await lastInputWrite;
      terminal.focus();
    });
    // The claim sends a grid only for the owner pane and only on a change.
    const sizeClaim =
      restoredScrollback === undefined
        ? terminalSizeRegistry.claim(
            session.id,
            ({ cols, rows }) =>
              ipc.terminal.resize({ sessionId: session.id, cols, rows }),
            (cause) => callbacksRef.current.onError(errorMessage(cause)),
            () => !processEndedRef.current,
          )
        : null;
    const reportSize = (cols: number, rows: number) => {
      sizeClaim?.resize({ cols, rows });
    };
    const resizeDisposable = terminal.onResize(({ cols, rows }) =>
      reportSize(cols, rows),
    );
    const scrollDisposable = terminal.onScroll(() => {
      atBottomRef.current = terminalIsAtBottom(terminal);
      if (atBottomRef.current) setHasNewOutput(false);
    });
    const titleDisposable = terminal.onTitleChange((value) => {
      if (preludeReplayingRef.current) return;
      const clean = value.replace(/[\u0000-\u001f\u007f]/g, "").trim();
      if (clean) callbacksRef.current.onTitle(clean);
    });
    const focusIn = () => callbacksRef.current.onActivate();
    host.addEventListener("focusin", focusIn);

    let fitTimer: number | undefined;
    let fitFrame: number | undefined;
    let followUpFitFrame: number | undefined;
    let fitDisposed = false;
    const fit = (remainingFollowUps = MAX_TERMINAL_FIT_FOLLOW_UPS) => {
      // A hidden surface still has a layout. Its pane fits when it shows.
      if (!visibleRef.current) return;
      const terminalElement = terminal.element;
      const screen =
        terminalElement?.querySelector<HTMLElement>(".xterm-screen");
      if (
        !terminalElement ||
        !screen ||
        terminal.cols <= 0 ||
        terminal.rows <= 0
      ) {
        return;
      }
      const paneBounds = pane.getBoundingClientRect();
      const toolbarBounds = toolbar.getBoundingClientRect();
      const terminalBounds = terminalElement.getBoundingClientRect();
      const screenBounds = screen.getBoundingClientRect();
      // Xterm has no public cell metric API. Keep the DOM fallback for private API changes.
      const renderDimensions = (terminal as TerminalWithRenderDimensions)._core
        ?._renderService?.dimensions;
      const cellSize = resolveTerminalCellSize(
        renderDimensions,
        {
          screenWidth: screenBounds.width,
          screenHeight: screenBounds.height,
          cols: terminal.cols,
          rows: terminal.rows,
        },
        window.devicePixelRatio,
      );
      if (!cellSize) return;
      const paneStyle = window.getComputedStyle(pane);
      const terminalStyle = window.getComputedStyle(terminalElement);
      const paneBorderTop = cssPixels(paneStyle, "border-top-width");
      const paneBorderRight = cssPixels(paneStyle, "border-right-width");
      const paneBorderBottom = cssPixels(paneStyle, "border-bottom-width");
      const paneBorderLeft = cssPixels(paneStyle, "border-left-width");
      const panePaddingTop = cssPixels(paneStyle, "padding-top");
      const panePaddingRight = cssPixels(paneStyle, "padding-right");
      const panePaddingBottom = cssPixels(paneStyle, "padding-bottom");
      const panePaddingLeft = cssPixels(paneStyle, "padding-left");
      const stageTop =
        paneBounds.top + paneBorderTop + panePaddingTop + toolbarBounds.height;
      const stageRight =
        paneBounds.right - paneBorderRight - panePaddingRight;
      const stageBottom =
        paneBounds.bottom - paneBorderBottom - panePaddingBottom;
      const stageLeft = paneBounds.left + paneBorderLeft + panePaddingLeft;
      const viewport = terminalElement.querySelector<HTMLElement>(
        ".xterm-viewport",
      );
      const dimensions = calculateTerminalFit({
        paneWidth: paneBounds.width,
        paneHeight: paneBounds.height,
        paneHeaderHeight: toolbarBounds.height,
        paddingTop:
          panePaddingTop +
          Math.max(0, terminalBounds.top - stageTop) +
          cssPixels(terminalStyle, "padding-top"),
        paddingRight:
          panePaddingRight +
          Math.max(0, stageRight - terminalBounds.right) +
          cssPixels(terminalStyle, "padding-right"),
        paddingBottom:
          panePaddingBottom +
          Math.max(0, stageBottom - terminalBounds.bottom) +
          cssPixels(terminalStyle, "padding-bottom"),
        paddingLeft:
          panePaddingLeft +
          Math.max(0, terminalBounds.left - stageLeft) +
          cssPixels(terminalStyle, "padding-left"),
        borderTop:
          paneBorderTop + cssPixels(terminalStyle, "border-top-width"),
        borderRight:
          paneBorderRight + cssPixels(terminalStyle, "border-right-width"),
        borderBottom:
          paneBorderBottom + cssPixels(terminalStyle, "border-bottom-width"),
        borderLeft:
          paneBorderLeft + cssPixels(terminalStyle, "border-left-width"),
        scrollbarWidth: viewport
          ? Math.max(0, viewport.offsetWidth - viewport.clientWidth)
          : 0,
        cellWidth: cellSize.width,
        cellHeight: cellSize.height,
        exactCellSize: cellSize.exact,
      });
      if (!dimensions) return;
      const wasAtBottom = terminalIsAtBottom(terminal);
      if (
        terminal.cols !== dimensions.cols ||
        terminal.rows !== dimensions.rows
      ) {
        terminal.resize(dimensions.cols, dimensions.rows);
        if (remainingFollowUps > 0 && !fitDisposed) {
          if (followUpFitFrame !== undefined) {
            window.cancelAnimationFrame(followUpFitFrame);
          }
          followUpFitFrame = window.requestAnimationFrame(() => {
            followUpFitFrame = undefined;
            fit(remainingFollowUps - 1);
          });
        }
      }
      if (wasAtBottom) terminal.scrollToBottom();
      // An unchanged grid has no resize event, but a new owner needs it.
      reportSize(terminal.cols, terminal.rows);
    };
    const scheduleFit = () => {
      window.clearTimeout(fitTimer);
      if (fitFrame !== undefined) window.cancelAnimationFrame(fitFrame);
      fitTimer = window.setTimeout(() => {
        fitTimer = undefined;
        fitFrame = window.requestAnimationFrame(() => {
          fitFrame = undefined;
          fit();
        });
      }, 72);
    };
    const resizeObserver = new ResizeObserver(scheduleFit);
    resizeObserver.observe(pane);
    resizeObserver.observe(toolbar);
    resizeObserver.observe(host);
    const observedScreen = terminal.element?.querySelector<HTMLElement>(
      ".xterm-screen",
    );
    if (observedScreen) resizeObserver.observe(observedScreen);
    let sawFirstRender = false;
    const renderDisposable = terminal.onRender(() => {
      if (sawFirstRender) return;
      sawFirstRender = true;
      scheduleFit();
    });
    void document.fonts?.ready.then(() => {
      if (!fitDisposed && terminalRef.current === terminal) scheduleFit();
    });
    window.addEventListener("resize", scheduleFit);
    fit();
    let appliedVisible = visibleRef.current;
    sizeClaim?.setVisible(appliedVisible);
    applyVisibleRef.current = (nextVisible) => {
      if (nextVisible === appliedVisible) return;
      appliedVisible = nextVisible;
      sizeClaim?.setVisible(nextVisible);
      // The pane skipped its fits while hidden. A surface that shows gets
      // its final layout some frames later, so the fit waits for it.
      if (nextVisible) scheduleFit();
    };

    const updateTheme = () => {
      try {
        const palette = readTerminalPalette(
          window.getComputedStyle(document.documentElement),
        );
        terminal.options.theme = palette;
        const minimumContrastRatio = resolveTerminalMinimumContrastRatio(
          palette.background,
        );
        // xterm clears its contrast cache when this option changes.
        if (terminal.options.minimumContrastRatio !== minimumContrastRatio) {
          terminal.options.minimumContrastRatio = minimumContrastRatio;
        }
      } catch (cause) {
        callbacksRef.current.onError(errorMessage(cause));
      }
    };
    const themeObserver = new MutationObserver(updateTheme);
    themeObserver.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class", "data-theme"],
    });

    if (restoredScrollback !== undefined) {
      terminal.reset();
      // Agent CLIs are full-screen TUIs. Their raw scrollback is a stream of
      // terminal queries, alt-screen switches, and screen clears that replay
      // into garbage (and echoed query responses), not the previous screen.
      // Show a clean, dimmed plain-text preview of the last output instead;
      // Resume redraws the real session live via --resume.
      const preview = detachedScrollbackPreview(restoredScrollback);
      if (preview) {
        terminal.write(
          `\x1b[2m${preview.replace(/\n/g, "\r\n")}\x1b[0m\r\n`,
        );
      }
      terminal.write(TERMINAL_DETACHED_MODE_RESET, () =>
        setSceneComplete(true),
      );
      attachingRef.current = false;
      setAttaching(false);
    } else {
      const attach = async () => {
        await listenerReadyRef.current;
        const attached = await ipc.terminal.attach({
          sessionId: session.id,
        });
        if (terminalRef.current !== terminal) return;
        sequenceRef.current = attached.sequence;
        terminal.reset();
        // The PTY has this grid now. A hidden pane has not fit yet. The
        // continuation prelude and the scrollback are drawn for this grid, so
        // the replay uses it too. A later fit to a new grid changes the PTY
        // size, and the agent repaints.
        const { cols, rows } = attached.session;
        if (cols > 0 && rows > 0) {
          sizeClaim?.ptySize({ cols, rows });
          if (
            !visibleRef.current &&
            (cols !== terminal.cols || rows !== terminal.rows)
          ) {
            terminal.resize(cols, rows);
          }
        }
        const prelude = attached.prelude;
        if (prelude) {
          // The last agent screen can hold terminal queries. Xterm answers
          // them during the replay, and those answers must not reach the shell.
          replayingRef.current = true;
          preludeReplayingRef.current = true;
          await new Promise<void>((resolve) => {
            terminal.write(terminalContinuationPrelude(prelude), resolve);
          });
          preludeReplayingRef.current = false;
          replayingRef.current = false;
        }
        if (attached.scrollback) {
          replayingRef.current = true;
          await new Promise<void>((resolve) => {
            terminal.write(attached.scrollback, resolve);
          });
          replayingRef.current = false;
        }
        if (attached.session.status !== "running") {
          activityTracker?.finish();
        }
        const pendingEvents = eventsAfterTerminalSnapshot(
          attached.sequence,
          pendingEventsRef.current.splice(0),
        );
        for (const event of pendingEvents) handleEvent(event);
        attachingRef.current = false;
        if (attached.session.status !== "running") {
          terminal.write(TERMINAL_EXIT_MODE_RESET);
          terminal.writeln(
            `\r\n\x1b[2m[process exited ${attached.session.exitCode ?? "?"}]\x1b[0m`,
          );
        }
        setSceneComplete(true);
        setAttaching(false);
        callbacksRef.current.onAttached(session.id);
        // A menu, a dialog or a tab rename that opened during the attach
        // keeps the focus.
        if (
          active &&
          !document.activeElement?.closest(
            '[role="menu"], [role="dialog"], [data-tab-rename-input="true"]',
          )
        ) {
          terminal.focus();
        }
      };
      void attach().catch((cause) => {
        const message = errorMessage(cause);
        attachingRef.current = false;
        const pendingEvents = eventsAfterTerminalSnapshot(
          sequenceRef.current,
          pendingEventsRef.current.splice(0),
        );
        for (const event of pendingEvents) handleEvent(event);
        setAttaching(false);
        callbacksRef.current.onAttachError(session.id, message);
        callbacksRef.current.onError(message);
      });
    }

    return () => {
      fitDisposed = true;
      handleEventRef.current = () => undefined;
      applyVisibleRef.current = () => undefined;
      sizeClaim?.dispose();
      resizeObserver.disconnect();
      themeObserver.disconnect();
      window.removeEventListener("resize", scheduleFit);
      window.clearTimeout(fitTimer);
      if (fitFrame !== undefined) window.cancelAnimationFrame(fitFrame);
      if (followUpFitFrame !== undefined) {
        window.cancelAnimationFrame(followUpFitFrame);
      }
      host.removeEventListener("focusin", focusIn);
      renderDisposable.dispose();
      titleDisposable.dispose();
      keyDisposable.dispose();
      scrollDisposable.dispose();
      resizeDisposable.dispose();
      unregisterDraft();
      unregisterDrop();
      releaseNativeDrop();
      pane.removeAttribute("data-drop-hover");
      dataDisposable.dispose();
      pasteFilter.dispose();
      compositionGuard?.dispose();
      terminal.dispose();
      if (terminalRef.current === terminal) terminalRef.current = null;
    };
  }, [restoredScrollback, session.id, shortcutOverrides]);

  useEffect(() => {
    // App starts tracking when it sends, even if this pane is not mounted.
    if (promptSubmitSignal <= 0) return;
    callbacksRef.current.onPromptSubmitSignalHandled(
      session.id,
      promptSubmitSignal,
    );
  }, [promptSubmitSignal, session.id]);

  useEffect(() => {
    if (active) terminalRef.current?.focus();
  }, [active]);

  useEffect(() => {
    applyVisibleRef.current(visible);
  }, [visible]);

  // A text drag from the DOM pastes its text. A file drag arrives through
  // the native bridge instead, so the DOM handlers leave it alone.
  function domTextDrag(event: DragEvent<HTMLElement>): boolean {
    const types = event.dataTransfer.types;
    return !types.includes("Files") && types.includes("text/plain");
  }
  function handleDragOver(event: DragEvent<HTMLElement>) {
    if (!domTextDrag(event)) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = "copy";
  }
  function handleDrop(event: DragEvent<HTMLElement>) {
    if (!domTextDrag(event)) return;
    event.preventDefault();
    const text = event.dataTransfer.getData("text/plain");
    const terminal = terminalRef.current;
    if (!text || !terminal || processEndedRef.current || attachingRef.current) return;
    callbacksRef.current.onActivate();
    callbacksRef.current.onInput(session.id);
    terminal.paste(sanitizeTerminalPaste(text));
    terminal.focus();
  }

  return (
    <section
      className={`terminal-pane${active ? " terminal-pane-active" : ""}`}
      data-terminal-scene-complete={sceneComplete ? "true" : "false"}
      data-terminal-session-id={session.id}
      onDragOver={handleDragOver}
      onDrop={handleDrop}
      onMouseDown={activate}
      ref={paneRef}
    >
      <div className="terminal-pane-header" ref={toolbarRef}>
        <div className="terminal-pane-toolbar">
          <span
            className="terminal-pane-title"
            title={`${title} · ${session.cwd}`}
          >
            {paneView ? (
              <AgentStateGlyph size="sm" view={paneView} />
            ) : (
              <span
                className={`agent-session-status-dot agent-session-status-dot-${displayStatus}`}
                aria-hidden
              />
            )}
            {title}
          </span>
          <span className="terminal-pane-actions">
            {supervisorReconnect ? (
              <span
                className="terminal-supervisor-reconnect"
                role="status"
                title={supervisorReconnect.message}
              >
                {t("Reconnecting to terminal supervisor… (attempt {attempt})", {
                  attempt: supervisorReconnect.attempt,
                })}
              </span>
            ) : null}
            {splitting || attaching ? (
              <span className="terminal-pane-opening" role="status">
                {t("Opening terminal…")}
              </span>
            ) : null}
            {onSwap ? (
              <button
                aria-label={t("Swap {title} with its neighbor", { title })}
                disabled={closing || splitting}
                onClick={onSwap}
                title={t("Swap places with the neighboring pane")}
                type="button"
              >
                ⇆
              </button>
            ) : null}
            {onRotate ? (
              <button
                aria-label={t("Rotate the split around {title}", { title })}
                disabled={closing || splitting}
                onClick={onRotate}
                title={t("Toggle the split between side by side and stacked")}
                type="button"
              >
                ⟳
              </button>
            ) : null}
            <TerminalSplitMenu
              choices={splitChoices}
              direction="vertical"
              disabled={closing || splitting}
              onSplit={onSplit}
            />
            <TerminalSplitMenu
              choices={splitChoices}
              direction="horizontal"
              disabled={closing || splitting}
              onSplit={onSplit}
            />
            <button
              aria-label={t("Close {title}", { title })}
              disabled={closing || splitting}
              onClick={onClose}
              title={
                session.status === "running"
                  ? t("Stop and close pane")
                  : t("Close pane")
              }
              type="button"
            >
              {closing ? "…" : "×"}
            </button>
          </span>
        </div>
      </div>
      <div
        className={`terminal-stage${processEnded ? " terminal-stage-exited" : ""}`}
        data-grace={exitGrace ? "true" : undefined}
      >
        <div className="terminal-xterm-host" ref={hostRef} />
        {hasNewOutput ? (
          <button
            className="terminal-new-output"
            onClick={() => {
              terminalRef.current?.scrollToBottom();
              terminalRef.current?.focus();
              atBottomRef.current = true;
              setHasNewOutput(false);
            }}
            type="button"
          >
            {t("New output ↓")}
          </button>
        ) : null}
        {session.agentKind ? null : (
          // Agent CLIs render their own mode indicator on the bottom line;
          // the badge would cover it (Shift+Tab looked broken).
          <span className="terminal-access-label">{t("Full access")}</span>
        )}
        {session.agentKind ? <AccountRestartChip sessionId={session.id} /> : null}
        {processEnded ? (
          <div
            className="terminal-exit-overlay"
            data-grace={exitGrace ? "true" : undefined}
          >
            <div className="terminal-exit-card">
              <div className="terminal-exit-heading">
                <span className="terminal-exit-icon" aria-hidden>■</span>
                <strong>
                  {resumeFailure
                    ? t("Resume failed")
                    : onResume
                      ? t("Saved terminal")
                      : session.status === "failed"
                      ? t("Terminal failed")
                      : t("Process exited ({exitCode})", {
                          exitCode: session.exitCode ?? "?",
                        })}
                </strong>
              </div>
              {resumeFailure ? (
                <p className="terminal-exit-message">
                  {t("The resumed process exited with code {exitCode}.", {
                    exitCode: resumeFailure.exitCode ?? "?",
                  })}
                </p>
              ) : supervisorFailureMessage ? (
                <p className="terminal-exit-message">
                  {supervisorFailureMessage}
                </p>
              ) : onResume ? (
                <p className="terminal-exit-message">
                  {t(
                    "Saved scrollback is read-only until the terminal resumes.",
                  )}
                </p>
              ) : onStartFresh ? (
                <p className="terminal-exit-message">
                  {t(
                    "The agent process stopped. Check the terminal output, then retry in the same folder and lane.",
                  )}
                </p>
              ) : session.signal ? (
                <p className="terminal-exit-message">{session.signal}</p>
              ) : null}
              <div className="terminal-exit-actions">
                {onRetrySupervisor ? (
                  <button
                    disabled={retryingSupervisor}
                    onClick={onRetrySupervisor}
                    type="button"
                  >
                    {t("Retry connection")}
                  </button>
                ) : null}
                {onResume ? (
                  <button disabled={resuming} onClick={onResume} type="button">
                    {resuming
                      ? t("Resuming…")
                      : resumeFailure
                        ? t("Retry")
                        : t("Resume")}
                  </button>
                ) : null}
                {onStartFresh ? (
                  <button disabled={resuming} onClick={onStartFresh} type="button">
                    {t("Start fresh")}
                  </button>
                ) : null}
                {onOpenShell ? (
                  <button disabled={resuming} onClick={onOpenShell} type="button">
                    {t("Open shell here")}
                  </button>
                ) : null}
                {onRestart ? (
                  <button disabled={resuming} onClick={onRestart} type="button">
                    {t("Restart")}
                  </button>
                ) : null}
                <button disabled={resuming} onClick={onClose} type="button">
                  {t("Close pane")}
                </button>
              </div>
            </div>
          </div>
        ) : null}
      </div>
    </section>
  );
}

function TerminalSplitMenu({
  choices,
  direction,
  disabled,
  onSplit,
}: {
  choices: TerminalPaneSplitChoice[];
  direction: TerminalPaneSplitDirection;
  disabled: boolean;
  onSplit: TerminalPaneProps["onSplit"];
}) {
  const onMenuToggle = useExclusiveDetailsMenu();
  const t = useT();
  return (
    <details
      className="terminal-split-menu"
      onToggle={(event) => {
        onMenuToggle(event);
        refreshLanesWhenSplitMenuOpens(event.currentTarget.open, () =>
          agentSessionStore.refreshLaneOptions(),
        );
      }}
    >
      <summary
        aria-label={
          direction === "vertical"
            ? t("Split terminal right")
            : t("Split terminal down")
        }
        aria-disabled={disabled}
        title={
          direction === "vertical"
            ? t("Split right (⌘D)")
            : t("Split down (⌘⇧D)")
        }
      >
        {direction === "vertical" ? "⇥" : "⇣"}
      </summary>
      {disabled ? null : (
        <div className="terminal-split-menu-popover" role="menu">
          <button
            onClick={(event) => {
              event.currentTarget.closest("details")?.removeAttribute("open");
              onSplit(direction, "duplicate");
            }}
            role="menuitem"
            type="button"
          >
            <strong>{t("Duplicate")}</strong>
            <small>{t("Same terminal")}</small>
          </button>
          {choices.map((choice) => (
            <button
              data-ready={choice.ready}
              disabled={!choice.ready}
              key={choice.id}
              onClick={(event) => {
                event.currentTarget.closest("details")?.removeAttribute("open");
                onSplit(direction, choice.id);
              }}
              role="menuitem"
              type="button"
            >
              <strong>{choice.label}</strong>
              <small className={choice.ready ? "is-ready" : "is-unavailable"}>
                <span aria-hidden>●</span>
                {choice.readiness}
              </small>
            </button>
          ))}
        </div>
      )}
    </details>
  );
}
