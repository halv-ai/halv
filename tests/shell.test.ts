import assert from "node:assert/strict";
import test from "node:test";

import {
  exitShellModeDraft,
  pinnedProjectIdForQueuedInput,
  requestProjectIdForInput,
  resolveInputRoute,
  shellCommandFromInput,
  shellModePresentation,
} from "../src/shell.ts";

test("recognizes direct shell mode only when bang is the first non-space character", () => {
  assert.equal(shellCommandFromInput("!git status --short"), "git status --short");
  assert.equal(shellCommandFromInput("  ! pwd  "), "pwd");
  assert.equal(shellCommandFromInput("\n\t!!important"), "!important");
  assert.equal(shellCommandFromInput("explain !important"), null);
  assert.equal(shellCommandFromInput("\\!not-shell"), null);
});

test("keeps shell mode active while the command is still empty", () => {
  assert.equal(shellCommandFromInput("!"), "");
  assert.equal(shellCommandFromInput("!   "), "");
});

test("routes chat and valid shell inputs through distinct App paths", () => {
  assert.deepEqual(resolveInputRoute("explain this", null), { kind: "chat" });
  assert.deepEqual(resolveInputRoute(" ! git status ", 17), {
    kind: "shell",
    command: "git status",
    projectId: 17,
  });
});

test("blocks empty shell commands and shell commands without projects", () => {
  assert.deepEqual(resolveInputRoute("!   ", 17), {
    kind: "blocked_shell",
    command: "",
    reason: "empty_command",
  });
  assert.deepEqual(resolveInputRoute("!pwd", null), {
    kind: "blocked_shell",
    command: "pwd",
    reason: "project_required",
  });
});

test("pins queued shell commands to their enqueue-time project", () => {
  const pinnedProjectId = pinnedProjectIdForQueuedInput("!pwd", 17);
  const replayProjectId = requestProjectIdForInput(false, 99, pinnedProjectId);

  assert.equal(pinnedProjectId, 17);
  assert.equal(replayProjectId, 17);
  assert.deepEqual(resolveInputRoute("!pwd", replayProjectId), {
    kind: "shell",
    command: "pwd",
    projectId: 17,
  });
  assert.equal(pinnedProjectIdForQueuedInput("hello", 17), undefined);
  assert.equal(pinnedProjectIdForQueuedInput("!pwd", null), null);
  assert.equal(requestProjectIdForInput(true, 23, pinnedProjectId), 23);
});

test("Escape exits shell mode without changing ordinary drafts", () => {
  assert.equal(exitShellModeDraft("  !  git status  "), "git status  ");
  assert.equal(exitShellModeDraft("!"), "");
  assert.equal(exitShellModeDraft("ordinary draft"), "ordinary draft");
});

test("shell presentation distinguishes runnable and project-required modes", () => {
  assert.deepEqual(shellModePresentation("halv"), {
    status: "direct · no model",
    placeholder: "Run in halv — model bypassed",
  });
  assert.deepEqual(shellModePresentation(null), {
    status: "select a project to run",
    placeholder: "Select a project before running a command",
  });
});
