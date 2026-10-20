import type {
  WalletAccountUsage,
  WalletStaleSession,
  WalletState,
} from "../accountWallet";
import type { ClaudeLocalUsageWindow } from "../providerPlanWindows";

const MINUTE_MS = 60_000;

/** The default harness wallet: Halv's own registry with no managed accounts. */
export function halvWalletState(): WalletState {
  return {
    claude: {
      activeId: null,
      systemDefault: { email: "auditor@example.com", plan: null },
      accounts: [],
    },
    codex: {
      activeId: null,
      systemDefault: { email: "auditor@example.com", plan: "pro" },
      accounts: [],
    },
    recovered: false,
    store: "halv",
  };
}

/** A wallet that Orca owns. Orca holds three Claude logins and one Codex login. */
export function orcaWalletState(): WalletState {
  const createdAt = new Date(0).toISOString();
  return {
    claude: {
      activeId: "orca-claude-work",
      systemDefault: { email: "auditor@example.com", plan: "pro" },
      accounts: [
        {
          id: "orca-claude-work",
          email: "dev@acme.example",
          organizationName: "Acme Inc.",
          plan: "max",
          missing: false,
          createdAt,
        },
        {
          id: "orca-claude-personal",
          email: "auditor.personal@example.com",
          organizationName: null,
          plan: "pro",
          missing: false,
          createdAt,
        },
        {
          id: "orca-claude-research",
          email: "research@lab.example",
          organizationName: "Research Lab",
          plan: "team",
          missing: false,
          createdAt,
        },
      ],
    },
    codex: {
      activeId: null,
      systemDefault: { email: "auditor@example.com", plan: "pro" },
      accounts: [
        {
          id: "orca-codex-ops",
          email: "ops@acme.example",
          organizationName: "Acme Inc.",
          plan: "plus",
          missing: false,
          createdAt,
        },
      ],
    },
    recovered: false,
    store: "orca",
  };
}

/** Orca's cached usage for every Orca account. */
export function orcaWalletUsage(nowMs: number): WalletAccountUsage[] {
  const resetsIn = (minutes: number) => nowMs + minutes * MINUTE_MS;
  const updatedAt = nowMs - 4 * MINUTE_MS;
  return [
    {
      provider: "claude",
      accountId: "orca-claude-work",
      windows: [
        { kind: "session", usedPercent: 13, resetsAt: resetsIn(234) },
        { kind: "weekly", usedPercent: 41, resetsAt: resetsIn(9_060) },
      ],
      updatedAt,
    },
    {
      provider: "claude",
      accountId: "orca-claude-personal",
      windows: [
        { kind: "session", usedPercent: 8, resetsAt: resetsIn(131) },
        { kind: "weekly", usedPercent: 64, resetsAt: resetsIn(4_380) },
      ],
      updatedAt,
    },
    {
      provider: "claude",
      accountId: "orca-claude-research",
      windows: [
        { kind: "session", usedPercent: 86, resetsAt: resetsIn(41) },
        { kind: "weekly", usedPercent: 92, resetsAt: resetsIn(1_680) },
        { kind: "fableWeekly", usedPercent: 35, resetsAt: resetsIn(1_680) },
      ],
      updatedAt,
    },
    {
      provider: "codex",
      accountId: "orca-codex-ops",
      windows: [
        { kind: "session", usedPercent: 22, resetsAt: resetsIn(95) },
        { kind: "weekly", usedPercent: 71, resetsAt: resetsIn(5_200) },
      ],
      updatedAt,
    },
  ];
}

/** The plan windows of the active Orca Claude account. */
export function orcaClaudeUsageWindows(nowMs: number): ClaudeLocalUsageWindow[] {
  const resetsIn = (minutes: number) =>
    new Date(nowMs + minutes * MINUTE_MS).toISOString();
  return [
    { label: "5h", usedPercent: 13, resetsAt: resetsIn(234) },
    { label: "7d", usedPercent: 41, resetsAt: resetsIn(9_060) },
  ];
}

/** Orca's usage while the active Claude account nears its 5-hour limit. */
export function orcaLimitWalletUsage(nowMs: number): WalletAccountUsage[] {
  const resetsIn = (minutes: number) => nowMs + minutes * MINUTE_MS;
  return orcaWalletUsage(nowMs).map((usage) =>
    usage.accountId === "orca-claude-work"
      ? {
          ...usage,
          windows: [
            { kind: "session", usedPercent: 92, resetsAt: resetsIn(41) },
            { kind: "weekly", usedPercent: 63, resetsAt: resetsIn(9_060) },
          ],
        }
      : usage,
  );
}

/** The live plan windows of the active Claude account in `usage`. */
export function orcaClaudeWindowsFor(
  usage: readonly WalletAccountUsage[],
  accountId: string | null,
): ClaudeLocalUsageWindow[] {
  const account = usage.find(
    (entry) => entry.provider === "claude" && entry.accountId === accountId,
  );
  const labels = { session: "5h", weekly: "7d" } as const;
  return (account?.windows ?? []).flatMap((window) =>
    window.kind === "session" || window.kind === "weekly"
      ? [
          {
            label: labels[window.kind],
            usedPercent: window.usedPercent,
            resetsAt:
              window.resetsAt === null ? null : new Date(window.resetsAt).toISOString(),
          },
        ]
      : [],
  );
}

/**
 * The Orca wallet after a switch: Codex moved from ops@ to a second login,
 * and Claude moved from the personal login to the work login.
 */
export function orcaRestartWalletState(): WalletState {
  const state = orcaWalletState();
  state.codex.accounts.push({
    id: "orca-codex-lab",
    email: "lab@acme.example",
    organizationName: "Acme Inc.",
    plan: "pro",
    missing: false,
    createdAt: new Date(0).toISOString(),
  });
  state.codex.activeId = "orca-codex-lab";
  return state;
}

/** The panes that still run the old accounts in `orcaRestartWalletState`. */
export function orcaRestartStaleSessions(ids: {
  codex: string;
  claude: string;
}): WalletStaleSession[] {
  return [
    {
      sessionId: ids.codex,
      provider: "codex",
      previousAccountId: "orca-codex-ops",
      nextAccountId: "orca-codex-lab",
    },
    {
      sessionId: ids.claude,
      provider: "claude",
      previousAccountId: "orca-claude-personal",
      nextAccountId: "orca-claude-work",
    },
  ];
}
