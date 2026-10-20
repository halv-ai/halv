import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import type { AgentPaneGlyph } from "../agentPaneStatus.ts";
import { useT } from "../i18n/runtime";
import type { PlusMenuItem } from "../workspaceTabs.ts";
import {
  dropBeforeKey,
  dropIndicatorEdge,
  dropTargetAt,
  hoverCardDelay,
  isDragDistance,
  NO_TAB_STRIP_OVERFLOW,
  revealScrollLeft,
  sameTabStripOverflow,
  TAB_DROP_VERTICAL_SLACK_PX,
  TAB_STRIP_DRAG_SCROLL_INTERVAL_MS,
  tabStripFadeClasses,
  tabStripOverflow,
  tabStripScrollStep,
  type DropSide,
  type TabSpan,
  type TabStripOverflow,
} from "../workspaceTabStrip.ts";
import { AgentGlyph } from "./AgentGlyph";
import { AgentStateGlyph } from "./AgentStateGlyph";
import { WorkspaceTabHoverCard } from "./WorkspaceTabHoverCard";
import {
  WorkspaceTabContextMenu,
  type WorkspaceTabMenuAction,
  type WorkspaceTabMenuShortcuts,
} from "./WorkspaceTabContextMenu";

// The tab strip of one workspace (ADR 0010). It sits in the app header row.
// Each tab is one open conversation. The draft tab is the new task that
// the user composes. Orca's anatomy: status glyph, identity icon, title,
// close button, and a fixed width, so a live title never resizes the strip.
// A shell tab (stage S5b) is a tab too. A double click renames it.
//
// Stage S7 copies Orca's gestures. A press activates its tab on release. A
// press that moves 12 px is a drag: the tabs stay in place, a blue bar
// shows where the tab goes, and the drop moves it. When the tabs do not
// fit, chevrons scroll the strip and the hidden edge fades out. A right
// click opens the tab menu. A double click, the menu or Cmd+R renames.
// A pointer that rests on a tab for 500 ms opens its hover card; the next
// tab then shows its card at once.

export type WorkspaceStripTab = {
  key: string;
  title: string;
  /** The agent kind for an agent conversation, "chat" for a chat
   *  conversation, null for a terminal without an agent. */
  identity: string | null;
  /** The state glyph, with Orca's vocabulary. */
  glyph: AgentPaneGlyph | null;
  /** An unread finished turn. The title is bold until the user views it. */
  unread?: boolean;
  closing: boolean;
  /** A tab without a running process of its own. Only a hint for tests. */
  live: boolean;
  /** A double click, the tab menu or Cmd+R renames the tab: the title of
   *  a shell tab, or the conversation of a conversation tab. */
  renamable?: boolean;
  /** The tab has a terminal panel, so the tab menu can split its pane. */
  splittable?: boolean;
  /** The program line of the hover card: the agent, "Chat", or the shell. */
  program?: string;
  /** The running panes of the tab, for the hover card. */
  paneCount?: number;
};

type WorkspaceTabStripProps = {
  workspaceLabel: string;
  tabs: WorkspaceStripTab[];
  /** The tab that the body shows. Null while the draft tab shows. */
  activeKey: string | null;
  /** The draft tab ("New task") shows. */
  draft: boolean;
  /** The title of the draft tab. "New task" by default. */
  draftTitle?: string;
  onActivate: (key: string) => void;
  onClose: (key: string) => void;
  onCloseDraft: () => void;
  plusItems: PlusMenuItem[];
  agentLabel: (agentKind: string) => string;
  onPlusAgent: (agentKind: string) => void;
  /** "Text conversation": the composer for a chat conversation (ADR 0020). */
  onPlusTextConversation: () => void;
  onPlusAgentSettings: () => void;
  onPlusNewTerminal?: () => void;
  /** A new title for a tab. For a shell tab, an empty title restores the
   *  default. The strip calls it only when the title changed. */
  onRename?: (key: string, title: string) => void;
  /** Cmd+R: the tab whose title the user edits now. The nonce repeats a
   *  request for the same tab. */
  renameRequest?: { key: string; nonce: number } | null;
  /** An action of the tab menu. The strip handles "rename" itself. */
  onTabMenu?: (key: string, action: Exclude<WorkspaceTabMenuAction, "rename">) => void;
  menuShortcuts?: WorkspaceTabMenuShortcuts;
  onPlusReopen?: () => void;
  /** A drop moves the tab before `beforeKey`, or to the end for null. */
  onMove?: (key: string, beforeKey: string | null) => void;
  /** The controls at the right end of the strip, for example the Run
   *  button (Orca's Quick Commands, right of the tabs). */
  trailing?: ReactNode;
};

/** One press on a tab: a click, or a drag after 12 px. */
type TabPress = { dragging: boolean; cleanup: () => void };

type TabDrag = {
  key: string;
  title: string;
  icon: ReactNode;
  /** The ghost box at the start of the drag. Moves write its style. */
  box: { left: number; top: number; width: number; height: number };
  indicator: { key: string; side: DropSide } | null;
};

function stopMiddleButton(event: { button: number; preventDefault: () => void }) {
  if (event.button === 1) event.preventDefault();
}

export function WorkspaceTabStrip({
  workspaceLabel,
  tabs,
  activeKey,
  draft,
  draftTitle,
  onActivate,
  onClose,
  onCloseDraft,
  plusItems,
  agentLabel,
  onPlusAgent,
  onPlusTextConversation,
  onPlusAgentSettings,
  onPlusNewTerminal,
  onRename,
  onMove,
  renameRequest = null,
  onTabMenu,
  menuShortcuts = NO_MENU_SHORTCUTS,
  onPlusReopen,
  trailing = null,
}: WorkspaceTabStripProps) {
  const t = useT();
  const scrollRef = useRef<HTMLDivElement>(null);
  const [renamingKey, setRenamingKey] = useState<string | null>(null);
  const draftKey = "draft";
  const draftLabel = draftTitle ?? t("New task");
  const selectedKey = draft && !activeKey ? draftKey : activeKey;
  const [overflow, setOverflow] = useState<TabStripOverflow>(NO_TAB_STRIP_OVERFLOW);
  const [drag, setDrag] = useState<TabDrag | null>(null);
  const pressRef = useRef<TabPress | null>(null);
  const ghostRef = useRef<HTMLDivElement>(null);
  const tabKeys = tabs.map((tab) => tab.key);
  const tabKeysRef = useRef(tabKeys);
  tabKeysRef.current = tabKeys;
  const [menu, setMenu] = useState<{ key: string; x: number; y: number } | null>(null);
  const [hover, setHover] = useState<{
    key: string;
    anchor: { left: number; bottom: number };
  } | null>(null);
  const hoverTimerRef = useRef<number | null>(null);
  const hoverClosedAtRef = useRef<number | null>(null);
  // A press on a tab hides its card until the pointer leaves the tab
  // (Orca's tooltip rule), so a click never leaves a card behind.
  const hoverPressedKeyRef = useRef<string | null>(null);

  const closeHover = useCallback(() => {
    if (hoverTimerRef.current !== null) {
      window.clearTimeout(hoverTimerRef.current);
      hoverTimerRef.current = null;
    }
    setHover((current) => {
      if (current) hoverClosedAtRef.current = performance.now();
      return null;
    });
  }, []);

  function onTabPointerEnter(key: string, element: HTMLElement) {
    if (drag || menu || renamingKey === key || hoverPressedKeyRef.current === key) return;
    if (hoverTimerRef.current !== null) window.clearTimeout(hoverTimerRef.current);
    const open = () => {
      hoverTimerRef.current = null;
      if (!element.isConnected) return;
      const box = element.getBoundingClientRect();
      setHover({ key, anchor: { left: box.left, bottom: box.bottom } });
    };
    const delay = hoverCardDelay(hoverClosedAtRef.current, performance.now());
    if (delay === 0) open();
    else hoverTimerRef.current = window.setTimeout(open, delay);
  }

  function onTabPointerLeave(key: string) {
    if (hoverPressedKeyRef.current === key) hoverPressedKeyRef.current = null;
    closeHover();
  }

  // A drag, the tab menu, a rename, a scroll and a window blur hide the card.
  useEffect(() => {
    if (drag || menu || renamingKey) closeHover();
  }, [closeHover, drag, menu, renamingKey]);
  useEffect(() => {
    const strip = scrollRef.current;
    strip?.addEventListener("scroll", closeHover, { passive: true });
    window.addEventListener("blur", closeHover);
    return () => {
      strip?.removeEventListener("scroll", closeHover);
      window.removeEventListener("blur", closeHover);
      if (hoverTimerRef.current !== null) window.clearTimeout(hoverTimerRef.current);
    };
  }, [closeHover]);

  // Cmd+R renames the tab that the request names.
  useEffect(() => {
    if (!renameRequest) return;
    const tab = tabs.find((item) => item.key === renameRequest.key);
    if (tab?.renamable && onRename && !tab.closing) setRenamingKey(tab.key);
    // Only a new request starts a rename.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [renameRequest?.key, renameRequest?.nonce]);

  function focusTab(key: string) {
    scrollRef.current
      ?.querySelector<HTMLElement>(`[data-workspace-tab-key="${CSS.escape(key)}"]`)
      ?.focus();
  }

  function onMenuAction(key: string, action: WorkspaceTabMenuAction) {
    setMenu(null);
    if (action === "rename") setRenamingKey(key);
    else onTabMenu?.(key, action);
  }

  const updateOverflow = useCallback(() => {
    const strip = scrollRef.current;
    if (!strip) return;
    const next = tabStripOverflow(strip);
    setOverflow((current) => (sameTabStripOverflow(current, next) ? current : next));
  }, []);

  // The selected tab scrolls into view when it changes, when a tab opens,
  // and when the strip gets narrower. A drag holds the strip still.
  useLayoutEffect(() => {
    const strip = scrollRef.current;
    if (!strip) return undefined;
    const reveal = () => {
      updateOverflow();
      if (!selectedKey || pressRef.current?.dragging) return;
      const tab = strip.querySelector<HTMLElement>(
        `[data-workspace-tab-key="${CSS.escape(selectedKey)}"]`,
      );
      if (!tab) return;
      const stripBox = strip.getBoundingClientRect();
      const tabBox = tab.getBoundingClientRect();
      const left = tabBox.left - stripBox.left - strip.clientLeft + strip.scrollLeft;
      const next = revealScrollLeft(strip, { left, right: left + tabBox.width });
      if (next !== null) strip.scrollLeft = next;
      updateOverflow();
    };
    reveal();
    if (typeof ResizeObserver === "undefined") return undefined;
    const observer = new ResizeObserver(reveal);
    observer.observe(strip);
    return () => observer.disconnect();
  }, [selectedKey, tabs.length, updateOverflow]);

  useEffect(() => {
    const strip = scrollRef.current;
    if (!strip) return undefined;
    strip.addEventListener("scroll", updateOverflow, { passive: true });
    return () => strip.removeEventListener("scroll", updateOverflow);
  }, [updateOverflow]);

  // A press that is still down when the strip unmounts drops its listeners.
  useEffect(() => () => pressRef.current?.cleanup(), []);

  function scrollStrip(direction: "start" | "end", behavior: ScrollBehavior = "smooth") {
    const strip = scrollRef.current;
    if (!strip) return;
    const reduced =
      typeof window.matchMedia === "function" &&
      window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    const step = tabStripScrollStep(strip.clientWidth);
    strip.scrollBy({
      left: direction === "start" ? -step : step,
      behavior: reduced ? "auto" : behavior,
    });
  }

  function tabSpans(): TabSpan[] {
    const strip = scrollRef.current;
    if (!strip) return [];
    return [...strip.querySelectorAll<HTMLElement>("[data-workspace-tab-key]")]
      .filter((node) => node.dataset.workspaceTabKey !== draftKey)
      .map((node) => {
        const box = node.getBoundingClientRect();
        return { key: node.dataset.workspaceTabKey ?? "", left: box.left, right: box.right };
      });
  }

  // Orca's press: the tab activates on release. A press that moves 12 px
  // becomes a drag, and its release drops the tab instead.
  function onTabPointerDown(
    event: ReactPointerEvent<HTMLElement>,
    tab: { key: string; title: string; icon: ReactNode; draggable: boolean },
    activate: () => void,
  ) {
    if (event.button !== 0) return;
    pressRef.current?.cleanup();
    const startX = event.clientX;
    const startY = event.clientY;
    const tabBox = event.currentTarget.getBoundingClientRect();
    let dragging = false;
    let target: { key: string; side: DropSide } | null = null;
    const previousCursor = document.body.style.cursor;
    const previousSelect = document.body.style.userSelect;
    const cleanup = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", cleanup);
      window.removeEventListener("blur", cleanup);
      window.removeEventListener("keydown", escape, true);
      if (dragging) {
        document.body.style.cursor = previousCursor;
        document.body.style.userSelect = previousSelect;
        setDrag(null);
      }
      if (pressRef.current === press) pressRef.current = null;
    };
    const move = (moveEvent: PointerEvent) => {
      if (!dragging) {
        if (!tab.draggable || !onMove) return;
        if (!isDragDistance(moveEvent.clientX - startX, moveEvent.clientY - startY)) return;
        dragging = true;
        press.dragging = true;
        document.body.style.cursor = "grabbing";
        document.body.style.userSelect = "none";
        setDrag({
          key: tab.key,
          title: tab.title,
          icon: tab.icon,
          box: { left: tabBox.left, top: tabBox.top, width: tabBox.width, height: tabBox.height },
          indicator: null,
        });
      }
      const ghost = ghostRef.current;
      if (ghost) {
        ghost.style.left = `${tabBox.left + moveEvent.clientX - startX}px`;
        ghost.style.top = `${tabBox.top + moveEvent.clientY - startY}px`;
      }
      const stripBox = scrollRef.current?.getBoundingClientRect();
      const inside =
        stripBox !== undefined &&
        moveEvent.clientY >= stripBox.top - TAB_DROP_VERTICAL_SLACK_PX &&
        moveEvent.clientY <= stripBox.bottom + TAB_DROP_VERTICAL_SLACK_PX;
      target = inside ? dropTargetAt(tabSpans(), tab.key, moveEvent.clientX) : null;
      const indicator = dropIndicatorEdge(tabKeysRef.current, target);
      setDrag((current) =>
        current &&
        (current.indicator?.key !== indicator?.key || current.indicator?.side !== indicator?.side)
          ? { ...current, indicator }
          : current,
      );
    };
    const up = (upEvent: PointerEvent) => {
      const wasDrag = dragging;
      const dropped = target;
      cleanup();
      if (wasDrag) {
        const beforeKey = dropBeforeKey(tabKeysRef.current, tab.key, dropped);
        if (beforeKey !== undefined) onMove?.(tab.key, beforeKey);
        return;
      }
      if (!isDragDistance(upEvent.clientX - startX, upEvent.clientY - startY)) activate();
    };
    const escape = (keyEvent: KeyboardEvent) => {
      if (!dragging || keyEvent.key !== "Escape") return;
      keyEvent.preventDefault();
      keyEvent.stopPropagation();
      cleanup();
    };
    const press: TabPress = { dragging: false, cleanup };
    pressRef.current = press;
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", cleanup);
    window.addEventListener("blur", cleanup);
    window.addEventListener("keydown", escape, true);
  }

  const orderedKeys = [...tabKeys, ...(draft ? [draftKey] : [])];

  function moveFocus(fromKey: string, offset: -1 | 1) {
    const index = orderedKeys.indexOf(fromKey);
    const next = orderedKeys[(index + offset + orderedKeys.length) % orderedKeys.length];
    scrollRef.current
      ?.querySelector<HTMLElement>(`[data-workspace-tab-key="${CSS.escape(next)}"]`)
      ?.focus();
  }

  function onTabKeyDown(event: ReactKeyboardEvent<HTMLElement>, key: string) {
    if (event.nativeEvent.isComposing) return;
    if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      event.preventDefault();
      moveFocus(key, event.key === "ArrowLeft" ? -1 : 1);
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      if (key !== draftKey) onActivate(key);
    } else if (event.key === "Delete") {
      event.preventDefault();
      if (key === draftKey) onCloseDraft();
      else onClose(key);
    }
  }

  return (
    <div
      className="workspace-tab-strip"
      data-tab-dragging={drag ? "true" : undefined}
      data-workspace-tab-strip
    >
      {overflow.hasOverflow ? (
        <ScrollChevron
          direction="start"
          dragging={drag !== null}
          enabled={overflow.canScrollStart}
          label={t("Scroll tabs left")}
          onScroll={scrollStrip}
          scrollable={() => tabStripOverflow(scrollRef.current ?? NO_SCROLL_BOX).canScrollStart}
        />
      ) : null}
      <div
        aria-label={t("Tabs in {workspace}", { workspace: workspaceLabel })}
        className={["workspace-tab-scroll", tabStripFadeClasses(overflow)].filter(Boolean).join(" ")}
        onWheel={(event) => {
          // A vertical wheel scrolls the strip sideways.
          const strip = scrollRef.current;
          if (!strip || Math.abs(event.deltaX) > Math.abs(event.deltaY)) return;
          strip.scrollLeft += event.deltaY;
        }}
        ref={scrollRef}
        role="tablist"
      >
        {tabs.map((tab) => {
          const selected = tab.key === selectedKey;
          const icon = <AgentGlyph kind={tab.identity} size={12} />;
          const renaming = renamingKey === tab.key;
          return (
            <TabItem
              closeLabel={t("Close tab {title}", { title: tab.title })}
              closing={tab.closing}
              dragged={drag?.key === tab.key}
              dropSide={drag?.indicator?.key === tab.key ? drag.indicator.side : null}
              glyph={tab.glyph}
              icon={icon}
              unread={tab.unread ?? false}
              key={tab.key}
              live={tab.live}
              onActivate={() => onActivate(tab.key)}
              onClose={() => onClose(tab.key)}
              onKeyDown={(event) => onTabKeyDown(event, tab.key)}
              onHoverStart={(element) => onTabPointerEnter(tab.key, element)}
              onHoverEnd={() => onTabPointerLeave(tab.key)}
              onPress={() => {
                hoverPressedKeyRef.current = tab.key;
                closeHover();
              }}
              onContextMenu={(event) => {
                event.preventDefault();
                if (tab.closing || renaming || !onTabMenu) return;
                pressRef.current?.cleanup();
                setMenu({ key: tab.key, x: event.clientX, y: event.clientY });
              }}
              onPointerDown={(event) =>
                onTabPointerDown(
                  event,
                  {
                    key: tab.key,
                    title: tab.title,
                    icon,
                    draggable: !tab.closing && !renaming,
                  },
                  () => {
                    if (!tab.closing) onActivate(tab.key);
                  },
                )
              }
              onRename={
                tab.renamable && onRename
                  ? {
                      active: renaming,
                      begin: () => setRenamingKey(tab.key),
                      commit: (title) => {
                        setRenamingKey(null);
                        onRename(tab.key, title);
                      },
                      cancel: () => setRenamingKey(null),
                      label: t("Rename {title}", { title: tab.title }),
                    }
                  : null
              }
              selected={selected}
              tabKey={tab.key}
              title={tab.title}
            />
          );
        })}
        {draft ? (
          <TabItem
            closeLabel={t("Close tab {title}", { title: draftLabel })}
            closing={false}
            draft
            glyph={null}
            icon={<DraftIcon />}
            live={false}
            onActivate={() => undefined}
            onClose={onCloseDraft}
            onContextMenu={(event) => event.preventDefault()}
            onKeyDown={(event) => onTabKeyDown(event, draftKey)}
            onPointerDown={(event) =>
              onTabPointerDown(
                event,
                { key: draftKey, title: draftLabel, icon: <DraftIcon />, draggable: false },
                () => undefined,
              )
            }
            selected={selectedKey === draftKey}
            tabKey={draftKey}
            title={draftLabel}
          />
        ) : null}
      </div>
      {overflow.hasOverflow ? (
        <ScrollChevron
          direction="end"
          dragging={drag !== null}
          enabled={overflow.canScrollEnd}
          label={t("Scroll tabs right")}
          onScroll={scrollStrip}
          scrollable={() => tabStripOverflow(scrollRef.current ?? NO_SCROLL_BOX).canScrollEnd}
        />
      ) : null}
      {hover
        ? (() => {
            const tab = tabs.find((item) => item.key === hover.key);
            if (!tab) return null;
            return (
              <WorkspaceTabHoverCard
                anchor={hover.anchor}
                info={{
                  title: tab.title,
                  identity: tab.identity,
                  program: tab.program ?? "",
                  glyph: tab.glyph,
                  paneCount: tab.paneCount ?? 0,
                }}
              />
            );
          })()
        : null}
      {menu
        ? (() => {
            const index = tabKeys.indexOf(menu.key);
            const tab = tabs[index];
            if (!tab) return null;
            return (
              <WorkspaceTabContextMenu
                canCloseLeft={index > 0}
                canCloseOthers={tabs.length > 1}
                canCloseRight={index < tabs.length - 1}
                onAction={(action) => onMenuAction(tab.key, action)}
                onDismiss={(restoreFocus) => {
                  setMenu(null);
                  if (restoreFocus) focusTab(tab.key);
                }}
                position={{ x: menu.x, y: menu.y }}
                renamable={Boolean(tab.renamable && onRename)}
                shortcuts={menuShortcuts}
                splittable={tab.splittable ?? false}
                title={tab.title}
              />
            );
          })()
        : null}
      {drag
        ? createPortal(
            <div
              aria-hidden
              className="workspace-tab-drag-ghost"
              ref={ghostRef}
              style={{
                left: drag.box.left,
                top: drag.box.top,
                width: drag.box.width,
                height: drag.box.height,
              }}
            >
              {drag.icon}
              <span className="workspace-tab-title">{drag.title}</span>
            </div>,
            document.body,
          )
        : null}
      <PlusMenu
        agentLabel={agentLabel}
        items={plusItems}
        onAgent={onPlusAgent}
        onAgentSettings={onPlusAgentSettings}
        onTextConversation={onPlusTextConversation}
        onNewTerminal={onPlusNewTerminal}
        onReopen={onPlusReopen}
      />
      {trailing ? <div className="workspace-tab-strip-end">{trailing}</div> : null}
    </div>
  );
}

function DraftIcon() {
  return (
    <svg
      aria-hidden
      className="agent-glyph shrink-0 text-content-muted"
      fill="none"
      height={12}
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      strokeWidth="2"
      viewBox="0 0 24 24"
      width={12}
    >
      <path d="M4 20h4L19 9l-4-4L4 16v4Z" />
    </svg>
  );
}

const NO_SCROLL_BOX = { scrollWidth: 0, clientWidth: 0, scrollLeft: 0 };

const NO_MENU_SHORTCUTS: WorkspaceTabMenuShortcuts = {
  splitRight: null,
  splitDown: null,
  close: null,
  rename: null,
};

// Orca's overflow chevron. A click scrolls three quarters of the strip.
// While a tab drags, the pointer over a chevron scrolls the strip in steps.
function ScrollChevron({
  direction,
  enabled,
  dragging,
  label,
  onScroll,
  scrollable,
}: {
  direction: "start" | "end";
  enabled: boolean;
  dragging: boolean;
  label: string;
  onScroll: (direction: "start" | "end", behavior?: ScrollBehavior) => void;
  scrollable: () => boolean;
}) {
  const intervalRef = useRef<number | null>(null);
  const stop = useCallback(() => {
    if (intervalRef.current !== null) {
      window.clearInterval(intervalRef.current);
      intervalRef.current = null;
    }
  }, []);
  useEffect(() => {
    if (!dragging) stop();
  }, [dragging, stop]);
  useEffect(() => stop, [stop]);
  return (
    <button
      aria-disabled={!enabled}
      aria-label={label}
      className="workspace-tab-chevron"
      data-direction={direction}
      disabled={!dragging && !enabled}
      onClick={() => onScroll(direction)}
      onPointerEnter={() => {
        stop();
        if (!dragging || !scrollable()) return;
        const tick = () => {
          if (!scrollable()) {
            stop();
            return;
          }
          onScroll(direction, "auto");
        };
        tick();
        intervalRef.current = window.setInterval(tick, TAB_STRIP_DRAG_SCROLL_INTERVAL_MS);
      }}
      onPointerLeave={stop}
      tabIndex={-1}
      title={label}
      type="button"
    >
      <svg aria-hidden fill="none" height="14" stroke="currentColor" strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" viewBox="0 0 24 24" width="14">
        <path d={direction === "start" ? "m15 18-6-6 6-6" : "m9 18 6-6-6-6"} />
      </svg>
    </button>
  );
}

function TabItem({
  tabKey,
  title,
  icon,
  glyph,
  unread = false,
  selected,
  closing,
  live,
  draft = false,
  closeLabel,
  onActivate,
  onClose,
  onKeyDown,
  onPointerDown,
  onContextMenu,
  onHoverStart,
  onHoverEnd,
  onPress,
  onRename = null,
  dragged = false,
  dropSide = null,
}: {
  tabKey: string;
  title: string;
  icon: ReactNode;
  glyph: AgentPaneGlyph | null;
  unread?: boolean;
  selected: boolean;
  closing: boolean;
  live: boolean;
  draft?: boolean;
  closeLabel: string;
  onActivate: () => void;
  onClose: () => void;
  onKeyDown: (event: ReactKeyboardEvent<HTMLElement>) => void;
  onPointerDown: (event: ReactPointerEvent<HTMLElement>) => void;
  onContextMenu: (event: ReactMouseEvent<HTMLElement>) => void;
  onHoverStart?: (element: HTMLElement) => void;
  onHoverEnd?: () => void;
  /** Any press in the tab, also on its close button. */
  onPress?: () => void;
  dragged?: boolean;
  dropSide?: DropSide | null;
  onRename?: {
    active: boolean;
    begin: () => void;
    commit: (title: string) => void;
    cancel: () => void;
    label: string;
  } | null;
}) {
  return (
    <div
      aria-busy={closing || undefined}
      aria-selected={selected}
      className="workspace-tab"
      data-active={selected}
      data-closing={closing || undefined}
      data-draft={draft || undefined}
      data-dragged={dragged || undefined}
      data-drop-indicator={dropSide ?? undefined}
      data-live={live || undefined}
      data-unread={unread || undefined}
      data-workspace-tab-key={tabKey}
      onAuxClick={(event) => {
        if (event.button !== 1) return;
        event.preventDefault();
        if (!closing && !onRename?.active) onClose();
      }}
      onClick={(event) => {
        // A pointer click activates on release (onPointerDown). A click
        // without a pointer, such as from assistive technology, has no
        // detail count.
        if (event.detail === 0 && !closing) onActivate();
      }}
      onDoubleClick={() => {
        if (!closing) onRename?.begin();
      }}
      onContextMenu={onContextMenu}
      onKeyDown={onKeyDown}
      onMouseDown={stopMiddleButton}
      onPointerDownCapture={() => onPress?.()}
      onPointerEnter={(event) => onHoverStart?.(event.currentTarget)}
      onPointerLeave={() => onHoverEnd?.()}
      onPointerDown={(event) => {
        if (onRename?.active) return;
        onPointerDown(event);
      }}
      role="tab"
      tabIndex={selected ? 0 : -1}
      title={title}
    >
      {dropSide ? <span aria-hidden className="workspace-tab-drop" data-side={dropSide} /> : null}
      {closing ? (
        <span aria-hidden className="workspace-tab-spinner conversation-spinner" />
      ) : glyph ? (
        <AgentStateGlyph glyph={glyph} size="sm" />
      ) : null}
      {icon}
      {onRename?.active ? (
        <TabRenameInput
          label={onRename.label}
          onCancel={onRename.cancel}
          onCommit={onRename.commit}
          title={title}
        />
      ) : (
        <span className="workspace-tab-title">{title}</span>
      )}
      <button
        aria-label={closeLabel}
        className="workspace-tab-close"
        disabled={closing}
        onClick={(event) => {
          event.stopPropagation();
          onClose();
        }}
        onKeyDown={(event) => event.stopPropagation()}
        // The close button never starts a press on its tab.
        onPointerDown={(event) => {
          if (event.button === 0) event.stopPropagation();
        }}
        tabIndex={-1}
        type="button"
      >
        <svg aria-hidden fill="none" height="10" stroke="currentColor" strokeLinecap="round" strokeWidth="1.8" viewBox="0 0 12 12" width="10">
          <path d="m3 3 6 6M9 3 3 9" />
        </svg>
      </button>
    </div>
  );
}

// The inline rename field of a tab. Enter or a blur saves, Escape cancels.
// The tab chords skip it (`data-tab-rename-input`).
function TabRenameInput({
  title,
  label,
  onCommit,
  onCancel,
}: {
  title: string;
  label: string;
  onCommit: (title: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(title);
  const doneRef = useRef(false);
  const finish = (save: boolean) => {
    if (doneRef.current) return;
    doneRef.current = true;
    // An unchanged title is no rename, so a conversation keeps its
    // automatic title and a shell tab keeps its live title.
    if (save && value.trim() !== title.trim()) onCommit(value.trim());
    else onCancel();
  };
  return (
    <input
      aria-label={label}
      autoFocus
      className="workspace-tab-rename"
      data-tab-rename-input="true"
      onBlur={() => finish(true)}
      onChange={(event) => setValue(event.target.value)}
      onClick={(event) => event.stopPropagation()}
      onDoubleClick={(event) => event.stopPropagation()}
      onPointerDown={(event) => event.stopPropagation()}
      onFocus={(event) => event.currentTarget.select()}
      onKeyDown={(event) => {
        event.stopPropagation();
        if (event.nativeEvent.isComposing) return;
        if (event.key === "Enter") {
          event.preventDefault();
          finish(true);
        } else if (event.key === "Escape") {
          event.preventDefault();
          finish(false);
        }
      }}
      value={value}
    />
  );
}

function PlusMenu({
  items,
  agentLabel,
  onAgent,
  onTextConversation,
  onAgentSettings,
  onNewTerminal,
  onReopen,
}: {
  items: PlusMenuItem[];
  agentLabel: (agentKind: string) => string;
  onAgent: (agentKind: string) => void;
  onTextConversation: () => void;
  onAgentSettings: () => void;
  onNewTerminal?: () => void;
  onReopen?: () => void;
}) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return undefined;
    menuRef.current
      ?.querySelector<HTMLButtonElement>('[role="menuitem"]:not([disabled])')
      ?.focus();
    const closeOnPointer = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const closeOnBlur = () => setOpen(false);
    document.addEventListener("pointerdown", closeOnPointer);
    window.addEventListener("blur", closeOnBlur);
    return () => {
      document.removeEventListener("pointerdown", closeOnPointer);
      window.removeEventListener("blur", closeOnBlur);
    };
  }, [open]);

  function choose(action: () => void) {
    setOpen(false);
    action();
  }

  function onMenuKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    if (event.nativeEvent.isComposing) return;
    const entries = [
      ...(menuRef.current?.querySelectorAll<HTMLButtonElement>(
        '[role="menuitem"]:not([disabled])',
      ) ?? []),
    ];
    const index = entries.indexOf(document.activeElement as HTMLButtonElement);
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      setOpen(false);
      buttonRef.current?.focus();
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const offset = event.key === "ArrowDown" ? 1 : -1;
      entries[(index + offset + entries.length) % entries.length]?.focus();
    } else if (event.key === "Tab") {
      setOpen(false);
    }
  }

  return (
    <div className="workspace-tab-plus" ref={rootRef}>
      <button
        aria-expanded={open}
        aria-haspopup="menu"
        aria-label={t("New tab")}
        className="workspace-tab-plus-button"
        onClick={() => setOpen((current) => !current)}
        ref={buttonRef}
        title={t("New tab")}
        type="button"
      >
        <svg aria-hidden fill="none" height="12" stroke="currentColor" strokeLinecap="round" strokeWidth="1.8" viewBox="0 0 12 12" width="12">
          <path d="M6 2v8M2 6h8" />
        </svg>
      </button>
      {open ? (
        <div
          aria-label={t("New tab")}
          className="workspace-tab-menu"
          onKeyDown={onMenuKeyDown}
          ref={menuRef}
          role="menu"
        >
          {items.map((item, index) => {
            if (item.type === "separator") {
              return <div className="workspace-tab-menu-separator" key={`separator-${index}`} role="separator" />;
            }
            if (item.type === "agent") {
              return (
                <button
                  className="workspace-tab-menu-item"
                  key={`agent-${item.agentKind}`}
                  onClick={() => choose(() => onAgent(item.agentKind))}
                  role="menuitem"
                  type="button"
                >
                  <AgentGlyph kind={item.agentKind} size={12} />
                  <span className="workspace-tab-menu-label">{agentLabel(item.agentKind)}</span>
                  {item.shortcut ? <kbd>{item.shortcut}</kbd> : null}
                </button>
              );
            }
            const label =
              item.id === "no-agents"
                ? t("No agents ready")
                : item.id === "text-conversation"
                  ? t("Text conversation")
                  : item.id === "agent-settings"
                    ? t("Agent settings…")
                    : item.id === "new-terminal"
                      ? t("New Terminal")
                      : t("Reopen closed tab");
            const action =
              item.id === "text-conversation"
                ? onTextConversation
                : item.id === "agent-settings"
                  ? onAgentSettings
                  : item.id === "new-terminal"
                    ? (onNewTerminal ?? null)
                    : item.id === "reopen"
                      ? (onReopen ?? null)
                      : null;
            return (
              <button
                className="workspace-tab-menu-item"
                disabled={item.disabled || !action}
                key={item.id}
                onClick={() => {
                  if (action) choose(action);
                }}
                role="menuitem"
                type="button"
              >
                <span className="workspace-tab-menu-label">{label}</span>
                {item.shortcut ? <kbd>{item.shortcut}</kbd> : null}
              </button>
            );
          })}
        </div>
      ) : null}
    </div>
  );
}
