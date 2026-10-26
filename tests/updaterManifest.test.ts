import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  buildUpdaterManifest,
  inferUpdaterCandidate,
} from "../scripts/build-updater-manifest.mjs";

const SIGNATURE = Buffer.from("minisign fixture").toString("base64");

function releaseWithArtifacts(artifactNames: string[]) {
  return {
    assets: artifactNames.flatMap((name) => [{ name }, { name: `${name}.sig` }]),
    body: "Release notes",
    published_at: null,
  };
}

function signaturesFor(artifactNames: string[]) {
  return new Map(
    artifactNames.map((name) => [`${name}.sig`, SIGNATURE]),
  );
}

const REQUIRED_ARTIFACTS = [
  "Halv_0.1.2_aarch64.app.tar.gz",
  "Halv_0.1.2_x64.app.tar.gz",
  "Halv_0.1.2_amd64.AppImage",
  "Halv_0.1.2_amd64.deb",
  "Halv_0.1.2_x86_64.rpm",
  "Halv_0.1.2_x64_en-US.msi",
];

test("builds complete installer-specific and generic updater targets", () => {
  const manifest = buildUpdaterManifest({
    release: releaseWithArtifacts(REQUIRED_ARTIFACTS),
    signatures: signaturesFor(REQUIRED_ARTIFACTS),
    repository: "pedr0v/halv-releases",
    tag: "v0.1.2",
    now: () => new Date("2026-08-08T00:00:00Z"),
  });

  assert.equal(manifest.version, "0.1.2");
  assert.equal(manifest.pub_date, "2026-08-08T00:00:00.000Z");
  assert.equal(manifest.notes, "Release notes");
  assert.deepEqual(Object.keys(manifest.platforms), [
    "darwin-aarch64",
    "darwin-aarch64-app",
    "darwin-x86_64",
    "darwin-x86_64-app",
    "linux-x86_64",
    "linux-x86_64-appimage",
    "linux-x86_64-deb",
    "linux-x86_64-rpm",
    "windows-x86_64",
    "windows-x86_64-msi",
  ]);
  assert.match(
    manifest.platforms["linux-x86_64-appimage"].url,
    /Halv_0\.1\.2_amd64\.AppImage$/,
  );
  assert.match(
    manifest.platforms["linux-x86_64-deb"].url,
    /Halv_0\.1\.2_amd64\.deb$/,
  );
  assert.match(
    manifest.platforms["linux-x86_64-rpm"].url,
    /Halv_0\.1\.2_x86_64\.rpm$/,
  );
  assert.equal(
    manifest.platforms["linux-x86_64"].url,
    manifest.platforms["linux-x86_64-appimage"].url,
  );
  assert.match(
    manifest.platforms["windows-x86_64-msi"].url,
    /Halv_0\.1\.2_x64_en-US\.msi$/,
  );
});

test("recognizes raw and archived Tauri updater artifact names", () => {
  assert.equal(
    inferUpdaterCandidate("Halv_0.1.2_amd64.AppImage")?.exactKey,
    "linux-x86_64-appimage",
  );
  assert.equal(
    inferUpdaterCandidate("Halv_0.1.2_amd64.AppImage.tar.gz")?.exactKey,
    "linux-x86_64-appimage",
  );
  assert.equal(
    inferUpdaterCandidate("Halv_0.1.2_x64_en-US.msi.zip")?.exactKey,
    "windows-x86_64-msi",
  );
  assert.equal(
    inferUpdaterCandidate("Halv_0.1.2_x64-setup.nsis.zip")?.exactKey,
    "windows-x86_64-nsis",
  );
});

test("prefers updater archives and portable generic installers deterministically", () => {
  const artifacts = [
    ...REQUIRED_ARTIFACTS,
    "Halv_0.1.2_amd64.AppImage.tar.gz",
    "Halv_0.1.2_x64_en-US.msi.zip",
  ];
  const manifest = buildUpdaterManifest({
    release: releaseWithArtifacts(artifacts),
    signatures: signaturesFor(artifacts),
    repository: "pedr0v/halv-releases",
    tag: "v0.1.2",
  });

  assert.match(
    manifest.platforms["linux-x86_64-appimage"].url,
    /AppImage\.tar\.gz$/,
  );
  assert.match(manifest.platforms["windows-x86_64-msi"].url, /\.msi\.zip$/);
  assert.equal(
    manifest.platforms["linux-x86_64"].url,
    manifest.platforms["linux-x86_64-appimage"].url,
  );
});

test("refuses to publish a manifest missing any shipped updater target", () => {
  const incomplete = REQUIRED_ARTIFACTS.filter((name) => !name.endsWith(".msi"));
  assert.throws(
    () =>
      buildUpdaterManifest({
        release: releaseWithArtifacts(incomplete),
        signatures: signaturesFor(incomplete),
        repository: "pedr0v/halv-releases",
        tag: "v0.1.2",
      }),
    /missing required targets: windows-x86_64-msi/,
  );
});

test("rejects orphaned and malformed updater signatures", () => {
  assert.throws(
    () =>
      buildUpdaterManifest({
        release: releaseWithArtifacts(REQUIRED_ARTIFACTS),
        signatures: new Map([["orphan.msi.sig", SIGNATURE]]),
        repository: "pedr0v/halv-releases",
        tag: "v0.1.2",
      }),
    /no matching release asset/,
  );

  const signatures = signaturesFor(REQUIRED_ARTIFACTS);
  signatures.set(`${REQUIRED_ARTIFACTS[0]}.sig`, "not base64!");
  assert.throws(
    () =>
      buildUpdaterManifest({
        release: releaseWithArtifacts(REQUIRED_ARTIFACTS),
        signatures,
        repository: "pedr0v/halv-releases",
        tag: "v0.1.2",
      }),
    /not valid base64/,
  );
});

test("release workflow keeps updater signing enabled and uses the tested builder", () => {
  const workflow = readFileSync(".github/workflows/build.yml", "utf8");
  assert.match(
    workflow,
    /tauri_args: --target x86_64-pc-windows-msvc --bundles msi/,
  );
  assert.doesNotMatch(
    workflow,
    /tauri_args:.*x86_64-pc-windows-msvc.*--no-sign/,
  );
  assert.match(workflow, /node scripts\/build-updater-manifest\.mjs/);
  assert.match(
    workflow,
    /^  RELEASE_NOTES: (?:\|\n    \S|[^|\s])/m,
  );
  assert.match(workflow, /--notes "\$RELEASE_NOTES"/);
  assert.match(
    workflow,
    /gh release edit "\$TAG_NAME" --repo "\$RELEASE_REPOSITORY"\s+\\\s+--notes "\$RELEASE_NOTES"/,
  );
  assert.match(
    workflow,
    /releaseBody: \$\{\{ env\.RELEASE_NOTES \}\}/,
  );
  assert.doesNotMatch(workflow, /TAURI_SIGNING_PRIVATE_KEY is configured/);
  assert.match(workflow, /Verify updater artifacts and signatures/);
  assert.match(workflow, /require_signed_artifact '\*\.msi\.sig'/);
  assert.match(workflow, /require_signed_artifact '\*\.AppImage\.sig'/);
  assert.match(workflow, /require_signed_artifact '\*\.deb\.sig'/);
  assert.match(workflow, /require_signed_artifact '\*\.rpm\.sig'/);
  assert.match(workflow, /--bundles appimage,deb,rpm/);
  assert.match(workflow, /AZURE_ARTIFACT_SIGNING_ENDPOINT/);
  assert.match(workflow, /AZURE_ARTIFACT_SIGNING_ACCOUNT/);
  assert.match(workflow, /AZURE_ARTIFACT_SIGNING_CERTIFICATE_PROFILE/);
  assert.match(workflow, /artifact-signing-cli\.exe/);
  assert.match(
    workflow,
    /ddc6f6ef68631bf367ad051d7d37109e58d46095406e1fbba60d553b10f94393/,
  );
  assert.match(workflow, /tauri\.windows-signing\.conf\.json/);
  assert.match(workflow, /Get-AuthenticodeSignature/);
  assert.match(workflow, /TimeStamperCertificate/);
  const publishIndex = workflow.indexOf(
    'gh api --method PATCH "$release_endpoint" -F draft=false --silent',
  );
  const legacyIndex = workflow.indexOf(
    "- name: Publish latest.json for Halv 1.0.0 and older",
  );
  const refreshWebsiteIndex = workflow.indexOf(
    "- name: Refresh website release metadata",
  );
  assert.ok(publishIndex >= 0);
  assert.ok(legacyIndex > publishIndex);
  assert.ok(refreshWebsiteIndex > legacyIndex);
  // Older releases stay public in both repositories.
  assert.doesNotMatch(workflow, /-F draft=true/);
  assert.doesNotMatch(workflow, /Hide older public releases/);
  assert.match(workflow, /^  RELEASE_REPOSITORY: \$\{\{ github\.repository \}\}$/m);
  assert.match(workflow, /^  LEGACY_RELEASE_REPOSITORY: pedr0v\/halv-releases$/m);
  assert.doesNotMatch(workflow, /owner: pedr0v/);
  // The release re-checks the latest mark after publication.
  const latestIndex = workflow.indexOf(
    "- name: Make sure that this release is the latest one",
  );
  assert.ok(latestIndex > publishIndex);
  assert.ok(legacyIndex > latestIndex);
  // The SBOM tool gets no token and runs after the keychain is gone.
  const sbomUses = workflow.match(/uses: anchore\/sbom-action@[0-9a-f]{40}/g) ?? [];
  const emptyTokens = workflow.match(/^          github-token: ""$/gm) ?? [];
  assert.ok(sbomUses.length >= 2);
  assert.equal(emptyTokens.length, sbomUses.length);
  assert.ok(
    workflow.indexOf("- name: Remove temporary Apple signing keychain") <
      workflow.indexOf("- name: Generate Headroom runtime SBOM"),
  );
  assert.match(workflow, /event_type: "halv-release-published"/);
  assert.match(workflow, /repos\/\$WEBSITE_REPOSITORY\/dispatches/);
  const websiteStep = workflow.slice(refreshWebsiteIndex);
  assert.match(websiteStep, /continue-on-error: true/);
  assert.match(
    websiteStep,
    /secrets\.WEBSITE_REPO_TOKEN \|\| secrets\.RELEASE_REPO_TOKEN/,
  );
  assert.doesNotMatch(workflow, /def infer_platforms\(/);
});

test("manifest builder CLI writes the validated release file used by CI", (context) => {
  const temporaryDirectory = mkdtempSync(join(tmpdir(), "halv-updater-test-"));
  context.after(() => rmSync(temporaryDirectory, { recursive: true, force: true }));
  const releasePath = join(temporaryDirectory, "release.json");
  const signaturesPath = join(temporaryDirectory, "signatures");
  const outputPath = join(temporaryDirectory, "latest.json");
  mkdirSync(signaturesPath);
  writeFileSync(
    releasePath,
    JSON.stringify({
      ...releaseWithArtifacts(REQUIRED_ARTIFACTS),
      tag_name: "v0.1.2",
    }),
  );
  for (const [name, signature] of signaturesFor(REQUIRED_ARTIFACTS)) {
    writeFileSync(join(signaturesPath, name), signature);
  }

  const result = spawnSync(
    process.execPath,
    [
      "scripts/build-updater-manifest.mjs",
      "--release",
      releasePath,
      "--signatures",
      signaturesPath,
      "--repository",
      "pedr0v/halv-releases",
      "--tag",
      "v0.1.2",
      "--output",
      outputPath,
    ],
    { encoding: "utf8" },
  );

  assert.equal(result.status, 0, result.stderr);
  const manifest = JSON.parse(readFileSync(outputPath, "utf8"));
  assert.equal(manifest.version, "0.1.2");
  assert.ok(manifest.platforms["windows-x86_64-msi"]);
  assert.ok(manifest.platforms["linux-x86_64-appimage"]);
});
