#!/usr/bin/env node

import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";

export const REQUIRED_UPDATER_TARGETS = [
  "darwin-aarch64-app",
  "darwin-x86_64-app",
  "linux-x86_64-appimage",
  "linux-x86_64-deb",
  "linux-x86_64-rpm",
  "windows-x86_64-msi",
];

const INSTALLER_PRIORITY = {
  app: 0,
  appimage: 0,
  msi: 0,
  nsis: 1,
  deb: 1,
  rpm: 2,
};

function updaterArtifactKind(assetName) {
  const lower = assetName.toLowerCase();
  if (lower.endsWith(".app.tar.gz")) {
    return { os: "darwin", installer: "app", archivePriority: 0 };
  }
  if (lower.endsWith(".appimage.tar.gz")) {
    return { os: "linux", installer: "appimage", archivePriority: 0 };
  }
  if (lower.endsWith(".appimage")) {
    return { os: "linux", installer: "appimage", archivePriority: 1 };
  }
  if (lower.endsWith(".deb")) {
    return { os: "linux", installer: "deb", archivePriority: 0 };
  }
  if (lower.endsWith(".rpm")) {
    return { os: "linux", installer: "rpm", archivePriority: 0 };
  }
  if (lower.endsWith(".msi.zip")) {
    return { os: "windows", installer: "msi", archivePriority: 0 };
  }
  if (lower.endsWith(".msi")) {
    return { os: "windows", installer: "msi", archivePriority: 1 };
  }
  if (
    lower.endsWith("-setup.exe.zip") ||
    lower.endsWith("-setup.nsis.zip")
  ) {
    return { os: "windows", installer: "nsis", archivePriority: 0 };
  }
  if (lower.endsWith("-setup.exe")) {
    return { os: "windows", installer: "nsis", archivePriority: 1 };
  }
  return null;
}

function updaterArchitecture(assetName) {
  const lower = assetName.toLowerCase();
  if (/(^|[_.-])(x64|x86_64|amd64)([_.-]|$)/.test(lower)) {
    return "x86_64";
  }
  if (/(^|[_.-])(aarch64|arm64)([_.-]|$)/.test(lower)) {
    return "aarch64";
  }
  if (/(^|[_.-])(i686|x86)([_.-]|$)/.test(lower)) {
    return "i686";
  }
  throw new Error(`Could not infer updater architecture from asset: ${assetName}`);
}

export function inferUpdaterCandidate(assetName) {
  const kind = updaterArtifactKind(assetName);
  if (!kind) {
    return null;
  }
  const arch = updaterArchitecture(assetName);
  return {
    exactKey: `${kind.os}-${arch}-${kind.installer}`,
    genericKey: `${kind.os}-${arch}`,
    installer: kind.installer,
    priority: [INSTALLER_PRIORITY[kind.installer], kind.archivePriority, assetName],
  };
}

function comparePriority(left, right) {
  for (let index = 0; index < left.length; index += 1) {
    if (left[index] < right[index]) {
      return -1;
    }
    if (left[index] > right[index]) {
      return 1;
    }
  }
  return 0;
}

function validateSignature(signature, signatureName) {
  const normalized = signature.trim();
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(normalized)) {
    throw new Error(`Updater signature is not valid base64: ${signatureName}`);
  }
  const decoded = Buffer.from(normalized, "base64");
  if (
    decoded.length === 0 ||
    decoded.toString("base64").replace(/=+$/, "") !==
      normalized.replace(/=+$/, "")
  ) {
    throw new Error(`Updater signature is not valid base64: ${signatureName}`);
  }
  return normalized;
}

function selectPlatformCandidate(selected, key, candidate) {
  const current = selected.get(key);
  if (!current || comparePriority(candidate.priority, current.priority) < 0) {
    selected.set(key, candidate);
  }
}

export function buildUpdaterManifest({
  release,
  signatures,
  repository,
  tag,
  now = () => new Date(),
  requiredTargets = REQUIRED_UPDATER_TARGETS,
}) {
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository)) {
    throw new Error(`Invalid release repository: ${repository}`);
  }
  if (!/^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(tag)) {
    throw new Error(`Invalid updater release tag: ${tag}`);
  }
  if (!release || !Array.isArray(release.assets)) {
    throw new Error("Release metadata must contain an assets array.");
  }
  if (release.tag_name && release.tag_name !== tag) {
    throw new Error(
      `Release tag ${release.tag_name} does not match updater tag ${tag}.`,
    );
  }

  const assetNames = new Set(
    release.assets.map((asset) => {
      if (!asset || typeof asset.name !== "string") {
        throw new Error("Release metadata contains an invalid asset.");
      }
      return asset.name;
    }),
  );
  const selected = new Map();

  for (const [signatureName, signatureValue] of [...signatures.entries()].sort()) {
    if (!/^[A-Za-z0-9._+-]+\.sig$/.test(signatureName)) {
      throw new Error(`Invalid updater signature asset name: ${signatureName}`);
    }
    const artifactName = signatureName.slice(0, -4);
    if (!assetNames.has(artifactName)) {
      throw new Error(`Updater signature has no matching release asset: ${signatureName}`);
    }
    const inferred = inferUpdaterCandidate(artifactName);
    if (!inferred) {
      continue;
    }
    const signature = validateSignature(signatureValue, signatureName);
    const encodedArtifactName = encodeURIComponent(artifactName);
    const platform = {
      signature,
      url: `https://github.com/${repository}/releases/download/${tag}/${encodedArtifactName}`,
    };
    const candidate = { ...inferred, platform };
    selectPlatformCandidate(selected, inferred.exactKey, candidate);
    selectPlatformCandidate(selected, inferred.genericKey, candidate);
  }

  const missingTargets = requiredTargets.filter((target) => !selected.has(target));
  if (missingTargets.length > 0) {
    throw new Error(
      `Updater manifest is missing required targets: ${missingTargets.join(", ")}`,
    );
  }

  const platforms = Object.fromEntries(
    [...selected.entries()]
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, candidate]) => [key, candidate.platform]),
  );
  const publishedAt = release.published_at || now().toISOString();

  return {
    notes: typeof release.body === "string" ? release.body : "",
    platforms,
    pub_date: publishedAt,
    version: tag.startsWith("v") ? tag.slice(1) : tag,
  };
}

function parseArguments(argumentsToParse) {
  const options = {};
  for (let index = 0; index < argumentsToParse.length; index += 2) {
    const name = argumentsToParse[index];
    const value = argumentsToParse[index + 1];
    if (!name?.startsWith("--") || !value) {
      throw new Error("Expected --release, --signatures, --repository, --tag, and --output.");
    }
    options[name.slice(2)] = value;
  }
  for (const required of ["release", "signatures", "repository", "tag", "output"]) {
    if (!options[required]) {
      throw new Error(`Missing required option: --${required}`);
    }
  }
  return options;
}

function runCli() {
  const options = parseArguments(process.argv.slice(2));
  const release = JSON.parse(readFileSync(options.release, "utf8"));
  const signatures = new Map(
    readdirSync(options.signatures)
      .filter((name) => name.endsWith(".sig"))
      .map((name) => [
        basename(name),
        readFileSync(join(options.signatures, name), "utf8"),
      ]),
  );
  const manifest = buildUpdaterManifest({
    release,
    signatures,
    repository: options.repository,
    tag: options.tag,
  });
  writeFileSync(options.output, `${JSON.stringify(manifest, null, 2)}\n`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    runCli();
  } catch (error) {
    console.error(`Updater manifest error: ${error.message}`);
    process.exitCode = 1;
  }
}
