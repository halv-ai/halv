# Embedded Dot panel

The fixed Dot pin opens a native child WebView inside Halv's main window,
loading `https://chatgpt.com/dots`. It does not open the installed ChatGPT app.
The same ChatGPT account can access its existing cloud Dot after signing in.
Halv does not import the desktop app's cookies, credentials, name, or avatar.

## Implementation

- `DotShortcut` selects `DotPanel`; the underlying conversation stays mounted.
- `dot_webview.rs` creates a `dot-remote` native child WebView using Tauri's
  `unstable` multi-WebView API. Layout/hide requests are serialized.
- Closing the panel hides its WebView; reopening preserves the page and login.
- Dot occupies the normal conversation area without a separate header or back button.
  Selecting another conversation or New chat hides the Dot and returns to that view.
- The remote view is hidden while Halv dialogs listed in App.tsx are open.
- Popups navigate inside the same child view. OAuth providers that require a
  separate popup/opener handshake may not work; email login is the initial path.
- Remote navigation permits HTTPS only. Local/app origins are rejected.
- Desktop capabilities match `webviews: ["main"]`, not all webviews in the
  main window. The remote page receives no Tauri capability. Dot commands also
  verify that their caller is the main WebView.

## Verified and pending

On 2026-10-01, native macOS QA reached the official OpenAI password login page
inside the Halv window. The user completed login and native QA then confirmed
the account name, the avatar, existing conversation history, and composer inside Halv.
The installed ChatGPT application was not used for this flow.

Session persistence across app restarts, social
login, account eligibility, and Windows/Linux remain pending. Dots require an
eligible account. The web route is not a public embedding/API contract and may
change. The local browser harness explicitly shows a preview placeholder and
must not be cited as live Dot integration.

## Checks

- `pnpm build`
- `node --experimental-strip-types --test tests/ipcContract.test.ts`
- `cargo test --manifest-path src-tauri/Cargo.toml dot_webview::tests --lib --quiet`
- `pnpm test:i18n`

Official product documentation: https://learn.chatgpt.com/docs/dots and
https://learn.chatgpt.com/docs/dots/channels.

After the native capture, the extra header/back button was removed at the user’s
request. Build passed; CUA timed out before the follow-up native capture. A process
sample showed an idle main event loop, without evidence of a main-thread deadlock.
