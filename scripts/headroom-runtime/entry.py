"""Frozen entry point for the Headroom runtime shipped with Halv."""

from __future__ import annotations

import json
import multiprocessing
import os
from pathlib import Path
import sys


BUILD_MANIFEST_NAME = "headroom-runtime-build.json"


def _load_build_manifest() -> dict[str, object]:
    override = os.environ.get("HALV_HEADROOM_BUILD_MANIFEST")
    if override:
        path = Path(override)
    else:
        frozen_root = getattr(sys, "_MEIPASS", None)
        path = Path(frozen_root or Path(__file__).resolve().parent) / BUILD_MANIFEST_NAME
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise RuntimeError(f"could not read immutable Headroom build metadata: {error}") from error
    if not isinstance(value, dict) or not value.get("runtimeVersion"):
        raise RuntimeError("immutable Headroom build metadata is incomplete")
    return value


BUILD_MANIFEST = _load_build_manifest()
RUNTIME_VERSION = str(BUILD_MANIFEST["runtimeVersion"])

# Headroom reads this before exposing its CLI version and /stats version. The
# frozen runtime therefore reports the exact fork build, not the public wheel's
# base version.
os.environ.setdefault("HEADROOM_BUILD_VERSION", RUNTIME_VERSION)


def _watch_parent() -> None:
    """Terminate the proxy if its supervising Halv process disappears."""
    import threading
    import time

    initial_parent = os.getppid()

    def loop() -> None:
        while True:
            parent = os.getppid()
            if parent != initial_parent or parent == 1:
                os._exit(0)
            time.sleep(2.0)

    threading.Thread(target=loop, daemon=True, name="halv-parent-watchdog").start()


def _run_proxy(arguments: list[str]) -> int:
    from headroom.cli import main as headroom_main

    if os.environ.get("HEADROOM_DESKTOP_SUPERVISED") == "1":
        _watch_parent()
    sys.argv = ["headroom", "proxy", *arguments]
    try:
        headroom_main()
    except SystemExit as error:
        return int(error.code or 0)
    return 0


def main() -> int:
    multiprocessing.freeze_support()
    os.environ.setdefault("HEADROOM_TELEMETRY", "off")
    os.environ.setdefault("HEADROOM_TELEMETRY_WARN", "off")

    if sys.argv[1:] == ["--version"]:
        print(f"headroom-runtime {RUNTIME_VERSION}")
        return 0
    if sys.argv[1:] == ["--build-info"]:
        print(json.dumps(BUILD_MANIFEST, sort_keys=True))
        return 0
    if len(sys.argv) >= 2 and sys.argv[1] == "proxy":
        return _run_proxy(sys.argv[2:])
    print("usage: headroom-runtime {--version|--build-info|proxy [args...]}", file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
