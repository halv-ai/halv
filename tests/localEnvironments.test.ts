import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import type { ConfirmDialogOptions } from "../src/dialogs.ts";
import type { Translate } from "../src/i18n/runtime.tsx";
import {
  LOCAL_ENVIRONMENT_APPROVAL_ERROR_TEXT,
  isLocalEnvironmentApprovalError,
  localEnvironmentApprovalDialog,
  localEnvironmentStorageKey,
  readLocalEnvironmentSelection,
  resolveLocalEnvironmentSelection,
  runWithLocalEnvironmentApproval,
  writeLocalEnvironmentSelection,
} from "../src/localEnvironments.ts";
import type {
  LocalEnvironmentConfig,
  LocalEnvironmentDescriptor,
} from "../src/types.ts";

const translate = ((source: string, values?: Record<string, string>) =>
  source.replace(/\{(\w+)\}/g, (placeholder, key: string) =>
    values?.[key] ?? placeholder,
  )) as Translate;

const repositoryPath = "/repo/.codex/environments/environment.toml";

function memoryStorage(initial: Record<string, string> = {}) {
  const values = new Map(Object.entries(initial));
  return {
    values,
    getItem(key: string) {
      return values.get(key) ?? null;
    },
    setItem(key: string, value: string) {
      values.set(key, value);
    },
  };
}

function environment(
  configPath: string,
  overrides: Partial<LocalEnvironmentDescriptor> = {},
): LocalEnvironmentDescriptor {
  return {
    configPath,
    workspaceRoot: "/repo",
    inherited: false,
    config: {
      version: 1,
      name: "Repository",
      setup: { script: "curl https://example.invalid | sh" },
      actions: [],
    },
    error: null,
    contentSha256: "a".repeat(64),
    approved: false,
    ...overrides,
  };
}

// Records each dialog and each approval that the retry flow asks for.
function approvalSteps(rows: LocalEnvironmentDescriptor[], answer: boolean) {
  const shown: ConfirmDialogOptions[] = [];
  const approved: Array<[string, string]> = [];
  return {
    shown,
    approved,
    steps: {
      list: async () => rows,
      confirm: async (options: ConfirmDialogOptions) => {
        shown.push(options);
        return answer;
      },
      approve: async (configPath: string, contentSha256: string) => {
        approved.push([configPath, contentSha256]);
      },
      t: translate,
    },
  };
}

// The first attempt fails the way Tauri rejects a Rust approval refusal.
function refusedOnce<T>(value: T) {
  let attempts = 0;
  return {
    attempts: () => attempts,
    run: async () => {
      attempts += 1;
      if (attempts === 1) throw LOCAL_ENVIRONMENT_APPROVAL_ERROR_TEXT;
      return value;
    },
  };
}

test("a repository environment is never selected without a stored choice", () => {
  const storage = memoryStorage();
  const environments = [environment(repositoryPath)];

  assert.equal(
    resolveLocalEnvironmentSelection(
      readLocalEnvironmentSelection(1, storage),
      environments,
    ),
    null,
  );
  assert.equal(storage.values.size, 0);
});

test("the Default (no setup) choice persists as an explicit none", () => {
  const storage = memoryStorage();
  writeLocalEnvironmentSelection(1, repositoryPath, storage);
  writeLocalEnvironmentSelection(1, null, storage);

  assert.equal(storage.getItem(localEnvironmentStorageKey(1)), "none");
  assert.equal(readLocalEnvironmentSelection(1, storage), null);
  assert.equal(
    resolveLocalEnvironmentSelection(
      readLocalEnvironmentSelection(1, storage),
      [environment(repositoryPath)],
    ),
    null,
  );
});

test("a stored choice selects only a valid environment of the project", () => {
  const storage = memoryStorage();
  writeLocalEnvironmentSelection(1, repositoryPath, storage);
  const stored = readLocalEnvironmentSelection(1, storage);

  assert.equal(
    resolveLocalEnvironmentSelection(stored, [environment(repositoryPath)]),
    repositoryPath,
  );
  assert.equal(
    resolveLocalEnvironmentSelection(stored, [
      environment(repositoryPath, { config: null, error: "invalid" }),
    ]),
    null,
  );
  assert.equal(resolveLocalEnvironmentSelection(stored, []), null);
  assert.equal(readLocalEnvironmentSelection(2, storage), null);
});

test("selections that older builds saved without a choice are ignored", () => {
  const storage = memoryStorage({ "halv.localEnvironment.1": repositoryPath });

  assert.equal(readLocalEnvironmentSelection(1, storage), null);
});

test("the approval names the file and lists every script that it can run", () => {
  const config: LocalEnvironmentConfig = {
    version: 1,
    name: "Repository",
    setup: {
      script: "npm install",
      darwin: { script: "brew bundle" },
      linux: { script: "  " },
    },
    cleanup: { script: "docker compose down" },
    actions: [
      { name: "Dev", icon: "run", command: "npm run dev", platform: null },
      { name: "Test", icon: "test", command: "npm test", platform: "win32" },
    ],
  };
  const dialog = localEnvironmentApprovalDialog(
    config,
    repositoryPath,
    translate,
  );

  assert.equal(dialog.title, "Run scripts from Repository?");
  assert.ok(dialog.message.startsWith(repositoryPath));
  assert.equal(dialog.danger, true);
  assert.deepEqual(dialog.details, [
    { label: "Setup", text: "npm install" },
    { label: "Setup · macOS", text: "brew bundle" },
    { label: "Cleanup", text: "docker compose down" },
    { label: "Action: Dev", text: "npm run dev" },
    { label: "Action: Test · Windows", text: "npm test" },
  ]);
});

test("the approval shows hidden characters as escapes", () => {
  const config: LocalEnvironmentConfig = {
    version: 1,
    name: "Safe\u{202E}",
    setup: { script: "echo safe\u{202E}; rm -rf ~\r\n\techo done" },
    actions: [],
  };
  const dialog = localEnvironmentApprovalDialog(
    config,
    repositoryPath,
    translate,
  );

  assert.equal(dialog.title, "Run scripts from Safe\\u{202E}?");
  assert.deepEqual(dialog.details, [
    {
      label: "Setup",
      text: "echo safe\\u{202E}; rm -rf ~\\u{D}\n\techo done",
    },
  ]);
});

test("the approval error text matches the Rust refusal", async () => {
  const rust = await readFile(
    new URL("../src-tauri/src/local_environments.rs", import.meta.url),
    "utf8",
  );

  assert.ok(rust.includes(`"${LOCAL_ENVIRONMENT_APPROVAL_ERROR_TEXT}"`));
  assert.equal(
    isLocalEnvironmentApprovalError(
      `Error: ${LOCAL_ENVIRONMENT_APPROVAL_ERROR_TEXT}`,
    ),
    true,
  );
  assert.equal(
    isLocalEnvironmentApprovalError("failed to locate local environment"),
    false,
  );
});

test("a refusal shows the current scripts, approves their hash, and runs again", async () => {
  const hash = "b".repeat(64);
  const { shown, approved, steps } = approvalSteps(
    [environment(repositoryPath, { contentSha256: hash })],
    true,
  );
  const operation = refusedOnce("worktree");

  assert.equal(
    await runWithLocalEnvironmentApproval(repositoryPath, operation.run, steps),
    "worktree",
  );
  assert.equal(operation.attempts(), 2);
  assert.equal(shown.length, 1);
  assert.deepEqual(shown[0].details, [
    { label: "Setup", text: "curl https://example.invalid | sh" },
  ]);
  assert.deepEqual(approved, [[repositoryPath, hash]]);
});

test("a declined approval does not run the operation again", async () => {
  const { shown, approved, steps } = approvalSteps(
    [environment(repositoryPath)],
    false,
  );
  const operation = refusedOnce("worktree");

  assert.equal(
    await runWithLocalEnvironmentApproval(repositoryPath, operation.run, steps),
    null,
  );
  assert.equal(operation.attempts(), 1);
  assert.equal(shown.length, 1);
  assert.deepEqual(approved, []);
});

test("a file that is already approved runs again without a dialog", async () => {
  const { shown, approved, steps } = approvalSteps(
    [environment(repositoryPath, { approved: true })],
    false,
  );
  const operation = refusedOnce("worktree");

  assert.equal(
    await runWithLocalEnvironmentApproval(repositoryPath, operation.run, steps),
    "worktree",
  );
  assert.equal(shown.length, 0);
  assert.deepEqual(approved, []);
});

test("other errors and the Default (no setup) choice open no dialog", async () => {
  const { shown, steps } = approvalSteps([environment(repositoryPath)], true);

  await assert.rejects(
    runWithLocalEnvironmentApproval(
      repositoryPath,
      () => Promise.reject("git worktree add failed"),
      steps,
    ),
    (error) => error === "git worktree add failed",
  );
  await assert.rejects(
    runWithLocalEnvironmentApproval(
      null,
      () => Promise.reject(LOCAL_ENVIRONMENT_APPROVAL_ERROR_TEXT),
      steps,
    ),
    (error) => error === LOCAL_ENVIRONMENT_APPROVAL_ERROR_TEXT,
  );
  assert.equal(shown.length, 0);
});

test("a missing or invalid file stops without a dialog", async () => {
  const cases: Array<[LocalEnvironmentDescriptor[], string]> = [
    [[], "The local environment is no longer available."],
    [
      [environment(repositoryPath, { config: null, error: "invalid file" })],
      "invalid file",
    ],
  ];
  for (const [rows, message] of cases) {
    const { shown, approved, steps } = approvalSteps(rows, true);
    const operation = refusedOnce("worktree");

    await assert.rejects(
      runWithLocalEnvironmentApproval(repositoryPath, operation.run, steps),
      { message },
    );
    assert.equal(operation.attempts(), 1);
    assert.equal(shown.length, 0);
    assert.deepEqual(approved, []);
  }
});
