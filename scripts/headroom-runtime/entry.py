"""Frozen entry point for the Headroom runtime shipped with Halv."""

from __future__ import annotations

import json
import multiprocessing
import os
from pathlib import Path
import sys


BUILD_MANIFEST_NAME = "headroom-runtime-build.json"
TIKTOKEN_CACHE_NAME = "tiktoken-cache"
TIKTOKEN_ENCODINGS = ("cl100k_base", "o200k_base", "p50k_base", "r50k_base")


def _support_dir() -> Path:
    frozen_root = getattr(sys, "_MEIPASS", None)
    return Path(frozen_root or Path(__file__).resolve().parent)


def _load_build_manifest() -> dict[str, object]:
    override = os.environ.get("HALV_HEADROOM_BUILD_MANIFEST")
    if override:
        path = Path(override)
    else:
        path = _support_dir() / BUILD_MANIFEST_NAME
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

# Halv sends no usage data without consent (ADR 0021). The bundled runtime
# turns off the Headroom usage beacon and the update check. Halv also sets
# these variables when it starts the proxy.
os.environ.setdefault("HEADROOM_BEACON", "off")
os.environ.setdefault("DO_NOT_TRACK", "1")
os.environ.setdefault("HEADROOM_UPDATE_CHECK", "off")

# Offline data. The build puts the tiktoken encodings that Headroom uses in the
# runtime, so token counting does not download them. LiteLLM reads its bundled
# model cost map instead of fetching it from GitHub at import time. These
# values replace a value from the parent environment.
_TIKTOKEN_CACHE = _support_dir() / TIKTOKEN_CACHE_NAME
if _TIKTOKEN_CACHE.is_dir():
    os.environ["TIKTOKEN_CACHE_DIR"] = str(_TIKTOKEN_CACHE)
os.environ["LITELLM_LOCAL_MODEL_COST_MAP"] = "True"

# The fork pins each HuggingFace model download to a commit SHA. The bundled
# runtime always keeps these pins: a parent value such as HEADROOM_HF_PIN=off
# has no effect here.
os.environ["HEADROOM_HF_PIN"] = "on"

# Malloc tuning. On macOS, upstream Headroom re-executes the proxy as
# `python -m headroom.cli` to set the two malloc variables below. A frozen
# runtime is not a Python interpreter, so that re-exec exits with code 2. Halv
# sets the malloc variables when it starts the proxy, because macOS reads them
# only at process start. The marker tells Headroom that the tuning is done.
# Child processes of the proxy get the malloc variables from these defaults.
os.environ.setdefault("_HEADROOM_MALLOC_TUNED", "1")
if sys.platform == "darwin":
    os.environ.setdefault("MallocAggressiveMadvise", "1")
    os.environ.setdefault("MallocLargeCache", "0")


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


def _offline_self_check() -> int:
    """Load the offline data with the network blocked and report the result."""
    import socket

    attempts: list[str] = []

    def blocked(*args: object, **_kwargs: object) -> object:
        attempts.append(repr(args[:2]))
        raise OSError("network blocked by the Halv offline self-check")

    socket.getaddrinfo = blocked  # type: ignore[assignment]
    socket.create_connection = blocked  # type: ignore[assignment]
    socket.socket.connect = lambda self, *args: blocked(*args)  # type: ignore[method-assign]
    socket.socket.connect_ex = lambda self, *args: blocked(*args)  # type: ignore[method-assign]

    report: dict[str, object] = {
        "env": {
            key: os.environ.get(key)
            for key in (
                "TIKTOKEN_CACHE_DIR",
                "LITELLM_LOCAL_MODEL_COST_MAP",
                "HEADROOM_HF_PIN",
                "HEADROOM_BEACON",
                "DO_NOT_TRACK",
                "HEADROOM_UPDATE_CHECK",
            )
        }
    }
    failures: list[str] = []

    import tiktoken

    encodings: dict[str, int] = {}
    for name in TIKTOKEN_ENCODINGS:
        try:
            encodings[name] = len(tiktoken.get_encoding(name).encode("halv offline check"))
        except Exception as error:  # noqa: BLE001 - reported to the caller
            failures.append(f"tiktoken {name}: {type(error).__name__}: {error}")
    report["tiktoken"] = encodings

    try:
        import litellm

        report["litellmModelCostEntries"] = len(getattr(litellm, "model_cost", {}) or {})
        if not report["litellmModelCostEntries"]:
            failures.append("litellm loaded an empty model cost map")
    except Exception as error:  # noqa: BLE001 - reported to the caller
        failures.append(f"litellm: {type(error).__name__}: {error}")

    from headroom import onnx_runtime

    pins = dict(getattr(onnx_runtime, "_PINNED_REVISIONS", {}))
    resolved = {repo: onnx_runtime._resolve_revision(repo, None) for repo in sorted(pins)}
    report["hfPins"] = resolved
    if not pins or any(resolved[repo] != pins[repo] for repo in pins):
        failures.append("HuggingFace revision pins are not active")

    from headroom.telemetry.beacon import is_beacon_enabled

    report["beaconEnabled"] = is_beacon_enabled()
    if report["beaconEnabled"]:
        failures.append("the Headroom beacon is enabled")

    report["networkAttempts"] = attempts
    if attempts:
        failures.append(f"{len(attempts)} network attempts during the offline self-check")
    report["failures"] = failures
    print(json.dumps(report, sort_keys=True))
    return 1 if failures else 0


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
    if sys.argv[1:] == ["--offline-self-check"]:
        return _offline_self_check()
    if len(sys.argv) >= 2 and sys.argv[1] == "proxy":
        return _run_proxy(sys.argv[2:])
    print(
        "usage: headroom-runtime {--version|--build-info|--offline-self-check|proxy [args...]}",
        file=sys.stderr,
    )
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
