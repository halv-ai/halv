import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  agentNeedsCliToRun,
  allConnectGateAccountsConnected,
  anyAgentCliInstalled,
  connectGateOutcome,
  nextConnectGateState,
  shouldOpenApiKeySection,
  type ConnectGateConnections,
} from "../src/connectGate.ts";
// @ts-expect-error Node's test runner needs the source extension.
import { hasProviderCredentials } from "../src/format.ts";
import type {
  AgentSessionLaneOptions,
  AgentSessionProbe,
} from "../src/agentSessions.ts";
import type {
  AnthropicCredentialStatus,
  AuthStatus,
  CredentialProfile,
} from "../src/types.ts";

const signedOut: AuthStatus = { signedOut: {} };
const noAnthropicKey: AnthropicCredentialStatus = { configured: false };
const noConnections: ConnectGateConnections = {
  anthropic: false,
  claude: false,
  codex: false,
  kimi: false,
  moonshot: false,
  openai: false,
  zai: false,
};

test("the update card covers the gate, so the gate has no update banner", () => {
  const component = readFileSync(
    new URL("../src/components/ConnectGate.tsx", import.meta.url),
    "utf8",
  );
  assert.doesNotMatch(component, /updateBannerText|Update and restart/);
  assert.doesNotMatch(component, /availableAppUpdate/);
});

test("ChatGPT sign-in waits as long as the backend and shows elapsed time", () => {
  const component = readFileSync(
    new URL("../src/components/ConnectGate.tsx", import.meta.url),
    "utf8",
  );
  const backend = readFileSync(
    new URL("../src-tauri/src/auth/mod.rs", import.meta.url),
    "utf8",
  );
  const frontendMs = Number(
    /OAUTH_LOGIN_TIMEOUT_MS = ([\d_]+);/.exec(component)?.[1]?.replaceAll("_", ""),
  );
  const backendSeconds = (
    /LOGIN_TIMEOUT: Duration = Duration::from_secs\(([\d\s*]+)\)/.exec(backend)?.[1] ?? "0"
  )
    .split("*")
    .reduce((product, factor) => product * Number(factor.trim()), 1);
  assert.equal(frontendMs, 300_000);
  assert.equal(frontendMs, backendSeconds * 1000);
  assert.match(component, /t\("\{seconds\}s elapsed", \{ seconds: oauthElapsedSeconds \}\)/);
});

test("a CLI that fails its version probe shows the reason on its row", () => {
  const component = readFileSync(
    new URL("../src/components/ConnectGate.tsx", import.meta.url),
    "utf8",
  );
  assert.match(component, /probeErrorText\(probeForAgent\(probe, "codex"\)\)/);
  assert.match(component, /const probeError = probeErrorText\(binary\)/);
  assert.match(component, /\{agent\} CLI check failed: \{error\}/);
});

test("centers the connect gate content column", () => {
  const component = readFileSync(
    new URL("../src/components/ConnectGate.tsx", import.meta.url),
    "utf8",
  );
  assert.match(component, /className="mx-auto my-auto flex w-full max-w-\[560px\]/);
});

test("Claude subscription connection surfaces the optional proxy preference", () => {
  const component = readFileSync(
    new URL("../src/components/ConnectGate.tsx", import.meta.url),
    "utf8",
  );
  const settings = readFileSync(
    new URL("../src/components/SettingsPanel.tsx", import.meta.url),
    "utf8",
  );

  assert.match(component, /\{claudeSubscriptionConnected \? \(/);
  assert.match(component, /Route Claude subscription through Halv Engine/);
  assert.match(component, /t\("Learn more"\)/);
  assert.match(component, /Anthropic's consumer terms/);
  assert.match(settings, /ipc\.preferences\.setClaudeSubscriptionProxy/);
  assert.match(settings, /Only affects new terminal\s+sessions\./);
});

test("provider terminal agents need their agent CLI to run", () => {
  for (const agentKind of [
    "antigravity",
    "claude",
    "gemini",
    "kimi",
    "zai",
  ] as const) {
    assert.equal(agentNeedsCliToRun(agentKind), true);
  }
});

test("ChatGPT can run without the Codex CLI", () => {
  assert.equal(agentNeedsCliToRun("codex"), false);
});

test("agent CLI detection waits for a resolved probe", () => {
  assert.equal(anyAgentCliInstalled(null), false);
});

test("agent CLI detection reports all missing CLIs", () => {
  assert.equal(anyAgentCliInstalled(missingAgentCliProbe()), false);
});

test("agent CLI detection reports one installed CLI", () => {
  assert.equal(
    anyAgentCliInstalled({
      ...missingAgentCliProbe(),
      codex: { found: true, binary: "/usr/local/bin/codex" },
    }),
    true,
  );
});

test("API key section stays closed while the probe is unresolved", () => {
  assert.equal(shouldOpenApiKeySection(null, noConnections), false);
});

test("API key section opens when no CLI or connection exists", () => {
  assert.equal(
    shouldOpenApiKeySection(missingAgentCliProbe(), noConnections),
    true,
  );
});

test("API key section stays closed when a connection exists", () => {
  assert.equal(
    shouldOpenApiKeySection(missingAgentCliProbe(), {
      ...noConnections,
      openai: true,
    }),
    false,
  );
});

test("API key section stays closed when a CLI exists", () => {
  assert.equal(
    shouldOpenApiKeySection(
      {
        ...missingAgentCliProbe(),
        claude: { found: true, binary: "/usr/local/bin/claude" },
      },
      noConnections,
    ),
    false,
  );
});

test("no connected account keeps the app hard-gated", () => {
  assert.equal(
    nextConnectGateState("resolving", false, [], [], false),
    "resolving",
  );
  assert.equal(
    nextConnectGateState(
      "resolving",
      true,
      signedOutLanes(),
      [],
      hasProviderCredentials(signedOut, noAnthropicKey),
    ),
    "required",
  );
  assert.equal(
    nextConnectGateState("required", true, [], [], false),
    "required",
  );
});

test("only a Claude CLI login passes the hard gate", () => {
  const lanes = signedOutLanes();
  lanes[0].lanes[0].loggedIn = true;

  assert.equal(connectGateOutcome(lanes, [], false), "some");
  assert.equal(
    nextConnectGateState("required", true, lanes, [], false),
    "optional",
  );
});

test("only an active Z.ai profile passes the hard gate", () => {
  const profiles = [profile("zai")];

  assert.equal(connectGateOutcome(signedOutLanes(), profiles, false), "some");
  assert.equal(
    nextConnectGateState("required", true, signedOutLanes(), profiles, false),
    "optional",
  );
});

test("only an active xAI profile passes the hard gate", () => {
  const profiles = [profile("xai")];

  assert.equal(connectGateOutcome(signedOutLanes(), profiles, false), "some");
  assert.equal(
    nextConnectGateState("required", true, signedOutLanes(), profiles, false),
    "optional",
  );
});

test("legacy OpenAI and Anthropic API keys still pass the hard gate", () => {
  const credentials: Array<
    [AuthStatus, AnthropicCredentialStatus]
  > = [
    [{ apiKey: { masked: "sk-...1234" } }, noAnthropicKey],
    [signedOut, { configured: true, masked: "sk-ant-...1234" }],
  ];

  for (const [auth, anthropic] of credentials) {
    assert.equal(
      nextConnectGateState(
        "resolving",
        true,
        signedOutLanes(),
        [],
        hasProviderCredentials(auth, anthropic),
      ),
      "optional",
    );
  }
});

test("all accounts includes both GLM and Grok API profiles", () => {
  const lanes = signedOutLanes();
  for (const option of lanes) {
    const subscription = option.lanes.find((lane) => lane.loggedIn !== null);
    if (subscription) subscription.loggedIn = true;
  }

  assert.equal(
    allConnectGateAccountsConnected(lanes, [profile("zai"), profile("xai")]),
    true,
  );
  assert.equal(allConnectGateAccountsConnected(lanes, []), false);
  assert.equal(
    allConnectGateAccountsConnected(signedOutLanes(), [
      profile("openai"),
      profile("anthropic"),
      profile("moonshot"),
      profile("zai"),
      profile("xai"),
    ]),
    true,
  );
  assert.equal(
    connectGateOutcome(lanes, [profile("zai"), profile("xai")], false),
    "all",
  );
});

test("disabled profiles do not pass the gate", () => {
  assert.equal(
    connectGateOutcome(
      signedOutLanes(),
      [profile("zai", "disabled")],
      false,
    ),
    "none",
  );
});

test("dismissing the launch gate is sticky for the app start", () => {
  assert.equal(
    nextConnectGateState("passed", true, signedOutLanes(), [], false),
    "passed",
  );
});

test("a dismissed partial gate stays passed on later launches", () => {
  const lanes = signedOutLanes();
  lanes[0].lanes[0].loggedIn = true;

  assert.equal(
    nextConnectGateState("resolving", true, lanes, [], false, true),
    "passed",
  );
});

test("dismissal never bypasses the no-account gate", () => {
  assert.equal(
    nextConnectGateState(
      "resolving",
      true,
      signedOutLanes(),
      [],
      false,
      true,
    ),
    "required",
  );
});

function signedOutLanes(): AgentSessionLaneOptions {
  return [
    {
      agentKind: "claude",
      lanes: [
        { kind: "claudeSubscription", loggedIn: false, credentialProfiles: [] },
        { kind: "claudeApiKey", loggedIn: null, credentialProfiles: [] },
      ],
    },
    {
      agentKind: "codex",
      lanes: [
        { kind: "codexChatGpt", loggedIn: false, credentialProfiles: [] },
      ],
    },
    {
      agentKind: "kimi",
      lanes: [
        { kind: "kimiSubscription", loggedIn: false, credentialProfiles: [] },
        { kind: "kimiApiKey", loggedIn: null, credentialProfiles: [] },
      ],
    },
    {
      agentKind: "zai",
      lanes: [{ kind: "zaiGlm", loggedIn: null, credentialProfiles: [] }],
    },
    {
      agentKind: "grok",
      lanes: [{ kind: "grokApiKey", loggedIn: null, credentialProfiles: [] }],
    },
  ];
}

function missingAgentCliProbe(): AgentSessionProbe {
  return {
    claude: { found: false },
    codex: { found: false },
    kimi: { found: false },
  };
}

function profile(
  provider: string,
  status: CredentialProfile["status"] = "active",
): CredentialProfile {
  return {
    id: `${provider}-${status}`,
    provider,
    name: provider,
    kind: "api_key",
    priority: 0,
    status,
    lastValidatedAt: null,
    metadataJson: null,
    createdAt: "2026-08-16T00:00:00Z",
    updatedAt: "2026-08-16T00:00:00Z",
  };
}
