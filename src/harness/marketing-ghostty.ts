import {
  MARKETING_GHOSTTY_PLAN_MODE_STATUS,
  MARKETING_GHOSTTY_WINDOW_TITLE,
} from "./marketing-exports.ts";

const MARKETING_GHOSTTY_SESSION_ID = "marketing-ghostty-claude-code";
const COMPOSER_INNER_WIDTH = 100;

type MarketingGhosttyTheme = "dark" | "light";

const TERMINAL_THEMES = {
  dark: {
    background: "#111315",
    foreground: "#e8e5df",
    cursor: "#d8d3ca",
    cursorAccent: "#111315",
    selectionBackground: "#62707f66",
    selectionForeground: "#ffffff",
    black: "#111315",
    red: "#e06c75",
    green: "#98c379",
    yellow: "#d8b26e",
    blue: "#76a9d4",
    magenta: "#c594c5",
    cyan: "#79b8b6",
    white: "#d9d5cf",
    brightBlack: "#777b80",
    brightRed: "#ef7d86",
    brightGreen: "#acd58c",
    brightYellow: "#e8c17b",
    brightBlue: "#8cbee6",
    brightMagenta: "#d9a9d8",
    brightCyan: "#91d0cc",
    brightWhite: "#f7f4ee",
  },
  light: {
    background: "#f5f3ef",
    foreground: "#2c2b28",
    cursor: "#3f3d38",
    cursorAccent: "#f5f3ef",
    selectionBackground: "#8aa1b64d",
    selectionForeground: "#171715",
    black: "#2c2b28",
    red: "#a9363e",
    green: "#397642",
    yellow: "#8a641d",
    blue: "#356b99",
    magenta: "#805080",
    cyan: "#347574",
    white: "#dad6cf",
    brightBlack: "#77736d",
    brightRed: "#bd4850",
    brightGreen: "#4e8956",
    brightYellow: "#9b7429",
    brightBlue: "#4c80ad",
    brightMagenta: "#956595",
    brightCyan: "#498986",
    brightWhite: "#ffffff",
  },
} as const;

const MARKETING_GHOSTTY_STYLES = `
  html,
  body,
  #root {
    width: 100%;
    height: 100%;
    margin: 0;
    overflow: hidden;
  }

  body {
    font-family: -apple-system, BlinkMacSystemFont, "SF Pro Text", sans-serif;
  }

  .marketing-ghostty-stage {
    --backdrop-top: #303238;
    --backdrop-bottom: #1b1e23;
    --backdrop-glow-a: rgba(215, 209, 197, 0.12);
    --backdrop-glow-b: rgba(107, 120, 140, 0.18);
    --window-edge: rgba(255, 255, 255, 0.14);
    --titlebar: #292a2d;
    --titlebar-edge: rgba(255, 255, 255, 0.08);
    --title-color: #deddd9;
    --terminal-background: #111315;
    box-sizing: border-box;
    display: grid;
    place-items: center;
    position: relative;
    width: 100vw;
    height: 100vh;
    overflow: hidden;
    background:
      radial-gradient(circle at 18% 18%, var(--backdrop-glow-a), transparent 34%),
      radial-gradient(circle at 82% 82%, var(--backdrop-glow-b), transparent 38%),
      linear-gradient(145deg, var(--backdrop-top), var(--backdrop-bottom));
  }

  .marketing-ghostty-stage[data-theme="light"] {
    --backdrop-top: #e8e5df;
    --backdrop-bottom: #c7cbd0;
    --backdrop-glow-a: rgba(255, 255, 255, 0.56);
    --backdrop-glow-b: rgba(137, 153, 170, 0.2);
    --window-edge: rgba(50, 49, 46, 0.22);
    --titlebar: #efeeec;
    --titlebar-edge: rgba(38, 37, 34, 0.14);
    --title-color: #3e3d3a;
    --terminal-background: #f5f3ef;
  }

  .marketing-ghostty-stage::before {
    content: "";
    position: absolute;
    inset: 12% 8%;
    border-radius: 42%;
    background: linear-gradient(120deg, transparent, var(--backdrop-glow-a));
    filter: blur(72px);
    opacity: 0.72;
    transform: rotate(-5deg);
  }

  .marketing-ghostty-window {
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    position: relative;
    width: 1060px;
    height: 720px;
    overflow: hidden;
    border: 1px solid var(--window-edge);
    border-radius: 15px;
    background: var(--terminal-background);
    box-shadow:
      0 34px 80px rgba(0, 0, 0, 0.35),
      0 9px 24px rgba(0, 0, 0, 0.24),
      0 1px 0 rgba(255, 255, 255, 0.08) inset;
  }

  .marketing-ghostty-stage[data-theme="light"] .marketing-ghostty-window {
    box-shadow:
      0 34px 80px rgba(54, 58, 63, 0.24),
      0 9px 24px rgba(54, 58, 63, 0.16),
      0 1px 0 rgba(255, 255, 255, 0.74) inset;
  }

  .marketing-ghostty-titlebar {
    box-sizing: border-box;
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: center;
    flex: 0 0 54px;
    min-height: 54px;
    padding: 0 18px;
    border-bottom: 1px solid var(--titlebar-edge);
    background: var(--titlebar);
    color: var(--title-color);
    -webkit-app-region: drag;
  }

  .marketing-ghostty-traffic-lights {
    display: flex;
    align-items: center;
    justify-self: start;
    gap: 8px;
  }

  .marketing-ghostty-traffic-light {
    box-sizing: border-box;
    width: 13px;
    height: 13px;
    padding: 0;
    border-radius: 50%;
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.42),
      0 1px 1px rgba(0, 0, 0, 0.2);
    pointer-events: none;
  }

  .marketing-ghostty-traffic-light[data-marketing-ghostty-traffic-light="close"] {
    border: 1px solid #e0443e;
    background: #ff5f57;
  }

  .marketing-ghostty-traffic-light[data-marketing-ghostty-traffic-light="minimize"] {
    border: 1px solid #d89b24;
    background: #febc2e;
  }

  .marketing-ghostty-traffic-light[data-marketing-ghostty-traffic-light="zoom"] {
    border: 1px solid #1aab29;
    background: #28c840;
  }

  .marketing-ghostty-title {
    justify-self: center;
    overflow: hidden;
    font-size: 13px;
    font-weight: 500;
    letter-spacing: -0.01em;
    line-height: 1;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .marketing-ghostty-titlebar-spacer {
    width: 71px;
    justify-self: end;
  }

  .marketing-ghostty-terminal-scene {
    box-sizing: border-box;
    flex: 1 1 auto;
    min-height: 0;
    padding: 22px 24px 20px;
    background: var(--terminal-background);
  }

  .marketing-ghostty-terminal-host,
  .marketing-ghostty-terminal-host .xterm {
    width: 100%;
    height: 100%;
  }

  .marketing-ghostty-terminal-host .xterm-viewport {
    overflow: hidden !important;
    background: transparent !important;
  }

  .marketing-ghostty-terminal-host .xterm-screen {
    margin: 0 auto;
  }
`;

export async function renderMarketingGhosttyHarness() {
  const root = document.querySelector<HTMLElement>("#root");
  if (!root) throw new Error("[ux-audit] Ghostty harness root is missing");

  const theme = marketingGhosttyTheme(window.localStorage);
  document.documentElement.classList.toggle("dark", theme === "dark");
  document.documentElement.classList.remove("theme-loading");
  document.documentElement.dataset.theme = theme;
  document.title = MARKETING_GHOSTTY_WINDOW_TITLE;

  const [{ Terminal }, { default: xtermStyles }] = await Promise.all([
    import("@xterm/xterm"),
    import("@xterm/xterm/css/xterm.css?inline"),
  ]);
  installMarketingGhosttyStyles(xtermStyles);

  const stage = document.createElement("main");
  stage.className = "marketing-ghostty-stage";
  stage.dataset.theme = theme;
  stage.setAttribute("aria-label", "Ghostty on macOS marketing scene");

  const windowFrame = document.createElement("section");
  windowFrame.className = "marketing-ghostty-window";
  windowFrame.dataset.marketingGhosttyWindow = "true";
  windowFrame.setAttribute("aria-label", MARKETING_GHOSTTY_WINDOW_TITLE);

  const titleBar = document.createElement("header");
  titleBar.className = "marketing-ghostty-titlebar";
  titleBar.dataset.marketingGhosttyTitleBar = "true";

  const trafficLights = document.createElement("div");
  trafficLights.className = "marketing-ghostty-traffic-lights";
  trafficLights.setAttribute("aria-label", "macOS window controls");
  trafficLights.append(
    marketingGhosttyTrafficLight("close", "Close"),
    marketingGhosttyTrafficLight("minimize", "Minimize"),
    marketingGhosttyTrafficLight("zoom", "Zoom"),
  );

  const title = document.createElement("span");
  title.className = "marketing-ghostty-title";
  title.dataset.marketingGhosttyTitle = "true";
  title.textContent = MARKETING_GHOSTTY_WINDOW_TITLE;

  const titleBarSpacer = document.createElement("span");
  titleBarSpacer.className = "marketing-ghostty-titlebar-spacer";
  titleBarSpacer.setAttribute("aria-hidden", "true");
  titleBar.append(trafficLights, title, titleBarSpacer);

  const terminalScene = document.createElement("section");
  terminalScene.className = "marketing-ghostty-terminal-scene";
  terminalScene.dataset.terminalSceneComplete = "false";
  terminalScene.dataset.terminalSessionId = MARKETING_GHOSTTY_SESSION_ID;
  terminalScene.setAttribute("aria-label", "Claude Code terminal scene");

  const terminalHost = document.createElement("div");
  terminalHost.className = "marketing-ghostty-terminal-host";
  terminalHost.setAttribute("aria-label", "Claude Code terminal");
  terminalScene.append(terminalHost);
  windowFrame.append(titleBar, terminalScene);
  stage.append(windowFrame);
  root.replaceChildren(stage);

  const terminal = new Terminal({
    allowTransparency: true,
    cols: 108,
    convertEol: false,
    cursorBlink: false,
    cursorStyle: "bar",
    customGlyphs: true,
    disableStdin: true,
    drawBoldTextInBrightColors: false,
    fontFamily:
      '"SF Mono", SFMono-Regular, ui-monospace, Menlo, Monaco, Consolas, monospace',
    fontSize: 15,
    fontWeight: 500,
    fontWeightBold: 700,
    letterSpacing: 0,
    lineHeight: 1.25,
    minimumContrastRatio: theme === "dark" ? 3 : 4.5,
    rows: 31,
    scrollback: 0,
    theme: TERMINAL_THEMES[theme],
  });
  terminal.open(terminalHost);

  await new Promise<void>((resolve, reject) => {
    terminal.write(marketingGhosttyTranscript(theme), () => {
      if (!terminalBufferIncludes(terminal, MARKETING_GHOSTTY_PLAN_MODE_STATUS)) {
        reject(new Error("[ux-audit] Ghostty composer status is missing"));
        return;
      }
      terminalHost.dataset.marketingGhosttyComposerStatus =
        MARKETING_GHOSTTY_PLAN_MODE_STATUS;
      terminalScene.dataset.terminalSceneComplete = "true";
      resolve();
    });
  });
}

function marketingGhosttyTheme(
  storage: Pick<Storage, "getItem">,
): MarketingGhosttyTheme {
  return storage.getItem("halv.theme") === "light" ? "light" : "dark";
}

function installMarketingGhosttyStyles(xtermStyles: string) {
  const existing = document.querySelector<HTMLStyleElement>(
    'style[data-marketing-ghostty-styles="true"]',
  );
  if (existing) return;
  const style = document.createElement("style");
  style.dataset.marketingGhosttyStyles = "true";
  style.textContent = `${xtermStyles}\n${MARKETING_GHOSTTY_STYLES}`;
  document.head.append(style);
}

function marketingGhosttyTrafficLight(
  action: "close" | "minimize" | "zoom",
  label: string,
) {
  const light = document.createElement("button");
  light.className = "marketing-ghostty-traffic-light";
  light.dataset.marketingGhosttyTrafficLight = action;
  light.setAttribute("aria-label", label);
  light.tabIndex = -1;
  light.type = "button";
  return light;
}

function marketingGhosttyTranscript(theme: MarketingGhosttyTheme) {
  const accent = ansiRgb(theme === "dark" ? [215, 171, 119] : [138, 79, 23]);
  const muted = ansiRgb(theme === "dark" ? [145, 148, 151] : [102, 99, 93]);
  const tool = ansiRgb(theme === "dark" ? [121, 184, 182] : [52, 117, 116]);
  const success = ansiRgb(theme === "dark" ? [152, 195, 121] : [57, 118, 66]);
  const status = ansiRgb(theme === "dark" ? [232, 193, 123] : [138, 100, 29]);
  const bold = "\x1b[1m";
  const dim = "\x1b[2m";
  const reset = "\x1b[0m";
  const composerTop = `╭${"─".repeat(COMPOSER_INNER_WIDTH)}╮`;
  const composerLine = `│ ${accent}❯${reset}${" ".repeat(COMPOSER_INNER_WIDTH - 2)}│`;
  const composerBottom = `╰${"─".repeat(COMPOSER_INNER_WIDTH)}╯`;

  return `\x1b[?25l${[
    `${bold}${accent}Claude Code${reset} ${muted}Sonnet 5 · ~/programming/halv${reset}`,
    "",
    `${bold}${accent}❯${reset} Add native Ghostty handoff for agent sessions on macOS.`,
    "",
    `${bold}●${reset} I’ll trace the terminal launch boundary before editing.`,
    "",
    `${tool}● Read${reset}(src-tauri/src/terminal.rs)`,
    `  ${dim}Read 1 file${reset}`,
    `  ${dim}Ran 3 shell commands${reset}`,
    "",
    `${tool}● Background command${reset} "cargo test terminal::ghostty" completed (exit code 0)`,
    `  ${success}↳ 8 passed${reset}`,
    "",
    `${tool}● Update${reset}(src-tauri/src/terminal.rs)`,
    `  ${success}↳ preserved the native window handoff and resume arguments${reset}`,
    `${tool}● Update${reset}(src-tauri/src/commands.rs)`,
    `  ${success}↳ routed Ghostty 1.3+ through the existing session launcher${reset}`,
    "",
    `${accent}✳ Churned 4m 9s · 1 shell still running${reset}`,
    "",
    `${tool}● Background command${reset} "pnpm exec tsc --noEmit" completed (exit code 0)`,
    `  ${success}↳ 0 errors${reset}`,
    "",
    `${bold}●${reset} The native handoff is ready for the macOS smoke check.`,
    `  ${muted}The plan keeps ${accent}TERM=xterm-ghostty${muted} and ${accent}--resume${muted} on the existing launch path.${reset}`,
    "",
    `${muted}${composerTop}${reset}`,
    composerLine,
    `${muted}${composerBottom}${reset}`,
    `${status}${MARKETING_GHOSTTY_PLAN_MODE_STATUS}${reset}`,
  ].join("\r\n")}`;
}

function terminalBufferIncludes(
  terminal: {
    buffer: {
      active: {
        length: number;
        getLine(index: number):
          | { translateToString(trimRight?: boolean): string }
          | undefined;
      };
    };
  },
  expected: string,
) {
  const buffer = terminal.buffer.active;
  for (let index = 0; index < buffer.length; index += 1) {
    if (buffer.getLine(index)?.translateToString(true).includes(expected)) {
      return true;
    }
  }
  return false;
}

function ansiRgb([red, green, blue]: readonly [number, number, number]) {
  return `\x1b[38;2;${red};${green};${blue}m`;
}
