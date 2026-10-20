import { expect, test } from "@playwright/test";

test("settings separates billing and AI connections and preserves provider access", async ({ page }, testInfo) => {
  await page.goto("/ux-audit.html?state=marketing-app-dark");
  await page.getByLabel("Workspace and application menu", { exact: true }).click();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Settings", exact: true });
  await expect(dialog.getByRole("button", { name: /^Account & plan/ })).toHaveAttribute("aria-current", "page");
  await expect(dialog.getByRole("button", { name: "Manage billing", exact: true })).toBeVisible();
  const cancellation = dialog.getByRole("button", { name: "Cancel subscription", exact: true });
  expect(await cancellation.evaluate((button) => Number.parseFloat(getComputedStyle(button).borderTopWidth))).toBe(0);
  await dialog.screenshot({ animations: "disabled", path: testInfo.outputPath("settings-account.png") });
  await dialog.getByRole("button", { name: /^AI connections/ }).click();
  await expect(dialog.getByRole("region", { name: "Codex", exact: true })).toBeVisible();
  await expect(dialog.getByRole("region", { name: "Claude Code", exact: true })).toBeVisible();
  await expect(dialog.getByRole("heading", { name: "Credential profiles", exact: true })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Manage billing", exact: true })).toBeHidden();
  const claudeCard = dialog.getByRole("region", { name: "Claude Code", exact: true });
  await expect(claudeCard.getByRole("button", { name: "Reconnect", exact: true })).toBeEnabled();
  await expect(claudeCard.getByRole("button", { name: "Add account", exact: true })).toBeEnabled();
  await expect(claudeCard.getByRole("radio", { name: /^System default/ })).toBeChecked();
  await dialog.screenshot({ animations: "disabled", path: testInfo.outputPath("settings-connections.png") });
  await dialog.getByRole("button", { name: /^Preferences/ }).click();
  await expect(dialog.getByRole("heading", { name: "Preferences", exact: true, level: 3 })).toBeVisible();
  await page.setViewportSize({ width: 390, height: 740 });
  await dialog.getByRole("button", { name: /^AI connections/ }).click();
  await expect(dialog.getByRole("region", { name: "Claude Code", exact: true })).toBeVisible();
  expect(await dialog.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
  await dialog.screenshot({ animations: "disabled", path: testInfo.outputPath("settings-narrow.png") });
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await expect(dialog).toBeHidden();
});

test("JEV appears above Preferences and saves an agent for each tier", async ({ page }, testInfo) => {
  await page.goto("/ux-audit.html?state=marketing-app-dark");
  await page.getByLabel("Workspace and application menu", { exact: true }).click();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Settings", exact: true });
  const categoryLabels = await dialog
    .locator("nav.settings-navigation button")
    .allTextContents();
  expect(categoryLabels.findIndex((label) => label.includes("JEV"))).toBeLessThan(
    categoryLabels.findIndex((label) => label.includes("Preferences")),
  );

  await dialog.getByRole("button", { name: /^JEV/ }).click();
  await expect(
    dialog.getByRole("heading", { name: "JEV", exact: true, level: 3 }),
  ).toBeVisible();
  const cheapAgent = dialog.getByRole("combobox", { name: "Cheap Agent" });
  await cheapAgent.selectOption("codex:gpt-6.1-sol:xhigh");
  await expect(dialog.getByRole("status")).toContainText(
    "Saved gpt-6.1-sol for Cheap.",
  );
  const saved = await page.evaluate(async () => {
    const { ipc } = await import("/src/ipc.ts");
    return ipc.preferences.get();
  });
  expect(saved.routingTierAgents.cheap).toEqual({
    lane: "codex",
    model: "gpt-6.1-sol",
    effort: "xhigh",
  });
  expect(saved.workerLane).toBe("auto");
  await dialog.screenshot({
    animations: "disabled",
    path: testInfo.outputPath("settings-jev.png"),
  });
});

test("recent memory extraction shows results and surfaces failures", async ({ page }) => {
  await page.goto("/ux-audit.html?state=marketing-app-dark");
  await page.evaluate(async () => {
    const modulePath = "/src/ipc.ts";
    const { ipc } = await import(modulePath);
    let extracted = false;
    let attempts = 0;
    ipc.storage.listAgentMemories = async () => extracted ? [{ id: "memory-test", projectId: null, projectName: null, content: "Use pnpm for this project.", sourceConversationId: null, createdAt: 1788972000, updatedAt: 1788972000 }] : [];
    ipc.storage.rebuildAgentMemories = async () => {
      if (++attempts === 1) throw new Error("Memory extraction connection test error");
      extracted = true;
      return 1;
    };
  });
  await page.getByLabel("Workspace and application menu", { exact: true }).click();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Settings", exact: true });
  await dialog.getByRole("button", { name: /^Agent & privacy/ }).click();
  const extract = dialog.getByRole("button", { name: "Extract recent history", exact: true });
  await expect(extract).toBeEnabled();
  await extract.click();
  await expect(dialog.getByRole("alert")).toContainText("Memory extraction connection test error");
  await extract.click();
  await expect(dialog.getByText("Use pnpm for this project.", { exact: true })).toBeVisible();
  await expect(dialog.getByText("Recent history checked. Only durable facts are saved.", { exact: true })).toBeVisible();
});

test("billing actions open the portal and each AI connection launches the right login", async ({ page }, testInfo) => {
  await page.goto("/ux-audit.html?state=marketing-app-light");
  await page.getByLabel("Workspace and application menu", { exact: true }).click();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.evaluate(async () => {
    const modulePath = "/src/ipc.ts";
    const { ipc } = await import(modulePath);
    const runtime = window as unknown as { __settingsCalls: Array<{ command: string; args?: Record<string, unknown> }> };
    runtime.__settingsCalls = [];
    ipc.billing.portal = async () => { runtime.__settingsCalls.push({ command: "billing_portal" }); };
    ipc.agent.loginLaunch = async (args: Record<string, unknown>) => {
      runtime.__settingsCalls.push({ command: "agent_login_launch", args });
      throw new Error("Login launch test error");
    };
  });
  const dialog = page.getByRole("dialog", { name: "Settings", exact: true });
  await dialog.getByRole("button", { name: "Manage billing", exact: true }).click();
  await dialog.getByRole("button", { name: "Cancel subscription", exact: true }).click();
  await dialog.getByRole("button", { name: /^AI connections/ }).click();
  for (const provider of ["Codex", "Claude Code"]) {
    await dialog.getByRole("region", { name: provider, exact: true }).getByRole("button", { name: "Reconnect", exact: true }).click();
    await expect(dialog.getByRole("alert")).toContainText("Login launch test error");
  }
  const calls = await page.evaluate(() => (window as unknown as { __settingsCalls: Array<{command: string; args?: Record<string, unknown>}> }).__settingsCalls);
  expect(calls.filter((call) => call.command === "billing_portal")).toHaveLength(2);
  expect(calls.filter((call) => call.command === "agent_login_launch").map((call) => call.args?.agentKind)).toEqual(["codex", "claude"]);
  await dialog.getByRole("button", { name: "Refresh", exact: true }).click();
  await expect(dialog.getByRole("alert")).toHaveCount(0);
  await dialog.screenshot({ animations: "disabled", path: testInfo.outputPath("settings-connections-light.png") });
});

test("Google CLI connections show installation guidance and launch the selected login", async ({ page }) => {
  await page.goto("/ux-audit.html?state=marketing-app-dark");
  await page.getByLabel("Workspace and application menu", { exact: true }).click();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Settings", exact: true });
  await dialog.getByRole("button", { name: /^AI connections/ }).click();
  for (const provider of ["Gemini CLI", "Antigravity CLI"]) {
    await dialog.getByRole("region", { name: provider, exact: true }).getByRole("button", { name: "Installation instructions" }).click();
    await expect(dialog.getByRole("alert")).toContainText(`${provider} was not found.`);
  }
  await page.evaluate(async () => {
    const modulePath = "/src/ipc.ts";
    const { ipc } = await import(modulePath);
    const original = ipc.agent.sessionProbe;
    ipc.agent.sessionProbe = async () => ({
      ...await original(),
      gemini: { found: true, binary: "/fixture/gemini", version: "test" },
      antigravity: { found: true, binary: "/fixture/agy", version: "test" },
    });
    const state = window as unknown as { googleLoginCalls: string[] };
    state.googleLoginCalls = [];
    ipc.agent.loginLaunch = async ({ agentKind }: { agentKind: string }) => {
      state.googleLoginCalls.push(agentKind);
      throw new Error("Fixture login reached");
    };
  });
  await dialog.getByRole("button", { name: "Refresh", exact: true }).click();
  for (const provider of ["Gemini CLI", "Antigravity CLI"]) {
    await dialog.getByRole("region", { name: provider, exact: true }).getByRole("button", { name: "Connect", exact: true }).click();
    await expect(dialog.getByRole("alert")).toContainText("Fixture login reached");
  }
  expect(await page.evaluate(() => (window as unknown as { googleLoginCalls: string[] }).googleLoginCalls)).toEqual(["gemini", "antigravity"]);
});

test("the account wallet adds, switches, and removes a Codex account", async ({ page }, testInfo) => {
  await page.goto("/ux-audit.html?state=marketing-app-dark");
  await page.getByLabel("Workspace and application menu", { exact: true }).click();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Settings", exact: true });
  await dialog.getByRole("button", { name: /^AI connections/ }).click();
  const codexCard = dialog.getByRole("region", { name: "Codex", exact: true });

  await codexCard.getByRole("button", { name: "Add account", exact: true }).click();
  const addSection = dialog.getByRole("region", { name: "Add Codex account", exact: true });
  await expect(addSection).toBeVisible();
  await addSection.getByRole("button", { name: "Save account", exact: true }).click();
  await expect(addSection).toBeHidden();

  const added = codexCard.getByRole("radio", { name: /work1@example\.com/ });
  await expect(added).toBeChecked();
  await codexCard.screenshot({ animations: "disabled", path: testInfo.outputPath("settings-wallet.png") });

  await codexCard.getByRole("radio", { name: /^System default/ }).check();
  await expect(codexCard.getByRole("radio", { name: /^System default/ })).toBeChecked();

  await codexCard.getByRole("button", { name: "Remove work1@example.com", exact: true }).click();
  await page.getByRole("dialog", { name: "Remove account", exact: true }).getByRole("button", { name: "Remove", exact: true }).click();
  await expect(added).toBeHidden();
});
