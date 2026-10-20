// Crux indexers run the build tools of a project. Rust starts indexing for a
// project only after the user approves it here.
import { confirmDialog } from "./dialogs.ts";
import type { Translate } from "./i18n/runtime.tsx";
import { ipc, type RequiredProjectWorkspaceArgs } from "./ipc.ts";
import type { CruxWorkspaceOpenResult } from "./types.ts";

const promptedProjectIds = new Set<number>();

export async function requestCruxIndexApproval(
  t: Translate,
  target: RequiredProjectWorkspaceArgs,
): Promise<boolean> {
  const approved = await confirmDialog({
    title: t("Allow Crux to index this project?"),
    message: t(
      "Indexing runs the build tools of this project, such as Cargo, npm, or Gradle. They can run code from the project outside the sandbox. Allow it only for projects you trust.",
    ),
    confirmLabel: t("Allow indexing"),
    cancelLabel: t("Not now"),
  });
  if (approved) {
    await ipc.crux.approveIndexing(target);
  }
  return approved;
}

// The prompt opens one time for each project in an app session. After
// "Not now", the Extensions panel still shows the approval button.
export async function offerCruxIndexApproval(
  t: Translate,
  target: RequiredProjectWorkspaceArgs,
  result: CruxWorkspaceOpenResult | null | undefined,
): Promise<boolean> {
  if (!result?.approvalRequired || promptedProjectIds.has(target.projectId)) {
    return false;
  }
  promptedProjectIds.add(target.projectId);
  return requestCruxIndexApproval(t, target);
}
