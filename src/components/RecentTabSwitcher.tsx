import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { AgentPaneGlyph } from "../agentPaneStatus.ts";
import { useT } from "../i18n/runtime";
import { shortcutMatchesEvent } from "../shortcuts.ts";
import { nextSwitcherIndex } from "../workspaceTabHistory.ts";
import { workspaceTabKeyTarget } from "../useWorkspaceTabKeys.ts";
import { AgentGlyph } from "./AgentGlyph";
import { AgentStateGlyph } from "./AgentStateGlyph";

// Orca's Ctrl+Tab switcher (ADR 0010, stage S7). Ctrl+Tab opens a list of
// the tabs of the workspace, most recent first, with the previous tab
// selected. Each Tab moves down, Shift+Tab moves up. The release of Ctrl
// shows the selected tab. Escape and a window blur cancel.

export type RecentTabItem = {
  key: string;
  title: string;
  identity: string | null;
  glyph: AgentPaneGlyph | null;
};

type Switcher = { items: RecentTabItem[]; selected: number };

export function RecentTabSwitcher({
  enabled,
  binding,
  items,
  onCommit,
}: {
  /** The strip shows and no dialog blocks the keys. */
  enabled: boolean;
  /** The chord without Shift, for example "ctrl+tab". */
  binding: string;
  /** The tabs, most recent first: the active tab, then the others. Null
   *  when fewer than two tabs show. */
  items: () => RecentTabItem[] | null;
  onCommit: (key: string) => void;
}) {
  const t = useT();
  const [switcher, setSwitcherState] = useState<Switcher | null>(null);
  const switcherRef = useRef<Switcher | null>(null);
  const latest = useRef({ items, onCommit });
  latest.current = { items, onCommit };

  useEffect(() => {
    const set = (next: Switcher | null) => {
      switcherRef.current = next;
      setSwitcherState(next);
    };
    const consume = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopImmediatePropagation();
    };
    const commit = () => {
      const current = switcherRef.current;
      set(null);
      const item = current?.items[current.selected];
      if (item) latest.current.onCommit(item.key);
    };
    const cancel = () => {
      if (switcherRef.current) set(null);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      const chord = shortcutMatchesEvent({ ...keyFields(event), shiftKey: false }, binding);
      if (chord && enabled && !event.isComposing && workspaceTabKeyTarget(event.target)) {
        consume(event);
        const direction = event.shiftKey ? -1 : 1;
        const current = switcherRef.current;
        if (current) {
          set({
            ...current,
            selected: nextSwitcherIndex(current.items.length, current.selected, direction),
          });
          return;
        }
        const listed = latest.current.items();
        if (!listed || listed.length < 2) return;
        // The active tab is first, so the first press selects the previous
        // tab and a quick press and release toggles between two tabs.
        set({ items: listed, selected: nextSwitcherIndex(listed.length, 0, direction) });
        return;
      }
      if (switcherRef.current && event.key === "Escape") {
        consume(event);
        cancel();
      }
    };
    const onKeyUp = (event: KeyboardEvent) => {
      if (!switcherRef.current) return;
      // Some engines report the last release as a Tab keyup after Control
      // is up, so that commits too (Orca).
      if (event.key === "Control" || (event.key === "Tab" && !event.ctrlKey)) {
        consume(event);
        commit();
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("keyup", onKeyUp, true);
    window.addEventListener("blur", cancel);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("keyup", onKeyUp, true);
      window.removeEventListener("blur", cancel);
    };
  }, [binding, enabled]);

  // A strip that goes away (a dialog, another view) drops the switcher.
  useEffect(() => {
    if (!enabled && switcherRef.current) {
      switcherRef.current = null;
      setSwitcherState(null);
    }
  }, [enabled]);

  if (!switcher) return null;
  return createPortal(
    <div className="recent-tab-switcher-layer">
      <div aria-label={t("Switch tabs")} className="recent-tab-switcher" role="listbox">
        <div className="recent-tab-switcher-header">{t("Switch tab")}</div>
        <div className="recent-tab-switcher-list">
          {switcher.items.map((item, index) => (
            <div
              aria-selected={index === switcher.selected}
              className="recent-tab-switcher-item"
              data-recent-tab-key={item.key}
              key={item.key}
              role="option"
            >
              <AgentGlyph kind={item.identity} size={14} />
              <span className="recent-tab-switcher-title">{item.title}</span>
              {item.glyph ? <AgentStateGlyph glyph={item.glyph} size="sm" /> : null}
            </div>
          ))}
        </div>
      </div>
    </div>,
    document.body,
  );
}

function keyFields(event: KeyboardEvent) {
  return {
    key: event.key,
    code: event.code,
    metaKey: event.metaKey,
    ctrlKey: event.ctrlKey,
    altKey: event.altKey,
  };
}
