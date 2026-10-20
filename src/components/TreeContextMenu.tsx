import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
} from "react";
import { createPortal } from "react-dom";
import type { TreeMenuItem } from "../projectTreeMenus";

// The row menu of the Projects tree (ADR 0010, stage S7). It opens at the
// pointer for a right click, or under the row for the "…" button and the
// keyboard (Shift+F10, the context menu key). It follows the WAI-ARIA menu
// pattern: Up and Down move, Right opens a submenu, Left closes it, Enter
// or Space runs an item, and Escape closes the menu.

export type TreeMenuRequest = {
  x: number;
  y: number;
  label: string;
  items: TreeMenuItem[];
  onSelect: (id: string) => void;
};

const VIEWPORT_MARGIN = 8;

function enabledItems(root: HTMLElement | null) {
  return [
    ...(root?.querySelectorAll<HTMLElement>(
      ':scope > [role="menuitem"]:not([aria-disabled="true"])',
    ) ?? []),
  ];
}

function moveFocus(root: HTMLElement | null, offset: number | "first" | "last") {
  const items = enabledItems(root);
  if (items.length === 0) return;
  const index = items.indexOf(document.activeElement as HTMLElement);
  const next =
    offset === "first"
      ? 0
      : offset === "last"
        ? items.length - 1
        : (index + offset + items.length) % items.length;
  items[next]?.focus();
}

function MenuPanel({
  label,
  items,
  x,
  y,
  level,
  onRun,
  onClose,
  onBack,
}: {
  label: string;
  items: TreeMenuItem[];
  x: number;
  y: number;
  level: number;
  onRun: (id: string) => void;
  onClose: () => void;
  onBack?: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ left: x, top: y });
  const [submenu, setSubmenu] = useState<{
    id: string;
    x: number;
    y: number;
    focus: boolean;
  } | null>(null);

  // Keep the menu inside the window.
  useLayoutEffect(() => {
    const node = ref.current;
    if (!node) return;
    const rect = node.getBoundingClientRect();
    const left = Math.max(
      VIEWPORT_MARGIN,
      Math.min(x, window.innerWidth - rect.width - VIEWPORT_MARGIN),
    );
    const top = Math.max(
      VIEWPORT_MARGIN,
      Math.min(y, window.innerHeight - rect.height - VIEWPORT_MARGIN),
    );
    setPosition({ left, top });
  }, [x, y]);

  useEffect(() => {
    moveFocus(ref.current, "first");
  }, []);

  function openSubmenu(id: string, target: HTMLElement, focus: boolean) {
    const rect = target.getBoundingClientRect();
    setSubmenu({ id, x: rect.right - 4, y: rect.top - 4, focus });
  }

  function handleKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.target !== document.activeElement) return;
    const current = (event.target as HTMLElement).closest<HTMLElement>('[role="menuitem"]');
    if (current?.parentElement !== ref.current) return;
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        moveFocus(ref.current, 1);
        return;
      case "ArrowUp":
        event.preventDefault();
        moveFocus(ref.current, -1);
        return;
      case "Home":
        event.preventDefault();
        moveFocus(ref.current, "first");
        return;
      case "End":
        event.preventDefault();
        moveFocus(ref.current, "last");
        return;
      case "ArrowRight": {
        const id = current?.dataset.menuId;
        const entry = items.find((item) => item.kind === "item" && item.id === id);
        if (current && entry?.kind === "item" && entry.children) {
          event.preventDefault();
          openSubmenu(entry.id, current, true);
        }
        return;
      }
      case "ArrowLeft":
        if (onBack) {
          event.preventDefault();
          event.stopPropagation();
          onBack();
        }
        return;
      case "Escape":
        event.preventDefault();
        event.stopPropagation();
        onClose();
        return;
      case "Tab":
        event.preventDefault();
        onClose();
        return;
    }
  }

  return (
    <>
      <div
        aria-label={label}
        className="tree-menu"
        data-menu-level={level}
        onKeyDown={handleKeyDown}
        ref={ref}
        role="menu"
        style={{ left: position.left, top: position.top }}
      >
        {items.map((entry, index) =>
          entry.kind === "separator" ? (
            <div className="tree-menu-separator" key={`separator:${index}`} role="separator" />
          ) : (
            <button
              aria-disabled={entry.disabled ? true : undefined}
              aria-expanded={entry.children ? submenu?.id === entry.id : undefined}
              aria-haspopup={entry.children ? "menu" : undefined}
              className="tree-menu-item"
              data-danger={entry.danger ? "" : undefined}
              data-menu-id={entry.id}
              key={entry.id}
              onClick={(event) => {
                if (entry.disabled) return;
                if (entry.children) {
                  openSubmenu(entry.id, event.currentTarget, true);
                  return;
                }
                onRun(entry.id);
              }}
              onPointerEnter={(event) => {
                if (entry.children && !entry.disabled) {
                  openSubmenu(entry.id, event.currentTarget, false);
                } else {
                  setSubmenu(null);
                }
              }}
              role="menuitem"
              tabIndex={-1}
              title={entry.title}
              type="button"
            >
              <span className="min-w-0 flex-1 truncate">{entry.label}</span>
              {entry.children ? (
                <span aria-hidden className="tree-menu-chevron">
                  {"›"}
                </span>
              ) : null}
            </button>
          ),
        )}
      </div>
      {submenu
        ? (() => {
            const parent = items.find(
              (item) => item.kind === "item" && item.id === submenu.id,
            );
            if (parent?.kind !== "item" || !parent.children) return null;
            return (
              <MenuPanel
                items={parent.children}
                key={submenu.id}
                label={parent.label}
                level={level + 1}
                onBack={() => {
                  setSubmenu(null);
                  ref.current
                    ?.querySelector<HTMLElement>(`[data-menu-id="${CSS.escape(parent.id)}"]`)
                    ?.focus();
                }}
                onClose={onClose}
                onRun={onRun}
                x={submenu.x}
                y={submenu.y}
              />
            );
          })()
        : null}
    </>
  );
}

export function TreeContextMenu({
  request,
  onClose,
}: {
  request: TreeMenuRequest;
  onClose: () => void;
}) {
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const closeOutside = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) onClose();
    };
    const closeOnBlur = () => onClose();
    document.addEventListener("pointerdown", closeOutside, true);
    window.addEventListener("blur", closeOnBlur);
    window.addEventListener("resize", closeOnBlur);
    return () => {
      document.removeEventListener("pointerdown", closeOutside, true);
      window.removeEventListener("blur", closeOnBlur);
      window.removeEventListener("resize", closeOnBlur);
    };
  }, [onClose]);

  return createPortal(
    <div className="tree-menu-layer" ref={rootRef}>
      <MenuPanel
        items={request.items}
        label={request.label}
        level={1}
        onClose={onClose}
        onRun={(id) => {
          onClose();
          request.onSelect(id);
        }}
        x={request.x}
        y={request.y}
      />
    </div>,
    document.body,
  );
}
