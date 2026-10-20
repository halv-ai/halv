import assert from "node:assert/strict";
import test from "node:test";

import {
  conversationHandoffNotice,
  conversationSendRoute,
  shouldAdoptPendingNewTurn,
} from "../src/conversationTurnRouting.ts";
import { NEW_THREAD } from "../src/types.ts";

test("an idle conversation sends while another conversation runs", () => {
  const running = new Set(["conversation-a"]);

  assert.equal(
    conversationSendRoute("conversation-b", running, NEW_THREAD),
    "send",
  );
  assert.equal(
    conversationSendRoute("conversation-a", running, NEW_THREAD),
    "queue",
  );
});

test("a new thread sends unless another new thread is still resolving", () => {
  assert.equal(
    conversationSendRoute(null, new Set(["conversation-a"]), NEW_THREAD),
    "send",
  );
  assert.equal(
    conversationSendRoute(null, new Set([NEW_THREAD]), NEW_THREAD),
    "queue",
  );
});

test("adopts only an unknown conversation without turn state or a running turn", () => {
  assert.equal(
    shouldAdoptPendingNewTurn("candidate", new Set(), new Set(), new Set()),
    true,
  );
  assert.equal(
    shouldAdoptPendingNewTurn(
      "candidate",
      new Set(["candidate"]),
      new Set(),
      new Set(),
    ),
    false,
  );
  assert.equal(
    shouldAdoptPendingNewTurn(
      "candidate",
      new Set(),
      new Set(["candidate"]),
      new Set(),
    ),
    false,
  );
  assert.equal(
    shouldAdoptPendingNewTurn(
      "candidate",
      new Set(),
      new Set(),
      new Set(["candidate"]),
    ),
    false,
  );
});

test("maps conversation surface handoffs to transcript notices", () => {
  assert.equal(
    conversationHandoffNotice("gui", "terminal_embedded"),
    "Continued in terminal — resumed session",
  );
  assert.equal(
    conversationHandoffNotice("terminal_ghostty", "gui"),
    "Continued in chat — resumed session",
  );
  assert.equal(conversationHandoffNotice("gui", "gui"), null);
  assert.equal(
    conversationHandoffNotice("terminal_embedded", "terminal_ghostty"),
    null,
  );
});
