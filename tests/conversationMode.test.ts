import assert from "node:assert/strict";
import test from "node:test";
// @ts-expect-error Node's test runner needs the source extension.
import {
  agentKindForBackend,
  agentSessionForConversationMode,
  conversationDisplayTitle,
  conversationNamingTerminal,
  conversationNamingTitle,
  conversationTerminalStatus,
  conversationProModeSwitch,
  modeForComposerDraft,
  modeForConversationSelection,
  modeForNewConversation,
} from "../src/conversationMode.ts";
import type { AgentSessionEntry } from "../src/agentSessions.ts";

function entry(
  id: string,
  mode: "gui" | "terminal_embedded",
): AgentSessionEntry {
  return {
    session: {
      id,
      agentKind: "codex",
      laneKind: "codexChatGpt",
      mode,
      conversationId: "conversation-1",
      cwd: "/tmp/project",
      pid: 1,
      status: "running",
      createdAt: "2026-08-17T12:00:00Z",
      lastActiveAt: "2026-08-17T12:00:00Z",
    },
    transcript: [],
    turns: [],
    totals: {
      inputTokens: 0,
      outputTokens: 0,
      cacheReadTokens: 0,
      cacheCreationTokens: 0,
      totalCostUsd: 0,
      savingsTokens: 0,
    },
    model: null,
    turnSequence: 0,
    turnActive: false,
  };
}

test("stores mode on the conversation while Pro only selects the next default", () => {
  const chat = { mode: "chat" as const };
  const agent = { mode: "agent" as const };

  assert.equal(modeForNewConversation(false), "chat");
  assert.equal(modeForNewConversation(true), "agent");
  assert.equal(modeForConversationSelection(chat), "chat");
  assert.equal(modeForConversationSelection(agent), "agent");
  assert.equal(modeForConversationSelection(undefined), "chat");
});

test("maps the Grok session backend to the Grok agent", () => {
  assert.equal(agentKindForBackend("grok_session"), "grok");
});

test("selection restores the session that matches the conversation mode", () => {
  const gui = entry("gui-session", "gui");
  const terminal = entry("terminal-session", "terminal_embedded");

  assert.equal(
    agentSessionForConversationMode([gui, terminal], "conversation-1", "chat")
      ?.session.id,
    "gui-session",
  );
  assert.equal(
    agentSessionForConversationMode(
      [gui, terminal],
      "conversation-1",
      "agent",
    )?.session.id,
    "terminal-session",
  );
});

test("agent rows use their layered terminal title without changing chat rows", () => {
  assert.equal(
    conversationDisplayTitle(
      {
        mode: "agent",
        title: "Original conversation",
        titleSource: "generated",
      },
      "Release audit",
    ),
    "Release audit",
  );
  assert.equal(
    conversationDisplayTitle(
      {
        mode: "chat",
        title: "Original conversation",
        titleSource: "generated",
      },
      "Release audit",
    ),
    "Original conversation",
  );
});

test("manual conversation title wins over agent title", () => {
  assert.equal(
    conversationDisplayTitle(
      {
        mode: "agent",
        title: "Manual conversation",
        titleSource: "manual",
      },
      "Release audit",
    ),
    "Manual conversation",
  );
});

test("conversation naming terminal prefers titled panes and has a stable fallback", () => {
  const terminals = [
    {
      id: "terminal-3",
      conversationId: "conversation-1",
      customTitle: null,
    },
    {
      id: "terminal-1",
      conversationId: "conversation-1",
      customTitle: "  ",
    },
    {
      id: "terminal-4",
      conversationId: "conversation-1",
      customTitle: "Later title",
    },
    {
      id: "terminal-2",
      conversationId: "conversation-1",
      customTitle: "Release audit",
    },
    {
      id: "terminal-0",
      conversationId: "conversation-2",
      customTitle: "Other conversation",
    },
  ];

  assert.equal(
    conversationNamingTerminal(terminals, "conversation-1")?.id,
    "terminal-2",
  );
  assert.equal(
    conversationNamingTerminal(
      terminals.filter(
        (terminal) => !["terminal-2", "terminal-4"].includes(terminal.id),
      ),
      "conversation-1",
    )?.id,
    "terminal-1",
  );
});

test("conversation terminal status prefers active work and preserves completion", () => {
  const terminals = [
    {
      conversationId: "conversation-1",
      status: "exited" as const,
      agentKind: "claude",
    },
    {
      conversationId: "conversation-1",
      status: "running" as const,
      agentKind: "codex",
    },
    {
      conversationId: "conversation-2",
      status: "failed" as const,
      agentKind: "claude",
    },
  ];

  assert.equal(
    conversationTerminalStatus(terminals, "conversation-1"),
    "running",
  );
  assert.equal(
    conversationTerminalStatus(terminals, "conversation-2"),
    "failed",
  );
  assert.equal(conversationTerminalStatus(terminals, "missing"), null);
});

test("a running plain shell never marks the conversation as working", () => {
  // After /exit the ended agent and its continuation shell share the pane.
  const terminals = [
    {
      conversationId: "conversation-1",
      status: "exited" as const,
      agentKind: "claude",
    },
    {
      conversationId: "conversation-1",
      status: "running" as const,
      agentKind: null,
    },
    {
      conversationId: "conversation-2",
      status: "running" as const,
      agentKind: null,
    },
  ];

  assert.equal(
    conversationTerminalStatus(terminals, "conversation-1"),
    "exited",
  );
  assert.equal(conversationTerminalStatus(terminals, "conversation-2"), null);
});

test("a plain shell names the conversation only with a custom title", () => {
  const agent = {
    id: "0f6d6c1e-agent",
    conversationId: "conversation-1",
    customTitle: null,
    agentKind: "claude",
    title: "Release audit",
  };
  const shell = {
    id: "terminal-shell",
    conversationId: "conversation-1",
    customTitle: null,
    agentKind: null,
    title: "halv",
  };

  assert.equal(
    conversationNamingTitle([shell, agent], "conversation-1"),
    "Release audit",
  );
  // The ended agent terminal is gone after a relaunch. The project name of
  // the shell must not replace the stored conversation title.
  assert.equal(conversationNamingTitle([shell], "conversation-1"), null);
  assert.equal(
    conversationDisplayTitle(
      { mode: "agent", title: "Stored title", titleSource: "generated" },
      conversationNamingTitle([shell], "conversation-1"),
    ),
    "Stored title",
  );
  const renamedShell = { ...shell, customTitle: "Deploy shell", title: "Deploy shell" };
  assert.equal(
    conversationNamingTitle([renamedShell, agent], "conversation-1"),
    "Deploy shell",
  );
  // The sidebar row still reaches the shell through the naming terminal.
  assert.equal(
    conversationNamingTerminal([shell], "conversation-1")?.id,
    "terminal-shell",
  );
});

test("turning Pro on converts only the open chat conversation", () => {
  const chat = { mode: "chat" as const };
  const agent = { mode: "agent" as const };

  assert.equal(conversationProModeSwitch(false, true, chat), "agent");
  assert.equal(conversationProModeSwitch(false, true, agent), null);
  assert.equal(conversationProModeSwitch(true, true, chat), null);
  assert.equal(conversationProModeSwitch(true, false, chat), null);
  assert.equal(conversationProModeSwitch(false, true, null), null);
});

test("a text conversation draft is a chat, also in Pro mode (ADR 0020)", () => {
  assert.equal(modeForComposerDraft(true, true), "chat");
  assert.equal(modeForComposerDraft(false, true), "chat");
  // Without the launcher choice, the Pro toggle decides as before.
  assert.equal(modeForComposerDraft(true, false), "agent");
  assert.equal(modeForComposerDraft(false, false), "chat");
});
