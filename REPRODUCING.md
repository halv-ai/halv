# Reproducing the Headroom runtime

Halv ships a frozen copy of the Headroom proxy. The copy is a PyInstaller
"onedir" folder named `headroom-runtime`. This document tells you how to
rebuild that folder from source. It also tells you how to compare your
rebuild, file by file, with the folder in a Halv release.

On macOS the folder is `Halv.app/Contents/Resources/headroom-runtime`. On
Linux and Windows it is the `headroom-runtime` folder in the Halv resource
folder.

## Inputs

`scripts/headroom-runtime.lock.json` fixes the inputs of the build.

| Lock field | Meaning |
| --- | --- |
| `repository`, `gitUrl` | The Headroom fork that Halv ships. |
| `commit` | The full Git commit of the fork. |
| `packageVersion` | The `version` in the `pyproject.toml` of the fork. |
| `sourceVersion` | The value of `headroom.__version__` at that commit. |
| `runtimeVersion` | The version that `headroom-runtime --version` prints. |
| `uvLockSha256` | The SHA-256 of the `uv.lock` of the fork. The build stops on another file. |
| `pythonVersion` | The CPython version. The build uses only a CPython that uv manages. |
| `uvVersion` | The exact uv version. uv selects the CPython build, so the build stops on another uv. |
| `pyinstallerVersion` | The PyInstaller version. |
| `artifactFormat` | The layout of the frozen folder. |

These files fix the other inputs:

- `scripts/headroom-runtime/build-requirements.txt`: PyInstaller,
  pyinstaller-hooks-contrib, altgraph, macholib, pefile, pywin32-ctypes and
  setuptools. Each line has an exact version and SHA-256 hashes.
- `scripts/headroom-runtime/build-constraints.txt`: maturin 1.15.0, the build
  backend of the fork, with SHA-256 hashes.
- `uv.lock` in the fork: every Python dependency of the runtime, with hashes.
- `rust-toolchain.toml` in the fork: the Rust version. rustup reads this file
  when maturin compiles the `headroom._core` extension.
- `scripts/headroom-runtime/headroom-runtime.spec` and
  `scripts/headroom-runtime/entry.py`: the PyInstaller recipe and the entry
  point.

The build writes what it used into the `toolchain` block of
`headroom-runtime-build.json`. The runtime has a copy at
`_internal/headroom-runtime-build.json` and at `build-manifest.json`.
`headroom-runtime --build-info` prints it. The block holds:

- The uv version, and the Python version, build and interpreter path.
- The versions of PyInstaller, pyinstaller-hooks-contrib, altgraph, macholib,
  pefile and the maturin version that built the fork wheel.
- The output of `rustc -vV` and `cargo -V`, the Node.js version, and the
  operating system and architecture.
- On macOS, the SDK version, the SDK build and the Xcode version from `xcrun`
  and `xcodebuild`.
- In GitHub Actions, `ImageOS`, `ImageVersion`, `GITHUB_SHA`,
  `GITHUB_REPOSITORY` and the URL of the workflow run.
- The determinism settings (see below).

## Requirements

PyInstaller cannot build for another platform. Build on a native host.

| Target | Host that Halv uses in CI |
| --- | --- |
| `aarch64-apple-darwin` | GitHub `macos-15` (Apple silicon) |
| `x86_64-apple-darwin` | GitHub `macos-15-intel` |
| `x86_64-unknown-linux-gnu` | GitHub `ubuntu-22.04` |
| `x86_64-pc-windows-msvc` | GitHub `windows-2025` |

You need these tools:

- Git and Node.js 22 or later.
- rustup. The fork selects its Rust version through `rust-toolchain.toml`.
- uv at the exact `uvVersion` of the lock. Set the `UV` environment variable
  when that uv is not first on `PATH`. For example:

  ```sh
  UV_VERSION=$(node -p "require('./scripts/headroom-runtime.lock.json').uvVersion")
  curl -LsSf "https://astral.sh/uv/$UV_VERSION/install.sh" | sh
  ```

- A C toolchain: the Xcode command line tools on macOS, `build-essential`
  on Linux, or the MSVC build tools on Windows.

The build downloads from PyPI, from GitHub (the CPython build that uv
selects), from crates.io, and from `openaipublic.blob.core.windows.net`
(the tiktoken encodings). Each download has a pinned hash, except the Rust
crates, which `Cargo.lock` pins.

## Rebuild

1. Get the Halv source at the release that you want to check:

   ```sh
   git clone https://github.com/halv-ai/halv.git
   cd halv
   git checkout v1.0.2   # the release tag that you want to check
   ```

2. On Windows, keep LF line endings. The lock digest of `uv.lock` uses the
   LF bytes:

   ```sh
   git config --global core.autocrlf false
   git config --global core.eol lf
   ```

3. Clone the fork at the commit in the lock:

   ```sh
   GIT_URL=$(node -p "require('./scripts/headroom-runtime.lock.json').gitUrl")
   COMMIT=$(node -p "require('./scripts/headroom-runtime.lock.json').commit")
   git clone "$GIT_URL" ../headroom-source
   git -C ../headroom-source checkout --detach "$COMMIT"
   ```

   If the clone fails with "not found", the fork repository is not public
   yet. Open an issue in `halv-ai/halv`.

4. Build into a new folder:

   ```sh
   node scripts/build-headroom-runtime.mjs \
     --source ../headroom-source \
     --output-dir ../headroom-runtime-rebuild \
     --work-dir ../headroom-runtime-work \
     --manifest-dir ../headroom-runtime-manifests
   ```

   Without `--output-dir`, the build writes to
   `src-tauri/resources/headroom-runtime`, the folder that the Halv bundle
   uses. Without `--manifest-dir`, the release copy of the file manifest goes
   to `src-tauri/target/headroom-runtime-manifests`.

The build does these steps:

1. It stops if the fork has a different commit, a modified file, or an
   untracked file. PyInstaller reads the editable source tree, so an
   untracked file can get into the runtime. Ignored files stay out of this
   check.
2. It stops if `uv --version` is not the `uvVersion` of the lock.
3. It runs `uv python install <pythonVersion>` with the `only-managed`
   preference. It stops if the interpreter is not under `uv python dir`.
4. It creates a new environment, installs maturin from
   `build-constraints.txt`, and runs `uv sync --frozen` with the `proxy` and
   `code` extras. The fork wheel builds without build isolation, so the
   pinned maturin builds it.
5. It installs PyInstaller from `build-requirements.txt` with
   `--require-hashes`.
6. It puts the tiktoken encodings and the license texts of the Python
   distributions into the build folder.
7. It runs PyInstaller and copies the result to the output folder.
8. It writes `runtime-files.sha256.json` into the output folder, and a copy
   named `runtime-files-<target>.sha256.json` into the manifest folder.

### License texts

The build copies the license and notice files of each Python distribution in
the build environment into `_internal/licenses/python/<name>-<version>/`.
`_internal/licenses/python/INDEX.json` lists each distribution with its
version, its license metadata and its files. The value `"missing"` in `files`
means that the distribution has no license or notice file. The build-only
PyInstaller tools are not in the list, because they do not ship. The license
and notice files of the fork are in `_internal/licenses/headroom`.

### Determinism settings

The build sets these variables and records them in the `toolchain` block:

- `SOURCE_DATE_EPOCH`: the commit time of the fork commit.
- `PYTHONHASHSEED=0`.
- `RUSTFLAGS` with `--remap-path-prefix` for the home folder (`/home`),
  `CARGO_HOME` (`/cargo`), the fork checkout (`/headroom-source`), the
  Cargo target folder (`/cargo-target`) and the `rust-src` folder of the
  toolchain (`/rustc/<commit>`). Without these flags, the compiled
  `_core.abi3.so` holds absolute paths of the build machine. The build
  stops if one of these paths contains a space.
- On macOS, `RUSTFLAGS` also has `-C link-arg=-Wl,-S`. The Apple linker
  calculates the `LC_UUID` of `_core.abi3.so` before rustc strips the file.
  Without `-S`, the linker first writes a debug map with the paths of the
  objects in the Cargo target folder, and the length of these paths changes
  the UUID. The fork strips all symbols, so `-S` changes only the UUID and
  the ad-hoc signature hash of the page that holds it.
- `MACOSX_DEPLOYMENT_TARGET=11.0` on macOS.
- `PYINSTALLER_CONFIG_DIR` in the work folder. PyInstaller keeps a binary
  cache in this folder, so files from an earlier build or a parallel build
  in the user folder cannot get into the runtime.

The PyInstaller spec also replaces metadata files that hold build-specific
values:

- It removes `uv_cache.json` from each `.dist-info` folder. uv writes the
  install time into this file.
- In `direct_url.json` and in the maturin SBOM of `headroom-ai`, it replaces
  the checkout path with `/headroom-source`. The SBOM gets a serial number
  from the fork commit and the commit time as its timestamp.
- In each `RECORD` file, it removes the lines for console scripts outside
  `site-packages`, because these scripts hold the path of the build
  environment. It updates the hashes of the files that it replaced.
- In the `_sysconfigdata_*` module, it replaces the uv Python install folder
  with `/install`, the prefix of the standalone CPython build.

## Compare with a release

Each Halv release attaches these assets for each target:

- `runtime-files-<target>.sha256.json`: each file of the runtime before code
  signing, with its path, SHA-256, size, mode and a Mach-O flag. The build
  writes this file as `runtime-files.sha256.json` in the runtime folder.
- `runtime-files-<target>.signed.sha256.json`: the same list after code
  signing. On macOS each Mach-O entry also has its code directory hash
  (`cdhash`). On Linux and Windows the sign script does not sign the runtime
  files, so the hashes are the same as in the first file.
- `SHA256SUMS`: the SHA-256 of each release asset.
- An SBOM of the release.
- A GitHub build provenance attestation.

On macOS each Mach-O entry of both lists also has two hashes of a copy
without the code signature: `unsignedSha256` (the output of
`codesign --remove-signature`) and `unsignedLinkeditSha256` (the same copy
with the `__LINKEDIT` vmsize set to zero). `--macos-normalize` uses them when
you compare with a list.

The file lists exclude `runtime-files.sha256.json` and
`runtime-files.signed.sha256.json`. They have no timestamps, and their keys
are sorted, so two builds of the same files give the same list.

Get and check the assets (example for Apple silicon):

```sh
gh release download v1.0.2 -R halv-ai/halv \
  -p 'runtime-files-aarch64-apple-darwin*.json' -p SHA256SUMS
shasum -a 256 --ignore-missing -c SHA256SUMS
gh attestation verify runtime-files-aarch64-apple-darwin.sha256.json -R halv-ai/halv
```

Compare your rebuild with the unsigned list:

```sh
node scripts/verify-runtime-manifest.mjs ../headroom-runtime-rebuild \
  --manifest runtime-files-aarch64-apple-darwin.sha256.json
```

On macOS, compare your rebuild with the signed list. `--macos-normalize`
removes the signature from a copy of each Mach-O file that differs and
compares the copy with the unsigned hashes in the list:

```sh
node scripts/verify-runtime-manifest.mjs ../headroom-runtime-rebuild \
  --manifest runtime-files-aarch64-apple-darwin.signed.sha256.json \
  --macos-normalize
```

Check that an installed Halv has the signed files of the release:

```sh
node scripts/verify-runtime-manifest.mjs \
  /Applications/Halv.app/Contents/Resources/headroom-runtime \
  --manifest runtime-files-aarch64-apple-darwin.signed.sha256.json
```

Compare your rebuild with an installed, signed Halv on macOS:

```sh
node scripts/verify-runtime-manifest.mjs ../headroom-runtime-rebuild \
  --reference /Applications/Halv.app/Contents/Resources/headroom-runtime \
  --macos-normalize
```

The script prints a table with one line for each result, then the paths
that do not match. It exits with 0 on a pass, 1 on a fail and 2 on a usage
error.

| Result | Pass | Meaning |
| --- | --- | --- |
| exact match | yes | Same SHA-256, size and mode. |
| Mach-O signature only | yes | The files are equal after `codesign --remove-signature` on copies of both files. |
| Mach-O `__LINKEDIT` vmsize only | yes | After signature removal, only the `__LINKEDIT` vmsize field is different. |
| build metadata toolchain block only | yes | `build-manifest.json` or `headroom-runtime-build.json` differ only in the `toolchain` block, which describes the build machine. |
| file mode differs | no | Same bytes, other permission bits. |
| content differs | no | Different bytes. |
| missing from runtime | no | The expected list has the file, the runtime folder does not. |
| not in expected list | no | The runtime folder has a file that the expected list does not have. |

## macOS signature caveat

Halv signs each Mach-O file of the runtime with its Developer ID, the
hardened runtime option and a secure timestamp. The signature is inside each
file, so a signed file and your unsigned rebuild have different bytes. You
cannot make the Developer ID signature yourself.

Use one of these two checks:

- Compare your rebuild with `runtime-files-<target>.sha256.json`. This list
  is from the same CI job, before the signature step. PyInstaller gives each
  Mach-O file an ad-hoc signature, and your rebuild gets the same ad-hoc
  signature.
- Compare your rebuild with the signed list or with an installed Halv, with
  `--macos-normalize`. The script removes the signatures from copies and
  compares again. `codesign --remove-signature` makes the file shorter, but
  it keeps the `__LINKEDIT` vmsize of the larger signature. The script
  reports such files apart.

To connect an installed file to the release, compare its `CDHash` from
`codesign -dvvv <file>` with the `cdhash` in
`runtime-files-<target>.signed.sha256.json`.

## Known residual differences

Halv does not claim a byte-for-byte equal folder on every machine. These are
the measured results and the known differences.

Measured on one Apple silicon Mac (macOS 26, Xcode 26.2) with this build
script: two builds of the same lock, from two fork clones, with different work
folders and different uv Python folders. Of 4,672 files, 4,670 were exactly
equal. This includes all 222 Mach-O files: `_core.abi3.so`,
`libpython3.12.dylib` and the `headroom-runtime` executable. The other two
files were `build-manifest.json` and `_internal/headroom-runtime-build.json`,
which differ only in the `toolchain` block.

Known differences:

- The `toolchain` block. It records paths and tool versions of the build
  machine, so it is different on almost every machine. The comparison script
  reports it as "build metadata toolchain block only" and accepts it when the
  rest of the file is equal.
- The Developer ID signature on macOS. See the signature caveat above.
- The `__LINKEDIT` vmsize after signature removal. The comparison script
  reports such files apart.
- Another build host. The measurement above used one machine. A different
  Xcode, SDK or linker version can change `_core.abi3.so` and the
  `headroom-runtime` executable. The `toolchain` block records `ImageOS`,
  `ImageVersion` and the Xcode and SDK versions of the CI host, so you can
  use the same host image.
- Linux and Windows. Halv did not measure these targets. On Windows the
  linker can write a time stamp into the PE header of `_core.pyd` and the
  executable, so expect differences in these files.
- Releases built before this build script. The Halv 1.0.1 runtime was built
  without the path remaps, the `-Wl,-S` linker flag and the metadata
  normalization. Its `_core.abi3.so` holds absolute build paths, and its
  `.dist-info` folders hold build paths and install times. It also has no
  `licenses/python` folder and no `tiktoken-cache` folder. A rebuild with this
  script does not match that runtime in these files.

## Runtime downloads

The runtime contains the tiktoken encodings that Headroom uses (`cl100k_base`,
`o200k_base`, `p50k_base`, `r50k_base`) in `_internal/tiktoken-cache`, and the
LiteLLM model cost map. `entry.py` sets `TIKTOKEN_CACHE_DIR` to that folder
and `LITELLM_LOCAL_MODEL_COST_MAP=True`. These values replace values from the
parent environment, so neither file downloads at run time.

Some features download models from Hugging Face on first use, into
`HF_HOME` (default `~/.cache/huggingface`). The fork pins each download to a
commit (`_PINNED_REVISIONS` in `headroom/onnx_runtime.py`). The fork turns the
pins off when `HEADROOM_HF_PIN=off`. `entry.py` sets `HEADROOM_HF_PIN=on`
and replaces a parent value, so the pins always apply in the Halv runtime.

The table shows the downloads that the Halv runtime can make, with the pins
of the fork at the time of writing. `FORK.md` in the fork lists every pin at
the lock commit. An older fork commit without `FORK.md` pins fewer
repositories, for example not `answerdotai/ModernBERT-base`; read
`_PINNED_REVISIONS` at that commit. `headroom-runtime --offline-self-check`
prints the pins (`hfPins`) of the runtime that you have.

| Feature | Hugging Face repository | Revision | Files and size | When it downloads | How to turn it off |
| --- | --- | --- | --- | --- | --- |
| Kompress text compression | `chopratejas/kompress-v2-base` | `b1563631b35bfdcee37587ad530147497d820d4c` | `onnx/kompress-int8-wo.onnx` (274.0 MB). If that file does not load, `onnx/kompress-fp32.onnx` (600.6 MB). | Kompress is on by default. The proxy starts the download in the background when it starts with optimization on, and it loads the model on first use. | `HEADROOM_DISABLE_KOMPRESS=1` or `--disable-kompress` |
| Kompress tokenizer | `answerdotai/ModernBERT-base` | `8949b909ec900327062f0ebf497f51aef5e6f0c8` | `tokenizer.json` and three small configuration files (2.2 MB) | Together with Kompress | Same as Kompress |
| Image compression router | `chopratejas/technique-router-onnx` | `27b0b4bfa510a1cff66d888072c0b807082721a8` | `model_quantized.onnx`, `tokenizer.json`, `config.json` (34.7 MB) | Image compression is on by default. The download starts on the first request that has an image. | Leave `image` out of `HEADROOM_COMPRESSORS` |
| Image encoder (SigLIP) | `chopratejas/siglip-image-encoder-onnx` | `d0a9fbd66d4bd8c761bff592d44831f7c2ae184e` | `image_encoder_int8.onnx` (99.1 MB) | Together with the image compression router | Same as image compression |
| Memory embedder | `Qdrant/all-MiniLM-L6-v2-onnx` | `5f1b8cd78bc4fb444dd171e59b18f3a3af89a079` | `model.onnx`, `tokenizer.json` (91.1 MB) | Only with persistent memory (`--memory` or `--learn`). Halv does not start the proxy with these options. | Memory is off by default |

These settings stop the downloads:

- `HEADROOM_OFFLINE=1` stops all outbound traffic of the proxy, which
  includes the Hugging Face downloads.
- `HEADROOM_DISABLE_KOMPRESS=1` (or `--disable-kompress`) turns Kompress off.
- When you pause optimization in Halv, Halv starts the proxy with
  `--no-optimize`, and the proxy does not compress.

## Telemetry settings

- The fork can upload an anonymous session summary (the "beacon"). Upstream
  Headroom turns the beacon on by default. The fork turns it off by default
  (`BEACON_DEFAULT_ON = False` in `headroom/telemetry/beacon.py`), so the
  beacon uploads nothing until a user sets `HEADROOM_BEACON=on`.
  `DO_NOT_TRACK=1` and `HEADROOM_OFFLINE=1` keep it off. An older fork commit
  without `FORK.md` keeps the upstream default. For those commits, the
  environment variables below keep the beacon off.
- `entry.py` sets `HEADROOM_BEACON=off`, `DO_NOT_TRACK=1`,
  `HEADROOM_UPDATE_CHECK=off`, `HEADROOM_TELEMETRY=off` and
  `HEADROOM_TELEMETRY_WARN=off` when the parent environment does not set
  them.
- Halv sets `HEADROOM_BEACON=off`, `DO_NOT_TRACK=1` and
  `HEADROOM_UPDATE_CHECK=off` when it starts the proxy
  (`HEADROOM_PRIVACY_ENV` in `src-tauri/src/sidecar/mod.rs`). These values
  replace values from the user's environment. See ADR 0021.
- `headroom-runtime --offline-self-check` reports the beacon state and fails
  if the beacon is on.
