// The open state of the "Connect phone" dialog (ADR 0009). The status bar
// button and the command palette open it. The dialog shows only while the
// mobileRemote feature flag is on.

type Listener = () => void;

export function createMobileConnectStore() {
  const listeners = new Set<Listener>();
  let open = false;
  const set = (next: boolean) => {
    if (open === next) return;
    open = next;
    for (const listener of listeners) listener();
  };
  return {
    getSnapshot: () => open,
    subscribe(listener: Listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    open: () => set(true),
    close: () => set(false),
  };
}

export const mobileConnectStore = createMobileConnectStore();
