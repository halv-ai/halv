#!/usr/bin/env node
// Exports the public part of one committed revision of the private Halv
// repository into a checkout of the public repository (halv-ai/halv).
//
// The script reads the source only through git, so uncommitted work never
// leaks. It never pushes. RELEASING.md describes the procedure.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  readlinkSync,
  realpathSync,
  rmSync,
  rmdirSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, resolve, sep } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const SCRIPT_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const CONFIG_PATH = "scripts/public-export.json";
// The private config holds the private rules: the exact private paths and the
// hashes of personal identifiers. The export merges it and never copies it.
const PRIVATE_CONFIG_PATH = "scripts/public-export.private.json";
const PRIVATE_CONFIG_KEYS = ["exclude", "denyPaths", "denyContent", "denyTokenHashes", "allowlist"];
const TRAILER = "Source-Commit";
const COMMIT_NAME = "pedr0v";
const COMMIT_EMAIL = "34296734+pedr0v@users.noreply.github.com";
// A fetch that does not finish in this time stops the export. The tests set
// HALV_EXPORT_FETCH_TIMEOUT_MS to a short value.
const FETCH_TIMEOUT_MS = Number(process.env.HALV_EXPORT_FETCH_TIMEOUT_MS) || 60_000;

const HELP = `Usage: node scripts/export-public.mjs [options]

Copy the public part of a committed revision of this repository into a
checkout of the public repository. ${CONFIG_PATH} (read from the revision)
gives the excluded paths, the overlay folder, and the leak guard rules.
${PRIVATE_CONFIG_PATH} (read from the revision when it exists)
adds the private rules. The export never copies the private config.

Options:
  --source <dir>       The private repository (default: this repository).
  --rev <rev>          The revision to export (default: HEAD).
  --target <dir>       The public checkout (default: "target" in the config,
                       relative to the source).
  --check              Write nothing. Exit 1 when the target differs.
  --dry-run            Write nothing. Show the plan.
  --accept-new         Allow the export to add paths that the target does not
                       have yet. Read the list first.
  --allow-public-commits
                       Write even when the target has commits after the last
                       export. Apply those commits to this repository first,
                       or the export reverts them.
  --commit             Commit the result in the target, with a
                       "${TRAILER}: <sha>" trailer. The script never pushes.
  --offline            Do not run "git fetch" in the target. Without this
                       option, the script stops when the target is behind
                       its upstream branch.
  --since <sha>        The source commit of the last export. The default is
                       the newest "${TRAILER}" trailer in the target.
  --message <subject>  The subject of the export commit.
  --omit-subjects      Do not list the source commit subjects in the commit
                       message. Use it when the leak guard stops on a subject.
  --hash-token <token> Print the SHA-256 of a token for denyTokenHashes in
                       ${PRIVATE_CONFIG_PATH}, for example
                       "home:<name>" or "email:<address>".
  --help               Show this help.

Exit status: 0 when the export is done or the target is current, 1 when
--check finds a difference, 2 for an error, a leak, a target that is behind
its upstream branch, or new paths without --accept-new.`;

class ExportError extends Error {}

function fail(message) {
  throw new ExportError(message);
}

// ---------------------------------------------------------------------------
// Arguments

function parseArguments(argv) {
  const options = {
    source: null,
    rev: "HEAD",
    target: null,
    check: false,
    dryRun: false,
    acceptNew: false,
    allowPublicCommits: false,
    commit: false,
    offline: false,
    omitSubjects: false,
    since: null,
    message: null,
    hashToken: null,
    help: false,
  };
  const valueOptions = {
    "--source": "source",
    "--rev": "rev",
    "--target": "target",
    "--since": "since",
    "--message": "message",
    "--hash-token": "hashToken",
  };
  const flagOptions = {
    "--check": "check",
    "--dry-run": "dryRun",
    "--accept-new": "acceptNew",
    "--allow-public-commits": "allowPublicCommits",
    "--commit": "commit",
    "--offline": "offline",
    "--omit-subjects": "omitSubjects",
    "--help": "help",
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    // pnpm can pass a literal "--" before the script arguments.
    if (argument === "--") {
      continue;
    }
    if (argument in flagOptions) {
      options[flagOptions[argument]] = true;
      continue;
    }
    const [name, inlineValue] = argument.split(/=(.*)/s, 2);
    if (name in valueOptions) {
      const value = inlineValue ?? argv[(index += 1)];
      if (value === undefined || value === "") {
        fail(`${name} needs a value.`);
      }
      options[valueOptions[name]] = value;
      continue;
    }
    fail(`Unknown option: ${argument}\n\n${HELP}`);
  }
  if (options.check && options.commit) {
    fail("--check and --commit cannot run together.");
  }
  if (options.dryRun && options.commit) {
    fail("--dry-run and --commit cannot run together.");
  }
  return options;
}

// ---------------------------------------------------------------------------
// Git

function git(cwd, args, { input, binary = false, env, timeout } = {}) {
  const result = spawnSync("git", args, {
    cwd,
    // A Buffer input does not depend on the output encoding.
    input: input === undefined ? undefined : Buffer.from(input, "utf8"),
    env: env ? { ...process.env, ...env } : process.env,
    encoding: binary ? "buffer" : "utf8",
    maxBuffer: 1024 * 1024 * 1024,
    timeout,
  });
  if (result.error?.code === "ETIMEDOUT") {
    fail(`git ${args[0]} did not finish in ${timeout / 1000} s.`);
  }
  if (result.error) {
    fail(`git ${args[0]} failed: ${result.error.message}`);
  }
  if (result.status !== 0) {
    const stderr = binary ? result.stderr.toString("utf8") : result.stderr;
    fail(`git ${args.join(" ")} failed in ${cwd}:\n${stderr.trim()}`);
  }
  return result.stdout;
}

function gitSucceeds(cwd, args) {
  const result = spawnSync("git", args, { cwd, stdio: "ignore" });
  return !result.error && result.status === 0;
}

function repositoryRoot(dir, role) {
  if (!existsSync(dir)) {
    fail(`The ${role} folder does not exist: ${dir}`);
  }
  if (!gitSucceeds(dir, ["rev-parse", "--git-dir"])) {
    fail(`The ${role} folder is not a git checkout: ${dir}`);
  }
  return realpathSync(git(dir, ["rev-parse", "--show-toplevel"]).trim());
}

function resolveCommit(root, rev) {
  if (!gitSucceeds(root, ["rev-parse", "--verify", "--quiet", `${rev}^{commit}`])) {
    fail(`The revision does not exist in ${root}: ${rev}`);
  }
  return git(root, ["rev-parse", "--verify", `${rev}^{commit}`]).trim();
}

// Lists every entry of the revision: mode, type, object name, and path.
function listTree(root, commit) {
  const output = git(root, ["ls-tree", "-r", "-z", "--full-tree", commit]);
  return output
    .split("\0")
    .filter(Boolean)
    .map((record) => {
      const tab = record.indexOf("\t");
      const [mode, type, sha] = record.slice(0, tab).split(" ");
      return { mode, type, sha, path: record.slice(tab + 1) };
    });
}

// Reads blobs through one "git cat-file --batch" call.
function readBlobs(root, shas) {
  const unique = [...new Set(shas)];
  const blobs = new Map();
  if (unique.length === 0) {
    return blobs;
  }
  const output = git(root, ["cat-file", "--batch"], {
    input: `${unique.join("\n")}\n`,
    binary: true,
  });
  let offset = 0;
  while (offset < output.length) {
    const newline = output.indexOf(0x0a, offset);
    const header = output.subarray(offset, newline).toString("utf8").split(" ");
    if (header[1] === "missing" || header.length !== 3) {
      fail(`git cat-file could not read object ${header[0]}.`);
    }
    const size = Number(header[2]);
    const start = newline + 1;
    blobs.set(header[0], output.subarray(start, start + size));
    offset = start + size + 1;
  }
  return blobs;
}

function readFileAtRevision(root, commit, path) {
  if (!gitSucceeds(root, ["cat-file", "-e", `${commit}:${path}`])) {
    return null;
  }
  return git(root, ["show", `${commit}:${path}`]);
}

// ---------------------------------------------------------------------------
// Config and globs

function parseConfig(file, text) {
  let config;
  try {
    config = JSON.parse(text);
  } catch (error) {
    fail(`${file} is not valid JSON: ${error.message}`);
  }
  if (config === null || typeof config !== "object" || Array.isArray(config)) {
    fail(`${file} must hold a JSON object.`);
  }
  for (const key of PRIVATE_CONFIG_KEYS) {
    if (config[key] !== undefined && !Array.isArray(config[key])) {
      fail(`${file}: "${key}" must be a list.`);
    }
  }
  return config;
}

// Reads the public config and, when the revision has it, the private config.
// The lists of the two files are merged. Only the public config sets
// "target", "overlayDir", and "requirePrivateConfig".
function loadConfig(root, commit) {
  const text = readFileAtRevision(root, commit, CONFIG_PATH);
  if (text === null) {
    fail(`${CONFIG_PATH} does not exist at ${commit.slice(0, 12)}.`);
  }
  const config = parseConfig(CONFIG_PATH, text);
  const files = [{ file: CONFIG_PATH, config }];
  const privateText = readFileAtRevision(root, commit, PRIVATE_CONFIG_PATH);
  if (privateText !== null) {
    const privateConfig = parseConfig(PRIVATE_CONFIG_PATH, privateText);
    const unknown = Object.keys(privateConfig).filter(
      (key) => !key.startsWith("$") && !PRIVATE_CONFIG_KEYS.includes(key),
    );
    if (unknown.length) {
      fail(
        `${PRIVATE_CONFIG_PATH} can only hold ${PRIVATE_CONFIG_KEYS.join(", ")}. ` +
          `Move ${unknown.join(", ")} to ${CONFIG_PATH}.`,
      );
    }
    // An empty private file must not turn off the private rules.
    if (
      config.requirePrivateConfig === true &&
      !["exclude", "denyTokenHashes"].every((key) => privateConfig[key]?.length > 0)
    ) {
      fail(
        `${PRIVATE_CONFIG_PATH} needs a non-empty "exclude" list and a non-empty ` +
          `"denyTokenHashes" list, because ${CONFIG_PATH} sets "requirePrivateConfig".`,
      );
    }
    files.push({ file: PRIVATE_CONFIG_PATH, config: privateConfig });
  } else if (config.requirePrivateConfig === true) {
    fail(
      `${PRIVATE_CONFIG_PATH} does not exist at ${commit.slice(0, 12)}. ` +
        `${CONFIG_PATH} sets "requirePrivateConfig", so the export stops.`,
    );
  }
  // Each entry keeps its file and its index in that file for the messages.
  const list = (key) =>
    files.flatMap(({ file, config: part }) =>
      (part[key] ?? []).map((value, index) => ({ value, index, file })),
    );
  const overlayDir = String(config.overlayDir ?? "").replace(/^\/+|\/+$/g, "");
  const denyContent = list("denyContent").map(({ value: rule, index, file }) => {
    if (!rule?.id || !rule?.pattern) {
      fail(`${file}: denyContent entry ${index} needs "id" and "pattern".`);
    }
    try {
      return { id: rule.id, regex: new RegExp(rule.pattern, "g") };
    } catch (error) {
      fail(`${file}: denyContent "${rule.id}" is not a valid regex: ${error.message}`);
    }
  });
  const denyTokenHashes = new Map(
    list("denyTokenHashes").map(({ value: entry, index, file }) => {
      const hash = String(entry?.sha256 ?? "").toLowerCase();
      if (!/^[0-9a-f]{64}$/.test(hash)) {
        fail(`${file}: denyTokenHashes entry ${index} needs a SHA-256 hex value.`);
      }
      return [hash, String(entry.label ?? `entry ${index}`)];
    }),
  );
  const allowlist = list("allowlist").map(({ value: entry, index, file }) => {
    if (!entry?.path || !entry?.rule) {
      fail(`${file}: allowlist entry ${index} needs "path" and "rule".`);
    }
    return {
      index,
      file,
      path: entry.path,
      matches: globMatcher([entry.path]),
      rule: entry.rule,
      match: entry.match ?? null,
      used: false,
    };
  });
  return {
    target: config.target ?? null,
    overlayDir,
    files: files.map(({ file }) => file),
    isExcluded: globMatcher(list("exclude").map(({ value }) => value)),
    isDenied: globMatcher(list("denyPaths").map(({ value }) => value)),
    pathNames: pathNameRules([
      ...list("exclude").map((entry) => ({ ...entry, key: "exclude" })),
      ...list("denyPaths").map((entry) => ({ ...entry, key: "denyPaths" })),
    ]),
    denyContent,
    denyTokenHashes,
    allowlist,
  };
}

// Makes the rules that find a private path name in a commit message. Each
// exclude or denyPaths entry without glob characters gives its path. A file
// entry also gives its file name, for example "NOTES.md" for "/NOTES.md".
// A name only counts when it starts a word, so "src/server/" does not match
// "server/". A leading "/", "./", or "../" can come before the name.
export function pathNameRules(entries) {
  const rules = [];
  const seen = new Set();
  for (const { value, index, file, key } of entries) {
    const path = String(value).trim().replace(/^\/+/, "");
    if (!path || /[*?[\]]/.test(path)) {
      continue;
    }
    const names = [path];
    if (!path.endsWith("/") && path.includes("/")) {
      names.push(path.slice(path.lastIndexOf("/") + 1));
    }
    for (const name of names) {
      if (seen.has(name.toLowerCase())) {
        continue;
      }
      seen.add(name.toLowerCase());
      const end = name.endsWith("/") ? "" : "(?![A-Za-z0-9_])";
      rules.push({
        regex: new RegExp(
          `(?:(?<![A-Za-z0-9_./-])|(?<![A-Za-z0-9_.-])\\.{0,2}/)${escapeRegExp(name)}${end}`,
          "gi",
        ),
        source: `${key} entry ${index} of ${file}`,
      });
    }
  }
  return rules;
}

function escapeRegExp(text) {
  return text.replace(/[\\^$.+(){}|[\]]/g, "\\$&");
}

// Converts one glob to a regex. "**" matches any number of folders, "*" and
// "?" match inside one path segment.
function globBody(glob) {
  let body = "";
  for (let index = 0; index < glob.length; index += 1) {
    const char = glob[index];
    if (char === "*" && glob[index + 1] === "*") {
      const slash = glob[index + 2] === "/";
      body += slash ? "(?:.*/)?" : ".*";
      index += slash ? 2 : 1;
    } else if (char === "*") {
      body += "[^/]*";
    } else if (char === "?") {
      body += "[^/]";
    } else {
      body += escapeRegExp(char);
    }
  }
  return body;
}

// Gitignore-like matching:
// - "dir/" matches the folder and everything in it, at the root.
// - A pattern with a slash, or a leading "/", matches from the root.
// - A pattern without a slash matches the file name at any depth.
export function globMatcher(patterns) {
  const regexes = patterns.map((raw) => {
    let pattern = String(raw).trim();
    const directory = pattern.endsWith("/");
    pattern = pattern.replace(/\/+$/, "");
    const anchored = pattern.startsWith("/") || pattern.includes("/") || directory;
    pattern = pattern.replace(/^\/+/, "");
    const prefix = anchored ? "^" : "^(?:.*/)?";
    const suffix = directory ? "/" : "$";
    return new RegExp(`${prefix}${globBody(pattern)}${suffix}`);
  });
  return (path) => regexes.some((regex) => regex.test(path));
}

// ---------------------------------------------------------------------------
// Export set

// Tells if a source path goes to the target: a path that is not excluded, or
// a file in the overlay folder. The private config never goes.
function isExportedSourcePath(path, config) {
  if (path === PRIVATE_CONFIG_PATH) {
    return false;
  }
  const overlayPrefix = config.overlayDir ? `${config.overlayDir}/` : null;
  if (overlayPrefix && path.startsWith(overlayPrefix)) {
    return path.slice(overlayPrefix.length) !== PRIVATE_CONFIG_PATH;
  }
  return !config.isExcluded(path);
}

function buildExport(entries, config) {
  const files = new Map();
  const overlayPrefix = config.overlayDir ? `${config.overlayDir}/` : null;
  const overlay = [];
  for (const entry of entries) {
    if (entry.type === "commit") {
      fail(`The export does not support submodules: ${entry.path}`);
    }
    // The private config never goes to the target, whatever the config says.
    if (entry.type !== "blob" || !isExportedSourcePath(entry.path, config)) {
      continue;
    }
    if (overlayPrefix && entry.path.startsWith(overlayPrefix)) {
      overlay.push(entry);
      continue;
    }
    files.set(entry.path, { ...entry, from: entry.path });
  }
  // Overlay files map onto the export root and replace a file at the same path.
  for (const entry of overlay) {
    const path = entry.path.slice(overlayPrefix.length);
    files.set(path, { ...entry, path, from: entry.path });
  }
  return { files, overlayCount: overlay.length };
}

// ---------------------------------------------------------------------------
// Leak guard

export function hashToken(token) {
  return createHash("sha256").update(token.toLowerCase()).digest("hex");
}

// Shows the kind, the first 4 characters, and the length of a hit. It never
// shows the full value.
function redact(text) {
  return `${JSON.stringify(text.slice(0, 4))}… (${text.length} chars)`;
}

const EMAIL_PATTERN = /[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}/g;
// Home folder forms: "/Users/<name>", "/home/<name>", JSON "\/Users\/<name>",
// Windows "C:\\Users\\<name>", and the Claude project slug "-Users-<name>-".
const HOME_PATTERN = /(?:\\?\/(?:Users|home)\\?\/|\\+(?:Users|home)\\+|-(?:Users|home)-)([A-Za-z0-9._-]+)/gi;
// Characters that show an escaped form that decodeEscapes can decode.
const ESCAPE_HINT = /%|\\[ux]|&/i;

function lineNumber(text, index) {
  let line = 1;
  for (let position = text.indexOf("\n"); position !== -1 && position < index; ) {
    line += 1;
    position = text.indexOf("\n", position + 1);
  }
  return line;
}

function allowed(config, path, rule, value) {
  const entry = config.allowlist.find(
    (candidate) =>
      candidate.matches(path) &&
      (candidate.rule === "*" || candidate.rule === rule) &&
      (candidate.match === null || value.includes(candidate.match)),
  );
  if (entry) {
    entry.used = true;
    return true;
  }
  return false;
}

function findTokens(text) {
  const tokens = [];
  for (const match of text.matchAll(EMAIL_PATTERN)) {
    tokens.push({ kind: "email", value: match[0], index: match.index });
  }
  for (const match of text.matchAll(HOME_PATTERN)) {
    tokens.push({ kind: "home", value: match[1], index: match.index });
  }
  return tokens;
}

// Returns the forms of a token that a hash can match. A home folder name
// gives the name without trailing punctuation, for example "<name>" for
// "<name>." at the end of a sentence. It also gives each prefix that ends
// before ".", "-", or "_", for example "<name>" for "<name>-old" or for the
// slug "<name>-work". An email address also gives the address without its
// "+tag".
export function tokenForms(kind, value) {
  const forms = new Set([value]);
  if (kind === "home") {
    forms.add(value.replace(/[._-]+$/, ""));
    for (const match of value.matchAll(/[._-]/g)) {
      forms.add(value.slice(0, match.index));
    }
  } else if (kind === "email") {
    forms.add(value.replace(/\+[^@]*@/, "@"));
  }
  forms.delete("");
  return [...forms];
}

// Decodes percent-encoded text, for example "%2FUsers%2F<name>", up to three
// times for double encoding. decodeURIComponent throws on a malformed
// sequence such as "100% sure". Then only the ASCII sequences are decoded.
export function decodePercent(text) {
  let current = text;
  for (let round = 0; round < 3 && current.includes("%"); round += 1) {
    let next;
    try {
      next = decodeURIComponent(current);
    } catch {
      next = current.replace(/%([0-7][0-9A-Fa-f])/g, (_, hex) =>
        String.fromCharCode(Number.parseInt(hex, 16)),
      );
    }
    if (next === current) {
      break;
    }
    current = next;
  }
  return current;
}

const NAMED_ENTITIES = { sol: "/", bsol: "\\", commat: "@", period: ".", lowbar: "_", hyphen: "-", dash: "-", amp: "&" };

function fromCodePoint(code) {
  return Number.isInteger(code) && code >= 0 && code <= 0x10ffff ? String.fromCodePoint(code) : "";
}

// Decodes percent sequences, "\uXXXX" and "\xXX" escapes, and HTML entities,
// for example "&#x2F;", "&#47;", or "&sol;". It decodes up to three times
// for mixed or double encoding.
export function decodeEscapes(text) {
  let current = text;
  for (let round = 0; round < 3; round += 1) {
    const next = decodePercent(current)
      .replace(/\\(?:u\{([0-9a-fA-F]{1,6})\}|u([0-9a-fA-F]{4})|x([0-9a-fA-F]{2}))/g, (match, braced, u4, x2) =>
        fromCodePoint(Number.parseInt(braced ?? u4 ?? x2, 16)) || match,
      )
      .replace(/&#(?:x([0-9a-fA-F]{1,6})|([0-9]{1,7}));?/g, (match, hex, decimal) =>
        fromCodePoint(hex ? Number.parseInt(hex, 16) : Number.parseInt(decimal, 10)) || match,
      )
      .replace(/&([A-Za-z]+);/g, (match, name) => NAMED_ENTITIES[name.toLowerCase()] ?? match);
    if (next === current) {
      break;
    }
    current = next;
  }
  return current;
}

export function scanText(config, path, text) {
  const hits = [];
  for (const rule of config.denyContent) {
    rule.regex.lastIndex = 0;
    for (const match of text.matchAll(rule.regex)) {
      if (!allowed(config, path, rule.id, match[0])) {
        hits.push({ path, line: lineNumber(text, match.index), rule: rule.id, shown: redact(match[0]) });
      }
    }
  }
  if (config.denyTokenHashes.size > 0) {
    // A decoded line can show a token that the raw scan found already.
    const found = new Set();
    const check = (token, line, encoded) => {
      let form = null;
      let label = null;
      for (const candidate of tokenForms(token.kind, token.value)) {
        label = config.denyTokenHashes.get(hashToken(`${token.kind}:${candidate}`));
        if (label) {
          form = candidate;
          break;
        }
      }
      if (!label) {
        return;
      }
      const rule = `private-${token.kind}`;
      const key = `${line}\0${rule}\0${form.toLowerCase()}`;
      if (encoded && found.has(key)) {
        return;
      }
      found.add(key);
      if (!allowed(config, path, rule, token.value)) {
        hits.push({
          path,
          line,
          rule,
          shown: `${label}${encoded ? ", encoded" : ""}, ${redact(token.value)}`,
        });
      }
    };
    for (const token of findTokens(text)) {
      check(token, lineNumber(text, token.index), false);
    }
    // The patterns do not see an encoded path or address, for example
    // "%2FUsers%2F<name>", "<name>%40<domain>", "\u002FUsers\u002F<name>", or
    // "&#x2F;Users&#x2F;<name>". Scan the decoded lines too.
    if (ESCAPE_HINT.test(text)) {
      text.split("\n").forEach((line, index) => {
        if (!ESCAPE_HINT.test(line)) {
          return;
        }
        const decoded = decodeEscapes(line);
        if (decoded === line) {
          return;
        }
        for (const token of findTokens(decoded)) {
          check(token, index + 1, true);
        }
      });
    }
    // A copy of a hash list at another path, for example a backup of the
    // private config, must not reach the public repository. A hash of a
    // short name is easy to reverse.
    const lower = text.toLowerCase();
    for (const [hash, label] of config.denyTokenHashes) {
      for (let index = lower.indexOf(hash); index !== -1; index = lower.indexOf(hash, index + hash.length)) {
        if (!allowed(config, path, "private-token-hash", hash)) {
          hits.push({
            path,
            line: lineNumber(text, index),
            rule: "private-token-hash",
            shown: `hash of ${label}, ${redact(hash)}`,
          });
        }
      }
    }
  }
  return hits;
}

// Finds private path names in a commit message, for example "NOTES.md" or
// "server/". Only the commit message gets this check, because public files
// can name a private folder for a good reason.
export function scanMessagePaths(config, message) {
  const hits = [];
  for (const rule of config.pathNames) {
    rule.regex.lastIndex = 0;
    for (const match of message.matchAll(rule.regex)) {
      hits.push({
        path: "<commit message>",
        line: lineNumber(message, match.index),
        rule: "private-path-name",
        shown: `${rule.source}, ${redact(match[0])}`,
      });
    }
  }
  return hits;
}

function guard(config, files, blobs) {
  const deniedPaths = [...files.keys()].filter((path) => config.isDenied(path)).sort();
  const hits = [];
  for (const [path, file] of files) {
    // latin1 keeps one character per byte, so binary files scan too.
    hits.push(...scanText(config, path, blobs.get(file.sha).toString("latin1")));
  }
  return { deniedPaths, hits };
}

function reportLeaks({ deniedPaths, hits }) {
  const lines = [];
  if (deniedPaths.length) {
    lines.push(`The export holds ${deniedPaths.length} denied path(s):`);
    lines.push(...deniedPaths.map((path) => `  ${path}`));
  }
  if (hits.length) {
    lines.push(`The export holds ${hits.length} denied content hit(s):`);
    lines.push(...hits.map((hit) => `  ${hit.path}:${hit.line} ${hit.rule} ${hit.shown}`));
  }
  lines.push(
    "Remove the content, exclude the path, or add a reviewed allowlist entry to " +
      `${CONFIG_PATH} or ${PRIVATE_CONFIG_PATH}.`,
  );
  return lines.join("\n");
}

// ---------------------------------------------------------------------------
// Target

function trackedFiles(targetRoot) {
  return new Set(git(targetRoot, ["ls-files", "-z"]).split("\0").filter(Boolean));
}

function isExecutable(mode) {
  return mode === "100755";
}

function sameAsTarget(targetRoot, file, content) {
  const path = join(targetRoot, file.path);
  let stat;
  try {
    stat = lstatSync(path);
  } catch {
    return false;
  }
  if (file.mode === "120000") {
    return stat.isSymbolicLink() && readlinkSync(path) === content.toString("utf8");
  }
  if (!stat.isFile()) {
    return false;
  }
  if (process.platform !== "win32" && Boolean(stat.mode & 0o111) !== isExecutable(file.mode)) {
    return false;
  }
  return readFileSync(path).equals(content);
}

function plan(targetRoot, files, blobs) {
  const tracked = trackedFiles(targetRoot);
  const added = [];
  const changed = [];
  for (const [path, file] of files) {
    if (sameAsTarget(targetRoot, file, blobs.get(file.sha))) {
      continue;
    }
    (tracked.has(path) ? changed : added).push(path);
  }
  const deleted = [...tracked].filter((path) => !files.has(path));
  return { added: added.sort(), changed: changed.sort(), deleted: deleted.sort() };
}

function removeEmptyParents(targetRoot, path) {
  let dir = dirname(join(targetRoot, path));
  while (dir.startsWith(targetRoot + sep) && dir !== targetRoot) {
    try {
      if (readdirSync(dir).length > 0) {
        return;
      }
      rmdirSync(dir);
    } catch {
      return;
    }
    dir = dirname(dir);
  }
}

function applyPlan(targetRoot, files, blobs, changes) {
  // Delete first, so a path that changes between file and folder can be written.
  for (const path of changes.deleted) {
    rmSync(join(targetRoot, path), { force: true });
    removeEmptyParents(targetRoot, path);
  }
  for (const path of [...changes.added, ...changes.changed]) {
    const file = files.get(path);
    const content = blobs.get(file.sha);
    const destination = join(targetRoot, path);
    mkdirSync(dirname(destination), { recursive: true });
    let stat = null;
    try {
      stat = lstatSync(destination);
    } catch {
      stat = null;
    }
    if (stat && (stat.isSymbolicLink() || file.mode === "120000")) {
      rmSync(destination, { force: true });
    }
    if (file.mode === "120000") {
      symlinkSync(content.toString("utf8"), destination);
      continue;
    }
    writeFileSync(destination, content);
    chmodSync(destination, isExecutable(file.mode) ? 0o755 : 0o644);
  }
}

// Fetches the upstream branch of the target and stops when the target is
// behind it, so that the export never starts from an old public tree. A dry
// run shows a warning. The fetch changes only the remote-tracking refs.
function checkUpstream(targetRoot, { offline, dryRun }) {
  if (offline) {
    console.log("Upstream: not checked (--offline).");
    return;
  }
  const upstream = spawnSync(
    "git",
    ["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"],
    { cwd: targetRoot, encoding: "utf8" },
  );
  if (upstream.error || upstream.status !== 0) {
    fail(
      "The target branch has no upstream branch, so the script cannot make sure that the " +
        `target is current. Run "git -C ${targetRoot} branch --set-upstream-to origin/main", ` +
        "or pass --offline.",
    );
  }
  const name = upstream.stdout.trim();
  try {
    // A fetch must not wait for a password prompt.
    git(targetRoot, ["fetch", "--quiet"], {
      env: { GIT_TERMINAL_PROMPT: "0", GIT_HTTP_LOW_SPEED_LIMIT: "1000", GIT_HTTP_LOW_SPEED_TIME: "30" },
      timeout: FETCH_TIMEOUT_MS,
    });
  } catch (error) {
    fail(`${error.message}\nThe script cannot make sure that the target is current. Pass --offline to skip the fetch.`);
  }
  const [behind, ahead] = git(targetRoot, ["rev-list", "--left-right", "--count", "@{upstream}...HEAD"])
    .trim()
    .split(/\s+/)
    .map(Number);
  console.log(`Upstream: ${name}, ${behind} behind, ${ahead} ahead.`);
  if (behind > 0) {
    const message =
      `The target is ${behind} commit(s) behind ${name}. ` +
      `Run "git -C ${targetRoot} pull --ff-only" first.`;
    if (dryRun) {
      console.warn(`WARNING: ${message}`);
      return;
    }
    fail(message);
  }
}

function requireCleanTarget(targetRoot) {
  const status = git(targetRoot, ["status", "--porcelain=v1", "--untracked-files=no"]);
  if (status.trim()) {
    fail(
      `The target has uncommitted changes to tracked files. Commit or discard them first:\n${status.trimEnd()}`,
    );
  }
}

// ---------------------------------------------------------------------------
// Commit

// Finds the newest target commit with a Source-Commit trailer. Returns the
// target commit, the source commit in the trailer, and the target commits
// after it, newest first.
export function lastExport(targetRoot) {
  if (!gitSucceeds(targetRoot, ["rev-parse", "--verify", "--quiet", "HEAD"])) {
    return null;
  }
  const output = git(targetRoot, [
    "log",
    `--format=%H%x1f%h %s%x1f%(trailers:key=${TRAILER},valueonly,separator=%x2C)%x1e`,
  ]);
  const later = [];
  for (const record of output.split("\x1e")) {
    if (!record.trim()) {
      continue;
    }
    const [targetCommit, line, trailers] = record.replace(/^\n/, "").split("\x1f");
    const sourceCommit = trailers.trim().split(",").pop()?.trim();
    if (sourceCommit) {
      return { targetCommit, sourceCommit, later };
    }
    later.push(line);
  }
  return null;
}

// Returns the subjects of the source commits after "since" that change at
// least one exported path. A commit that changes only private paths, for
// example only server/, keeps its subject private. A merge commit shows no
// changed paths, so its subject is not listed.
export function sourceSubjects(sourceRoot, since, commit, config) {
  const output = git(sourceRoot, [
    "log",
    "--reverse",
    "--no-renames",
    "--name-only",
    "-z",
    "--format=%x1e%H%x1f%s",
    `${since}..${commit}`,
  ]);
  const subjects = [];
  for (const record of output.split("\x1e")) {
    if (!record) {
      continue;
    }
    const [header, ...paths] = record.split("\0");
    const subject = header.slice(header.indexOf("\x1f") + 1).trim();
    const exported = paths.some((path) => {
      const name = path.replace(/^\n/, "");
      return name !== "" && isExportedSourcePath(name, config);
    });
    if (subject && exported) {
      subjects.push(subject);
    }
  }
  return subjects;
}

export function commitMessage({ subjects, commit, subject }) {
  const lines = [];
  if (subjects.length === 1 && !subject) {
    lines.push(subjects[0]);
  } else {
    lines.push(
      subject ??
        (subjects.length
          ? `Export ${subjects.length} commits from the private repository`
          : `Export ${commit.slice(0, 12)} from the private repository again`),
    );
    if (subjects.length) {
      lines.push("", ...subjects.map((line) => `- ${line}`));
    }
  }
  lines.push("", `${TRAILER}: ${commit}`, "");
  return lines.join("\n");
}

function resolveSince(sourceRoot, previous, commit, explicit) {
  const value = explicit ?? previous?.sourceCommit;
  if (!value) {
    fail(
      `The target has no "${TRAILER}" trailer. For the first export, pass --since <sha> ` +
        "with the source commit that the target mirrors now.",
    );
  }
  if (!gitSucceeds(sourceRoot, ["cat-file", "-e", `${value}^{commit}`])) {
    fail(`The last exported commit does not exist in the source: ${value}`);
  }
  const since = resolveCommit(sourceRoot, value);
  if (!gitSucceeds(sourceRoot, ["merge-base", "--is-ancestor", since, commit])) {
    fail(`The last exported commit ${since.slice(0, 12)} is not an ancestor of ${commit.slice(0, 12)}.`);
  }
  return since;
}

function commitTarget(targetRoot, changes, message) {
  const paths = [...changes.added, ...changes.changed, ...changes.deleted];
  git(
    targetRoot,
    ["--literal-pathspecs", "add", "-A", "-f", "--pathspec-from-file=-", "--pathspec-file-nul"],
    { input: `${paths.join("\0")}\0` },
  );
  // "whitespace" keeps a subject line that starts with "#".
  git(targetRoot, ["commit", "--quiet", "--cleanup=whitespace", "-F", "-"], {
    input: message,
    env: {
      GIT_AUTHOR_NAME: COMMIT_NAME,
      GIT_AUTHOR_EMAIL: COMMIT_EMAIL,
      GIT_COMMITTER_NAME: COMMIT_NAME,
      GIT_COMMITTER_EMAIL: COMMIT_EMAIL,
    },
  });
  return git(targetRoot, ["rev-parse", "HEAD"]).trim();
}

// ---------------------------------------------------------------------------
// Main

function printList(title, paths) {
  console.log(`${title} (${paths.length}):`);
  for (const path of paths) {
    console.log(`  ${path}`);
  }
}

function run(argv) {
  const options = parseArguments(argv);
  if (options.help) {
    console.log(HELP);
    return 0;
  }
  if (options.hashToken) {
    console.log(hashToken(options.hashToken));
    return 0;
  }

  const sourceRoot = repositoryRoot(resolve(options.source ?? SCRIPT_ROOT), "source");
  const commit = resolveCommit(sourceRoot, options.rev);
  const config = loadConfig(sourceRoot, commit);
  const targetRoot = repositoryRoot(
    resolve(options.target ?? resolve(sourceRoot, config.target ?? "../oss")),
    "target",
  );
  if (targetRoot === sourceRoot) {
    fail("The target is the source repository. Pass --target with the public checkout.");
  }

  const subject = git(sourceRoot, ["log", "-1", "--format=%s", commit]).trim();
  console.log(`Source: ${sourceRoot} at ${commit.slice(0, 12)} (${subject})`);
  console.log(`Target: ${targetRoot}`);
  console.log(`Config: ${config.files.join(" + ")}`);

  const { files, overlayCount } = buildExport(listTree(sourceRoot, commit), config);
  const blobs = readBlobs(
    sourceRoot,
    [...files.values()].map((file) => file.sha),
  );
  console.log(`Export: ${files.size} files, ${overlayCount} from ${config.overlayDir || "no overlay"}`);

  const leaks = guard(config, files, blobs);
  for (const entry of config.allowlist.filter((candidate) => !candidate.used)) {
    console.warn(
      `WARNING: allowlist entry ${entry.index} of ${entry.file} (${entry.path}, ${entry.rule}) matched nothing.`,
    );
  }
  if (leaks.deniedPaths.length || leaks.hits.length) {
    console.error(reportLeaks(leaks));
    return 2;
  }
  console.log("Leak guard: no denied path and no denied content.");

  checkUpstream(targetRoot, options);
  const changes = plan(targetRoot, files, blobs);
  printList("Added", changes.added);
  printList("Changed", changes.changed);
  printList("Deleted", changes.deleted);
  const differs = changes.added.length + changes.changed.length + changes.deleted.length > 0;

  const previous = lastExport(targetRoot);
  if (previous?.later.length) {
    console.warn(
      `The target has ${previous.later.length} commit(s) after the last export ` +
        `(${previous.targetCommit.slice(0, 12)}). The export reverts their changes:`,
    );
    for (const line of previous.later) {
      console.warn(`  ${line}`);
    }
    if (!options.check && !options.dryRun && differs && !options.allowPublicCommits) {
      console.error(
        "Apply those commits to the private repository first, then run again with " +
          "--allow-public-commits.",
      );
      return 2;
    }
  }
  if (options.check) {
    console.log(differs ? "The target differs from the export." : "The target is current.");
    return differs ? 1 : 0;
  }
  if (changes.added.length && !options.acceptNew) {
    console.error(
      `The export adds ${changes.added.length} path(s) that the target does not have. ` +
        "Make sure that each one is public, then run again with --accept-new.",
    );
    if (!options.dryRun) {
      return 2;
    }
  }

  let message = null;
  if (options.commit || (options.dryRun && (options.since || previous))) {
    let since;
    try {
      since = resolveSince(sourceRoot, previous, commit, options.since);
    } catch (error) {
      if (!options.dryRun) {
        throw error;
      }
      console.warn(`WARNING: no commit message preview. ${error.message}`);
    }
    message = since
      ? commitMessage({
          subjects: options.omitSubjects ? [] : sourceSubjects(sourceRoot, since, commit, config),
          commit,
          subject: options.message,
        })
      : null;
  }
  if (message) {
    const messageHits = [
      ...scanText(config, "<commit message>", message),
      ...scanMessagePaths(config, message),
    ];
    if (messageHits.length) {
      console.error(reportLeaks({ deniedPaths: [], hits: messageHits }));
      console.error(
        "The commit message names a private path or holds denied content. " +
          'Run again with --omit-subjects and --message "<subject>".',
      );
      return 2;
    }
    console.log(`Commit message:\n${message.replace(/^/gm, "  ")}`);
  }

  if (options.dryRun) {
    console.log("Dry run: the script wrote nothing.");
    return 0;
  }
  if (!differs) {
    console.log("The target is current. The script wrote nothing.");
    return 0;
  }

  requireCleanTarget(targetRoot);
  applyPlan(targetRoot, files, blobs, changes);
  console.log(
    `Wrote the export: ${changes.added.length} added, ${changes.changed.length} changed, ` +
      `${changes.deleted.length} deleted.`,
  );
  if (options.commit) {
    const sha = commitTarget(targetRoot, changes, message);
    console.log(`Committed ${sha.slice(0, 12)} in the target. Push it after you review it.`);
  }
  return 0;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    process.exitCode = run(process.argv.slice(2));
  } catch (error) {
    if (error instanceof ExportError) {
      console.error(`ERROR: ${error.message}`);
      process.exitCode = 2;
    } else {
      throw error;
    }
  }
}
