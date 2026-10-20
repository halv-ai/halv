export type ShortcutCategory = "General" | "Task" | "Input" | "Workspace";

export type ShortcutDefinition = {
  id: string;
  label: string;
  description: string;
  category: ShortcutCategory;
  /** The macOS default. Also the default elsewhere when no other value exists. */
  defaultBinding: string;
  /** The Windows and Linux default, when it differs from macOS. */
  defaultBindingOther?: string;
};

export const shortcutDefinitions: ShortcutDefinition[] = [
  {
    id: "application.command_menu",
    label: "Command menu",
    description: "Open the command palette.",
    category: "General",
    defaultBinding: "mod+k",
  },
  {
    id: "application.settings",
    label: "Settings",
    description: "Open application settings.",
    category: "General",
    defaultBinding: "mod+comma",
  },
  {
    id: "application.keyboard_shortcuts",
    label: "Keyboard shortcuts",
    description: "Review or customize keyboard shortcuts.",
    category: "General",
    defaultBinding: "mod+shift+slash",
  },
  {
    id: "workspace.open_folder",
    label: "Open folder",
    description: "Add a local project folder.",
    category: "Workspace",
    defaultBinding: "mod+o",
  },
  {
    id: "navigation.back",
    label: "Navigate back",
    description: "Open the previous task in the list.",
    category: "General",
    defaultBinding: "mod+bracketleft",
  },
  {
    id: "navigation.forward",
    label: "Navigate forward",
    description: "Open the next task in the list.",
    category: "General",
    defaultBinding: "mod+bracketright",
  },
  {
    id: "appearance.increase_font",
    label: "Increase font size",
    description: "Increase the app UI scale.",
    category: "General",
    defaultBinding: "mod+plus",
  },
  {
    id: "appearance.decrease_font",
    label: "Decrease font size",
    description: "Decrease the app UI scale.",
    category: "General",
    defaultBinding: "mod+minus",
  },
  {
    id: "application.toggle_sidebar",
    label: "Toggle sidebar",
    description: "Show or hide the task sidebar.",
    category: "General",
    defaultBinding: "mod+b",
  },
  {
    id: "workspace.open_review",
    label: "Open diff rail",
    description: "Open and focus the terminal diff rail.",
    category: "Workspace",
    defaultBinding: "ctrl+shift+g",
  },
  {
    id: "workspace.toggle_review",
    label: "Toggle diff rail",
    description: "Show or hide the terminal diff rail.",
    category: "Workspace",
    defaultBinding: "mod+alt+b",
  },
  {
    id: "workspace.open_browser",
    label: "Open shared browser",
    description: "Open the browser preview and annotation surface.",
    category: "Workspace",
    defaultBinding: "mod+shift+b",
  },
  {
    id: "workspace.toggle_bottom_panel",
    label: "Toggle bottom panel",
    description: "Show or hide the integrated terminal panel.",
    category: "Workspace",
    defaultBinding: "mod+j",
  },
  {
    id: "workspace.toggle_terminal",
    label: "Toggle terminal",
    description: "Show or hide the integrated terminal panel.",
    category: "Workspace",
    defaultBinding: "ctrl+backquote",
  },
  {
    id: "workspace.clear_terminal",
    label: "Clear terminal",
    description: "Clear the visible integrated terminal buffer.",
    category: "Workspace",
    // Not ctrl+l: the CLIs in agent terminals use ctrl+l themselves.
    defaultBinding: "mod+k",
  },
  {
    id: "workspace.previous_terminal_tab",
    label: "Previous tab",
    description: "Show the previous tab of the workspace.",
    category: "Workspace",
    defaultBinding: "mod+shift+bracketleft",
  },
  {
    id: "workspace.next_terminal_tab",
    label: "Next tab",
    description: "Show the next tab of the workspace.",
    category: "Workspace",
    defaultBinding: "mod+shift+bracketright",
  },
  {
    id: "workspace.select_tab_by_number",
    label: "Select tab by number",
    description: "Show tab 1 to 8 of the workspace. 9 shows the last tab.",
    category: "Workspace",
    defaultBinding: "mod+digit",
  },
  {
    id: "workspace.new_agent_tab",
    label: "New agent tab",
    description: "Start the default agent in a new tab of the workspace.",
    category: "Workspace",
    defaultBinding: "mod+alt+t",
    // Ctrl+Alt+T opens a terminal window on many Linux desktops.
    defaultBindingOther: "disabled",
  },
  {
    id: "workspace.new_shell_tab",
    label: "New terminal tab",
    description: "Open a shell in a new tab of the workspace.",
    category: "Workspace",
    defaultBinding: "mod+t",
    // Ctrl+T stays with the CLIs in the terminal.
    defaultBindingOther: "mod+shift+t",
  },
  {
    id: "workspace.close_pane_or_tab",
    label: "Close pane or tab",
    description: "Close the focused pane, else the tab. With nothing to close, hide the window.",
    category: "Workspace",
    defaultBinding: "mod+w",
    // Ctrl+W stays "delete word" in the terminal.
    defaultBindingOther: "mod+shift+w",
  },
  {
    id: "workspace.reopen_closed_tab",
    label: "Reopen closed tab",
    description: "Open the last closed tab of the workspace again.",
    category: "Workspace",
    defaultBinding: "mod+shift+t",
    // Ctrl+Shift+T opens a terminal tab on Windows and Linux.
    defaultBindingOther: "disabled",
  },
  {
    id: "workspace.rename_tab",
    label: "Rename tab",
    description: "Rename the visible tab of the workspace.",
    category: "Workspace",
    defaultBinding: "mod+r",
    // Ctrl+R stays reverse search in the terminal.
    defaultBindingOther: "disabled",
  },
  {
    id: "workspace.recent_tab",
    label: "Switch to a recent tab",
    description: "Hold Ctrl and press Tab to pick a recent tab of the workspace. Shift goes back.",
    category: "Workspace",
    defaultBinding: "ctrl+tab",
  },
  {
    id: "workspace.previous_workspace",
    label: "Previous workspace",
    description: "Show the workspace above in the sidebar.",
    category: "Workspace",
    defaultBinding: "mod+alt+arrowup",
    // Ctrl+Alt+Up switches the desktop on many Linux desktops. Orca uses
    // Ctrl+Shift+Up there.
    defaultBindingOther: "mod+shift+arrowup",
  },
  {
    id: "workspace.next_workspace",
    label: "Next workspace",
    description: "Show the workspace below in the sidebar.",
    category: "Workspace",
    defaultBinding: "mod+alt+arrowdown",
    defaultBindingOther: "mod+shift+arrowdown",
  },
  {
    id: "task.quick_chat",
    label: "Quick Chat",
    description: "Start a new chat without a project.",
    category: "Task",
    defaultBinding: "mod+alt+n",
  },
  {
    id: "task.new",
    label: "New task",
    description: "Start a task in the selected project.",
    category: "Task",
    defaultBinding: "mod+n",
  },
  {
    id: "task.search",
    label: "Search tasks",
    description: "Focus task and message search.",
    category: "Task",
    defaultBinding: "mod+g",
  },
  {
    id: "task.find",
    label: "Find in task",
    description: "Find text inside the open task.",
    category: "Task",
    defaultBinding: "mod+f",
  },
  {
    id: "task.previous",
    label: "Previous task",
    description: "Open the previous task.",
    category: "Task",
    defaultBinding: "mod+shift+bracketleft",
  },
  {
    id: "task.next",
    label: "Next task",
    description: "Open the next task.",
    category: "Task",
    defaultBinding: "mod+shift+bracketright",
  },
  {
    id: "task.previous_pinned",
    label: "Previous pinned conversation",
    description: "Cycle to the previous pinned conversation.",
    category: "Task",
    defaultBinding: "mod+alt+arrowleft",
  },
  {
    id: "task.next_pinned",
    label: "Next pinned conversation",
    description: "Cycle to the next pinned conversation.",
    category: "Task",
    defaultBinding: "mod+alt+arrowright",
  },
  {
    id: "input.dictation",
    label: "Dictation",
    description: "Start or stop speech dictation in the composer.",
    category: "Input",
    defaultBinding: "ctrl+shift+d",
  },
];

export type KeyboardShortcutOverrides = Record<string, string>;

const definitionById = new Map(
  shortcutDefinitions.map((definition) => [definition.id, definition]),
);

export function isMacPlatform(platform?: string) {
  const value =
    platform ??
    (typeof navigator === "undefined" ? "MacIntel" : navigator.platform);
  return /mac|iphone|ipad|ipod/i.test(value);
}

/** The default binding of an action on this platform. */
export function defaultShortcut(
  definition: Pick<ShortcutDefinition, "defaultBinding" | "defaultBindingOther">,
  platform?: string,
) {
  return isMacPlatform(platform)
    ? definition.defaultBinding
    : definition.defaultBindingOther ?? definition.defaultBinding;
}

export function effectiveShortcut(
  id: string,
  overrides: KeyboardShortcutOverrides,
  platform?: string,
) {
  const definition = definitionById.get(id);
  return overrides[id] ?? (definition ? defaultShortcut(definition, platform) : "disabled");
}

// Each pair shares one chord on purpose. The workspace strip takes the
// chord while it shows. Otherwise the task action keeps it.
const ALLOWED_SHARED_CHORDS: ReadonlyArray<readonly [string, string]> = [
  ["workspace.previous_terminal_tab", "task.previous"],
  ["workspace.next_terminal_tab", "task.next"],
];

export function shortcutsMayShareChord(left: string, right: string) {
  return ALLOWED_SHARED_CHORDS.some(
    ([first, second]) =>
      (first === left && second === right) || (first === right && second === left),
  );
}

/** The digit 1 to 9 of a key event, or null. The physical key wins, so a
 *  shifted digit or another keyboard layout still gives its number. */
export function eventDigit(event: Pick<KeyboardEvent, "key" | "code">) {
  const fromCode = /^(?:Digit|Numpad)([1-9])$/.exec(event.code ?? "");
  if (fromCode) return Number(fromCode[1]);
  return /^[1-9]$/.test(event.key) ? Number(event.key) : null;
}

/** For a binding with the `digit` token: the digit of a matching event.
 *  Null when the event does not match. */
export function shortcutDigitForEvent(
  event: KeyboardEventLike,
  binding: string,
  platform?: string,
) {
  if (!binding.endsWith("+digit") && binding !== "digit") return null;
  const digit = eventDigit(event);
  if (digit === null) return null;
  return shortcutMatchesEvent(event, binding, platform) ? digit : null;
}

/** A recorded chord for an action. A digit action stores the `digit`
 *  token, so one chord covers every number. */
export function recordedShortcutBinding(id: string, binding: string) {
  const definition = definitionById.get(id);
  if (!definition?.defaultBinding.endsWith("+digit")) return binding;
  return binding.replace(/\+[1-9]$/, "+digit");
}

export function normalizeShortcutKey(key: string, code = "") {
  const normalizedCode = code.toLowerCase();
  if (normalizedCode === "equal" || key === "+" || key === "=") return "plus";
  if (normalizedCode === "minus" || key === "-") return "minus";
  if (normalizedCode === "comma" || key === ",") return "comma";
  if (normalizedCode === "period" || key === ".") return "period";
  if (normalizedCode === "slash" || key === "/" || key === "?") return "slash";
  if (normalizedCode === "bracketleft" || key === "[") return "bracketleft";
  if (normalizedCode === "bracketright" || key === "]") return "bracketright";
  if (normalizedCode === "backquote" || key === "`") return "backquote";
  if (key === " ") return "space";
  return key.toLowerCase();
}

type KeyboardEventLike = Pick<
  KeyboardEvent,
  "key" | "code" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey"
>;

export function shortcutFromKeyboardEvent(
  event: KeyboardEventLike,
  platform?: string,
) {
  const key = normalizeShortcutKey(event.key, event.code);
  if (["meta", "control", "alt", "shift"].includes(key)) return null;

  const mac = isMacPlatform(platform);
  const modifiers: string[] = [];
  if (mac ? event.metaKey : event.ctrlKey) modifiers.push("mod");
  if (mac && event.ctrlKey) modifiers.push("ctrl");
  if (event.altKey) modifiers.push("alt");
  if (event.shiftKey && key !== "plus") modifiers.push("shift");
  if (modifiers.length === 0 || (modifiers.length === 1 && modifiers[0] === "shift")) {
    return null;
  }
  return [...modifiers, key].join("+");
}

export function shortcutMatchesEvent(
  event: KeyboardEventLike,
  binding: string,
  platform?: string,
) {
  if (!binding || binding === "disabled") return false;
  const tokens = binding.split("+");
  const key = tokens[tokens.length - 1];
  const modifiers = new Set(tokens.slice(0, -1));
  const mac = isMacPlatform(platform);
  const expectedMeta = mac && modifiers.has("mod");
  const expectedCtrl = mac
    ? modifiers.has("ctrl")
    : modifiers.has("mod") || modifiers.has("ctrl");
  const expectedAlt = modifiers.has("alt");
  const expectedShift = modifiers.has("shift") || key === "plus";
  const keyMatches =
    key === "digit"
      ? eventDigit(event) !== null
      : normalizeShortcutKey(event.key, event.code) === key;

  return (
    keyMatches &&
    event.metaKey === expectedMeta &&
    event.ctrlKey === expectedCtrl &&
    event.altKey === expectedAlt &&
    event.shiftKey === expectedShift
  );
}

export function formatShortcut(binding: string, platform?: string) {
  if (!binding || binding === "disabled") return "Unassigned";
  const mac = isMacPlatform(platform);
  const labels: Record<string, string> = mac
    ? { mod: "⌘", ctrl: "⌃", alt: "⌥", shift: "⇧" }
    : { mod: "Ctrl", ctrl: "Ctrl", alt: "Alt", shift: "Shift" };
  const keyLabels: Record<string, string> = {
    plus: "+",
    minus: "−",
    comma: ",",
    period: ".",
    slash: "/",
    bracketleft: "[",
    bracketright: "]",
    backquote: "`",
    space: "Space",
    enter: "Enter",
    arrowleft: "←",
    arrowright: "→",
    arrowup: "↑",
    arrowdown: "↓",
    tab: "Tab",
    digit: "1…9",
  };
  const parts = binding.split("+").map((part) => labels[part] ?? keyLabels[part] ?? part.toUpperCase());
  return mac ? parts.join("") : parts.join("+");
}

export function shortcutForAction(
  id: string,
  overrides: KeyboardShortcutOverrides,
  platform?: string,
) {
  return effectiveShortcut(id, overrides, platform);
}

export function shortcutConflict(
  id: string,
  binding: string,
  overrides: KeyboardShortcutOverrides,
  platform?: string,
) {
  if (binding === "disabled") return null;
  return (
    shortcutDefinitions.find(
      (definition) =>
        definition.id !== id &&
        !shortcutsMayShareChord(id, definition.id) &&
        effectiveShortcut(definition.id, overrides, platform) === binding,
    ) ?? null
  );
}

export function shortcutSearchText(definition: ShortcutDefinition) {
  return `${definition.label} ${definition.description} ${definition.category}`.toLowerCase();
}
