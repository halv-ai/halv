import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { matchCommands } from "../src/commandPalette.ts";
import { parseMemoriesCommand } from "../src/memoriesCommand.ts";
import { parseFastCommand } from "../src/fastCommand.ts";

const noop = () => undefined;
const commands = [
  {
    id: "conversation.new",
    name: "clear",
    argsHint: "",
    description: "Start a fresh conversation.",
    category: "Conversation" as const,
    aliases: ["new"],
    handler: noop,
  },
  {
    id: "conversation.search",
    name: "search",
    argsHint: "",
    description: "Focus full-text conversation search.",
    category: "Conversation" as const,
    keywords: ["find", "history"],
    handler: noop,
  },
  {
    id: "workspace.terminal",
    name: "terminal",
    argsHint: "",
    description: "Open the persistent shell.",
    category: "Workspace" as const,
    keywords: ["pty", "console"],
    handler: noop,
  },
];

test("command matching prioritizes exact names and aliases", () => {
  assert.equal(matchCommands(commands, "terminal")[0]?.command.name, "terminal");
  assert.equal(matchCommands(commands, "new")[0]?.command.name, "clear");
});

test("command matching searches descriptions, categories, and keywords", () => {
  assert.equal(matchCommands(commands, "pty")[0]?.command.name, "terminal");
  assert.ok(
    matchCommands(commands, "conversation history").some(
      ({ command }) => command.name === "search",
    ),
  );
});

test("empty command query preserves deterministic registry order", () => {
  assert.deepEqual(
    matchCommands(commands, "").map(({ command }) => command.id),
    commands.map((command) => command.id),
  );
});

test("command registry includes the compact command", () => {
  const source = readFileSync(
    new URL("../src/commands.ts", import.meta.url),
    "utf8",
  );
  assert.match(
    source,
    /id:\s*"conversation\.compact"[\s\S]*?name:\s*"compact"[\s\S]*?category:\s*"Conversation"/,
  );
});

test("command registry includes Codex CLI parity commands and aliases", () => {
  const source = readFileSync(
    new URL("../src/commands.ts", import.meta.url),
    "utf8",
  );
  for (const name of [
    "approve",
    "logout",
    "quit",
    "archive",
    "personality",
    "ponytail",
    "memories",
    "doctor",
  ]) {
    assert.match(
      source,
      new RegExp(`name:\\s*"${name}"[\\s\\S]*?description:\\s*"[^"]+"`),
    );
  }
  assert.match(source, /name:\s*"quit"[\s\S]*?aliases:\s*\["exit"\]/);
  assert.match(
    source,
    /name:\s*"doctor"[\s\S]*?aliases:\s*\["debug-config"\]/,
  );
});

test("command registry opens a shell without a keyboard binding", () => {
  const source = readFileSync(
    new URL("../src/commands.ts", import.meta.url),
    "utf8",
  );
  const entry = source.match(
    /\{\s*id:\s*"workspace\.new_shell"[\s\S]*?\n {2}\},/,
  )?.[0];
  assert.ok(entry, "the /shell command must exist");
  assert.match(entry, /name:\s*"shell"/);
  assert.match(entry, /category:\s*"Workspace"/);
  assert.match(entry, /openShell\(\)/);
  // Cmd+T stays free for terminal tabs.
  assert.doesNotMatch(entry, /shortcut/);
  assert.match(
    source,
    /case "Open a shell beside the active terminal pane\.":\s*return t\("Open a shell beside the active terminal pane\."\);/,
  );
});

test("restart-proxy never starts Halv Engine while the switch is off", () => {
  const source = readFileSync(
    new URL("../src/commands.ts", import.meta.url),
    "utf8",
  );
  const entry = source.match(
    /\{\s*id:\s*"application\.restart-proxy"[\s\S]*?\n {2}\},/,
  )?.[0];
  assert.ok(entry, "the /restart-proxy command must exist");
  assert.match(
    entry,
    /unavailableReason\(context\) \{\s*return context\.halvEngineOff\s*\? "Halv Engine is off\. Turn it on in Settings\."\s*: null;/,
  );
  assert.match(
    source,
    /case "Halv Engine is off\. Turn it on in Settings\.":\s*return t\("Halv Engine is off\. Turn it on in Settings\."\);/,
  );
  // App.tsx keeps the command for an engine error, so Settings and the
  // palette both repair a failed Chat mode start.
  const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
  assert.match(
    app,
    /halvEngineOff: !headroomEnabled && status\.sidecar\?\.state !== "error"/,
  );
});

test("/memories parses empty, on, off, and invalid arguments", async () => {
  assert.deepEqual(parseMemoriesCommand(""), { kind: "open" });
  assert.deepEqual(parseMemoriesCommand("ON"), { kind: "set", mode: "on" });
  assert.deepEqual(parseMemoriesCommand(" off "), {
    kind: "set",
    mode: "off",
  });
  assert.throws(
    () => parseMemoriesCommand("inherit"),
    /Usage: \/memories \[on\|off\]/,
  );
});

test("/fast parses empty, status, on, off, and invalid arguments", () => {
  assert.deepEqual(parseFastCommand(""), { kind: "status" });
  assert.deepEqual(parseFastCommand(" STATUS "), { kind: "status" });
  assert.deepEqual(parseFastCommand("ON"), { kind: "set", enabled: true });
  assert.deepEqual(parseFastCommand(" off "), {
    kind: "set",
    enabled: false,
  });
  assert.throws(
    () => parseFastCommand("toggle"),
    /Usage: \/fast \[on\|off\|status\]/,
  );
});
