import { expect, test } from "@playwright/test";

// A window focus starts the agent refresh. Before, one focus ran the lane
// options and the CLI probes twice. These commands blocked the app main
// thread, which also delivers the keys. The spec counts the calls that the
// focus handler starts synchronously, so the 5-second status poll cannot add
// a call.
test("a window focus runs each refresh command once", async ({ page }) => {
  await page.goto("/ux-audit.html?state=marketing-app-dark&ghostty=off");
  await expect(
    page.locator('.terminal-pane[data-terminal-scene-complete="true"]').first(),
  ).toBeVisible({ timeout: 30_000 });

  const counts = await page.evaluate(async () => {
    // @ts-expect-error The dev server serves the source module.
    const { ipc } = await import("/src/ipc.ts");
    const watched: Record<string, string[]> = {
      agent: ["sessionLaneOptions", "sessionProbe"],
      auth: ["status", "credentialStatus", "anthropicStatus"],
      chat: ["listModels"],
      sidecar: ["status"],
      storage: ["listCredentialProfiles"],
    };
    const counts: Record<string, number> = {};
    let counting = true;
    for (const [group, methods] of Object.entries(watched)) {
      for (const method of methods) {
        const original = ipc[group][method];
        ipc[group][method] = (...args: unknown[]) => {
          const name = `${group}.${method}`;
          if (counting) counts[name] = (counts[name] ?? 0) + 1;
          return original(...args);
        };
      }
    }
    window.dispatchEvent(new Event("focus"));
    counting = false;
    return counts;
  });

  expect(counts).toEqual({
    "agent.sessionLaneOptions": 1,
    "agent.sessionProbe": 1,
    "auth.anthropicStatus": 1,
    "auth.credentialStatus": 1,
    "auth.status": 1,
    "chat.listModels": 1,
    "sidecar.status": 1,
    "storage.listCredentialProfiles": 1,
  });
});
