// A native file drop on a terminal pane pastes the paths into that PTY.
// Tauri handles file drags natively, so the DOM never sees them. The bridge
// finds the pane under the drop point and pastes through the pane's own paste
// path, which keeps bracketed paste intact and never writes an Enter.

export type TerminalQuoteStyle = "posix" | "windows";

export type DropBounds = {
  left: number;
  top: number;
  right: number;
  bottom: number;
};

export type TerminalDropTarget = {
  /** The live pane bounds, or null when the pane is not on screen. */
  bounds: () => DropBounds | null;
  /** The shell of the session, for example /bin/zsh or pwsh.exe. */
  shell: string;
  /** Pastes prepared text into the PTY. */
  paste: (text: string) => void;
  /** Shows or hides the drop highlight on the pane. */
  setHover?: (hover: boolean) => void;
};

const WINDOWS_SHELL = /^(?:cmd|powershell|pwsh)(?:\.exe)?$/iu;
// Characters that need no quotes in a POSIX shell. A leading tilde expands,
// so the tilde stays out of this set.
const POSIX_SAFE = /^[A-Za-z0-9_./+:@%,=-]+$/u;
const WINDOWS_UNSAFE = /[\s&()^;,=%'`]/u;
const CONTROL_CHARS = /[\u0000-\u001f\u007f-\u009f]/u;

export function terminalQuoteStyle(shell: string): TerminalQuoteStyle {
  const name = shell.replace(/\\/gu, "/").split("/").pop() ?? "";
  return WINDOWS_SHELL.test(name) ? "windows" : "posix";
}

export function quoteDroppedPath(path: string, style: TerminalQuoteStyle): string {
  if (style === "windows") {
    return WINDOWS_UNSAFE.test(path) ? `"${path}"` : path;
  }
  if (POSIX_SAFE.test(path)) return path;
  return `'${path.replace(/'/gu, "'\\''")}'`;
}

/** Quoted paths separated by one space, with one trailing space. */
export function droppedPathsText(
  paths: readonly string[],
  style: TerminalQuoteStyle,
): string {
  const quoted = paths
    .map((path) => path.trim())
    .filter((path) => path.length > 0 && !CONTROL_CHARS.test(path))
    .map((path) => quoteDroppedPath(path, style));
  return quoted.length > 0 ? `${quoted.join(" ")} ` : "";
}

function contains(bounds: DropBounds, x: number, y: number): boolean {
  return x >= bounds.left && x <= bounds.right && y >= bounds.top && y <= bounds.bottom;
}

function area(bounds: DropBounds): number {
  return Math.max(0, bounds.right - bounds.left) * Math.max(0, bounds.bottom - bounds.top);
}

export function createTerminalDropBridge() {
  const targets = new Map<string, TerminalDropTarget>();
  let hovered: string | null = null;

  const setHovered = (id: string | null) => {
    if (hovered === id) return;
    if (hovered) targets.get(hovered)?.setHover?.(false);
    hovered = id;
    if (id) targets.get(id)?.setHover?.(true);
  };

  return {
    register(id: string, target: TerminalDropTarget) {
      targets.set(id, target);
      return () => {
        if (targets.get(id) !== target) return;
        if (hovered === id) hovered = null;
        targets.delete(id);
      };
    },
    get size() {
      return targets.size;
    },
    /** The id of the smallest visible pane under the point, or null. */
    targetAt(x: number, y: number): string | null {
      let best: { id: string; area: number } | null = null;
      for (const [id, target] of targets) {
        const bounds = target.bounds();
        if (!bounds || area(bounds) <= 0 || !contains(bounds, x, y)) continue;
        const size = area(bounds);
        if (!best || size < best.area) best = { id, area: size };
      }
      return best?.id ?? null;
    },
    hover(x: number, y: number): string | null {
      const id = this.targetAt(x, y);
      setHovered(id);
      return id;
    },
    clearHover() {
      setHovered(null);
    },
    /** Pastes the paths into the pane under the point. True when a pane took them. */
    drop(x: number, y: number, paths: readonly string[]): boolean {
      const id = this.targetAt(x, y);
      setHovered(null);
      if (!id) return false;
      const target = targets.get(id);
      if (!target) return false;
      const text = droppedPathsText(paths, terminalQuoteStyle(target.shell));
      if (!text) return false;
      target.paste(text);
      return true;
    },
  };
}

export const terminalDropBridge = createTerminalDropBridge();
