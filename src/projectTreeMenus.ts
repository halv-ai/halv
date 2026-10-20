import type { Translate } from "./i18n/runtime";

// The row menus of the Projects tree (ADR 0010, stage S7). Orca opens them
// with a right click on a row; the project and workspace rows also have a
// "…" button. This module only decides the items, their order, and when
// an item is off. The tree component runs the actions.

export type TreeMenuItem =
  | {
      kind: "item";
      id: string;
      label: string;
      disabled?: boolean;
      /** A destructive action, shown in the danger color. */
      danger?: boolean;
      /** The reason that an item is off, or a hint. */
      title?: string;
      /** A submenu. */
      children?: TreeMenuItem[];
    }
  | { kind: "separator" };

export type MenuPlatform = "mac" | "windows" | "linux";

export function menuPlatform(platform: string): MenuPlatform {
  if (/mac/i.test(platform)) return "mac";
  if (/win/i.test(platform)) return "windows";
  return "linux";
}

function item(
  id: string,
  label: string,
  extra: Omit<Extract<TreeMenuItem, { kind: "item" }>, "kind" | "id" | "label"> = {},
): TreeMenuItem {
  return { kind: "item", id, label, ...extra };
}

const SEPARATOR: TreeMenuItem = { kind: "separator" };

/** Drops leading, trailing and doubled separators. */
export function tidyMenu(items: readonly TreeMenuItem[]): TreeMenuItem[] {
  const result: TreeMenuItem[] = [];
  for (const entry of items) {
    if (entry.kind === "separator") {
      if (result.length === 0 || result[result.length - 1].kind === "separator") continue;
    }
    result.push(entry);
  }
  while (result.at(-1)?.kind === "separator") result.pop();
  return result;
}

export function fileManagerLabel(platform: MenuPlatform, t: Translate) {
  if (platform === "mac") return t("Open in Finder");
  if (platform === "windows") return t("Open in File Explorer");
  return t("Open in file manager");
}

/**
 * An agent row: a conversation, or a terminal without one. Orca's agent
 * menu (Open, Copy Title, Copy Path, Mark Read or Unread, Clear from List)
 * plus Halv's Rename, Close tab and Archive. A terminal row of the Chats
 * area also offers "Open in tab" (ADR 0020).
 */
export function agentRowMenu(
  row: {
    kind: "conversation" | "terminal";
    unread: boolean;
    /** The row has an open tab (workspace tabs). */
    tabbed: boolean;
    /** The folder that "Copy path" copies, when one is known. */
    path: string | null;
    /** A terminal without a tab can move into a shell tab (ADR 0020). */
    openInTab?: boolean;
  },
  t: Translate,
): TreeMenuItem[] {
  const conversation = row.kind === "conversation";
  return tidyMenu([
    item("open", t("Open")),
    ...(!conversation && row.openInTab ? [item("open-in-tab", t("Open in tab"))] : []),
    item("rename", t("Rename")),
    SEPARATOR,
    item("copy-title", t("Copy title")),
    ...(row.path ? [item("copy-path", t("Copy path"))] : []),
    SEPARATOR,
    ...(conversation
      ? [
          row.unread
            ? item("mark-read", t("Mark as read"))
            : item("mark-unread", t("Mark as unread")),
        ]
      : []),
    SEPARATOR,
    ...(conversation && row.tabbed ? [item("close-tab", t("Close tab"))] : []),
    ...(conversation ? [item("archive", t("Archive"))] : []),
  ]);
}

/**
 * A workspace row. The primary workspace is the project root: it cannot be
 * removed, so its menu offers the project removal instead (Orca).
 */
export function workspaceRowMenu(
  workspace: {
    primary: boolean;
    managed: boolean;
    pinned: boolean;
    available: boolean;
    /** Workspace tabs: "New agent here" opens an agent tab. */
    agentTabs: boolean;
  },
  platform: MenuPlatform,
  t: Translate,
): TreeMenuItem[] {
  const unavailable = workspace.available
    ? {}
    : { disabled: true, title: t("Workspace is no longer available on disk.") };
  return tidyMenu([
    ...(workspace.agentTabs ? [item("new-agent", t("New agent here"), unavailable)] : []),
    item("new-terminal", t("New terminal here"), unavailable),
    SEPARATOR,
    item("open-file-manager", fileManagerLabel(platform, t), unavailable),
    item("open-editor", t("Open in editor"), unavailable),
    item("copy-path", t("Copy path")),
    item("copy-branch", t("Copy branch name")),
    SEPARATOR,
    ...(workspace.primary
      ? []
      : [workspace.pinned ? item("unpin", t("Unpin")) : item("pin", t("Pin"))]),
    SEPARATOR,
    workspace.primary
      ? item("remove-worktree", t("Remove worktree…"), {
          danger: true,
          disabled: true,
          title: t("The primary workspace is the project root. Remove the project instead."),
        })
      : item("remove-worktree", t("Remove worktree…"), {
          danger: true,
          disabled: !workspace.managed,
          title: workspace.managed ? undefined : t("Halv did not create this worktree."),
        }),
    ...(workspace.primary
      ? [item("remove-project", t("Remove project from Halv…"), { danger: true })]
      : []),
  ]);
}

/** A project row. */
export function projectRowMenu(
  project: {
    /** Every workspace of the project is collapsed. */
    allCollapsed: boolean;
    /** Halv creates worktrees only in a Git repository. */
    gitRepo: boolean;
    /** The project groups, and the group of this project. */
    groups?: readonly { id: string; name: string }[];
    groupId?: string | null;
  },
  platform: MenuPlatform,
  t: Translate,
): TreeMenuItem[] {
  const groups = project.groups ?? [];
  const groupId = project.groupId ?? null;
  return tidyMenu([
    item("new-worktree", t("New worktree…"), {
      disabled: !project.gitRepo,
      title: project.gitRepo ? undefined : t("Halv creates worktrees only in a Git repository."),
    }),
    SEPARATOR,
    item("open-file-manager", fileManagerLabel(platform, t)),
    item("copy-path", t("Copy path")),
    project.allCollapsed
      ? item("expand-all", t("Expand all"))
      : item("collapse-all", t("Collapse all")),
    SEPARATOR,
    // Orca's group items: a new group, a submenu of the groups (the current
    // group is off), and the way out of a group.
    item("new-group", t("New group from project…")),
    ...(groups.length > 0
      ? [
          item("move-to-group", t("Move to group"), {
            children: groups.map((group) =>
              item(`move-to-group:${group.id}`, group.name, {
                disabled: group.id === groupId,
              }),
            ),
          }),
        ]
      : []),
    ...(groupId ? [item("remove-from-group", t("Remove from group"))] : []),
    SEPARATOR,
    item("remove-project", t("Remove project from Halv…"), { danger: true }),
  ]);
}

/** A project group header: Orca's "Rename group" and "Delete group". */
export function groupRowMenu(t: Translate): TreeMenuItem[] {
  return [
    item("rename-group", t("Rename group…")),
    SEPARATOR,
    item("delete-group", t("Delete group…"), { danger: true }),
  ];
}
