import assert from "node:assert/strict";
import test from "node:test";
import {
  credentialProfileKeyHint,
  credentialProfileKeyPlaceholder,
  credentialProfileIndicatorLabel,
  credentialProfilePinNotice,
  credentialProfileProviderLabel,
  eligibleCredentialProfiles,
  nextMissingCredentialProfileProvider,
} from "../src/credentialProfiles.ts";
import type { CredentialProfile } from "../src/types.ts";

function profile(
  id: string,
  provider: string,
  status: CredentialProfile["status"] = "active",
): CredentialProfile {
  return {
    id,
    provider,
    name: `${provider} ${id}`,
    kind: "api_key",
    priority: 0,
    status,
    lastValidatedAt: null,
    metadataJson: null,
    createdAt: "2026-08-16T00:00:00Z",
    updatedAt: "2026-08-16T00:00:00Z",
  };
}

test("filters active profiles for the selected model provider", () => {
  const profiles = [
    profile("primary", "openai"),
    profile("disabled", "openai", "disabled"),
    profile("anthropic", "anthropic"),
  ];

  assert.deepEqual(
    eligibleCredentialProfiles(profiles, "openai").map((item) => item.id),
    ["primary"],
  );
  assert.deepEqual(eligibleCredentialProfiles(profiles, undefined), []);
});

test("adds the selected profile to the compact response label", () => {
  const profiles = [profile("primary", "openai")];

  assert.equal(
    credentialProfileIndicatorLabel(
      "GPT-5.6",
      "high",
      true,
      "primary",
      profiles,
    ),
    "GPT-5.6 · high · Fast · openai primary",
  );
  assert.equal(
    credentialProfileIndicatorLabel("GPT-5.6", "high", false, null, profiles),
    "GPT-5.6 · high",
  );
});

test("warns without replacing unavailable pins", () => {
  const profiles = [profile("disabled", "openai", "disabled")];

  assert.equal(
    credentialProfilePinNotice("disabled", profiles),
    "Pinned profile openai disabled is disabled — requests will fail until you change it",
  );
  assert.equal(
    credentialProfilePinNotice("removed-id", profiles),
    "Pinned profile removed-id is removed — requests will fail until you change it",
  );
  assert.equal(credentialProfilePinNotice(null, profiles), null);
});

test("supports agent API credential providers", () => {
  assert.equal(credentialProfileProviderLabel("zai"), "GLM (Z.ai)");
  assert.equal(credentialProfileProviderLabel("xai"), "Grok (xAI)");
  assert.equal(credentialProfileProviderLabel("moonshot"), "Moonshot");
  assert.equal(
    credentialProfileKeyPlaceholder("zai"),
    "GLM Coding Plan API key",
  );
  assert.equal(
    credentialProfileKeyHint("zai"),
    "Paste the API key from your GLM Coding Plan.",
  );
  assert.equal(credentialProfileKeyPlaceholder("xai"), "xAI API key");
  assert.equal(credentialProfileKeyHint("xai"), "Paste your xAI API key.");
  assert.equal(
    credentialProfileKeyPlaceholder("moonshot"),
    "Moonshot API key",
  );
});

test("selects the next provider without a profile after save", () => {
  const profiles = [
    profile("openai", "openai"),
    profile("anthropic", "anthropic"),
  ];
  assert.equal(
    nextMissingCredentialProfileProvider(profiles, "openai"),
    "zai",
  );
  assert.equal(
    nextMissingCredentialProfileProvider(
      [...profiles, profile("zai", "zai")],
      "zai",
    ),
    "xai",
  );
  assert.equal(
    nextMissingCredentialProfileProvider(
      [
        ...profiles,
        profile("zai", "zai"),
        profile("xai", "xai"),
      ],
      "xai",
    ),
    "moonshot",
  );
});
