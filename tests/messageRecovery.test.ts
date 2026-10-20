import assert from "node:assert/strict";
import test from "node:test";

import {
  PAUSED_RESTARTS_MESSAGE,
  TRANSIENT_PROXY_MESSAGE,
} from "../src/harness/fixtures.ts";
import { isProxyRecoveryError } from "../src/messageRecovery.ts";

test("recognizes current proxy recovery messages", () => {
  assert.equal(
    isProxyRecoveryError(PAUSED_RESTARTS_MESSAGE.toLowerCase(), false),
    true,
  );
  assert.equal(
    isProxyRecoveryError(TRANSIENT_PROXY_MESSAGE.toLowerCase(), false),
    true,
  );
});

test("keeps historical proxy recovery messages and excludes usage limits", () => {
  assert.equal(
    isProxyRecoveryError("timed out waiting for headroom", false),
    true,
  );
  assert.equal(
    isProxyRecoveryError("halv engine exited unexpectedly", false),
    true,
  );
  assert.equal(isProxyRecoveryError("proxy usage limit reached", true), false);
});
