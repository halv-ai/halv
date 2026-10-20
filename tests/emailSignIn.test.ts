import assert from "node:assert/strict";
import test from "node:test";

import { requestSignInCode, type EmailSignInIpc } from "../src/emailSignIn.ts";
import type { RequestCodeArgs, RequestCodeResult } from "../src/ipc.ts";

type FakeAuth = EmailSignInIpc & {
  requests: RequestCodeArgs[];
  challenges: number;
};

function fakeAuth(
  results: RequestCodeResult[],
  challenge: () => Promise<string> = async () => "turnstile-token",
): FakeAuth {
  const auth: FakeAuth = {
    requests: [],
    challenges: 0,
    async requestCode(args) {
      auth.requests.push(args);
      const result = results.shift();
      assert.ok(result, "unexpected code request");
      return result;
    },
    async emailChallenge() {
      auth.challenges += 1;
      return challenge();
    },
  };
  return auth;
}

test("sends one request without a token when the server needs no challenge", async () => {
  const auth = fakeAuth([{ ok: true, devCode: "424242" }]);
  let challengeShown = false;

  const outcome = await requestSignInCode(auth, "person@example.com", () => {
    challengeShown = true;
  });

  assert.deepEqual(outcome, {
    status: "requested",
    result: { ok: true, devCode: "424242" },
  });
  assert.deepEqual(auth.requests, [{ email: "person@example.com" }]);
  assert.equal(auth.challenges, 0);
  assert.equal(challengeShown, false);
});

test("shows the challenge and retries with its token", async () => {
  const auth = fakeAuth([
    { ok: false, challengeRequired: true },
    { ok: true },
  ]);
  let challengeShown = false;

  const outcome = await requestSignInCode(auth, "person@example.com", () => {
    challengeShown = true;
  });

  assert.deepEqual(outcome, { status: "requested", result: { ok: true } });
  assert.equal(challengeShown, true);
  assert.equal(auth.challenges, 1);
  assert.deepEqual(auth.requests, [
    { email: "person@example.com" },
    { email: "person@example.com", challengeToken: "turnstile-token" },
  ]);
});

test("stops when the challenge window closes without a token", async () => {
  const auth = fakeAuth([{ ok: false, challengeRequired: true }], async () => {
    throw new Error("The security check was cancelled.");
  });

  const outcome = await requestSignInCode(auth, "person@example.com");

  assert.deepEqual(outcome, { status: "challengeIncomplete" });
  assert.equal(auth.requests.length, 1);
});

test("retries one time only when the server refuses the token", async () => {
  const auth = fakeAuth([
    { ok: false, challengeRequired: true },
    { ok: false, challengeRequired: true },
  ]);

  const outcome = await requestSignInCode(auth, "person@example.com");

  assert.deepEqual(outcome, { status: "challengeIncomplete" });
  assert.equal(auth.challenges, 1);
  assert.equal(auth.requests.length, 2);
});

test("passes request errors to the caller", async () => {
  const auth: EmailSignInIpc = {
    async requestCode() {
      throw new Error("Could not request sign-in code: code requested too recently");
    },
    async emailChallenge() {
      assert.fail("no challenge expected");
    },
  };

  await assert.rejects(
    requestSignInCode(auth, "person@example.com"),
    /code requested too recently/,
  );
});
