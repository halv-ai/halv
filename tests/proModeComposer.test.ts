import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

test("places the composer Pro toggle immediately before Commands", () => {
  const source = readFileSync(
    new URL("../src/App.tsx", import.meta.url),
    "utf8",
  );
  const composerToggle = source.indexOf("<AgentSessionProModeToggle");
  const commands = source.indexOf(">\n              Commands", composerToggle);

  assert.notEqual(composerToggle, -1);
  assert.notEqual(commands, -1);
  assert.match(
    source.slice(composerToggle, commands),
    /ghostty=\{probeForGhostty\(connectGateProbe\)\}/,
  );
  assert.match(source, /await agentSessionStore\.kill\(entry\.session\.id\)/);
  assert.match(source, /await agentSessionStore\.create\(\{/);
  assert.match(source, /showProTerminalSurface[\s\S]*?<TerminalPanel/);
  assert.doesNotMatch(source, /<SessionSavingsMeter/);
  assert.match(
    source,
    /sessionBacked=\{isSessionBackend\([\s\S]*?currentConversation\?\.agentBackend,[\s\S]*?\)\}/,
  );
  assert.match(source, /className="conversation-mode-stack"/);
  assert.match(
    source,
    /terminalAgentSavingsTokens[\s\S]*?refreshSavingsTotals\(\)/,
  );
});

test("uses one Pro mode store in the composer and panel", () => {
  const toggle = readFileSync(
    new URL("../src/components/AgentSessionProModeToggle.tsx", import.meta.url),
    "utf8",
  );
  const panel = readFileSync(
    new URL("../src/components/AgentSessionsPanel.tsx", import.meta.url),
    "utf8",
  );

  assert.match(toggle, /agentSessionProModeStore\.toggle\(\)/);
  assert.match(panel, /agentSessionProModeStore\.getSnapshot/);
  assert.doesNotMatch(panel, /useState\(readAgentSessionProMode\)/);
});

test("keeps the prepared tour and Pro default synchronized", () => {
  const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
  const firstRunCard = readFileSync(
    new URL("../src/components/FirstRunCard.tsx", import.meta.url),
    "utf8",
  );

  assert.match(
    app,
    /modeForNewConversation\(agentSessionProModeStore\.getSnapshot\(\)\) ===[\s\S]*?"agent"/,
  );
  assert.match(
    app,
    /const \[draftAgentMode, setDraftAgentMode\] = useState\(agentModeForNewDraft\)/,
  );
  assert.ok(
    (app.match(/setDraftAgentMode\(agentModeForNewDraft\(\)\)/g) ?? [])
      .length >= 4,
  );
  assert.match(
    app,
    /!changed &&[\s\S]*?agentSessionProModeStore\.getSnapshot\(\) === enabled[\s\S]*?agentSessionProModeStore\.set\(effectiveAgentMode\)/,
  );
  assert.match(
    app,
    /currentConversation\?\.draftOnly[\s\S]*?conversationProModeSwitch/,
  );
  assert.match(
    app,
    /const requestedConversationMode = modeForNewConversation\(enabled\)[\s\S]*?ipc\.storage\.setConversationMode[\s\S]*?mode: requestedConversationMode/,
  );
  assert.match(app, /syncPreparedTaskMode\(proMode\)/);
  assert.match(app, /<PreparedFirstTask[\s\S]*?error=\{chatError\b/);
  assert.match(firstRunCard, /role="alert"/);
});

test("falls back to Chat for a project-free Pro draft", () => {
  const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");

  assert.match(app, /modeForComposerDraft\(proMode && hasProject, textDraftRef\.current\)/);
  assert.match(app, /agentMode: draftAgentMode && hasProject/);
  assert.match(
    app,
    /const targetsProjectFreeDraft =[\s\S]*?requestProjectId === null[\s\S]*?startingConversation\.draftOnly/,
  );
  assert.match(
    app,
    /const requestConversationMode = targetsProjectFreeDraft[\s\S]*?\? "chat"/,
  );
  assert.match(
    app,
    /const requestAgentMode =[\s\S]*?!targetsProjectFreeDraft &&/,
  );
});

test("binds every guided tour project pick before draft creation", () => {
  const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
  const createDraft = app.slice(
    app.indexOf("  function createConversationDraft"),
    app.indexOf("  async function flushConversationDraft"),
  );
  const updateProject = app.slice(
    app.indexOf("  function updateSelectedProjectId"),
    app.indexOf("  async function loadProjects"),
  );

  assert.match(
    createDraft,
    /const projectId = selectedProjectIdRef\.current;/,
  );
  assert.doesNotMatch(createDraft, /\bselectedProjectId\b/);
  assert.match(createDraft, /projectId,/);
  assert.match(
    app,
    /const selectedProjectIdRef = useRef\(selectedProjectId\);[\s\S]*?useEffect\(\(\) => \{[\s\S]*?selectedProjectIdRef\.current = selectedProjectId;[\s\S]*?\}, \[selectedProjectId\]\);/,
  );
  assert.match(
    updateProject,
    /selectedProjectIdRef\.current = projectId;[\s\S]*?setSelectedProjectId\(projectId\);/,
  );

  const paths = [
    ["handleSelectProject", "handleSwitchProjectByFragment"],
    ["handleAddFolder", "handleCloneGithub"],
    ["handleCloneGithub", "handleRemoveProject"],
  ] as const;
  for (const [start, end] of paths) {
    const path = app.slice(
      app.indexOf(`  async function ${start}`),
      app.indexOf(`  async function ${end}`),
    );
    assert.match(path, /updateSelectedProjectId\(project\.id\)/);
    assert.match(
      path,
      /updateSelectedProjectId\(project\.id\)[\s\S]*?prepareGuidedFirstTask\([^)]*\)/,
    );
  }
});

test("presents Pro mode as a terminal-first flow", () => {
  const toggle = readFileSync(
    new URL("../src/components/AgentSessionProModeToggle.tsx", import.meta.url),
    "utf8",
  );
  const panel = readFileSync(
    new URL("../src/components/AgentSessionsPanel.tsx", import.meta.url),
    "utf8",
  );

  assert.match(panel, /"Open agent terminal"/);
  assert.match(panel, /<summary>\{t\("Advanced options"\)\}<\/summary>/);
  assert.match(
    panel,
    /Agents run in real terminals — savings still measured\./,
  );
  assert.match(panel, /agentKinds\.map\(\(kind\) =>/);
  assert.match(toggle, /claimAgentSessionProModeHint\(\)/);
  assert.match(
    toggle,
    /Pro mode is on: conversations open in a real terminal running the[\s\S]*agent CLI, with live token savings\./,
  );
});

test("shows handoff context and terminal identity details", () => {
  const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");

  assert.match(app, /appendNotice\(currentConversation\.id, notice\)/);
  assert.match(app, /conversation-terminal-handoff-notice/);
  assert.match(
    app,
    /focusSessionId=\{entry\?\.session\.id \?\? null\}/,
  );
});
