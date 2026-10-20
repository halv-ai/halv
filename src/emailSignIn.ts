import type { RequestCodeArgs, RequestCodeResult } from "./ipc.ts";

/** The account IPC calls that the email sign-in flow uses. */
export type EmailSignInIpc = {
  requestCode(args: RequestCodeArgs): Promise<RequestCodeResult>;
  emailChallenge(): Promise<string>;
};

export type SignInCodeOutcome =
  | { status: "requested"; result: RequestCodeResult }
  | { status: "challengeIncomplete" };

/**
 * Request an email sign-in code. When the server asks for a Turnstile
 * challenge, show the challenge and send the request again with its token.
 * The flow retries one time only. Errors from the code request go to the
 * caller.
 */
export async function requestSignInCode(
  auth: EmailSignInIpc,
  email: string,
  onChallenge?: () => void,
): Promise<SignInCodeOutcome> {
  const first = await auth.requestCode({ email });
  if (!first.challengeRequired) {
    return { status: "requested", result: first };
  }

  onChallenge?.();
  let challengeToken: string;
  try {
    challengeToken = await auth.emailChallenge();
  } catch {
    return { status: "challengeIncomplete" };
  }

  const second = await auth.requestCode({ email, challengeToken });
  if (second.challengeRequired) {
    return { status: "challengeIncomplete" };
  }
  return { status: "requested", result: second };
}
