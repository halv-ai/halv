# Standard license texts

This folder holds the standard text of each SPDX license that
`scripts/collect-third-party-licenses.mjs` can use as a fallback.
The collector uses a text from this folder only when a package has no
license file of its own.

Each file name is the SPDX license identifier with the `.txt` extension.
The texts come from the SPDX License List data, release v3.29.0
(https://github.com/spdx/license-list-data/tree/v3.29.0/text).
Do not change the texts. To add a license, copy its file from the same
release.

The MIT, ISC, BSD-2-Clause, BSD-3-Clause, and 0BSD texts have a template
copyright line. The collector writes the package authors into that line.
If a package has no authors, the collector stops with an error.

## Package license texts

The `packages/` folder holds license texts that a published package does not
contain. `EXTRA_LICENSE_TEXTS` in the collector names the packages that use
each file. Do not change these texts. Copy a new text from the source below.

| File | Source | Packages |
| --- | --- | --- |
| `packages/rust-alloc-no-stdlib/LICENSE` | `LICENSE` in github.com/dropbox/rust-alloc-no-stdlib at commit ae42d22078b98549e987d2f03d12df7b984fde47 (the alloc-stdlib 0.2.4 release) | alloc-stdlib |
| `packages/dlopen2/LICENSE` | `LICENSE` in github.com/OpenByteDev/dlopen2 at commit cc80e4a0a90d499b677fdf7743699b4b3a43a989 (the dlopen2 0.8.2 release) | dlopen2, dlopen2_derive |
| `packages/objc2/LICENSE.md` | `LICENSE.md` in github.com/madsmtm/objc2 at commit 8852b424193ca41602281b3d7540d7c8ed51e49a (the objc2 0.6.4 release). The file is the same at the release commits of the other three crates. | objc2, objc2-encode, objc2-foundation, block2 |
| `packages/objc2/LICENSE-MIT.txt` | `LICENSE-MIT.txt` in github.com/madsmtm/objc2 at commit ee9a7ada2131f5944b8750428e265c15632f2a19. This commit added the MIT text with the copyright holder. | objc2, objc2-encode, objc2-foundation, block2 |
| `packages/webview2-rs/LICENSE` | `LICENSE` in github.com/wravery/webview2-rs at commit b74dc5e2b394044bea5191052868ce7a106c202c (the webview2-com 0.38.2 release) | webview2-com, webview2-com-macros, webview2-com-sys |
| `packages/webview2-sdk/LICENSE.txt`, `packages/webview2-sdk/NOTICE.txt` | The Microsoft.Web.WebView2 1.0.3650.58 NuGet package (SHA-256 911a472128c82ac8baa0c486c23342cc9dd6e7dc50d754e676726642ca065c60). Its `build/native/x64/WebView2LoaderStatic.lib` is equal to the file in webview2-com-sys 0.38.2. | webview2-com-sys |
