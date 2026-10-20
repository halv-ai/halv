import { expect, test, type Page } from "@playwright/test";

async function prepareTask(page: Page, goal = "Understand this project") {
  await page.getByRole("radio", { name: new RegExp(goal) }).check();
  await page
    .getByRole("button", { name: "Choose a project folder", exact: true })
    .click();
  await expect(
    page.getByRole("textbox", { name: "Your first task", exact: true }),
  ).toBeVisible();
}

test("a new user can connect, choose a task, edit it, and reach a completed reply", async ({
  page,
}, testInfo) => {
  await page.goto("/ux-audit.html?state=welcome");
  await expect(
    page.getByText("More getting it done.", { exact: true }),
  ).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("01-welcome.png") });
  await page
    .getByRole("button", { name: "Let's try it on my project" })
    .click();
  await expect(
    page.getByRole("heading", { name: "Bring the account you already use." }),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Add GLM key" })).toHaveCount(
    0,
  );
  await page.screenshot({ path: testInfo.outputPath("02-connect.png") });
  await page
    .getByRole("button", { name: "Sign in with ChatGPT", exact: true })
    .click();
  await page.getByRole("button", { name: "Choose my first task" }).click();
  await expect(
    page.getByRole("heading", { name: "Make this useful for you." }),
  ).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("03-choose-task.png") });
  await prepareTask(page, "Find a quick win");
  const prompt = page.getByRole("textbox", {
    name: "Your first task",
    exact: true,
  });
  await expect(prompt).toHaveValue(/one small, useful improvement/);
  await prompt.fill(
    "Review the README and suggest one small improvement. Do not edit files.",
  );
  await page.getByText(/Run in .* · change/).click();
  await page.getByRole("button", { name: "Chat mode" }).click();
  await page.getByText(/Run in .* · change/).click();
  await page.screenshot({ path: testInfo.outputPath("04-ready.png") });
  await page.evaluate(async () => {
    const { ipc } = await import("/src/ipc.ts");
    const original = ipc.chat.send;
    ipc.chat.send = async (args) => {
      (window as any).__firstTaskRequest = args;
      (window as any).__taskSendCount = ((window as any).__taskSendCount ?? 0) + 1;
      return original(args);
    };
  });
  await page.getByRole("button", { name: "Run my first task" }).click();
  await expect(
    page.getByText("Your first result. Your next move.", { exact: true }),
  ).toBeVisible({ timeout: 30_000 });
  await expect
    .poll(() =>
      page.evaluate(() => localStorage.getItem("halv.guidedFirstTask")),
    )
    .toBe("completed");
  const request = await page.evaluate(() => (window as any).__firstTaskRequest);
  expect(request.agentMode).toBe(true);
  expect(request.approvalPolicy).toBe("read_only");
  expect(request.projectId).toBeGreaterThan(0);
  expect(request.model).toMatch(/^gpt-/);
  await page.screenshot({ path: testInfo.outputPath("05-first-result.png") });
  await page.getByRole("button", { name: "Plan my next step" }).click();
  await expect(
    page.getByRole("textbox", { name: "Message", exact: true }),
  ).toHaveValue(/small implementation plan/);
  await expect(
    page.getByText("Your first result. Your next move.", { exact: true }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "Send message", exact: true }).click();
  const trial = page.getByRole("dialog").filter({ has: page.getByRole("button", { name: "Sign in to start my trial" }) });
  await expect(trial).toBeVisible();
  expect(await page.evaluate(() => (window as any).__taskSendCount)).toBe(1);
  await trial.getByRole("button", { name: "Close", exact: true }).click();
  await expect(page.getByRole("textbox", { name: "Message", exact: true })).toHaveValue(/small implementation plan/);
  await page.getByRole("button", { name: "Send message", exact: true }).click();
  await expect(trial).toBeVisible();
  expect(await page.evaluate(() => (window as any).__taskSendCount)).toBe(1);
});

test("changing folders preserves the edited task and empty tasks cannot run", async ({
  page,
}) => {
  await page.goto("/ux-audit.html?state=first-run-fresh");
  await prepareTask(page, "Find a test gap");
  const prompt = page.getByRole("textbox", {
    name: "Your first task",
    exact: true,
  });
  await expect(prompt).toHaveValue(/test configuration/);
  await prompt.fill("My custom read-only question");
  await page
    .getByRole("button", { name: "Change folder", exact: true })
    .click();
  await expect(prompt).toHaveValue("My custom read-only question");
  await prompt.fill("   ");
  await expect(
    page.getByRole("button", { name: "Run my first task" }),
  ).toBeDisabled();
});

test("connection failure keeps a retry and alternative providers accessible", async ({
  page,
}) => {
  await page.goto("/ux-audit.html?state=connect-gate-failed");
  await expect(page.getByRole("alert")).toContainText(
    "ChatGPT connection was not completed",
  );
  await expect(
    page.getByRole("button", { name: "Sign in with ChatGPT", exact: true }),
  ).toBeEnabled();
  await page.getByRole("button", { name: "Kimi, GLM, or Grok" }).click();
  await expect(page.getByRole("button", { name: "Add GLM key" })).toBeVisible();
});

test("the first task fits a compact desktop window", async ({
  page,
}, testInfo) => {
  await page.setViewportSize({ width: 900, height: 650 });
  await page.goto("/ux-audit.html?state=first-run-codex");
  await expect(
    page.getByRole("button", { name: "Choose a project folder", exact: true }),
  ).toBeInViewport();
  await page.screenshot({ path: testInfo.outputPath("06-compact.png") });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
});

test("a failed first task can retry without a false completion", async ({
  page,
}) => {
  await page.goto("/ux-audit.html?state=first-run-fresh");
  await prepareTask(page);
  await page.getByText(/Run in .* · change/).click();
  await page.getByRole("button", { name: "Chat mode" }).click();
  await page.evaluate(async () => {
    const { ipc } = await import("/src/ipc.ts");
    const original = ipc.chat.send;
    ipc.chat.send = async (...args) => {
      ipc.chat.send = original;
      throw new Error("Connection interrupted. Please try again.");
    };
  });
  await page.getByRole("button", { name: "Run my first task" }).click();
  const guide = page.getByRole("region", { name: "First task progress" });
  await expect(guide).toContainText("Let's get this task working.");
  expect(
    await page.evaluate(() => localStorage.getItem("halv.guidedFirstTask")),
  ).not.toBe("completed");
  await guide.getByRole("button", { name: "Try again", exact: true }).click();
  await expect(guide).toContainText("Your first result. Your next move.", {
    timeout: 30_000,
  });
  await expect
    .poll(() =>
      page.evaluate(() => localStorage.getItem("halv.guidedFirstTask")),
    )
    .toBe("completed");
});

test("Pro prepares the next task in the same terminal and repeated Enter cannot bypass trial", async ({ page }, testInfo) => {
  await page.goto("/ux-audit.html?state=first-run-codex&ghostty=off");
  await expect(page.getByRole("radio", { name: /Find a quick win/ })).toBeChecked();
  await prepareTask(page, "Find a quick win");
  await page.getByText(/Run in .* · change/).click();
  await page.getByRole("button", { name: /Pro mode/ }).click();
  await page.getByRole("button", { name: "Run my first task" }).click();
  const pane = page.locator('.terminal-pane:visible').first();
  await expect(pane).toBeVisible();
  const sessionId = (await pane.getAttribute("data-terminal-session-id"))!;
  await page.evaluate(async (sessionId) => {
    const { emitHarnessEvent } = await import("/src/harness/mock-ipc.ts");
    await emitHarnessEvent("agent-session-event", { sessionId,
      event: { type: "embeddedTurnStarted", turnId: "guided-first" } });
    await emitHarnessEvent("agent-session-event", { sessionId,
      event: { type: "embeddedTurnFinished", turnId: "guided-first", outcome: "completed" } });
    (window as any).__harnessTerminalWrites = [];
  }, sessionId);
  const guide = page.getByRole("region", { name: "First task progress" });
  await expect(guide).toContainText("Your first result. Your next move.");
  await guide.getByRole("button", { name: "Prepare next task", exact: true }).click();
  await guide.getByRole("textbox", { name: "Next task", exact: true }).fill("Review the proposed fix\nwithout changing files.");
  await page.screenshot({ path: testInfo.outputPath("onboarding-next-task.png"), fullPage: true });
  await guide.getByRole("button", { name: "Prepare in terminal", exact: true }).click();
  await expect(guide).toHaveCount(0);
  const writes = await page.evaluate(() => (window as any).__harnessTerminalWrites);
  expect(writes).toHaveLength(1);
  expect(writes[0].sessionId).toBe(sessionId);
  expect(writes[0].data).toContain("Review the proposed fix without changing files.");
  expect(writes[0].data).not.toMatch(/[\r\n]/);
  const trial = page.getByRole("dialog").filter({ has: page.getByRole("button", { name: "Sign in to start my trial" }) });
  for (let attempt = 0; attempt < 2; attempt++) {
    await pane.locator(".xterm-helper-textarea").focus();
    await page.keyboard.press("Enter");
    await expect(trial).toBeVisible();
    expect(await page.evaluate(() => (window as any).__harnessTerminalWrites)).toEqual(writes);
    await trial.getByRole("button", { name: "Close", exact: true }).click();
  }
});

test("an aborted Pro task never counts as onboarding completion", async ({ page }) => {
  await page.goto("/ux-audit.html?state=first-run-codex&ghostty=off");
  await prepareTask(page);
  await page.getByText(/Run in .* · change/).click();
  await page.getByRole("button", { name: /Pro mode/ }).click();
  await page.getByRole("button", { name: "Run my first task" }).click();
  const pane = page.locator('.terminal-pane:visible').first();
  await expect(pane).toBeVisible();
  const sessionId = (await pane.getAttribute("data-terminal-session-id"))!;
  await page.evaluate(async (sessionId) => {
    const { emitHarnessEvent } = await import("/src/harness/mock-ipc.ts");
    await emitHarnessEvent("agent-session-event", { sessionId,
      event: { type: "embeddedTurnStarted", turnId: "aborted-first" } });
    await emitHarnessEvent("agent-session-event", { sessionId,
      event: { type: "embeddedTurnFinished", turnId: "aborted-first", outcome: "aborted" } });
  }, sessionId);
  await expect(page.getByRole("region", { name: "First task progress" })).toContainText("Let's get this task working.");
  expect(await page.evaluate(() => localStorage.getItem("halv.guidedFirstTask"))).not.toBe("completed");
  await page.getByRole("region", { name: "First task progress" })
    .getByRole("button", { name: "Try again", exact: true }).click();
  await expect(page.getByText("Your first result. Your next move.", { exact: true })).toHaveCount(0);
  expect(await page.evaluate(() => localStorage.getItem("halv.guidedFirstTask"))).not.toBe("completed");
  await page.evaluate(async (sessionId) => {
    const { emitHarnessEvent } = await import("/src/harness/mock-ipc.ts");
    await emitHarnessEvent("agent-session-event", { sessionId,
      event: { type: "embeddedTurnStarted", turnId: "retry-first" } });
    await emitHarnessEvent("agent-session-event", { sessionId,
      event: { type: "embeddedTurnFinished", turnId: "retry-first", outcome: "completed" } });
  }, sessionId);
  await expect(page.getByRole("region", { name: "First task progress" }))
    .toContainText("Your first result. Your next move.");
});
