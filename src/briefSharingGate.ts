import { BriefSharingGate } from "./briefSharing.ts";
import { ipc } from "./ipc.ts";

/**
 * The one task brief gate of the app (ADR 0023). `App` connects it to the
 * IPC launch commands. Settings tells it about a saved answer.
 */
export const briefSharingGate = new BriefSharingGate({
  // `ipc` changes when the harness sets its transport, so each call reads it.
  loadPreferences: () => ipc.preferences.get(),
  save: (answer) => ipc.preferences.setBriefSharing({ briefSharing: answer }),
  now: () => Date.now(),
});
