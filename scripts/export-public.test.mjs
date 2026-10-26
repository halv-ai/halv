import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  commitMessage,
  decodeEscapes,
  decodePercent,
  globMatcher,
  pathNameRules,
  scanMessagePaths,
  tokenForms,
} from "./export-public.mjs";

const SCRIPT = resolve(dirname(fileURLToPath(import.meta.url)), "export-public.mjs");
// The test builds the fake token at run time, so this file holds no token.
const FAKE_TOKEN = ["ghp", "A".repeat(36)].join("_");
const PRIVATE_CONFIG = "scripts/public-export.private.json";

function sha256(text) {
  return createHash("sha256").update(text).digest("hex");
}

function baseConfig(extra = {}) {
  return {
    overlayDir: "scripts/public-overlay",
    exclude: ["server/", "/CLAUDE.md", "/docs/private.md"],
    denyPaths: ["server/", "*.pem", ".env*"],
    denyContent: [{ id: "github-token", pattern: "\\bgh[pousr]_[A-Za-z0-9]{36,}" }],
    allowlist: [],
    ...extra,
  };
}

// The token hashes stay in the private config, as in the real repository.
// The export stops when a hash value reaches the target.
function basePrivateConfig(extra = {}) {
  return {
    exclude: [],
    denyTokenHashes: [{ label: "test home", sha256: sha256("home:alice") }],
    ...extra,
  };
}

function git(cwd, ...args) {
  const result = spawnSync("git", args, { cwd, encoding: "utf8" });
  assert.equal(result.status, 0, `git ${args.join(" ")}: ${result.stderr}`);
  return result.stdout.trim();
}

function writeFiles(root, files) {
  for (const [path, content] of Object.entries(files)) {
    const file = join(root, path);
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, content);
  }
}

function initRepository(root) {
  mkdirSync(root, { recursive: true });
  git(root, "init", "-q", "-b", "main");
  git(root, "config", "user.name", "Test");
  git(root, "config", "user.email", "test@example.invalid");
  git(root, "config", "commit.gpgsign", "false");
}

function commitAll(root, message) {
  git(root, "add", "-A");
  git(root, "commit", "-q", "--allow-empty", "-m", message);
  return git(root, "rev-parse", "HEAD");
}

// Makes a source repository, a target repository, and a bare upstream
// repository of the target in one temp folder.
function setup({
  source = {},
  target = { "README.md": "public\n" },
  config = baseConfig(),
  privateConfig = basePrivateConfig(),
} = {}) {
  const root = mkdtempSync(join(tmpdir(), "halv-export-test-"));
  const sourceRoot = join(root, "app");
  const targetRoot = join(root, "oss");
  const remoteRoot = join(root, "public.git");
  initRepository(sourceRoot);
  writeFiles(sourceRoot, {
    "scripts/public-export.json": `${JSON.stringify(config, null, 2)}\n`,
    ...(privateConfig ? { [PRIVATE_CONFIG]: `${JSON.stringify(privateConfig, null, 2)}\n` } : {}),
    "README.md": "public\n",
    ...source,
  });
  const base = commitAll(sourceRoot, "Start the app");
  initRepository(targetRoot);
  writeFiles(targetRoot, target);
  commitAll(targetRoot, "Start the public tree");
  git(root, "init", "-q", "--bare", "-b", "main", remoteRoot);
  git(targetRoot, "remote", "add", "origin", remoteRoot);
  git(targetRoot, "push", "-q", "-u", "origin", "main");
  return {
    root,
    sourceRoot,
    targetRoot,
    remoteRoot,
    base,
    cleanup: () => rmSync(root, { recursive: true, force: true }),
  };
}

// Pushes one commit to the upstream repository from a second clone, as a
// merged pull request does.
function pushUpstreamCommit(context, files, message) {
  const other = join(context.root, "other");
  git(context.root, "clone", "-q", context.remoteRoot, other);
  git(other, "config", "user.name", "Test");
  git(other, "config", "user.email", "test@example.invalid");
  git(other, "config", "commit.gpgsign", "false");
  writeFiles(other, files);
  commitAll(other, message);
  git(other, "push", "-q");
}

function runExport(context, ...args) {
  return runExportWithEnv(context, {}, ...args);
}

function runExportWithEnv(context, env, ...args) {
  const result = spawnSync(
    process.execPath,
    [SCRIPT, "--source", context.sourceRoot, "--target", context.targetRoot, ...args],
    { encoding: "utf8", env: { ...process.env, ...env } },
  );
  return { status: result.status, stdout: result.stdout, stderr: result.stderr, output: result.stdout + result.stderr };
}

function exists(root, path) {
  return existsSync(join(root, path));
}

test("globs match folders, anchored paths, and file names at any depth", () => {
  const matches = globMatcher(["server/", "/CLAUDE.md", "*.pem", "docs/**/*.html", ".env*"]);
  assert.equal(matches("server/src/main.rs"), true);
  assert.equal(matches("src/server/main.rs"), false);
  assert.equal(matches("CLAUDE.md"), true);
  assert.equal(matches("docs/CLAUDE.md"), false);
  assert.equal(matches("keys/dev.pem"), true);
  assert.equal(matches("docs/private.html"), true);
  assert.equal(matches("docs/a/b/private.html"), true);
  assert.equal(matches("src/.env.local"), true);
  assert.equal(matches("src/main.rs"), false);
});

test("exports committed files without excluded paths or uncommitted work", (t) => {
  const context = setup({
    source: {
      "src/a.txt": "committed\n",
      "server/main.rs": "private\n",
      "CLAUDE.md": "private\n",
      "docs/CLAUDE.md": "public\n",
      "docs/private.md": "private\n",
    },
  });
  t.after(context.cleanup);
  writeFileSync(join(context.sourceRoot, "src/a.txt"), "uncommitted\n");
  writeFileSync(join(context.sourceRoot, "src/untracked.txt"), "untracked\n");

  const result = runExport(context, "--accept-new");

  assert.equal(result.status, 0, result.output);
  assert.equal(readFileSync(join(context.targetRoot, "src/a.txt"), "utf8"), "committed\n");
  assert.equal(exists(context.targetRoot, "src/untracked.txt"), false);
  assert.equal(exists(context.targetRoot, "server"), false);
  assert.equal(exists(context.targetRoot, "CLAUDE.md"), false);
  assert.equal(exists(context.targetRoot, "docs/private.md"), false);
  assert.equal(exists(context.targetRoot, "docs/CLAUDE.md"), true);
  assert.equal(exists(context.targetRoot, "scripts/public-export.json"), true);
});

test("maps the overlay folder onto the export root", (t) => {
  const context = setup({
    source: {
      "scripts/public-overlay/.github/dependabot.yml": "version: 2\n",
      "scripts/public-overlay/NOTICE": "public notice\n",
      NOTICE: "private notice\n",
    },
  });
  t.after(context.cleanup);

  const result = runExport(context, "--accept-new");

  assert.equal(result.status, 0, result.output);
  assert.equal(readFileSync(join(context.targetRoot, ".github/dependabot.yml"), "utf8"), "version: 2\n");
  assert.equal(readFileSync(join(context.targetRoot, "NOTICE"), "utf8"), "public notice\n");
  assert.equal(exists(context.targetRoot, "scripts/public-overlay"), false);
  assert.match(result.stdout, /2 from scripts\/public-overlay/);
});

test("fails on a denied path and writes nothing", (t) => {
  const context = setup({ source: { "keys/dev.pem": "not a key\n", "src/a.txt": "a\n" } });
  t.after(context.cleanup);

  const result = runExport(context, "--accept-new");

  assert.equal(result.status, 2, result.output);
  assert.match(result.stderr, /denied path[\s\S]*keys\/dev\.pem/);
  assert.equal(exists(context.targetRoot, "src/a.txt"), false);
});

test("fails on denied content without printing it, and accepts an allowlisted hit", (t) => {
  const context = setup({ source: { "src/fixture.txt": `token = "${FAKE_TOKEN}"\n` } });
  t.after(context.cleanup);

  const denied = runExport(context, "--accept-new");

  assert.equal(denied.status, 2, denied.output);
  assert.match(denied.stderr, /src\/fixture\.txt:1 github-token "ghp_"… \(40 chars\)/);
  assert.equal(denied.output.includes(FAKE_TOKEN), false);
  assert.equal(exists(context.targetRoot, "src/fixture.txt"), false);

  const config = baseConfig({ allowlist: [{ path: "/src/fixture.txt", rule: "github-token" }] });
  writeFiles(context.sourceRoot, { "scripts/public-export.json": JSON.stringify(config) });
  commitAll(context.sourceRoot, "Allow the fixture token");

  const allowed = runExport(context, "--accept-new");

  assert.equal(allowed.status, 0, allowed.output);
  assert.equal(exists(context.targetRoot, "src/fixture.txt"), true);
});

test("fails on a denied home folder name and a denied email address", (t) => {
  const privateConfig = basePrivateConfig({
    denyTokenHashes: [
      { label: "test home", sha256: sha256("home:alice") },
      { label: "test email", sha256: sha256("email:alice@example.org") },
    ],
  });
  const context = setup({
    privateConfig,
    source: {
      "src/paths.txt": "cwd=/Users/Alice/work\nC:\\Users\\alice\\code\n/Users/demo/work\n",
      "src/mail.txt": "contact: Alice@Example.org\n",
    },
  });
  t.after(context.cleanup);

  const result = runExport(context, "--accept-new");

  assert.equal(result.status, 2, result.output);
  assert.match(result.stderr, /src\/paths\.txt:1 private-home test home/);
  assert.match(result.stderr, /src\/paths\.txt:2 private-home test home/);
  assert.doesNotMatch(result.stderr, /src\/paths\.txt:3/);
  assert.match(result.stderr, /src\/mail\.txt:1 private-email test email/);
  assert.equal(result.output.includes("Alice@Example.org"), false);
});

test("requires --accept-new before it adds new paths", (t) => {
  const context = setup({ source: { "docs/new.md": "new\n" } });
  t.after(context.cleanup);

  const refused = runExport(context);

  assert.equal(refused.status, 2, refused.output);
  assert.match(refused.stdout, /Added \(2\):\n  docs\/new\.md\n  scripts\/public-export\.json/);
  assert.match(refused.stderr, /--accept-new/);
  assert.equal(exists(context.targetRoot, "docs/new.md"), false);

  const accepted = runExport(context, "--accept-new");

  assert.equal(accepted.status, 0, accepted.output);
  assert.equal(exists(context.targetRoot, "docs/new.md"), true);
});

test("deletes only stale tracked files", (t) => {
  const context = setup({
    target: { "README.md": "public\n", "stale.txt": "old\n", "old/deep/file.txt": "old\n" },
  });
  t.after(context.cleanup);
  writeFileSync(join(context.targetRoot, "local.txt"), "untracked\n");

  const result = runExport(context, "--accept-new");

  assert.equal(result.status, 0, result.output);
  assert.match(result.stdout, /Deleted \(2\):\n  old\/deep\/file\.txt\n  stale\.txt/);
  assert.equal(exists(context.targetRoot, "stale.txt"), false);
  assert.equal(exists(context.targetRoot, "old"), false);
  assert.equal(exists(context.targetRoot, "local.txt"), true);
  assert.equal(exists(context.targetRoot, ".git"), true);
});

test("commits with a Source-Commit trailer and reads it back", (t) => {
  const context = setup({ source: { "src/a.txt": "a\n" } });
  t.after(context.cleanup);

  const first = runExport(context, "--accept-new", "--commit");
  assert.equal(first.status, 2, first.output);
  assert.match(first.stderr, /--since/);

  writeFiles(context.sourceRoot, { "src/a.txt": "a2\n" });
  const second = commitAll(context.sourceRoot, "Change a");
  writeFiles(context.sourceRoot, { "src/b.txt": "b\n" });
  const third = commitAll(context.sourceRoot, "Add b");

  const initial = runExport(context, "--accept-new", "--commit", "--since", context.base);
  assert.equal(initial.status, 0, initial.output);
  assert.equal(
    git(context.targetRoot, "log", "-1", "--format=%an <%ae>|%cn <%ce>"),
    "pedr0v <34296734+pedr0v@users.noreply.github.com>|pedr0v <34296734+pedr0v@users.noreply.github.com>",
  );
  assert.equal(
    git(context.targetRoot, "log", "-1", "--format=%B"),
    `Export 2 commits from the private repository\n\n- Change a\n- Add b\n\nSource-Commit: ${third}`,
  );
  assert.notEqual(second, third);
  assert.equal(git(context.targetRoot, "status", "--porcelain"), "");

  writeFiles(context.sourceRoot, { "src/c.txt": "c\n" });
  const fourth = commitAll(context.sourceRoot, "Add c");

  const next = runExport(context, "--accept-new", "--commit");
  assert.equal(next.status, 0, next.output);
  assert.equal(git(context.targetRoot, "log", "-1", "--format=%B"), `Add c\n\nSource-Commit: ${fourth}`);

  const check = runExport(context, "--check");
  assert.equal(check.status, 0, check.output);
  assert.match(check.stdout, /The target is current/);

  const again = runExport(context, "--commit");
  assert.equal(again.status, 0, again.output);
  assert.equal(git(context.targetRoot, "rev-list", "--count", "HEAD"), "3");
});

test("--check exits 1 when the target differs", (t) => {
  const context = setup({ source: { "src/a.txt": "a\n" } });
  t.after(context.cleanup);
  assert.equal(runExport(context, "--accept-new").status, 0);
  commitAll(context.targetRoot, "Copy the export");
  assert.equal(runExport(context, "--check").status, 0);

  writeFiles(context.targetRoot, { "src/a.txt": "edited\n" });
  commitAll(context.targetRoot, "Edit a in the public tree");

  const result = runExport(context, "--check");
  assert.equal(result.status, 1, result.output);
  assert.match(result.stdout, /Changed \(1\):\n  src\/a\.txt/);
});

test("stops when the target has commits after the last export", (t) => {
  const context = setup({ source: { "src/a.txt": "a\n" } });
  t.after(context.cleanup);
  assert.equal(runExport(context, "--accept-new", "--commit", "--since", context.base).status, 0);
  writeFiles(context.targetRoot, { "src/a.txt": "merged pull request\n" });
  commitAll(context.targetRoot, "Merge a pull request");

  const stopped = runExport(context);
  assert.equal(stopped.status, 2, stopped.output);
  assert.match(stopped.stderr, /1 commit\(s\) after the last export[\s\S]*Merge a pull request/);
  assert.equal(readFileSync(join(context.targetRoot, "src/a.txt"), "utf8"), "merged pull request\n");

  const allowed = runExport(context, "--allow-public-commits");
  assert.equal(allowed.status, 0, allowed.output);
  assert.equal(readFileSync(join(context.targetRoot, "src/a.txt"), "utf8"), "a\n");
});

test("refuses to write into a target with uncommitted changes", (t) => {
  const context = setup({ source: { "src/a.txt": "a\n" } });
  t.after(context.cleanup);
  writeFiles(context.targetRoot, { "README.md": "local edit\n" });

  const result = runExport(context, "--accept-new");

  assert.equal(result.status, 2, result.output);
  assert.match(result.stderr, /uncommitted changes/);
  assert.equal(exists(context.targetRoot, "src/a.txt"), false);
});

test("merges the private config and never exports it", (t) => {
  const config = baseConfig({ exclude: ["server/"], denyTokenHashes: [], requirePrivateConfig: true });
  const privateConfig = {
    $comment: "test",
    exclude: ["/docs/private.md"],
    denyTokenHashes: [{ label: "test email", sha256: sha256("email:alice@example.org") }],
  };
  const context = setup({
    config,
    source: {
      [PRIVATE_CONFIG]: JSON.stringify(privateConfig),
      [`scripts/public-overlay/${PRIVATE_CONFIG}`]: "{}\n",
      "docs/private.md": "private\n",
      "docs/public.md": "public\n",
    },
    target: { "README.md": "public\n", [PRIVATE_CONFIG]: "{}\n" },
  });
  t.after(context.cleanup);

  const result = runExport(context, "--accept-new");

  assert.equal(result.status, 0, result.output);
  assert.match(result.stdout, /Config: scripts\/public-export\.json \+ scripts\/public-export\.private\.json/);
  assert.match(result.stdout, /0 from scripts\/public-overlay/);
  assert.match(result.stdout, /Deleted \(1\):\n  scripts\/public-export\.private\.json/);
  assert.equal(exists(context.targetRoot, PRIVATE_CONFIG), false);
  assert.equal(exists(context.targetRoot, "docs/private.md"), false);
  assert.equal(exists(context.targetRoot, "docs/public.md"), true);
  const publicConfig = readFileSync(join(context.targetRoot, "scripts/public-export.json"), "utf8");
  assert.equal(publicConfig.includes("docs/private.md"), false);
  assert.equal(publicConfig.includes(sha256("email:alice@example.org")), false);

  // The token hash of the private config applies too.
  writeFiles(context.sourceRoot, { "docs/public.md": "contact: alice@example.org\n" });
  commitAll(context.sourceRoot, "Add a contact");

  const denied = runExport(context, "--accept-new");

  assert.equal(denied.status, 2, denied.output);
  assert.match(denied.stderr, /docs\/public\.md:1 private-email test email/);
  assert.equal(denied.output.includes("alice@example.org"), false);
});

test("stops when the public config requires a private config that is missing or wrong", (t) => {
  const context = setup({ config: baseConfig({ requirePrivateConfig: true }), privateConfig: null });
  t.after(context.cleanup);

  const missing = runExport(context, "--accept-new");

  assert.equal(missing.status, 2, missing.output);
  assert.match(missing.stderr, /public-export\.private\.json does not exist/);

  writeFiles(context.sourceRoot, { [PRIVATE_CONFIG]: JSON.stringify({ target: "../elsewhere" }) });
  commitAll(context.sourceRoot, "Add a wrong private config");

  const wrong = runExport(context, "--accept-new");

  assert.equal(wrong.status, 2, wrong.output);
  assert.match(wrong.stderr, /can only hold[\s\S]*Move target/);

  // An empty or partial private config must not turn off the private rules.
  for (const partial of [{}, { exclude: ["/docs/private.md"] }, { exclude: [], denyTokenHashes: [] }]) {
    writeFiles(context.sourceRoot, { [PRIVATE_CONFIG]: JSON.stringify(partial) });
    commitAll(context.sourceRoot, "Add a partial private config");

    const empty = runExport(context, "--accept-new");

    assert.equal(empty.status, 2, empty.output);
    assert.match(empty.stderr, /needs a non-empty "exclude" list and a non-empty "denyTokenHashes" list/);
  }
  assert.equal(exists(context.targetRoot, "scripts/public-export.json"), false);
});

test("decodes percent-encoded text and survives malformed sequences", () => {
  assert.equal(decodePercent("%2FUsers%2Fdemo"), "/Users/demo");
  assert.equal(decodePercent("%252FUsers%252Fdemo"), "/Users/demo");
  assert.equal(decodePercent("100% of %2Fhome%2Fdemo"), "100% of /home/demo");
  assert.equal(decodePercent("%E0%A4%A"), "%E0%A4%A");
  assert.equal(decodePercent("plain"), "plain");
});

test("fails on URL-encoded home folder names and email addresses", (t) => {
  const privateConfig = basePrivateConfig({
    denyTokenHashes: [
      { label: "test home", sha256: sha256("home:alice") },
      { label: "test email", sha256: sha256("email:alice@example.org") },
    ],
  });
  const context = setup({
    privateConfig,
    source: {
      "src/encoded.txt": [
        "file%3A%2F%2F%2FUsers%2Falice%2Fwork",
        "C%3A%5CUsers%5Calice%5Ccode",
        "100% sure: %2Fhome%2Falice",
        "mailto:alice%40example.org",
        "%252FUsers%252FAlice",
        "%2FUsers%2Fdemo",
        "/Users/alice%2Fwork",
      ].join("\n"),
    },
  });
  t.after(context.cleanup);

  const result = runExport(context, "--accept-new");

  assert.equal(result.status, 2, result.output);
  for (const line of [1, 2, 3, 5]) {
    assert.match(result.stderr, new RegExp(`src/encoded\\.txt:${line} private-home test home, encoded`));
  }
  assert.match(result.stderr, /src\/encoded\.txt:4 private-email test email, encoded/);
  assert.doesNotMatch(result.stderr, /src\/encoded\.txt:6 /);
  // The raw scan finds line 7. The decoded scan does not report it again.
  assert.equal(result.stderr.split("src/encoded.txt:7 ").length - 1, 1);
  assert.equal(result.output.includes("alice@example.org"), false);
  assert.equal(exists(context.targetRoot, "src/encoded.txt"), false);
});

test("stops when the target is behind its upstream branch, unless --offline", (t) => {
  const context = setup({ source: { "src/a.txt": "a\n" } });
  t.after(context.cleanup);
  pushUpstreamCommit(context, { "src/a.txt": "public edit\n" }, "Merge a pull request on GitHub");

  const stopped = runExport(context, "--accept-new");

  assert.equal(stopped.status, 2, stopped.output);
  assert.match(stopped.stdout, /Upstream: origin\/main, 1 behind, 0 ahead/);
  assert.match(stopped.stderr, /1 commit\(s\) behind origin\/main[\s\S]*pull --ff-only/);
  assert.equal(exists(context.targetRoot, "src/a.txt"), false);

  const dryRun = runExport(context, "--dry-run");
  assert.equal(dryRun.status, 0, dryRun.output);
  assert.match(dryRun.stderr, /WARNING: The target is 1 commit\(s\) behind/);

  const offline = runExport(context, "--check", "--offline");
  assert.equal(offline.status, 1, offline.output);
  assert.match(offline.stdout, /Upstream: not checked \(--offline\)/);

  git(context.targetRoot, "pull", "-q", "--ff-only");
  const current = runExport(context, "--accept-new");
  assert.equal(current.status, 0, current.output);
  assert.match(current.stdout, /Upstream: origin\/main, 0 behind, 0 ahead/);
  assert.equal(readFileSync(join(context.targetRoot, "src/a.txt"), "utf8"), "a\n");
});

test("stops without an upstream branch or a working fetch, unless --offline", (t) => {
  const context = setup({ source: { "src/a.txt": "a\n" } });
  t.after(context.cleanup);

  rmSync(context.remoteRoot, { recursive: true, force: true });
  const unreachable = runExport(context, "--accept-new");
  assert.equal(unreachable.status, 2, unreachable.output);
  assert.match(unreachable.stderr, /git fetch[\s\S]*--offline/);

  git(context.targetRoot, "branch", "--unset-upstream");
  const noUpstream = runExport(context, "--accept-new");
  assert.equal(noUpstream.status, 2, noUpstream.output);
  assert.match(noUpstream.stderr, /no upstream branch[\s\S]*--offline/);
  assert.equal(exists(context.targetRoot, "src/a.txt"), false);

  const offline = runExport(context, "--accept-new", "--offline");
  assert.equal(offline.status, 0, offline.output);
  assert.equal(exists(context.targetRoot, "src/a.txt"), true);
});

test("decodes escapes and makes the token forms", () => {
  assert.equal(decodeEscapes("&#x2F;Users&#47;demo&sol;x"), "/Users/demo/x");
  assert.equal(decodeEscapes("\\u002FUsers\\x2Fdemo"), "/Users/demo");
  assert.equal(decodeEscapes("demo&#64;example.org %2Fhome"), "demo@example.org /home");
  assert.equal(decodeEscapes("&amp;#x2F;x"), "/x");
  assert.equal(decodeEscapes("a && b &unknown; &#xFFFFFFF;"), "a && b &unknown; &#xFFFFFFF;");
  assert.deepEqual(tokenForms("home", "demo."), ["demo.", "demo"]);
  assert.deepEqual(tokenForms("home", "demo-old.x"), ["demo-old.x", "demo", "demo-old"]);
  assert.deepEqual(tokenForms("email", "demo+git@example.org"), ["demo+git@example.org", "demo@example.org"]);
});

test("fails on home folder names in other forms and on plus addresses", (t) => {
  const privateConfig = basePrivateConfig({
    denyTokenHashes: [
      { label: "test home", sha256: sha256("home:alice") },
      { label: "test email", sha256: sha256("email:alice@example.org") },
    ],
  });
  const context = setup({
    privateConfig,
    source: {
      "src/forms.txt": [
        "The plan is in /Users/alice.",
        '{"cwd":"\\/Users\\/alice\\/work"}',
        "~/.claude/projects/-Users-alice-work/session.jsonl",
        "/Users/alice-old/notes",
        "&#x2F;Users&#x2F;alice",
        "\\u002FUsers\\u002Falice",
        "alice+git@example.org",
        "/Users/alicex/work",
        "/Users/demo/alice",
      ].join("\n"),
    },
  });
  t.after(context.cleanup);

  const result = runExport(context, "--accept-new");

  assert.equal(result.status, 2, result.output);
  for (const line of [1, 2, 3, 4, 5, 6]) {
    assert.match(result.stderr, new RegExp(`src/forms\\.txt:${line} private-home test home`));
  }
  assert.match(result.stderr, /src\/forms\.txt:7 private-email test email/);
  assert.doesNotMatch(result.stderr, /src\/forms\.txt:[89] /);
  assert.equal(result.output.includes("alice+git@example.org"), false);
  assert.equal(exists(context.targetRoot, "src/forms.txt"), false);
});

test("fails on a copy of a token hash at another path", (t) => {
  const context = setup({
    source: { "docs/backup.json": `${JSON.stringify(basePrivateConfig(), null, 2)}\n` },
  });
  t.after(context.cleanup);

  const result = runExport(context, "--accept-new");

  assert.equal(result.status, 2, result.output);
  assert.match(result.stderr, /docs\/backup\.json:\d+ private-token-hash hash of test home, "[0-9a-f]{4}"… \(64 chars\)/);
  assert.equal(result.output.includes(sha256("home:alice")), false);
  assert.equal(exists(context.targetRoot, "docs/backup.json"), false);
});

test("lists only subjects of commits that change exported paths, and stops on private names", (t) => {
  const context = setup({ source: { "src/a.txt": "a\n" } });
  t.after(context.cleanup);
  assert.equal(runExport(context, "--accept-new", "--commit", "--since", context.base).status, 0);

  writeFiles(context.sourceRoot, { "server/auth.rs": "fixed\n" });
  commitAll(context.sourceRoot, "Fix the auth hole in server/src/auth.rs; see docs/private.md");
  writeFiles(context.sourceRoot, { [PRIVATE_CONFIG]: JSON.stringify(basePrivateConfig({ exclude: ["/x"] })) });
  commitAll(context.sourceRoot, "Tune the private rules");
  writeFiles(context.sourceRoot, { "src/server/helper.txt": "public\n" });
  commitAll(context.sourceRoot, "Rename the src/server/ helpers");

  const preview = runExport(context, "--accept-new", "--dry-run");

  assert.equal(preview.status, 0, preview.output);
  assert.match(preview.stdout, /Commit message:\n {2}Rename the src\/server\/ helpers\n/);
  assert.doesNotMatch(preview.output, /auth hole|private rules/);

  writeFiles(context.sourceRoot, { "src/a.txt": "b\n" });
  commitAll(context.sourceRoot, "Fix tab order and update private.md");
  const head = git(context.targetRoot, "rev-parse", "HEAD");

  const stopped = runExport(context, "--accept-new", "--commit");

  assert.equal(stopped.status, 2, stopped.output);
  assert.match(
    stopped.stderr,
    /<commit message>:4 private-path-name exclude entry 2 of scripts\/public-export\.json, "priv"… \(10 chars\)/,
  );
  assert.match(stopped.stderr, /--omit-subjects/);
  assert.equal(git(context.targetRoot, "rev-parse", "HEAD"), head);

  const source = git(context.sourceRoot, "rev-parse", "HEAD");
  const omitted = runExport(context, "--accept-new", "--commit", "--omit-subjects", "--message", "Release 1.0.2");

  assert.equal(omitted.status, 0, omitted.output);
  assert.equal(git(context.targetRoot, "log", "-1", "--format=%B"), `Release 1.0.2\n\nSource-Commit: ${source}`);

  // A home folder name at the end of a subject also stops the export.
  writeFiles(context.sourceRoot, { "src/a.txt": "c\n" });
  commitAll(context.sourceRoot, "Add a demo note. The plan is in /Users/alice.");

  const home = runExport(context, "--dry-run");

  assert.equal(home.status, 2, home.output);
  assert.match(home.stderr, /<commit message>:1 private-home test home/);
  assert.equal(home.stderr.includes("/Users/alice"), false);
});

test("stops when the fetch does not finish in time", (t) => {
  const context = setup({ source: { "src/a.txt": "a\n" } });
  t.after(context.cleanup);
  git(context.targetRoot, "config", "remote.origin.uploadpack", "sleep 5; git-upload-pack");

  const slow = runExportWithEnv(context, { HALV_EXPORT_FETCH_TIMEOUT_MS: "500" }, "--accept-new");

  assert.equal(slow.status, 2, slow.output);
  assert.match(slow.stderr, /git fetch did not finish in 0\.5 s[\s\S]*--offline/);
  assert.equal(exists(context.targetRoot, "src/a.txt"), false);
});

test("builds the commit message from the source subjects", () => {
  const commit = "a".repeat(40);
  assert.equal(commitMessage({ subjects: ["Fix the tab order"], commit }), `Fix the tab order\n\nSource-Commit: ${commit}\n`);
  assert.equal(
    commitMessage({ subjects: ["One", "Two"], commit, subject: "Release 1.0.2" }),
    `Release 1.0.2\n\n- One\n- Two\n\nSource-Commit: ${commit}\n`,
  );
  assert.equal(
    commitMessage({ subjects: [], commit }),
    `Export ${commit.slice(0, 12)} from the private repository again\n\nSource-Commit: ${commit}\n`,
  );
});

test("finds a private path name after a leading slash, dot slash, or dot dot slash", () => {
  const config = {
    pathNames: pathNameRules([
      { value: "/NOTES.md", index: 0, file: "private.json", key: "exclude" },
      { value: "server/", index: 1, file: "private.json", key: "exclude" },
    ]),
  };
  for (const message of ["Edit NOTES.md", "Edit /NOTES.md", "Edit ./NOTES.md", "Edit ../NOTES.md", "Move server/ code"]) {
    assert.equal(scanMessagePaths(config, message).length, 1, message);
  }
  for (const message of ["Edit src/server/main.rs", "Edit docs/NOTES.md.bak2", "Edit MYNOTES.md"]) {
    assert.equal(scanMessagePaths(config, message).length, 0, message);
  }
});
