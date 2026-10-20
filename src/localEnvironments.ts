import type { ConfirmDialogOptions, DialogDetail } from "./dialogs";
import type { Translate } from "./i18n/runtime";
import type {
  LocalEnvironmentAction,
  LocalEnvironmentConfig,
  LocalEnvironmentDescriptor,
  LocalEnvironmentLifecycle,
} from "./types";

type SelectionStorage = Pick<Storage, "getItem" | "setItem">;

// Older builds saved the first environment of a repository without a user
// choice. The new prefix ignores those values.
export const LOCAL_ENVIRONMENT_SELECTION_STORAGE_PREFIX =
  "halv.localEnvironment.v2.";
// The stored value for an explicit "Default (no setup)" choice.
export const NO_LOCAL_ENVIRONMENT = "none";
// Rust returns this text when a script needs an approval of the current file.
export const LOCAL_ENVIRONMENT_APPROVAL_ERROR_TEXT =
  "local environment needs approval before its scripts run";

const SCRIPT_PLATFORMS = ["darwin", "linux", "win32"] as const;

export function localEnvironmentStorageKey(projectId: number) {
  return `${LOCAL_ENVIRONMENT_SELECTION_STORAGE_PREFIX}${projectId}`;
}

export function readLocalEnvironmentSelection(
  projectId: number,
  storage: SelectionStorage,
) {
  try {
    const value = storage.getItem(localEnvironmentStorageKey(projectId));
    return value && value !== NO_LOCAL_ENVIRONMENT ? value : null;
  } catch {
    return null;
  }
}

export function writeLocalEnvironmentSelection(
  projectId: number,
  configPath: string | null,
  storage: SelectionStorage,
) {
  try {
    storage.setItem(
      localEnvironmentStorageKey(projectId),
      configPath ?? NO_LOCAL_ENVIRONMENT,
    );
  } catch {
    // Persistence is a convenience; worktree creation still uses live state.
  }
}

// Only a stored user choice selects an environment. Halv never picks a default.
export function resolveLocalEnvironmentSelection(
  storedPath: string | null,
  environments: readonly LocalEnvironmentDescriptor[],
) {
  return storedPath &&
    environments.some(
      (environment) =>
        environment.configPath === storedPath &&
        environment.config &&
        !environment.error,
    )
    ? storedPath
    : null;
}

export function isLocalEnvironmentApprovalError(message: string) {
  return message.toLowerCase().includes(LOCAL_ENVIRONMENT_APPROVAL_ERROR_TEXT);
}

// App.tsx supplies the IPC calls and the dialog for an approval.
export type LocalEnvironmentApprovalSteps = {
  list: () => Promise<LocalEnvironmentDescriptor[]>;
  confirm: (options: ConfirmDialogOptions) => Promise<boolean>;
  approve: (configPath: string, contentSha256: string) => Promise<void>;
  t: Translate;
};

// Rust refuses to run an unapproved script. This helper then shows the scripts
// of the current file content, approves the hash of that content, and tries
// the operation one more time. It returns null when the user declines.
export async function runWithLocalEnvironmentApproval<T>(
  configPath: string | null | undefined,
  run: () => Promise<T>,
  steps: LocalEnvironmentApprovalSteps,
): Promise<T | null> {
  const { t } = steps;
  try {
    return await run();
  } catch (error) {
    if (!configPath || !isLocalEnvironmentApprovalError(errorMessage(error))) {
      throw error;
    }
  }
  const environment = (await steps.list()).find(
    (row) => row.configPath === configPath,
  );
  if (!environment?.config || environment.error || !environment.contentSha256) {
    throw new Error(
      environment?.error ?? t("The local environment is no longer available."),
    );
  }
  if (!environment.approved) {
    const approved = await steps.confirm(
      localEnvironmentApprovalDialog(environment.config, configPath, t),
    );
    if (!approved) return null;
    // Rust records the approval only if the file still has this hash.
    await steps.approve(configPath, environment.contentSha256);
  }
  return run();
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

// The approval names the file and shows each command that the file can run.
export function localEnvironmentApprovalDialog(
  config: LocalEnvironmentConfig,
  configPath: string,
  t: Translate,
): ConfirmDialogOptions {
  return {
    title: t("Run scripts from {name}?", { name: visibleText(config.name) }),
    message: t(
      "{file} can run these commands on your computer with your user rights, outside the agent sandbox. Approve them only if you trust this file. Halv asks again when the file changes.",
      { file: visibleText(configPath) },
    ),
    details: localEnvironmentScripts(config, t),
    confirmLabel: t("Approve and run"),
    danger: true,
  };
}

// Lists each script of the file for each platform, so the approval shows all
// commands that the file can run.
function localEnvironmentScripts(
  config: LocalEnvironmentConfig,
  t: Translate,
): DialogDetail[] {
  return [
    ...lifecycleScripts(t("Setup"), config.setup, t),
    ...lifecycleScripts(t("Cleanup"), config.cleanup, t),
    ...config.actions.map((action) => ({
      label: withPlatform(
        t("Action: {name}", { name: visibleText(action.name) }),
        action.platform,
        t,
      ),
      text: action.command,
    })),
  ]
    .filter((script) => script.text.trim())
    .map((script) => ({ ...script, text: visibleText(script.text) }));
}

// Shows hidden characters, such as bidirectional controls, as escapes. The
// user then sees the same characters that the shell gets.
function visibleText(text: string) {
  return text.replace(/[\p{Cc}\p{Cf}]/gu, (character) =>
    character === "\n" || character === "\t"
      ? character
      : `\\u{${(character.codePointAt(0) ?? 0).toString(16).toUpperCase()}}`,
  );
}

function lifecycleScripts(
  label: string,
  lifecycle: LocalEnvironmentLifecycle | null | undefined,
  t: Translate,
) {
  if (!lifecycle) return [];
  return [
    { label, text: lifecycle.script },
    ...SCRIPT_PLATFORMS.map((platform) => ({
      label: withPlatform(label, platform, t),
      text: lifecycle[platform]?.script ?? "",
    })),
  ];
}

function withPlatform(
  label: string,
  platform: LocalEnvironmentAction["platform"],
  t: Translate,
) {
  switch (platform) {
    case "darwin":
      return `${label} · ${t("macOS")}`;
    case "linux":
      return `${label} · ${t("Linux")}`;
    case "win32":
      return `${label} · ${t("Windows")}`;
    default:
      return label;
  }
}
