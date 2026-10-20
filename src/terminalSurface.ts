/*
 * The terminal mode reset and contrast techniques adapt Orca terminal modules.
 * Copyright (c) 2026 Lovecast Inc. Orca uses the MIT License.
 * See THIRD_PARTY_NOTICES.md for the license text.
 */

export const TERMINAL_CSS_TOKEN_NAMES = {
  background: "--terminal-background",
  foreground: "--terminal-foreground",
  cursor: "--terminal-cursor",
  selection: "--terminal-selection",
  selectionForeground: "--terminal-selection-foreground",
  black: "--terminal-ansi-black",
  red: "--terminal-ansi-red",
  green: "--terminal-ansi-green",
  yellow: "--terminal-ansi-yellow",
  blue: "--terminal-ansi-blue",
  magenta: "--terminal-ansi-magenta",
  cyan: "--terminal-ansi-cyan",
  white: "--terminal-ansi-white",
  brightBlack: "--terminal-ansi-bright-black",
  brightRed: "--terminal-ansi-bright-red",
  brightGreen: "--terminal-ansi-bright-green",
  brightYellow: "--terminal-ansi-bright-yellow",
  brightBlue: "--terminal-ansi-bright-blue",
  brightMagenta: "--terminal-ansi-bright-magenta",
  brightCyan: "--terminal-ansi-bright-cyan",
  brightWhite: "--terminal-ansi-bright-white",
} as const;

export type TerminalCssToken = keyof typeof TERMINAL_CSS_TOKEN_NAMES;
export type TerminalCssTokens = Record<TerminalCssToken, string>;

export type TerminalPalette = {
  background: string;
  foreground: string;
  cursor: string;
  cursorAccent: string;
  selectionBackground: string;
  selectionForeground: string;
  selectionInactiveBackground: string;
  black: string;
  red: string;
  green: string;
  yellow: string;
  blue: string;
  magenta: string;
  cyan: string;
  white: string;
  brightBlack: string;
  brightRed: string;
  brightGreen: string;
  brightYellow: string;
  brightBlue: string;
  brightMagenta: string;
  brightCyan: string;
  brightWhite: string;
};

function normalizedRgbToken(value: string, name: string) {
  const channels = value
    .trim()
    .split(/\s+/)
    .map((channel) => Number(channel));
  if (
    channels.length !== 3 ||
    channels.some(
      (channel) => !Number.isFinite(channel) || channel < 0 || channel > 255,
    )
  ) {
    throw new Error(`Invalid terminal color token ${name}.`);
  }
  return channels.map((channel) => Math.round(channel)).join(" ");
}

export const LIGHT_BG_MIN_CONTRAST = 4.5;
export const DARK_BG_MIN_CONTRAST = 3;

export function resolveTerminalMinimumContrastRatio(background: string) {
  const rgbMatch = background.trim().match(/^rgb\((.*)\)$/i);
  const channels = normalizedRgbToken(
    rgbMatch?.[1] ?? background,
    "background",
  )
    .split(" ")
    .map(Number);
  const relativeLuminance = channels.reduce((luminance, channel, index) => {
    const normalized = channel / 255;
    const linear =
      normalized <= 0.03928
        ? normalized / 12.92
        : ((normalized + 0.055) / 1.055) ** 2.4;
    return luminance + linear * [0.2126, 0.7152, 0.0722][index];
  }, 0);
  const contrastWithBlack = (relativeLuminance + 0.05) / 0.05;
  const contrastWithWhite = 1.05 / (relativeLuminance + 0.05);

  return contrastWithBlack >= contrastWithWhite
    ? LIGHT_BG_MIN_CONTRAST
    : DARK_BG_MIN_CONTRAST;
}

export function deriveTerminalPalette(tokens: TerminalCssTokens): TerminalPalette {
  const colors = Object.fromEntries(
    Object.entries(tokens).map(([name, value]) => [
      name,
      normalizedRgbToken(value, name),
    ]),
  ) as TerminalCssTokens;
  const color = (name: TerminalCssToken) => `rgb(${colors[name]})`;
  return {
    background: color("background"),
    foreground: color("foreground"),
    cursor: color("cursor"),
    cursorAccent: color("background"),
    // A translucent selection tint keeps each cell's own text colour visible
    // (the DOM renderer does not reliably apply selectionForeground, so an
    // opaque selection just hides the text). Requires allowTransparency: true.
    selectionBackground: `rgb(${colors.selection} / 0.40)`,
    selectionForeground: color("foreground"),
    selectionInactiveBackground: `rgb(${colors.selection} / 0.24)`,
    black: color("black"),
    red: color("red"),
    green: color("green"),
    yellow: color("yellow"),
    blue: color("blue"),
    magenta: color("magenta"),
    cyan: color("cyan"),
    white: color("white"),
    brightBlack: color("brightBlack"),
    brightRed: color("brightRed"),
    brightGreen: color("brightGreen"),
    brightYellow: color("brightYellow"),
    brightBlue: color("brightBlue"),
    brightMagenta: color("brightMagenta"),
    brightCyan: color("brightCyan"),
    brightWhite: color("brightWhite"),
  };
}

export function readTerminalPalette(
  style: Pick<CSSStyleDeclaration, "getPropertyValue">,
) {
  const tokens = Object.fromEntries(
    Object.entries(TERMINAL_CSS_TOKEN_NAMES).map(([name, cssName]) => [
      name,
      style.getPropertyValue(cssName),
    ]),
  ) as TerminalCssTokens;
  return deriveTerminalPalette(tokens);
}

export function eventsAfterTerminalSnapshot<T extends { sequence: number }>(
  snapshotSequence: number,
  events: T[],
): T[] {
  const eventsBySequence = new Map<number, T>();
  for (const event of events) {
    if (event.sequence > snapshotSequence) {
      eventsBySequence.set(event.sequence, event);
    }
  }
  return [...eventsBySequence.values()].sort(
    (left, right) => left.sequence - right.sequence,
  );
}

type TerminalChunkWrite = (chunk: string, callback: () => void) => void;

export function createTerminalReplayGuard(write: TerminalChunkWrite) {
  const replayQueue: boolean[] = [];
  let replayDepth = 0;

  return {
    write(chunk: string, replay: boolean) {
      const startsImmediately = replayQueue.length === 0;
      replayQueue.push(replay);
      if (startsImmediately && replay) replayDepth += 1;

      write(chunk, () => {
        const completedReplay = replayQueue.shift();
        if (completedReplay) replayDepth -= 1;
        if (replayQueue[0]) replayDepth += 1;
      });
    },
    replayDepth() {
      return replayDepth;
    },
  };
}

const RESET_MOUSE_REPORTING =
  "\x1b[?9l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1016l";

export const TERMINAL_EXIT_MODE_RESET =
  `\x18\x1b[?1049l\x1b[?1047l\x1b[?47l\x1b[0m\x1b[0 q` +
  `\x1b[<99u\x1b[=0u\x1b[?25h${RESET_MOUSE_REPORTING}` +
  "\x1b[?1004l\x1b[?2004l";

export const TERMINAL_DETACHED_MODE_RESET =
  `${TERMINAL_EXIT_MODE_RESET}\x1b[?25l`;

// The product name of an agent terminal, as the pane header shows it.
export function terminalAgentLabel(agentKind: string | null | undefined) {
  if (agentKind === "claude") return "Claude Code";
  if (agentKind === "codex") return "Codex";
  if (agentKind === "gemini") return "Gemini CLI";
  if (agentKind === "antigravity") return "Antigravity";
  if (agentKind === "kimi") return "Kimi";
  if (agentKind === "zai") return "GLM";
  if (agentKind === "grok") return "Grok";
  return null;
}

// A continuation shell shows the last screen of the ended agent above its
// own prompt. The mode reset undoes the agent TUI modes (alternate screen,
// mouse reporting) before the dim marker line.
// The reset of mode 1049 also restores the saved cursor, even in the normal
// screen. The cursor save (DECSC) before the reset keeps the cursor below the
// agent output, so the marker and the prompt do not overwrite it. The cancel
// (CAN) first ends a partial escape sequence at the end of the agent output.
export function terminalContinuationPrelude(prelude: {
  scrollback: string;
  agentKind?: string | null;
  exitCode?: number | null;
}) {
  const label =
    terminalAgentLabel(prelude.agentKind) ?? prelude.agentKind ?? "process";
  const marker = `[${label} exited ${prelude.exitCode ?? "?"} · shell]`;
  return `${prelude.scrollback}\x18\x1b7${TERMINAL_EXIT_MODE_RESET}\r\n\x1b[2m${marker}\x1b[0m\r\n`;
}

export function middleTruncate(value: string, maximumLength: number) {
  if (value.length <= maximumLength || maximumLength < 5) return value;
  const sideLength = Math.floor((maximumLength - 1) / 2);
  return `${value.slice(0, sideLength)}…${value.slice(-sideLength)}`;
}
