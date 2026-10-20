# -*- mode: python ; coding: utf-8 -*-
"""PyInstaller onedir recipe for Halv's pinned Headroom fork."""

import os
from pathlib import Path

from PyInstaller.utils.hooks import (
    collect_all,
    collect_data_files,
    collect_submodules,
    copy_metadata,
)


SCRIPT_DIR = Path(SPECPATH).resolve()
SOURCE_DIR = Path(os.environ["HALV_HEADROOM_SOURCE_DIR"]).resolve()
BUILD_MANIFEST = Path(os.environ["HALV_HEADROOM_BUILD_MANIFEST"]).resolve()

datas = [(str(BUILD_MANIFEST), ".")]
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

python_archive = PYZ(analysis.pure)

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

collection = COLLECT(
    executable,
    analysis.binaries,
    analysis.datas,
    strip=False,
    upx=False,
    upx_exclude=[],
    name="headroom-runtime",
)
