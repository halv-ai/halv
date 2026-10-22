# Releasing Halv

Halv uses Tauri v2 updater artifacts hosted on GitHub Releases. The updater endpoint is:

```text
https://github.com/pedr0v/halv-releases/releases/latest/download/latest.json
```

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

If the key and the endpoint do not match signed release assets, the app cannot install updates.

## 2. Protected release environment and secrets

Allow the `release` environment to deploy only from `main`. Move all release-signing secrets into that environment, then delete any repository- or organization-level copies. Public repositories support required reviewers on all plans: require an operator and prevent self-review. The workflow's stable `github.actor_id` allowlist is a second, in-repository authorization check. Update that ID if the release operator account changes. Keep the workflow's third-party actions pinned to full commit SHAs.

Required for updater releases:

- `TAURI_SIGNING_PRIVATE_KEY`
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` if the private key is encrypted
- `HEADROOM_REPO_TOKEN`, a fine-grained read-only token scoped only to the
  private `pedr0v/headroom` repository
- `RELEASE_REPO_TOKEN`, a fine-grained token with contents:write scoped only to
  the public `pedr0v/halv-releases` repository

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

`GITHUB_TOKEN` is provided by GitHub Actions. Release artifacts and
`latest.json` publish to `pedr0v/halv-releases` with `RELEASE_REPO_TOKEN`.

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

## 3. Release flow

1. Update `package.json` and `src-tauri/tauri.conf.json` / `Cargo.toml` versions.
2. Merge the version change into protected `main` and wait for its Test check to pass.
3. As the release operator, create the protected tag at that exact `main` commit and push it. A tag push does not run the signing workflow.

   ```sh
   git tag v0.1.3
   git push origin v0.1.3
   ```

4. Dispatch the trusted default-branch workflow with the matching tag:

   ```sh
   gh api --method POST repos/halv-ai/halv/dispatches --input - <<'JSON'
   {"event_type":"release-halv","client_payload":{"tag":"v0.1.3"}}
   JSON
   ```

5. Approve pending `release` environment deployments when the repository plan supports reviewers. CI checks the stable release-operator account ID, tests the captured `main` commit, verifies that all three source versions and the protected tag point to that exact SHA, and creates or safely reuses one draft release.
6. GitHub Actions checks out the exact Headroom fork commit recorded in
   `scripts/headroom-runtime.lock.json`, freezes it natively, and builds macOS
   arm64, macOS x64, Linux x64, and Windows x64 artifacts from the tagged Halv SHA.
   The Linux build creates DEB, RPM, and AppImage packages.
7. Tauri signs updater artifacts with `TAURI_SIGNING_PRIVATE_KEY`. Do not pass
   Tauri's `--no-sign` flag to a release target: it disables updater signatures
   as well as platform signing.
8. CI revalidates the protected tag, assembles and uploads `latest.json` from
   the release assets and `.sig` files, then publishes the release. Publication
   fails closed unless the manifest contains macOS arm64 and x64, Linux
   AppImage, DEB, RPM, and Windows x64 MSI updater targets.
9. After publication, CI converts every older public release to a draft so only
   the newest version and its installers remain public. The operation is
   reversible; old artifacts are retained for maintainers rather than deleted.
10. CI dispatches `halv-release-published` to `pedr0v/halv-web`. The website
    refreshes its checked-in fallback metadata and Cloudflare redeploys it,
    while the live page also reads the newest complete release directly from
    GitHub for immediate version, size, filename, and direct-installer links.
11. Clients check the GitHub Releases `latest.json`, download the matching signed artifact, install it, and relaunch.

Create an active tag ruleset for `v*` that lets only release operators create tags and prevents everyone from updating or deleting them; do not grant the generic GitHub Actions app a bypass. The workflow never creates tags and no longer runs from tag-push content; `repository_dispatch` always uses the workflow on the default branch.

If updater signing secrets are missing, CI leaves the release as a draft and does not publish `latest.json`.

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
checks out the private fork at the full Git SHA in
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

1. Push the intended Headroom commit to `pedr0v/headroom`; release CI cannot
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
