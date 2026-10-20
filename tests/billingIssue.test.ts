import assert from "node:assert/strict";
import test from "node:test";

import {
  paymentIssueMessage,
  shouldPromptForPaymentIssue,
} from "../src/billingIssue.ts";
import type { Translate } from "../src/i18n/runtime.tsx";

const identityTranslate = ((source: string) => source) as Translate;

test("past-due access prompts once the account status is current", () => {
  assert.equal(shouldPromptForPaymentIssue("past_due", true, false), true);
  assert.equal(shouldPromptForPaymentIssue("past_due", true, true), false);
  assert.equal(shouldPromptForPaymentIssue("unpaid", false, false), false);
  assert.equal(shouldPromptForPaymentIssue(null, true, false), false);
});

test("payment issue copy distinguishes grace from lost access", () => {
  assert.match(
    paymentIssueMessage("past_due", identityTranslate),
    /avoid losing access/,
  );
  assert.match(
    paymentIssueMessage("unpaid", identityTranslate),
    /restore access/,
  );
});
