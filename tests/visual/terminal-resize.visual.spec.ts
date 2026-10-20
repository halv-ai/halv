import { expect, test, type Page } from "@playwright/test";

// Each terminal_resize that reaches the PTY sends SIGWINCH, and the agent
// repaints its full screen. A repaint that is lost or late leaves a stale
// prompt box. These specs count the resize calls that the panes send.

type Resize = { sessionId: string; cols: number; rows: number };

const SURFACE_PANE =
  '.conversation-mode-surface .terminal-pane[data-terminal-scene-complete="true"]';

async function openSurface(page: Page) {
  await page.goto("/ux-audit.html?state=marketing-app-dark&ghostty=off");
  await expect(page.locator(SURFACE_PANE)).toHaveCount(3, { timeout: 30_000 });
  // Let the first fits and the font load finish.
  await page.waitForTimeout(1000);
  await page.evaluate(async () => {
    // @ts-expect-error The dev server serves the source module.
    const { ipc } = await import("/src/ipc.ts");
    const target = window as typeof window & { __terminalResizes?: Resize[] };
    target.__terminalResizes = [];
    const original = ipc.terminal.resize;
    ipc.terminal.resize = (args: Resize) => {
      target.__terminalResizes?.push({
        sessionId: args.sessionId,
        cols: args.cols,
        rows: args.rows,
      });
      return original(args);
    };
  });
}

async function takeResizes(page: Page) {
  return page.evaluate(() => {
    const target = window as typeof window & { __terminalResizes?: Resize[] };
    const resizes = target.__terminalResizes ?? [];
    target.__terminalResizes = [];
    return resizes;
  });
}

function lastSizes(resizes: Resize[]) {
  const sizes: Record<string, string> = {};
  for (const { sessionId, cols, rows } of resizes) {
    sizes[sessionId] = `${cols}x${rows}`;
  }
  return sizes;
}

test("a conversation switch does not resize the hidden panes", async ({ page }) => {
  await openSurface(page);

  await page.getByText("Audit agent refactor", { exact: true }).first().click();
  await expect(
    page.locator('.conversation-agent-mode-surface[data-active="true"] .terminal-pane'),
  ).toHaveCount(0);
  // The hidden surface loses its task board and diff rail, so its panes
  // get a new layout. Before, each pane sent that layout to its PTY.
  await page.waitForTimeout(1000);
  expect(await takeResizes(page)).toEqual([]);

  await page
    .getByText("Authentication middleware refactor", { exact: true })
    .first()
    .click();
  await expect(
    page.locator('.conversation-agent-mode-surface[data-active="true"] .terminal-pane'),
  ).toHaveCount(3);
  await page.waitForTimeout(1000);
  // The surface is back in its earlier layout, so the PTY size is correct.
  expect(await takeResizes(page)).toEqual([]);
});

test("the Agent sessions overlay gives the PTY size back when it closes", async ({ page }) => {
  await openSurface(page);
  await page.setViewportSize({ width: 1360, height: 860 });
  await expect.poll(async () => {
    const sizes = await page.evaluate(() => {
      const target = window as typeof window & { __terminalResizes?: Resize[] };
      return target.__terminalResizes ?? [];
    });
    return Object.keys(lastSizes(sizes)).length;
  }).toBe(3);
  await page.waitForTimeout(500);
  const surfaceSizes = lastSizes(await takeResizes(page));

  await page.locator('summary[aria-label="Workspace and application menu"]').click();
  await page.getByRole("button", { name: "Agent sessions", exact: true }).click();
  await expect(
    page.locator(
      '.agent-sessions-overlay .terminal-pane[data-terminal-scene-complete="true"]',
    ),
  ).toHaveCount(3);
  await page.waitForTimeout(1000);
  const overlaySizes = lastSizes(await takeResizes(page));
  expect(Object.keys(overlaySizes)).toHaveLength(3);
  for (const [sessionId, size] of Object.entries(overlaySizes)) {
    expect(size).not.toBe(surfaceSizes[sessionId]);
  }

  await page.locator(".agent-sessions-close").click();
  await expect(page.locator(".agent-sessions-overlay")).toHaveCount(0);
  await page.waitForTimeout(1000);
  const restored = await takeResizes(page);
  // One resize for each session puts back the size of the surface pane.
  expect(restored).toHaveLength(3);
  expect(lastSizes(restored)).toEqual(surfaceSizes);
});

test("a pane inside an xterm column band resizes at most once", async ({ page }) => {
  await openSurface(page);
  const sessionId = await page
    .locator(SURFACE_PANE)
    .first()
    .getAttribute("data-terminal-session-id");

  const band = await page.evaluate((id) => {
    const pane = document.querySelector<HTMLElement>(
      `.terminal-pane[data-terminal-session-id="${id}"]`,
    )!;
    const child = pane.closest<HTMLElement>(".terminal-split-child")!;
    const terminal = pane.querySelector<HTMLElement>(".xterm")!;
    const screen = pane.querySelector<HTMLElement>(".xterm-screen")!;
    const viewport = pane.querySelector<HTMLElement>(".xterm-viewport");
    // Measure the cell as xterm 6 does: canvas text metrics of "W".
    const font = getComputedStyle(pane.querySelector(".xterm-rows")!);
    const context = new OffscreenCanvas(100, 100).getContext("2d")!;
    context.font = `${font.fontSize} ${font.fontFamily}`;
    const charWidth = context.measureText("W").width;
    const ratio = window.devicePixelRatio;
    // DomRenderer: screen width = Math.round(deviceCell * cols / ratio).
    const screenWidth = (cols: number) =>
      Math.round((charWidth * ratio * cols) / ratio);
    const availableWidth = () => {
      const style = getComputedStyle(terminal);
      const px = (name: string) =>
        Number.parseFloat(style.getPropertyValue(name)) || 0;
      return (
        terminal.getBoundingClientRect().width -
        px("padding-left") -
        px("padding-right") -
        px("border-left-width") -
        px("border-right-width") -
        (viewport ? Math.max(0, viewport.offsetWidth - viewport.clientWidth) : 0)
      );
    };
    const cols = Math.round(Number.parseFloat(screen.style.width) / charWidth);
    const measured = screenWidth(cols) === Number.parseFloat(screen.style.width);

    // Before, a fit at grid k chose k + 1 inside [lower, upper), and a fit
    // at k + 1 chose k again. Bands are 0.15-1 px wide, by font and ratio.
    let found: { cols: number; lower: number; upper: number } | null = null;
    for (let k = 12; k <= cols + 30; k += 1) {
      const lower = ((k + 1) * screenWidth(k)) / k;
      const upper = screenWidth(k + 1);
      const wider = !found || upper - lower > found.upper - found.lower;
      if (upper - lower >= 0.15 && wider) found = { cols: k, lower, upper };
    }
    if (!found) return { measured, found, charWidth, cols };

    const target = (found.lower + found.upper) / 2;
    const width = child.getBoundingClientRect().width + target - availableWidth();
    const recorder = window as typeof window & { __terminalResizes?: Resize[] };
    recorder.__terminalResizes = [];
    child.style.flex = `0 0 ${width}px`;
    return {
      measured,
      found,
      available: availableWidth(),
      screenWidth: screenWidth(found.cols),
    };
  }, sessionId);

  // The spec measures the cell itself. It must match the rendered screen.
  expect(band.measured).toBe(true);
  expect(band.found, JSON.stringify(band)).not.toBeNull();
  expect(band.available).toBeGreaterThanOrEqual(band.found!.lower);
  expect(band.available).toBeLessThan(band.found!.upper);

  await page.waitForTimeout(1500);
  const resizes = (await takeResizes(page)).filter(
    (resize) => resize.sessionId === sessionId,
  );
  // Before, the pane flipped between both grids and sent 36-59 resizes in
  // this wait.
  expect(resizes.length).toBeLessThanOrEqual(1);
  const screenWidth = await page
    .locator(`.terminal-pane[data-terminal-session-id="${sessionId}"] .xterm-screen`)
    .evaluate((screen) => Number.parseFloat((screen as HTMLElement).style.width));
  expect(screenWidth).toBe(band.screenWidth);
  expect(screenWidth).toBeLessThanOrEqual(band.available!);
});

test("a pane that mounts hidden replays at the PTY size and resizes once on show", async ({ page }) => {
  // Halv starts on another conversation, as after a restart with agent
  // sessions in more than one conversation. The surface panes mount hidden.
  await page.addInitScript(() => {
    const getItem = Storage.prototype.getItem;
    Storage.prototype.getItem = function (key: string) {
      if (key === "halv.workspace.last-active-conversation.v1") return "audit-agent-refactor";
      return getItem.call(this, key);
    };
  });
  await page.goto("/ux-audit.html?state=marketing-app-dark&ghostty=off");
  const hiddenPanes = page.locator(
    '.conversation-mode-surface[data-active="false"] .terminal-pane[data-terminal-scene-complete="true"]',
  );
  await expect(hiddenPanes).toHaveCount(3, { timeout: 30_000 });
  await page.waitForTimeout(500);
  const grids = await hiddenPanes.evaluateAll((panes) => {
    const sizes: Record<string, string> = {};
    for (const pane of panes) {
      const screen = pane.querySelector<HTMLElement>(".xterm-screen")!;
      const font = getComputedStyle(pane.querySelector(".xterm-rows")!);
      const context = new OffscreenCanvas(100, 100).getContext("2d")!;
      context.font = `${font.fontSize} ${font.fontFamily}`;
      const cols = Math.round(
        Number.parseFloat(screen.style.width) / context.measureText("W").width,
      );
      const rows = pane.querySelectorAll(".xterm-rows > div").length;
      sizes[pane.getAttribute("data-terminal-session-id")!] = `${cols}x${rows}`;
    }
    return sizes;
  });
  const ptySizes = await page.evaluate(async () => {
    // @ts-expect-error The dev server serves the source module.
    const { ipc } = await import("/src/ipc.ts");
    const sessions: Resize[] = (await ipc.terminal.list()).map(
      (session: { id: string; cols: number; rows: number }) => ({
        sessionId: session.id,
        cols: session.cols,
        rows: session.rows,
      }),
    );
    const target = window as typeof window & { __terminalResizes?: Resize[] };
    target.__terminalResizes = [];
    const original = ipc.terminal.resize;
    ipc.terminal.resize = (args: Resize) => {
      target.__terminalResizes?.push({
        sessionId: args.sessionId,
        cols: args.cols,
        rows: args.rows,
      });
      return original(args);
    };
    return sessions;
  });
  const ptyGrids = lastSizes(ptySizes);
  // Before, the hidden panes kept the xterm default of 80x24.
  for (const [sessionId, grid] of Object.entries(grids)) {
    expect(grid).toBe(ptyGrids[sessionId]);
  }

  await page
    .getByText("Authentication middleware refactor", { exact: true })
    .first()
    .click();
  await expect(
    page.locator('.conversation-agent-mode-surface[data-active="true"] .terminal-pane'),
  ).toHaveCount(3);
  await page.waitForTimeout(1000);
  const resizes = await takeResizes(page);
  // Each pane sends its new grid once. The PTY size changes, so the agent
  // repaints.
  expect(resizes).toHaveLength(3);
  for (const { sessionId, cols, rows } of resizes) {
    expect(`${cols}x${rows}`).not.toBe(ptyGrids[sessionId]);
  }
});

test("closing the overlay does not resize a session that ended", async ({ page }) => {
  await openSurface(page);
  await page.locator('summary[aria-label="Workspace and application menu"]').click();
  await page.getByRole("button", { name: "Agent sessions", exact: true }).click();
  await expect(
    page.locator(
      '.agent-sessions-overlay .terminal-pane[data-terminal-scene-complete="true"]',
    ),
  ).toHaveCount(3);
  await page.waitForTimeout(1000);
  await page.locator(".agent-session-list-row-selected .agent-session-list-kill").click();
  const endedPanes = page.locator(".terminal-pane:has(.terminal-stage-exited)");
  await expect(endedPanes).toHaveCount(2);
  const ended = await endedPanes.first().getAttribute("data-terminal-session-id");
  await takeResizes(page);

  await page.locator(".agent-sessions-close").click();
  await expect(page.locator(".agent-sessions-overlay")).toHaveCount(0);
  await page.waitForTimeout(1000);
  const resizes = await takeResizes(page);
  // The live sessions get the size of the surface panes back. The PTY of the
  // ended session is gone, so a resize for it fails.
  expect(resizes).toHaveLength(2);
  expect(resizes.map((resize) => resize.sessionId)).not.toContain(ended);
});
