# Third-party notices

Halv is licensed under the Apache License, Version 2.0 (see `LICENSE`).
Halv includes code, icons, and programs from other projects.
This file lists each component, its license, and the files that contain it.
The app bundle includes this file and each license text in its
`licenses/` resource folder.

## Orca

- Project: Orca (https://github.com/stablyai/orca)
- License: MIT
- Copyright (c) 2026 Lovecast Inc.

Halv adapts code, techniques, and layouts from Orca.
These files contain the adapted code:

Terminal:

- `src/terminalSurface.ts` adapts the terminal mode reset and contrast
  techniques.
- `src/terminalLayout.ts` adapts the binary terminal split model.

Account wallet:

- `src-tauri/src/wallet/` keeps one CLI folder for each account.
  It links shared resources and history into each folder.
  It applies the Orca settings filter and the Codex daemon socket guard.
  `src-tauri/src/wallet/orca.rs` also mirrors the Orca account store schema.
- `src/accountWallet.ts` and `src/components/AccountWallet.tsx` show the
  account switcher.
- `src/accountRestart.ts` and `src/components/AccountRestartChip.tsx` adapt
  the Codex restart notice.

Status bar:

- `src/statusBar.ts` and `src/components/StatusBar.tsx` adapt the status bar.
- `src/appUpdateCard.ts` and `src/components/AppUpdateSegment.tsx` adapt the
  update card and the update status segment.
- `src/resourceUsage.ts` and `src/components/ResourceUsageSegment.tsx` adapt
  the resource usage segment.

Agent pane status:

- `src-tauri/src/agent_status/title.rs` adapts the agent title rules.
- `src-tauri/src/agent_status/machine.rs` adapts the hook timing rules.
- `src-tauri/src/agent_status/mod.rs` uses Orca terminal captures as test
  data.
- `src/agentPaneStatus.ts` and `src/components/AgentStateGlyph.tsx` adapt
  the agent state vocabulary, order, and glyphs.

Folder projects:

- `src-tauri/src/projects/nested_repos.rs` adapts the nested repository
  scan rules.
- `src/folderProjects.ts` adapts the folder project groups.

Project tree and workspace tabs:

- `src/projectTree.ts`
- `src/projectTreeMenus.ts`
- `src/projectTreeOperations.ts`
- `src/projectTreeState.ts`
- `src/shortAge.ts`
- `src/workspaceTabHistory.ts`
- `src/workspaceTabStrip.ts`
- `src/workspaceTabs.ts`
- `src/components/ProjectTree.tsx`
- `src/components/ProjectTreeDialogs.tsx`
- `src/components/RecentTabSwitcher.tsx`
- `src/components/WorkspaceLauncher.tsx`
- `src/components/WorkspaceRunButton.tsx`
- `src/components/WorkspaceTabContextMenu.tsx`
- `src/components/WorkspaceTabHoverCard.tsx`
- `src/components/WorkspaceTabStrip.tsx`

Styles:

- `src/styles.css` contains blocks that adapt Orca styles. A comment that
  names Orca marks each block.

Some other files name Orca only to read or remove `ORCA_*` environment
variables, or to talk to a running Orca app. Those files do not contain
Orca code.

```
MIT License

Copyright (c) 2026 Lovecast Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Ponytail

- Project: Ponytail (https://github.com/DietrichGebert/ponytail)
- License: MIT
- Copyright (c) 2026 DietrichGebert

Halv vendors Ponytail at commit 2ed6c52. These files contain the vendored
code:

- `src-tauri/src/ponytail/SKILL.md`
- `src-tauri/src/ponytail.rs`

```
MIT License

Copyright (c) 2026 DietrichGebert

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Lucide

- Project: Lucide (https://github.com/lucide-icons/lucide)
- License: ISC. Some icons that Lucide derives from Feather are under MIT.
- Copyright (c) 2026 Lucide Icons and Contributors

Halv copies Lucide icon paths into these files:

- `src/components/AgentStateGlyph.tsx` (CircleCheck, CircleDashed,
  MessageCircleQuestionMark)
- `src/components/ProjectTree.tsx` (History, FolderTree, Folder)

```
ISC License

Copyright (c) 2026 Lucide Icons and Contributors

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

---

The following Lucide icons are derived from the Feather project:

airplay, alert-circle, alert-octagon, alert-triangle, aperture, arrow-down-circle, arrow-down-left, arrow-down-right, arrow-down, arrow-left-circle, arrow-left, arrow-right-circle, arrow-right, arrow-up-circle, arrow-up-left, arrow-up-right, arrow-up, at-sign, calendar, cast, check, chevron-down, chevron-left, chevron-right, chevron-up, chevrons-down, chevrons-left, chevrons-right, chevrons-up, circle, clipboard, clock, code, columns, command, compass, corner-down-left, corner-down-right, corner-left-down, corner-left-up, corner-right-down, corner-right-up, corner-up-left, corner-up-right, crosshair, database, divide-circle, divide-square, dollar-sign, download, external-link, feather, frown, hash, headphones, help-circle, info, italic, key, layout, life-buoy, link-2, link, loader, lock, log-in, log-out, maximize, meh, minimize, minimize-2, minus-circle, minus-square, minus, monitor, moon, more-horizontal, more-vertical, move, music, navigation-2, navigation, octagon, pause-circle, percent, plus-circle, plus-square, plus, power, radio, rss, search, server, share, shopping-bag, sidebar, smartphone, smile, square, table-2, tablet, target, terminal, trash-2, trash, triangle, tv, type, upload, x-circle, x-octagon, x-square, x, zoom-in, zoom-out

The MIT License (MIT) (for the icons listed above)

Copyright (c) 2013-present Cole Bemis

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Crux

- Project: Crux (https://github.com/pedr0v/crux)
- License: MIT
- Copyright (c) 2026 Pedro Villaca

The installers bundle the Crux v0.12.0 binary.
The binary statically links the Rust crates that Crux depends on, under their
own licenses.
`licenses/THIRD_PARTY_LICENSES.md` does not list these crates.
The `Cargo.lock` file in the Crux v0.12.0 source lists them.

```
MIT License

Copyright (c) 2026 Pedro Villaca

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## RTK

- Project: RTK (https://github.com/rtk-ai/rtk)
- License: Apache-2.0
- Copyright 2024 rtk-ai and rtk-ai Labs

The app bundles the RTK 0.42.4 binary.
`src-tauri/resources/licenses/rtk-0.42.4-LICENSE` contains the license text.
The binary statically links the Rust crates that RTK depends on, under their
own licenses. It also contains a bundled copy of SQLite, which is in the
public domain.
The x86_64 Linux binary is a static musl build. It also contains musl libc
code, under the MIT license.
`licenses/THIRD_PARTY_LICENSES.md` does not list these crates.
The `Cargo.lock` file in the RTK 0.42.4 source lists them.

## Headroom

- License: Apache-2.0
- Copyright 2025 Headroom Contributors. Modified by Halv.

The app bundles a modified Headroom runtime.
The runtime data folder contains the Headroom `LICENSE` and `NOTICE` files in
`licenses/headroom/`.

## Runtime dependencies

The app uses Rust crates, npm packages, and Python packages under their own
licenses. These licenses include MIT, Apache-2.0, BSD, ISC, Unicode-3.0,
MPL-2.0, CDLA-Permissive-2.0, PSF, Zlib, 0BSD, CC0, Artistic-2.0, and WTFPL.
`src-tauri/Cargo.lock` and `src-tauri/supervisor/Cargo.lock` list the Rust
crates. `pnpm-lock.yaml` lists the npm packages.
`scripts/headroom-runtime.lock.json` pins the Headroom fork, and its
`uv.lock` lists the Python packages. The Headroom runtime directory contains
the package metadata of its main Python packages, with their license files.

`licenses/THIRD_PARTY_LICENSES.md` in the app bundle gives the license text of
each Rust crate and npm package that the Halv app contains. The Rust crates
include those of the `halv-supervisor` sidecar. The file also gives the
license texts of native code that some crates compile or link into the app,
for example libdbus on Linux and the Microsoft WebView2 loader on Windows.
The file does not list the Rust crates that the RTK and Crux binaries link.
The RTK and Crux sections above tell where to find them.
`scripts/collect-third-party-licenses.mjs` makes that file from the lock files.
Run `pnpm licenses:collect` after a dependency change.
`pnpm check:third-party-licenses` fails when the file is not current.
