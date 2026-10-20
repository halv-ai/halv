import assert from "node:assert/strict";
import test from "node:test";
import {
  approvalPolicyForMode,
  chatModeFor,
  chatModeLabel,
  effectivePolicy,
  safeGuidedTourChatMode,
} from "../src/modes.ts";

test("maps the user-facing modes onto approval contracts", () => {
  assert.equal(chatModeFor(false, "skip_all"), "chat");
  assert.equal(chatModeFor(true, "on_request"), "sandbox");
  assert.equal(chatModeFor(true, "on_failure"), "on_failure");
  assert.equal(chatModeFor(true, "ask"), "ask");
  assert.equal(chatModeFor(true, "skip_all"), "full_access");

  assert.equal(approvalPolicyForMode("sandbox"), "on_request");
  assert.equal(approvalPolicyForMode("on_failure"), "on_failure");
  assert.equal(approvalPolicyForMode("ask"), "ask");
  assert.equal(approvalPolicyForMode("full_access"), "skip_all");
});

test("does not silently broaden legacy restricted policies", () => {
  assert.equal(chatModeFor(true, "read_only"), "legacy_restricted");
  assert.equal(chatModeFor(true, "never"), "legacy_restricted");
  assert.equal(chatModeLabel("legacy_restricted"), "Restricted (legacy)");
});

test("labels sandbox mode as Auto", () => {
  assert.equal(chatModeLabel("sandbox"), "Auto");
});

test("keeps guided tour agent mode within approval-prompt policies", () => {
  assert.equal(safeGuidedTourChatMode("on_request"), "sandbox");
  assert.equal(safeGuidedTourChatMode("on_failure"), "on_failure");
  assert.equal(safeGuidedTourChatMode("ask"), "ask");
  assert.equal(safeGuidedTourChatMode("skip_all"), "sandbox");
  assert.equal(safeGuidedTourChatMode("read_only"), "sandbox");
  assert.equal(safeGuidedTourChatMode("never"), "sandbox");

  for (const policy of [
    "on_request",
    "on_failure",
    "ask",
    "skip_all",
    "read_only",
    "never",
  ] as const) {
    const mode = safeGuidedTourChatMode(policy);
    const chosenPolicy = approvalPolicyForMode(mode);
    assert.notEqual(chosenPolicy, "never");
    assert.notEqual(chosenPolicy, "skip_all");
  }
});

test("defaults to Auto and caps unsafe global defaults for untrusted projects", () => {
  assert.equal(effectivePolicy(null, "skip_all", "untrusted"), "on_request");
  assert.equal(effectivePolicy(null, "never", null), "on_request");
  assert.equal(effectivePolicy(null, "skip_all", "trusted"), "skip_all");
  assert.equal(effectivePolicy(null, null, "trusted"), "on_request");
  assert.equal(effectivePolicy(null, null, "untrusted"), "on_request");
  assert.equal(effectivePolicy(null, "ask", "untrusted"), "ask");
});

test("keeps stored approval policies above trust and global defaults", () => {
  assert.equal(effectivePolicy("read_only", "skip_all", "trusted"), "read_only");
  assert.equal(effectivePolicy("ask", "never", "untrusted"), "ask");
  assert.equal(effectivePolicy("on_request", "ask", null), "on_request");
});
