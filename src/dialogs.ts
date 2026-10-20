export type DialogDetail = { label: string; text: string };

export type ConfirmDialogOptions = {
  title?: string;
  message: string;
  // Preformatted text that the user must read, such as scripts to approve.
  details?: DialogDetail[];
  confirmLabel?: string;
  cancelLabel?: string;
  danger?: boolean;
};

export type PromptDialogOptions = {
  title?: string;
  message?: string;
  defaultValue?: string;
  placeholder?: string;
  confirmLabel?: string;
};

type ConfirmDialogRequest = ConfirmDialogOptions & {
  id: number;
  kind: "confirm";
  resolve: (value: boolean) => void;
};

type PromptDialogRequest = PromptDialogOptions & {
  id: number;
  kind: "prompt";
  resolve: (value: string | null) => void;
};

export type AppDialogRequest = ConfirmDialogRequest | PromptDialogRequest;

type DialogListener = () => void;

let activeDialog: AppDialogRequest | null = null;
let nextDialogId = 1;
const dialogQueue: AppDialogRequest[] = [];
const listeners = new Set<DialogListener>();

function emitDialogChange() {
  for (const listener of listeners) {
    listener();
  }
}

function enqueueDialog(dialog: AppDialogRequest) {
  if (activeDialog) {
    dialogQueue.push(dialog);
    return;
  }
  activeDialog = dialog;
  emitDialogChange();
}

function takeActiveDialog() {
  const dialog = activeDialog;
  if (!dialog) {
    return null;
  }
  activeDialog = dialogQueue.shift() ?? null;
  emitDialogChange();
  return dialog;
}

export function confirmDialog(options: ConfirmDialogOptions): Promise<boolean> {
  return new Promise((resolve) => {
    enqueueDialog({
      ...options,
      id: nextDialogId++,
      kind: "confirm",
      resolve,
    });
  });
}

export function promptDialog(
  options: PromptDialogOptions,
): Promise<string | null> {
  return new Promise((resolve) => {
    enqueueDialog({
      ...options,
      id: nextDialogId++,
      kind: "prompt",
      resolve,
    });
  });
}

export function subscribeDialogs(listener: DialogListener) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function getActiveDialog() {
  return activeDialog;
}

export function acceptActiveDialog(promptValue?: string) {
  const dialog = takeActiveDialog();
  if (!dialog) {
    return;
  }
  if (dialog.kind === "confirm") {
    dialog.resolve(true);
  } else {
    dialog.resolve(promptValue ?? "");
  }
}

export function cancelActiveDialog() {
  const dialog = takeActiveDialog();
  if (!dialog) {
    return;
  }
  if (dialog.kind === "confirm") {
    dialog.resolve(false);
  } else {
    dialog.resolve(null);
  }
}
