# -*- mode: python ; coding: utf-8 -*-
"""PyInstaller onedir recipe for Halv's pinned Headroom fork."""

import os
from pathlib import Path
import sys

from PyInstaller.utils.hooks import (
    collect_all,
    collect_data_files,
    collect_submodules,
    copy_metadata,
)


SCRIPT_DIR = Path(SPECPATH).resolve()
SOURCE_DIR = Path(os.environ["HALV_HEADROOM_SOURCE_DIR"]).resolve()
BUILD_MANIFEST = Path(os.environ["HALV_HEADROOM_BUILD_MANIFEST"]).resolve()
# Offline data and license texts that the build script prepares before
# PyInstaller runs (scripts/headroom-runtime/halv_runtime_build.py).
LICENSES_DIR = Path(os.environ["HALV_HEADROOM_LICENSES_DIR"]).resolve()
TIKTOKEN_DIR = Path(os.environ["HALV_HEADROOM_TIKTOKEN_DIR"]).resolve()
METADATA_DIR = Path(os.environ["HALV_HEADROOM_METADATA_DIR"]).resolve()
REPOSITORY = os.environ["HALV_HEADROOM_REPOSITORY"]
COMMIT = os.environ["HALV_HEADROOM_COMMIT"]
SOURCE_DATE_EPOCH = int(os.environ["SOURCE_DATE_EPOCH"])

sys.path.insert(0, str(SCRIPT_DIR))
from halv_runtime_build import normalize_metadata_toc, normalize_sysconfig_toc  # noqa: E402

for required in (LICENSES_DIR / "INDEX.json", TIKTOKEN_DIR):
    if not required.exists():
        raise SystemExit(f"headroom-runtime.spec: missing build input {required}")

datas = [
    (str(BUILD_MANIFEST), "."),
    (str(LICENSES_DIR), "licenses/python"),
    (str(TIKTOKEN_DIR), "tiktoken-cache"),
]
binaries = []
hiddenimports = []


def _merge(packages, *, data_only=()):
    global datas, binaries, hiddenimports
    for package in packages:
        try:
            package_data, package_binaries, package_hidden = collect_all(package)
        except Exception:
            continue
        datas += package_data
        binaries += package_binaries
        hiddenimports += package_hidden
    for package in data_only:
        try:
            datas += collect_data_files(package)
        except Exception:
            pass


_merge(
    [
        "headroom",
        "magika",
        "onnxruntime",
        "sqlite_vec",
        "tiktoken",
        "tiktoken_ext",
        "tree_sitter",
        "tree_sitter_language_pack",
    ]
)

for package in (
    "uvicorn",
    "fastapi",
    "starlette",
    "websockets",
    "zstandard",
    "tokenizers",
    "huggingface_hub",
    "transformers",
    "safetensors",
    "anyio",
    "sse_starlette",
    "hf_xet",
):
    try:
        hiddenimports += collect_submodules(package)
    except Exception:
        pass

_merge(
    [],
    data_only=(
        "transformers",
        "huggingface_hub",
        "litellm",
        "certifi",
    ),
)

for distribution in (
    "headroom-ai",
    "transformers",
    "tokenizers",
    "huggingface-hub",
    "safetensors",
    "tiktoken",
    "regex",
    "requests",
    "packaging",
    "filelock",
    "numpy",
    "pyyaml",
    "tqdm",
    "onnxruntime",
    "litellm",
    "fastapi",
    "uvicorn",
    "openai",
    "httpx",
    "aiohttp",
    "pydantic",
    "hf-xet",
    "tree-sitter",
    "tree-sitter-language-pack",
):
    try:
        datas += copy_metadata(distribution)
    except Exception:
        pass

for license_name in ("LICENSE", "NOTICE"):
    license_path = SOURCE_DIR / license_name
    if license_path.is_file():
        datas.append((str(license_path), "licenses/headroom"))

excludes = [
    "torch",
    "torchvision",
    "torchaudio",
    "tensorflow",
    "jax",
    "flax",
    "matplotlib",
    "tkinter",
    "PyQt5",
    "PyQt6",
    "PySide2",
    "PySide6",
    "IPython",
    "jupyter",
    "notebook",
    "pytest",
    "_pytest",
    "unittest.mock",
]

analysis = Analysis(
    [str(SCRIPT_DIR / "entry.py")],
    pathex=[str(SOURCE_DIR)],
    binaries=binaries,
    datas=datas,
    hiddenimports=hiddenimports,
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=excludes,
    noarchive=False,
    optimize=0,
)

# uv writes its Python install folder into the sysconfig data module.
pure_modules = normalize_sysconfig_toc(
    list(analysis.pure),
    METADATA_DIR / "sysconfig",
    [os.path.realpath(sys.base_prefix), sys.base_prefix],
)

python_archive = PYZ(pure_modules)

executable = EXE(
    python_archive,
    analysis.scripts,
    [],
    exclude_binaries=True,
    name="headroom-runtime",
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=False,
    console=True,
    disable_windowed_traceback=False,
    argv_emulation=False,
    target_arch=None,
    codesign_identity=None,
    entitlements_file=None,
)

# Replace dist-info files that carry the build path or the install time.
runtime_datas = normalize_metadata_toc(
    list(analysis.datas),
    METADATA_DIR / "dist-info",
    SOURCE_DIR,
    REPOSITORY,
    COMMIT,
    SOURCE_DATE_EPOCH,
)

collection = COLLECT(
    executable,
    analysis.binaries,
    runtime_datas,
    strip=False,
    upx=False,
    upx_exclude=[],
    name="headroom-runtime",
)
