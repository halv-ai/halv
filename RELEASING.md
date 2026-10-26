# Releasing Halv

Halv uses Tauri v2 updater artifacts hosted on GitHub Releases. From 1.0.2,
each release publishes to the Releases of `halv-ai/halv`. CI builds it from
the public code of this repository. Since 1.0.1, the updater tries two
endpoints in this order:

```text
https://github.com/halv-ai/halv/releases/latest/download/latest.json
https://github.com/pedr0v/halv-releases/releases/latest/download/latest.json
```

The updater skips an endpoint that returns an error status, such as 404, and
tries the next one. Halv 1.0.0 and older know only the `pedr0v/halv-releases`
endpoint. For each release, CI also puts `latest.json` into a release with the
same tag in `pedr0v/halv-releases`. The URLs in that file point to the assets
in `halv-ai/halv`, so old installs still find updates. Never delete or archive
`pedr0v/halv-releases`.

## 1. Generate updater signing keys

Run this once on an operator machine:

```sh
pnpm tauri signer generate -w ~/.tauri/halv-updater.key
```

Create a GitHub Actions environment named `release` and store the private key there as `TAURI_SIGNING_PRIVATE_KEY`. If you set a password, store it as `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.

`src-tauri/tauri.conf.json` contains the public key of the official Halv releases in `plugins.updater.pubkey`.
Do not change that key for official releases.

A fork that publishes its own releases must do these steps:

1. Generate its own key pair with the command above.
2. Put the new public key in `plugins.updater.pubkey`.
3. Point `plugins.updater.endpoints` to the `latest.json` file of its own releases repository.
4. In `.github/workflows/build.yml`, change the `github.repository` and
   `github.actor_id` checks of the release jobs.
5. Remove the step "Publish latest.json for Halv 1.0.0 and older". It writes
   to `pedr0v/halv-releases`.

If the key and the endpoint do not match signed release assets, the app cannot install updates.

## 2. Protected release environment and secrets

Allow the `release` environment to deploy only from `main`. Move all release-signing secrets into that environment, then delete any repository- or organization-level copies. Public repositories support required reviewers on all plans: require a release operator. Prevent self-review only when a second release operator exists, because the operator who sends the dispatch cannot approve it then. The workflow's stable `github.actor_id` allowlist is a second, in-repository authorization check. Update that ID if the release operator account changes. Keep the workflow's third-party actions pinned to full commit SHAs.

Required for updater releases:

- `TAURI_SIGNING_PRIVATE_KEY`
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` if the private key is encrypted
- `RELEASE_REPO_TOKEN`, a fine-grained token with contents:write scoped only to
  the public `pedr0v/halv-releases` repository. CI uses it only to put
  `latest.json` there for Halv 1.0.0 and older.

Required for the website refresh:

- `WEBSITE_REPO_TOKEN`, a fine-grained token with contents:write scoped only to
  the website repository. CI uses it only to send the `halv-release-published`
  dispatch.

Optional variable:

- `WEBSITE_REPOSITORY`, the repository that gets the website dispatch. The
  default is `pedr0v/halv-web`.

CI reads the Headroom fork from the public `halv-ai/headroom` repository and
needs no token for it. Delete the old `HEADROOM_REPO_TOKEN` secret.

Required for signed and notarized macOS artifacts:

- `APPLE_CERTIFICATE`
- `APPLE_CERTIFICATE_PASSWORD`
- `APPLE_SIGNING_IDENTITY`
- `APPLE_ID`
- `APPLE_PASSWORD`
- `APPLE_TEAM_ID`

Required secrets for Windows Artifact Signing:

- `AZURE_CLIENT_ID`
- `AZURE_CLIENT_SECRET`
- `AZURE_TENANT_ID`

Required variables for Windows Artifact Signing:

- `AZURE_ARTIFACT_SIGNING_ENDPOINT`
- `AZURE_ARTIFACT_SIGNING_ACCOUNT`
- `AZURE_ARTIFACT_SIGNING_CERTIFICATE_PROFILE`

Create the account and public trust profile with
[Azure Artifact Signing](https://learn.microsoft.com/azure/artifact-signing/quickstart).
Assign the service principal the `Artifact Signing Certificate Profile Signer`
role for the profile.

GitHub Actions provides `GITHUB_TOKEN`. The release jobs use it to create,
fill, and publish the release in `halv-ai/halv`. Each job gets only the
permissions that it needs:

| Job | `GITHUB_TOKEN` permissions | Secrets |
| --- | --- | --- |
| Validate release | contents: read | none |
| Authorize and prepare draft release | contents: write | none |
| Generate app SBOM | contents: read | none |
| Build | contents: write | Tauri, Apple, and Azure signing |
| Verify and publish release | contents: write, id-token: write, attestations: write | `RELEASE_REPO_TOKEN`, `WEBSITE_REPO_TOKEN` |

The SBOM tool (syft, through `anchore/sbom-action`) gets no token. The workflow
sets the `github-token` input of the action to an empty value. The tool runs
only in the app SBOM job, which can only read, and at the end of each build
job. A build job runs it after it uploaded its release files and removed the
Apple signing keychain. The jobs give the SBOMs to the publish job as workflow
artifacts.

Only `halv-ai/halv` runs the release jobs. A `release-halv` dispatch to another
repository, for example the private repository, runs only the tests.

## Local preflight before release

Enable the committed pre-push hook once per checkout, then run the fast gate
before pushing:

```sh
node scripts/setup-hooks.mjs
node scripts/preflight.mjs
```

Before dispatching a release, run the heavier macOS arm64 build/sign check and,
when configured, validate the local notarytool keychain credentials:

```sh
APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID1234)" \
HALV_PREFLIGHT_NOTARY_PROFILE=your-profile \
node scripts/preflight.mjs --release
```

On macOS arm64, the build/sign check fails when `APPLE_SIGNING_IDENTITY` is
empty. The release-only environmental checks are skipped when the login
keychain has no matching Developer ID certificate or named notary profile. Run this preflight before the
`release-halv` dispatch to catch local build, lint, signing, and credential
failures without consuming the three release jobs; macOS GitHub-hosted runners
consume credits at 10 times the base rate.

## Public export before release

The maintainer develops Halv in a private repository.
The private repository also holds the account server, the relay, and the phone app.
`scripts/export-public.mjs` copies the public part of one private commit into a
checkout of this repository. `scripts/public-export.json` controls the export:

- `exclude` lists the private paths.
- `overlayDir` holds files that only this repository gets, for example `.github/dependabot.yml`.
- `denyPaths`, `denyContent`, and `denyTokenHashes` stop the export when a private path,
  a secret, or a personal identifier would leak. `allowlist` lists the reviewed safe hits.
  The leak guard also decodes escaped text, for example `%2FUsers%2F<name>`, `\u002F`, or `&#x2F;`.
  It also stops when an exported file holds a value from `denyTokenHashes`.

`scripts/public-export.json` holds only the generic rules.
The private repository also has `scripts/public-export.private.json`.
That file holds the rules that name a private file or a person, for example the hashes for `denyTokenHashes`.
The script adds the lists of the private file to the lists of `scripts/public-export.json`.
The script never copies `scripts/public-export.private.json`.
`"requirePrivateConfig": true` stops the export when the private file is missing.
It also stops the export when the private file has an empty `exclude` or `denyTokenHashes` list.

The script reads only committed content through git. Uncommitted work never leaks.
It never pushes.

Do these steps in the private repository before each release:

0. Run `git -C ../oss pull --ff-only`.
1. Commit the release change.
2. Run `node scripts/export-public.mjs --dry-run`. Read the added, changed, and deleted paths.
   Read the commit message.
3. Make sure that each added path is public. The script refuses to add paths without `--accept-new`.
4. Run `node scripts/export-public.mjs --accept-new --commit`.
   The script commits in `../oss` with a `Source-Commit: <sha>` trailer.
   The commit message lists the subjects of the private commits since the last export.
   It lists only the commits that change an exported path.
   The script stops when the message names an `exclude` or `denyPaths` entry, for example `server/`.
   Then run again with `--omit-subjects` and `--message "<subject>"`.
5. Review the commit in `../oss`, then push it to `main` of `halv-ai/halv`.

The script runs `git fetch` in `../oss`. It stops when `../oss` is behind its upstream branch or has no upstream branch.
It also stops when the fetch does not finish in 60 seconds.
When `../oss` is behind, a dry run shows a warning and continues. `--offline` skips the fetch and the check.
The script reads the last exported commit from the newest `Source-Commit` trailer.
The first export after a manual sync needs `--since <private sha>`.
The script stops when this repository has commits after the last export, for example a merged pull request.
Apply those commits to the private repository first, then run again with `--allow-public-commits`.
`node scripts/export-public.mjs --check` exits with status 1 when this repository differs from the export.

## 3. Release flow

### Before the dispatch

Do these checks before each release:

1. Make sure that `halv-ai/headroom` has the Headroom commit of
   `scripts/headroom-runtime.lock.json`. Release CI cannot build a commit that
   is not there. This command must print the full commit SHA:

   ```sh
   gh api repos/halv-ai/headroom/commits/"$(jq -r .commit scripts/headroom-runtime.lock.json)" --jq .sha
   ```

2. Make sure that `halv-ai/halv` has the active tag ruleset for `v*`. See the
   end of this section. `gh api repos/halv-ai/halv/rulesets` must list a
   ruleset with `"target": "tag"`.
3. Make sure that the `release` environment of `halv-ai/halv` has the secrets
   of section 2 and no `HEADROOM_REPO_TOKEN`. To list them, run
   `gh secret list --env release -R halv-ai/halv`.
4. Make sure that the deployed website reads its release metadata from
   `halv-ai/halv`. From 1.0.2, a release in `pedr0v/halv-releases` holds only
   `latest.json`. A website that reads only `pedr0v/halv-releases` keeps the
   old version.

### Steps

1. In the private repository, update the version in `package.json`,
   `src-tauri/tauri.conf.json`, and `src-tauri/Cargo.toml`. Commit the change.
2. Export the commit to `../oss` and push it to `main` of `halv-ai/halv`. See
   "Public export before release". Wait for the Test check of that commit in
   `halv-ai/halv` to pass.
3. As the release operator, create the protected tag in `../oss` at the
   exported `main` commit and push it to `halv-ai/halv`. The tag in the private
   repository has no effect, because the exported commit has a different SHA.
   A tag push does not run the signing workflow.

   ```sh
   git -C ../oss fetch origin
   git -C ../oss tag v1.0.2 origin/main
   git -C ../oss push origin v1.0.2
   ```

4. Dispatch the trusted default-branch workflow with the matching tag:

   ```sh
   gh api --method POST repos/halv-ai/halv/dispatches --input - <<'JSON'
   {"event_type":"release-halv","client_payload":{"tag":"v1.0.2"}}
   JSON
   ```

5. Approve the pending `release` environment deployments. Each run asks three
   times: for the draft, for the builds, and for the publication. CI checks the
   stable release-operator account ID and tests the captured `main` commit. It
   verifies that all three source versions and the protected tag point to that
   exact SHA. Then it creates or safely reuses one draft release in
   `halv-ai/halv`.
6. GitHub Actions checks out the exact Headroom fork commit from the public
   `halv-ai/headroom` repository, as `scripts/headroom-runtime.lock.json`
   records it. It freezes the fork natively. Then it builds macOS arm64, macOS
   x64, Linux x64, and Windows x64 artifacts from the tagged Halv SHA.
   The Linux build creates DEB, RPM, and AppImage packages.
7. Tauri signs updater artifacts with `TAURI_SIGNING_PRIVATE_KEY`. Do not pass
   Tauri's `--no-sign` flag to a release target: it disables updater signatures
   as well as platform signing.
8. Each build job uploads its installers, updater archives, and `.sig` files to
   the draft. It also uploads the two runtime file manifests of its target:
   `runtime-files-<target>.sha256.json` and
   `runtime-files-<target>.signed.sha256.json`. Then it makes
   `sbom-headroom-runtime-<target>.cdx.json` and keeps it as a workflow
   artifact.
9. The app SBOM job makes `sbom-halv-app.cdx.json`, the SBOM of the Rust
   crates and npm packages of the app. It keeps the file as a workflow
   artifact.
10. The publish job revalidates the protected tag. Then it does these steps on
    the draft:
    1. It makes `latest.json` from the release assets and the `.sig` files.
       Publication fails closed unless the manifest contains macOS arm64 and
       x64, Linux AppImage, DEB, RPM, and Windows x64 MSI updater targets.
    2. It downloads the five SBOMs, checks that each one is a CycloneDX file,
       and uploads them to the draft.
    3. It checks that each target uploaded its runtime manifests and that both
       DMG installers are present.
    4. It downloads each asset of the draft and writes `SHA256SUMS`.
    5. It makes one build provenance attestation for the installers, the
       updater archives, the runtime manifests, the SBOMs, and `SHA256SUMS`.
    6. It publishes the release. It stops when an asset changed after it wrote
       `SHA256SUMS`.
    7. It makes sure that GitHub marks the release as the latest one. The
       updater reads `/releases/latest`. CI does not take the mark from a newer
       release.
11. CI puts the same `latest.json` into a release with the same tag in
    `pedr0v/halv-releases`, and marks that release as the latest one. It
    replaces the file only when the content changed. It uploads the new file
    under a temporary name first, then renames it.
12. Older releases stay public in both repositories. CI does not change them.
13. CI dispatches `halv-release-published` to `WEBSITE_REPOSITORY`. The website
    refreshes its checked-in fallback metadata and Cloudflare redeploys it,
    while the live page also reads the newest complete release directly from
    GitHub for immediate version, size, filename, and direct-installer links.
14. Clients check the GitHub Releases `latest.json`, download the matching signed artifact, install it, and relaunch.

If the publish job fails after publication, for example because
`RELEASE_REPO_TOKEN` expired, fix the cause and re-run the failed job. For a
published release, the job checks the latest mark, does the
`pedr0v/halv-releases` step, and sends the website dispatch again. If the job
fails before publication, a re-run replaces `latest.json`, the SBOMs, and
`SHA256SUMS` on the draft. The SBOM workflow artifacts stay for 30 days. After
that time, a re-run of only the publish job fails. Then re-run all jobs.

Create an active tag ruleset for `v*` that lets only release operators create tags and prevents everyone from updating or deleting them; do not grant the generic GitHub Actions app a bypass. The workflow never creates tags and no longer runs from tag-push content; `repository_dispatch` always uses the workflow on the default branch.
Because the ruleset blocks a move of the tag, a fix that needs a new `main`
commit after you pushed the tag needs a new patch version.

If updater signing secrets are missing, the build fails, the release stays a draft, and CI does not publish `latest.json`.

## Verify a release

Each release in `halv-ai/halv` has these verification assets:

- `SHA256SUMS`: the SHA-256 of each other asset, in `sha256sum` format. CI
  writes it from the assets that it downloads from the draft after all
  uploads.
- `runtime-files-<target>.sha256.json` and
  `runtime-files-<target>.signed.sha256.json` for each of the four targets.
  `REPRODUCING.md` tells how to compare a rebuild with them.
- `sbom-halv-app.cdx.json`: a CycloneDX SBOM of the Rust crates and npm
  packages of the app.
- `sbom-headroom-runtime-<target>.cdx.json`: a CycloneDX SBOM of the staged
  Headroom runtime of each target.
- A GitHub build provenance attestation for the installers, the updater
  archives, the runtime manifests, the SBOMs, and `SHA256SUMS`.

To check a release, do these steps:

1. Download the assets. To get only some files, add `-p <pattern>` for each
   file and `-p SHA256SUMS`:

   ```sh
   gh release download v1.0.2 -R halv-ai/halv -D halv-v1.0.2
   cd halv-v1.0.2
   ```

2. Check each file against `SHA256SUMS`. On macOS, use `shasum -a 256` instead
   of `sha256sum`. `--ignore-missing` skips the files that you did not
   download:

   ```sh
   sha256sum --ignore-missing -c SHA256SUMS
   ```

3. Check the attestation of `SHA256SUMS` and of each file that you use:

   ```sh
   gh attestation verify SHA256SUMS -R halv-ai/halv \
     --signer-workflow halv-ai/halv/.github/workflows/build.yml
   gh attestation verify Halv_1.0.2_aarch64.dmg -R halv-ai/halv \
     --signer-workflow halv-ai/halv/.github/workflows/build.yml
   ```

The `.sig` files and `latest.json` have no attestation of their own.
`SHA256SUMS` lists them, and `SHA256SUMS` has an attestation.

### What the runtime manifests describe

The runtime manifests describe the Headroom runtime folder of the build job:

- `runtime-files-<target>.sha256.json` describes the runtime before code
  signing.
- `runtime-files-<target>.signed.sha256.json` describes the runtime after
  code signing. `scripts/sign-headroom-runtime.mjs` writes it before Tauri
  bundles the runtime. On macOS with the signing secrets, this script signs
  each Mach-O file. On Linux, on Windows, and in an unsigned macOS build, it
  signs nothing and writes `"signing": "none"`.

On macOS, Tauri signs the app bundle, and the runtime files keep their bytes.
On Linux, Tauri does not sign the runtime files.

On Windows with Windows Artifact Signing on, Tauri signs each unsigned `.exe`
and `.dll` file of the runtime folder in place while it builds the MSI. Then
the build step "Write the signed Windows runtime manifest after Tauri signing"
writes `runtime-files-x86_64-pc-windows-msvc.signed.sha256.json` again from
the runtime folder, with `"signing": "authenticode"`. The step shows a warning
when Tauri changed other files. The copy of `runtime-files.signed.sha256.json`
inside the MSI is from before the build and keeps `"signing": "none"`. Without
Windows Artifact Signing, Tauri does not sign the runtime files.

Known gap: when the AppImage tool assembles the AppImage, it can rewrite the
run path of ELF files under `usr/lib`. Then files in the AppImage differ from
the signed manifest. No such change is known for the DMG, DEB, RPM, or MSI.

The build step "Compare bundled Headroom runtimes with the release manifest"
checks these claims for each release. It extracts each bundle and compares its
runtime with the signed manifest. It writes the result to the job summary and
does not stop the release. Read the result before you tell users that a bundle
matches its manifest.

## 4. macOS signing and notarization

macOS builds stay unsigned when any Apple secret is missing. To enable signed and notarized builds, export a Developer ID Application certificate as a password-protected `.p12`, base64-encode it into `APPLE_CERTIFICATE`, and set the matching password and signing identity. `APPLE_ID`, `APPLE_PASSWORD`, and `APPLE_TEAM_ID` must be valid for notarization.

## 5. Windows signing

The Windows release uses Azure Artifact Signing through Tauri's custom sign command.
The workflow verifies the pinned signing client before it runs.
CI rejects the release unless both `halv.exe` and the MSI have valid signatures.
CI also requires a trusted RFC 3161 timestamp on each signature.

This Azure flow avoids the legacy PFX setup.
Tauri limits that setup to OV certificates issued before June 1, 2023.
See the [Tauri Windows signing guide](https://v2.tauri.app/distribute/sign/windows/)
for the certificate limitation and SmartScreen behavior.

## 6. Frozen Headroom fork

Halv does not install `headroom-ai` from PyPI on customer devices. Each release
checks out the public fork `halv-ai/headroom` at the full Git SHA in
`scripts/headroom-runtime.lock.json`, verifies its `pyproject.toml` version and
`uv.lock` SHA-256, and creates a PyInstaller onedir runtime on the native target.
The complete directory is bundled as a Tauri resource, including Python,
compiled extensions, models shipped by dependencies, licenses, and immutable
build metadata. The runtime reports both the fork version and short Git SHA via
`--version` and `/stats`.

Release CI caches the verified unsigned runtime separately for each native
platform and target. The exact cache key covers the immutable runtime lock and
the builder, staging helper, entry point, and PyInstaller spec; there is no
partial-key fallback. A restored runtime is still smoke-tested and verified
before packaging, then signed and notarized fresh so signing material is never
stored in the cache. Changing any frozen-runtime input creates a new cache.

To update the fork coupled to Halv:

1. Push the intended Headroom commit to `halv-ai/headroom`; release CI cannot
   build a local-only commit.
2. Update the commit, versions, and `uv.lock` digest in
   `scripts/headroom-runtime.lock.json`.
3. Update the matching immutable checkout `ref` in `.github/workflows/build.yml`.
4. Run `pnpm test:headroom-runtime`. For a local native freeze, run:

   ```sh
   pnpm build:headroom-runtime -- --source /path/to/headroom
   pnpm verify:headroom-runtime
   ```

For signed macOS releases, Halv signs every Mach-O file in the frozen onedir
tree before Tauri seals the app bundle, then CI verifies the finished `.app`
recursively with `codesign --deep --strict`.

## 7. Release QA checklist

Do these manual checks on the release installers before you publish them.

### Download link (`halv://`)

The site opens `halv://welcome?d=<download_id>` after a download starts
(ADR 0021). Test the link on macOS, Windows (in the VM), and Linux.

Halv has no screen that shows the download id. Use these two checks:

- The file `attribution.json` in the app data directory holds the id that
  Halv accepted. Halv deletes this file after the server accepts `first_open`.
- The `download_id` column of the newest `first_open` row in the
  `journey_events` table on the server holds the id that Halv sent.

The app data directory is:

- macOS: `~/Library/Application Support/ai.halv.app`
- Windows: `%APPDATA%\ai.halv.app`
- Linux: `~/.local/share/ai.halv.app`

To open a link, use the command for the system:

- macOS: `open "<link>"`
- Windows: `start "" "<link>"`
- Linux: `xdg-open "<link>"`

#### Prepare the machine

1. Use a machine where no Halv dev build ran. A dev build on Windows or Linux
   registers `halv://` only when `HALV_REGISTER_LINK=1` is set. An older dev
   build always registered it, and its entry wins over the installed release app.
   To remove such an entry:
   - Windows: delete the registry key `HKCU\Software\Classes\halv`.
   - Linux: delete `~/.local/share/applications/halv-handler.desktop`. Delete
     the `x-scheme-handler/halv` line in `~/.config/mimeapps.list`.
2. Install Halv from the release installer. On Linux, test the AppImage and
   one of the DEB or RPM packages.
3. On Linux DEB and RPM, run
   `grep -E '^(Exec|MimeType)=' /usr/share/applications/Halv.desktop`.
   Make sure that `Exec` ends with `%u` and that `MimeType` has
   `x-scheme-handler/halv`. Without `%u`, the launcher starts Halv without
   the link.

#### Test A: link before the first open

Do this test on macOS, Windows, and the Linux DEB and RPM packages. An
AppImage registers the scheme only when it starts, so skip this test for it.

1. Quit Halv. Delete the app data directory.
2. Open `halv://welcome?d=bad.id`. Make sure that Halv shows no error and
   that `attribution.json` does not exist. Quit Halv.
3. Open `halv://welcome?d=test_123`. Make sure that Halv starts and that
   `attribution.json` holds `test_123`.
4. Set usage events to on. Make sure that the newest `first_open` row on the
   server has `download_id` with the value `test_123`.

#### Test B: link while Halv runs

Do this test on all systems. For the Linux AppImage, start the file once and
quit it first, so that it registers the scheme.

1. Quit Halv. Delete the app data directory. Start Halv. Do not set usage
   events to on.
2. Close the main window, so that Halv runs only in the tray.
3. Open `halv://welcome?d=bad.id`. Make sure that Halv shows no error and
   that `attribution.json` does not exist.
4. Open `halv://welcome?d=test_123`. Make sure that `attribution.json` holds
   `test_123`. On Windows and Linux, also make sure that the main window comes
   back and that no second Halv process starts.
5. Set usage events to on. Make sure that the newest `first_open` row on the
   server has `download_id` with the value `test_123`.

### Analytics v2 (ADR 0022)

Check that Halv sends the data in the "What is sent" list and no other
data. Do these checks on the release installer. Use a machine that has no
Halv dev build.

Test A runs across a UTC midnight. Do Tests B, C, and D on a second
machine, or after Test A.

#### Prepare traffic capture

Halv trusts only its own list of root certificates. A proxy certificate in
the system store has no effect, so `mitmproxy` cannot read the traffic of
Halv. The server log holds only the method and the path of a request. Use a
reverse proxy on loopback instead:

1. Quit Halv. Delete the app data directory.
2. Start the recorder:
   `mitmdump --mode reverse:https://api.halv.ai -p 8787 -w capture.flows`
3. Start Halv from a terminal with `HALV_API_URL=http://127.0.0.1:8787`.
   A release build reads this variable and accepts HTTP on loopback. On
   Windows, run `set HALV_API_URL=http://127.0.0.1:8787` in `cmd` first.
   Start Halv this way each time in this section, and before you sign in.
   A saved session keeps its own server URL, and Halv then skips the
   recorder.
4. Read the capture with `mitmdump -nr capture.flows --flow-detail 4`. Look
   only at the requests to `/v1/analytics/`. The recorder also sees the
   other requests to the account service.

The `journey_events` table on the server gives a second view of the same
data. Its `props` column holds the version 2 fields, `locale`, and
`consent_version` of each event.

#### Test A: list and traffic

The savings meter counts local days. The summary counts UTC days. Set the
time zone of the machine to UTC, so that both use the same day.

1. Start Halv as in step 3 of the capture preparation. Answer "Yes" to the
   usage events question.
2. Use Halv on this UTC day. Call it day D. Use at least one agent session,
   so that the savings meter shows a value for day D.
3. Compare the traffic with the "What is sent" list in Settings, Privacy.
   Make sure each event, field, and value in the traffic is in the list.
4. Keep the recorder running past UTC midnight. Then restart Halv, or wait
   up to 6 h. Halv can summarize day D only after day D ends.
5. Make sure the traffic has `usage_summarized` with `day` D. Make sure
   `tokens_saved` and `usd_saved` have at most 2 significant digits.
6. Compare `usd_saved` with the savings meter for day D only. Use the daily
   chart in the savings panel. Do not use the all-time value, because it
   also counts later days. A rounding difference of 5 percent is normal.
7. Make sure no connection from Halv goes to a PostHog host. Use a
   network monitor that lists host names, for example Little Snitch. In
   this test, Halv sends analytics to the recorder on loopback. The
   recorder sends them on to `api.halv.ai`.

#### Test B: version 1 install

1. Quit Halv. Delete the app data directory. Do this before you install
   the previous release.
2. Install the previous release. Answer "Yes" to the usage events question.
   Quit Halv.
3. Start the release candidate over the previous release. Do not answer
   the notice.
4. Use Halv. Make sure the traffic has no version 2 event:
   `usage_summarized`, `crash_detected`, `proxy_failed`, `app_updated`,
   `update_failed`.
5. Make sure the traffic has no `locale`, no `consent_version` 2, no
   `agent_kind`, no `auth_method`, and no `surface`.
6. Answer "No" to the notice. Make sure the traffic stays the same as in
   steps 4 and 5.

#### Test C: notice shows until you answer

1. Repeat steps 1 to 3 of Test B.
2. Make sure the banner "Halv now also sends a daily usage summary" shows.
   Make sure it does not block work. The banner does not show on the
   welcome screen, in the first-run card, or in the prepared first task.
   Finish these first.
3. Quit Halv without an answer. Start Halv. Make sure the banner shows
   again.
4. Answer "Yes". Quit Halv. Start Halv. Make sure the banner does not show
   again.
5. Trigger an event that this install did not send yet. A batch goes out
   only when Halv has a new event. Look at the capture for the events that
   Halv already sent. The Halv Cloud pitch card waits for the banner. If it
   did not show before, it shows right after "Yes", and Halv sends
   `trial_prompted` with `surface` `pitch`. Or keep the capture running to
   the next UTC day, and look at the `active_day` batch.
6. Make sure the new batch has `consent_version` 2 and `locale`.

#### Test D: no consent, no traffic

1. Quit Halv. Delete the app data directory.
2. Start Halv. Do not answer the usage events question.
3. Connect an agent account. Run one task. Quit Halv. Start Halv.
4. Make sure the capture has no request to `/v1/analytics/`.
5. In Settings, Privacy, set usage events to off. Do step 3 again. Make
   sure the capture still has no request to `/v1/analytics/`.
