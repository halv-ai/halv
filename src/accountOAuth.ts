export type AccountOAuthProvider = "google" | "apple";

export type AccountOAuthButton = {
  provider: AccountOAuthProvider;
  label: string;
};

const ACCOUNT_OAUTH_BUTTONS: readonly AccountOAuthButton[] = [
  { provider: "google", label: "Continue with Google" },
  { provider: "apple", label: "Continue with Apple" },
];

export function availableAccountOAuthButtons(
  providers: unknown,
): AccountOAuthButton[] {
  if (!Array.isArray(providers)) {
    return [];
  }

  const available = new Set(
    providers.filter(
      (provider): provider is AccountOAuthProvider =>
        provider === "google" || provider === "apple",
    ),
  );

  return ACCOUNT_OAUTH_BUTTONS.filter(({ provider }) =>
    available.has(provider),
  );
}
