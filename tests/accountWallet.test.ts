import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import { setIpcTransport } from "../src/ipc.ts";
// @ts-expect-error Node's test runner needs the source extension.
import {
  EMPTY_WALLET_STATE,
  WalletStore,
  activeWalletLabel,
  formatWalletPlan,
  walletAccountLabel,
  walletAccountProblem,
  walletSwitchNotice,
  walletSyncedWithOrca,
  type WalletState,
} from "../src/accountWallet.ts";

const t = (source: string, values?: Record<string, string | number>) =>
  source.replace(/\{(\w+)\}/g, (_, key: string) => String(values?.[key] ?? ""));

function stateWithActive(activeId: string | null): WalletState {
  return {
    ...EMPTY_WALLET_STATE,
    codex: {
      activeId,
      systemDefault: { email: "me@example.com", plan: "plus" },
      accounts: [
        {
          id: "work",
          email: "work@example.com",
          organizationName: null,
          plan: "pro",
          missing: false,
          createdAt: "2026-10-08T00:00:00Z",
        },
      ],
    },
  };
}

type Invoke = (name: string, args?: Record<string, unknown>) => Promise<unknown>;

function installTransport(invoke: Invoke) {
  setIpcTransport({
    invoke: invoke as never,
    listen: async () => () => undefined,
  } as never);
}

test("a slow refresh never overwrites a newer switch", async () => {
  let releaseList: (state: WalletState) => void = () => undefined;
  const listGate = new Promise<WalletState>((resolve) => {
    releaseList = resolve;
  });
  installTransport(async (name) => {
    if (name === "wallet_list") return listGate;
    if (name === "wallet_select") {
      return {
        state: stateWithActive("work"),
        staleSessions: [
          {
            sessionId: "pane-1",
            provider: "codex",
            previousAccountId: null,
            nextAccountId: "work",
          },
        ],
      };
    }
    throw new Error(`unexpected command ${name}`);
  });
  const store = new WalletStore();

  const refresh = store.refresh();
  const result = await store.select("codex", "work");
  assert.equal(result.staleSessions.length, 1);
  assert.equal(store.getSnapshot().state.codex.activeId, "work");

  // The refresh answer was computed before the switch. It must not win.
  releaseList(stateWithActive(null));
  await refresh;
  assert.equal(store.getSnapshot().state.codex.activeId, "work");
});

test("the version changes only when the wallet changes", async () => {
  let current = stateWithActive(null);
  installTransport(async (name) => {
    if (name === "wallet_list") return current;
    if (name === "wallet_select") {
      current = stateWithActive("work");
      return { state: current, staleSessions: [] };
    }
    throw new Error(`unexpected command ${name}`);
  });
  const store = new WalletStore();

  await store.refresh();
  assert.equal(store.getSnapshot().loaded, true);
  assert.equal(store.getSnapshot().version, 0, "the first load is not a change");
  await store.refresh();
  assert.equal(store.getSnapshot().version, 0);
  await store.select("codex", "work");
  assert.equal(store.getSnapshot().version, 1);
  await store.refresh();
  assert.equal(store.getSnapshot().version, 1);
});

test("the switch notice counts only the panes of the provider that still run the old account", () => {
  const stale = (sessionId: string, provider: "claude" | "codex") => ({
    sessionId,
    provider,
    previousAccountId: "old",
    nextAccountId: "new",
  });
  // Claude panes in Orca's shared default folder follow a switch, so Rust
  // never reports them, and the notice stays away.
  assert.equal(walletSwitchNotice("claude", [stale("codex-1", "codex")], t), null);
  assert.equal(
    walletSwitchNotice("codex", [stale("codex-1", "codex")], t),
    "1 Codex session is still on the old account. Restart it from its pane or the status bar.",
  );
  assert.equal(
    walletSwitchNotice(
      "codex",
      [stale("codex-1", "codex"), stale("codex-2", "codex"), stale("claude-1", "claude")],
      t,
    ),
    "2 Codex sessions are still on the old account. Restart them from their panes or the status bar.",
  );
});

test("prompt labels name the account, System default, or an old account", () => {
  const codex = stateWithActive("work").codex;
  assert.equal(walletAccountLabel(codex, "work", t), "work@example.com");
  assert.equal(walletAccountLabel(codex, null, t), "System default");
  assert.equal(walletAccountLabel(codex, "removed", t), "the old account");
});

test("labels name the active account or the system default", () => {
  assert.equal(
    activeWalletLabel(stateWithActive(null).codex, t),
    "System default · me@example.com",
  );
  assert.equal(
    activeWalletLabel(stateWithActive("work").codex, t),
    "work@example.com",
  );
  assert.equal(formatWalletPlan("pro"), "Pro");
  assert.equal(formatWalletPlan("max"), "Max");
  assert.equal(formatWalletPlan("custom_tier"), "Custom_tier");
  assert.equal(formatWalletPlan("  "), null);
});

test("rows that cannot be picked tell the user why", () => {
  const account = stateWithActive(null).codex.accounts[0];
  assert.equal(walletAccountProblem(account, t), null);
  assert.equal(
    walletAccountProblem({ ...account, needsSignIn: true }, t),
    "Sign in again in Orca",
  );
  assert.equal(
    walletAccountProblem({ ...account, missing: true, needsSignIn: true }, t),
    "Account folder missing. Remove it and add it again.",
  );
});

test("the wallet reloads when the window gets the focus", async () => {
  let lists = 0;
  installTransport(async (name) => {
    if (name === "wallet_list") {
      lists += 1;
      return stateWithActive(null);
    }
    throw new Error(`unexpected command ${name}`);
  });
  const globals = globalThis as { window?: unknown };
  const previous = globals.window;
  const target = new EventTarget();
  globals.window = target;
  const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
  try {
    const store = new WalletStore();
    store.ensureLoaded();
    await settle();
    assert.equal(lists, 1, "the first load");
    // A second view mounts. It adds no second focus listener.
    store.ensureLoaded();
    await settle();
    assert.equal(lists, 1, "a loaded store does not load again");

    target.dispatchEvent(new Event("focus"));
    await settle();
    assert.equal(lists, 2, "one focus gives one reload");
  } finally {
    globals.window = previous;
  }
});

test("the wallet reports when Orca owns the accounts", () => {
  assert.equal(EMPTY_WALLET_STATE.store, "halv");
  assert.equal(walletSyncedWithOrca(EMPTY_WALLET_STATE), false);
  assert.equal(
    walletSyncedWithOrca({ ...EMPTY_WALLET_STATE, store: "orca" }),
    true,
  );
});
