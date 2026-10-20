import assert from "node:assert/strict";
import test from "node:test";
import {
  composerModeAccessLabel,
  composerModePresentation,
  composerPlaceholder,
  composerPrimaryAction,
} from "../src/composerPresentation.ts";

const baseAction = {
  attachmentCount: 0,
  cancelAndRevertPending: false,
  hasText: true,
  inputRoute: "chat" as const,
  proxyDown: false,
  sendPending: false,
  slashCommand: false,
  steerAvailable: false,
};

test("composer modes distinguish workflow from tool access", () => {
  assert.deepEqual(composerModePresentation("plan", "full_access"), {
    label: "Plan · read-only",
    description: "Explore and propose a plan without changing files.",
  });
  assert.equal(
    composerModePresentation("default", "ask").label,
    "Agent · Ask first",
  );
  assert.equal(
    composerModePresentation("default", "chat").label,
    "Chat · no tools",
  );
  assert.deepEqual(composerModePresentation("default", "sandbox"), {
    label: "Agent · Auto",
    description:
      "Work sandboxed in the workspace without approval prompts. Ask only when more access is needed.",
  });
});

test("composer mode access labels exclude the mode segment", () => {
  assert.equal(composerModeAccessLabel("chat"), null);
  assert.equal(composerModeAccessLabel("sandbox"), "Auto");
  assert.equal(composerModeAccessLabel("ask"), "Ask first");
  assert.equal(composerModeAccessLabel("legacy_restricted"), "Restricted (legacy)");
});

test("composer placeholder explains the current project and safety contract", () => {
  assert.match(composerPlaceholder("plan", "ask", "Halv"), /no changes/i);
  assert.match(composerPlaceholder("default", "chat", "Halv"), /switch to Agent/i);
  assert.match(composerPlaceholder("default", "sandbox", null), /Choose a project/i);
});

test("primary action makes send, steer, queue, run, and stop explicit", () => {
  assert.equal(composerPrimaryAction(baseAction).kind, "send");
  assert.equal(
    composerPrimaryAction({ ...baseAction, sendPending: true }).kind,
    "queue",
  );
  assert.equal(
    composerPrimaryAction({
      ...baseAction,
      sendPending: true,
      steerAvailable: true,
    }).kind,
    "steer",
  );
  assert.equal(
    composerPrimaryAction({
      ...baseAction,
      attachmentCount: 1,
      sendPending: true,
      steerAvailable: true,
    }).kind,
    "queue",
  );
  assert.equal(
    composerPrimaryAction({
      ...baseAction,
      inputRoute: "shell",
      sendPending: true,
    }).kind,
    "queue",
  );
  assert.equal(
    composerPrimaryAction({ ...baseAction, hasText: false, sendPending: true }).kind,
    "stop",
  );
  assert.equal(
    composerPrimaryAction({ ...baseAction, inputRoute: "shell" }).label,
    "Run command",
  );
  assert.equal(
    composerPrimaryAction({
      ...baseAction,
      inputRoute: "blocked-empty-shell",
    }).label,
    "Run command",
  );
  assert.equal(
    composerPrimaryAction({ ...baseAction, slashCommand: true }).ariaLabel,
    "Run slash command",
  );
});

test("primary action exposes actionable disabled reasons", () => {
  assert.match(
    composerPrimaryAction({
      ...baseAction,
      inputRoute: "blocked-project-shell",
    }).disabledReason ?? "",
    /Choose a project/i,
  );
  assert.match(
    composerPrimaryAction({
      ...baseAction,
      attachmentCount: 1,
      slashCommand: true,
    }).disabledReason ?? "",
    /Remove image attachments/i,
  );
  assert.match(
    composerPrimaryAction({ ...baseAction, proxyDown: true }).disabledReason ?? "",
    /restart-proxy/i,
  );
});
