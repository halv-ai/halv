import assert from "node:assert/strict";
import test from "node:test";
import {
  latestUserForkSequence,
  userMessageForkSequence,
} from "../src/forkConversation.ts";

test("latest user fork point uses the persisted user turn before trailing replies", () => {
  assert.equal(
    latestUserForkSequence([
      { role: "user", sequence: 4 },
      { role: "assistant", sequence: 5 },
      { role: "user", sequence: 8 },
      { role: "assistant", sequence: 9 },
    ]),
    8,
  );
});

test("latest user fork point rejects an unpersisted synthetic turn", () => {
  assert.equal(
    latestUserForkSequence([
      { role: "user", sequence: 4 },
      { role: "assistant", sequence: 5 },
      { role: "user" },
    ]),
    null,
  );
});

test("only positive safe integer user sequences are forkable", () => {
  assert.equal(userMessageForkSequence({ role: "assistant", sequence: 3 }), null);
  assert.equal(userMessageForkSequence({ role: "user", sequence: 0 }), null);
  assert.equal(userMessageForkSequence({ role: "user", sequence: 2.5 }), null);
  assert.equal(userMessageForkSequence({ role: "user", sequence: 3 }), 3);
});
