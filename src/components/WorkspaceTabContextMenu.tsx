import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { createPortal } from "react-dom";
import { useT } from "../i18n/runtime";

// The context menu of a strip tab (ADR 0010, stage S7). Orca's order:
// the split items, the copy item, the close items, then the rename item.
// "Close other tabs" and the two side closes ask once for the whole set.

export type WorkspaceTabMenuAction =
  | "split-right"
  | "split-down"
  | "copy-path"
  | "close"
  | "close-others"
  | "close-right"
  | "close-left"
  | "rename";

export type WorkspaceTabMenuShortcuts = {
  splitRight: string | null;
  splitDown: string | null;
  close: string | null;
  rename: string | null;
};

type MenuItem =
  | { type: "separator" }
  | {
      type: "item";
      action: WorkspaceTabMenuAction;
      label: string;
      shortcut: string | null;
      disabled: boolean;
    };

export function WorkspaceTabContextMenu({
  title,
  position,
  splittable,
  renamable,
  canCloseOthers,
  canCloseRight,
  canCloseLeft,
  shortcuts,
  onAction,
  onDismiss,
}: {
  title: string;
  position: { x: number; y: number };
  splittable: boolean;
  renamable: boolean;
  canCloseOthers: boolean;
  canCloseRight: boolean;
  canCloseLeft: boolean;
  shortcuts: WorkspaceTabMenuShortcuts;
  onAction: (action: WorkspaceTabMenuAction) => void;
  /** `restoreFocus`: the menu closed with Escape, so the tab takes the
   *  focus back. */
  onDismiss: (restoreFocus: boolean) => void;
}) {
  const t = useT();
  const menuRef = useRef<HTMLDivElement>(null);
  const [place, setPlace] = useState(position);

  // The menu opens at the pointer and stays inside the window.
  useLayoutEffect(() => {
    const menu = menuRef.current;
    if (!menu) return;
    const margin = 8;
    const box = menu.getBoundingClientRect();
    setPlace({
      x: Math.max(margin, Math.min(position.x, window.innerWidth - box.width - margin)),
      y: Math.max(margin, Math.min(position.y, window.innerHeight - box.height - margin)),
    });
  }, [position.x, position.y]);

  useEffect(() => {
    menuRef.current
      ?.querySelector<HTMLButtonElement>('[role="menuitem"]:not([disabled])')
      ?.focus();
    const closeOnPointer = (event: PointerEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) onDismiss(false);
    };
    const closeOnBlur = () => onDismiss(false);
    document.addEventListener("pointerdown", closeOnPointer, true);
    window.addEventListener("blur", closeOnBlur);
    window.addEventListener("resize", closeOnBlur);
    return () => {
      document.removeEventListener("pointerdown", closeOnPointer, true);
      window.removeEventListener("blur", closeOnBlur);
      window.removeEventListener("resize", closeOnBlur);
    };
    // The menu mounts once per open.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const items: MenuItem[] = [
    ...(splittable
      ? ([
          {
            type: "item",
            action: "split-right",
            label: t("Split terminal right"),
            shortcut: shortcuts.splitRight,
            disabled: false,
          },
          {
            type: "item",
            action: "split-down",
            label: t("Split terminal down"),
            shortcut: shortcuts.splitDown,
            disabled: false,
          },
          { type: "separator" },
        ] satisfies MenuItem[])
      : []),
    { type: "item", action: "copy-path", label: t("Copy path"), shortcut: null, disabled: false },
    { type: "separator" },
    { type: "item", action: "close", label: t("Close tab"), shortcut: shortcuts.close, disabled: false },
    {
      type: "item",
      action: "close-others",
      label: t("Close other tabs"),
      shortcut: null,
      disabled: !canCloseOthers,
    },
    {
      type: "item",
      action: "close-right",
      label: t("Close tabs to the right"),
      shortcut: null,
      disabled: !canCloseRight,
    },
    {
      type: "item",
      action: "close-left",
      label: t("Close tabs to the left"),
      shortcut: null,
      disabled: !canCloseLeft,
    },
    ...(renamable
      ? ([
          { type: "separator" },
          {
            type: "item",
            action: "rename",
            label: t("Rename"),
            shortcut: shortcuts.rename,
            disabled: false,
          },
        ] satisfies MenuItem[])
      : []),
  ];

  function onKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
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
      onDismiss(true);
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const offset = event.key === "ArrowDown" ? 1 : -1;
      entries[(index + offset + entries.length) % entries.length]?.focus();
    } else if (event.key === "Home" || event.key === "End") {
      event.preventDefault();
      entries[event.key === "Home" ? 0 : entries.length - 1]?.focus();
    } else if (event.key === "Tab") {
      event.preventDefault();
      onDismiss(true);
    }
  }

  return createPortal(
    <div
      aria-label={t("Actions for {title}", { title })}
      className="workspace-tab-menu workspace-tab-context-menu"
      data-workspace-tab-context-menu
      onContextMenu={(event) => event.preventDefault()}
      onKeyDown={onKeyDown}
      ref={menuRef}
      role="menu"
      style={{ left: place.x, top: place.y }}
    >
      {items.map((item, index) =>
        item.type === "separator" ? (
          <div className="workspace-tab-menu-separator" key={`separator-${index}`} role="separator" />
        ) : (
          <button
            className="workspace-tab-menu-item"
            disabled={item.disabled}
            key={item.action}
            onClick={() => onAction(item.action)}
            role="menuitem"
            type="button"
          >
            <span className="workspace-tab-menu-label">{item.label}</span>
            {item.shortcut ? <kbd>{item.shortcut}</kbd> : null}
          </button>
        ),
      )}
    </div>,
    document.body,
  );
}
