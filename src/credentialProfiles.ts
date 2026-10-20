import type { CredentialProfile } from "./types";

export const CREDENTIAL_PROFILE_PROVIDERS = [
  "openai",
  "anthropic",
  "zai",
  "xai",
  "moonshot",
] as const;

export type CredentialProfileProvider =
  (typeof CREDENTIAL_PROFILE_PROVIDERS)[number];

export function credentialProfileProviderLabel(
  provider: CredentialProfileProvider | string,
) {
  if (provider === "openai") return "OpenAI";
  if (provider === "anthropic") return "Anthropic";
  if (provider === "zai") return "GLM (Z.ai)";
  if (provider === "xai") return "Grok (xAI)";
  if (provider === "moonshot") return "Moonshot";
  return provider;
}

export function credentialProfileKeyPlaceholder(
  provider: CredentialProfileProvider,
) {
  if (provider === "anthropic") return "sk-ant-...";
  if (provider === "zai") return "GLM Coding Plan API key";
  if (provider === "xai") return "xAI API key";
  if (provider === "moonshot") return "Moonshot API key";
  return "sk-...";
}

export function credentialProfileKeyHint(
  provider: CredentialProfileProvider | string,
) {
  if (provider === "zai") {
    return "Paste the API key from your GLM Coding Plan.";
  }
  if (provider === "xai") {
    return "Paste your xAI API key.";
  }
  return null;
}

export function nextMissingCredentialProfileProvider(
  profiles: CredentialProfile[],
  current: CredentialProfileProvider,
): CredentialProfileProvider {
  const configured = new Set(profiles.map((profile) => profile.provider));
  const start = CREDENTIAL_PROFILE_PROVIDERS.indexOf(current);
  for (
    let offset = 1;
    offset <= CREDENTIAL_PROFILE_PROVIDERS.length;
    offset += 1
  ) {
    const provider =
      CREDENTIAL_PROFILE_PROVIDERS[
        (start + offset) % CREDENTIAL_PROFILE_PROVIDERS.length
      ];
    if (!configured.has(provider)) {
      return provider;
    }
  }
  return CREDENTIAL_PROFILE_PROVIDERS[
    (start + 1) % CREDENTIAL_PROFILE_PROVIDERS.length
  ];
}

export function eligibleCredentialProfiles(
  profiles: CredentialProfile[],
  provider: string | undefined,
) {
  if (!provider) {
    return [];
  }
  return profiles.filter(
    (profile) =>
      profile.provider === provider && profile.status === "active",
  );
}

export function credentialProfileIndicatorLabel(
  modelLabel: string,
  effort: string,
  fastMode: boolean,
  credentialProfileId: string | null,
  profiles: CredentialProfile[],
) {
  const base = `${modelLabel} · ${effort}${fastMode ? " · Fast" : ""}`;
  if (!credentialProfileId) {
    return base;
  }
  const profile = profiles.find((item) => item.id === credentialProfileId);
  return `${base} · ${profile?.name ?? "Unknown profile"}`;
}

export function credentialProfilePinNotice(
  credentialProfileId: string | null,
  profiles: CredentialProfile[],
) {
  if (!credentialProfileId) {
    return null;
  }
  const profile = profiles.find((item) => item.id === credentialProfileId);
  if (!profile) {
    return `Pinned profile ${credentialProfileId} is removed — requests will fail until you change it`;
  }
  if (profile.status !== "active") {
    return `Pinned profile ${profile.name} is ${profile.status} — requests will fail until you change it`;
  }
  return null;
}
