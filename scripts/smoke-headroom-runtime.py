#!/usr/bin/env python3
"""Exercise frozen first-request Responses compression through a local upstream."""

from __future__ import annotations

import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request


class MockResponsesHandler(BaseHTTPRequestHandler):
    received: dict[str, object] = {}

    def log_message(self, _format: str, *_args: object) -> None:
        return

    def do_POST(self) -> None:  # noqa: N802 - stdlib handler API
        length = int(self.headers.get("content-length", "0"))
        body = json.loads(self.rfile.read(length))
        type(self).received = {
            "path": self.path,
            "headers": {key.lower(): value for key, value in self.headers.items()},
            "body": body,
        }
        response = {
            "id": "resp_halv_frozen_smoke",
            "object": "response",
            "status": "completed",
            "model": body.get("model", "gpt-5.5"),
            "output": [],
            "usage": {"input_tokens": 100, "output_tokens": 1, "total_tokens": 101},
        }
        encoded = json.dumps(response).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)


def free_port() -> int:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


def get_json(url: str, timeout: float = 3.0) -> dict[str, object]:
    with urllib.request.urlopen(url, timeout=timeout) as response:
        return json.loads(response.read())


def wait_ready(port: int, process: subprocess.Popen[bytes], timeout: float = 120.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"frozen proxy exited early with {process.returncode}")
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/livez", timeout=2) as response:
                if response.status == 200:
                    return
        except Exception:
            time.sleep(0.25)
    raise RuntimeError("timed out waiting for the frozen proxy")


def extensive_first_message() -> str:
    segment = "\n".join(
        f"{line:03d}: def process_record_{line}(record): "
        f"return record.get('field_{line}')  # repeated source context for compression"
        for line in range(120)
    )
    return "\n".join([segment, segment, segment])


def runtime_path(argument: str) -> Path:
    path = Path(argument).resolve()
    if path.is_dir():
        path = path / ("headroom-runtime.exe" if os.name == "nt" else "headroom-runtime")
    if not path.is_file():
        raise RuntimeError(f"frozen runtime not found: {path}")
    return path


def runtime_file_digests(root: Path) -> dict[str, str]:
    """Return a SHA-256 digest of each file under the runtime folder."""
    digests: dict[str, str] = {}
    for folder, directories, names in os.walk(root):
        for name in directories + names:
            path = Path(folder) / name
            relative = path.relative_to(root).as_posix()
            if path.is_symlink():
                digests[relative] = f"link:{os.readlink(path)}"
            elif path.is_file():
                digest = hashlib.sha256()
                with path.open("rb") as handle:
                    for chunk in iter(lambda: handle.read(1 << 20), b""):
                        digest.update(chunk)
                digests[relative] = digest.hexdigest()
    return digests


def check_runtime_unchanged(root: Path, before: dict[str, str]) -> int:
    """Fail if a run changed, added, or removed a file in the runtime folder."""
    after = runtime_file_digests(root)
    changes = {
        "changed": [name for name in before if name in after and before[name] != after[name]],
        "added": [name for name in after if name not in before],
        "removed": [name for name in before if name not in after],
    }
    problems = [
        f"{label} {len(names)}: {', '.join(sorted(names))}"
        for label, names in changes.items()
        if names
    ]
    if problems:
        raise RuntimeError(f"the runtime changed its own files ({'; '.join(problems)})")
    return len(before)


def check_offline_data(runtime: Path) -> str:
    """Run the frozen offline self-check and inspect the bundled license index."""
    # Parent values that would turn the offline data and the HF pins off. The
    # frozen entry point must replace each one.
    environment = {
        **os.environ,
        "HEADROOM_HF_PIN": "off",
        "TIKTOKEN_CACHE_DIR": str(Path(tempfile.gettempdir()) / "halv-smoke-no-tiktoken-cache"),
        "CUSTOM_TIKTOKEN_CACHE_DIR": str(
            Path(tempfile.gettempdir()) / "halv-smoke-no-custom-tiktoken-cache"
        ),
        "LITELLM_LOCAL_MODEL_COST_MAP": "False",
    }
    result = subprocess.run(
        [str(runtime), "--offline-self-check"],
        capture_output=True,
        text=True,
        timeout=180,
        env=environment,
    )
    try:
        report = json.loads(result.stdout.strip().splitlines()[-1])
    except (IndexError, ValueError) as error:
        raise RuntimeError(
            f"offline self-check printed no report (exit {result.returncode}): "
            f"{result.stderr[-4000:]}"
        ) from error
    if result.returncode != 0 or report.get("failures"):
        raise RuntimeError(f"offline self-check failed: {report.get('failures')!r}")
    env = report.get("env", {})
    cache_dir = Path(str(env.get("TIKTOKEN_CACHE_DIR") or ""))
    if not cache_dir.is_dir() or runtime.parent.resolve() not in cache_dir.resolve().parents:
        raise RuntimeError(f"tiktoken cache is not the bundled one: {cache_dir}")
    # LiteLLM uses its own tokenizers folder unless this variable names the
    # bundled folder. The Windows wheel of LiteLLM has CRLF files in that folder.
    if env.get("CUSTOM_TIKTOKEN_CACHE_DIR") != env.get("TIKTOKEN_CACHE_DIR"):
        raise RuntimeError(
            "LiteLLM does not use the bundled tiktoken cache: CUSTOM_TIKTOKEN_CACHE_DIR is "
            f"{env.get('CUSTOM_TIKTOKEN_CACHE_DIR')!r}, not {str(cache_dir)!r}"
        )
    if env.get("LITELLM_LOCAL_MODEL_COST_MAP") != "True":
        raise RuntimeError("LiteLLM does not use its local model cost map")
    if env.get("HEADROOM_HF_PIN") != "on":
        raise RuntimeError(f"HEADROOM_HF_PIN is {env.get('HEADROOM_HF_PIN')!r}, not on")
    if not report.get("hfPins"):
        raise RuntimeError("the frozen runtime reports no HuggingFace revision pins")
    encodings = report.get("tiktoken", {})
    if sorted(encodings) != ["cl100k_base", "o200k_base", "p50k_base", "r50k_base"]:
        raise RuntimeError(f"offline tiktoken encodings are incomplete: {encodings!r}")

    index_path = runtime.parent / "_internal" / "licenses" / "python" / "INDEX.json"
    index = json.loads(index_path.read_text(encoding="utf-8"))
    distributions = index.get("distributions") or []
    names = {entry.get("name") for entry in distributions}
    for required in ("headroom-ai", "tiktoken", "litellm", "setuptools"):
        if required not in names:
            raise RuntimeError(f"license index has no entry for {required}")
    for build_tool in ("pyinstaller", "pyinstaller-hooks-contrib", "altgraph"):
        if build_tool in names:
            raise RuntimeError(f"license index lists the build-only tool {build_tool}")
    missing = 0
    for entry in distributions:
        files = entry.get("files")
        if files == "missing":
            missing += 1
            continue
        if not isinstance(files, list) or not files:
            raise RuntimeError(f"license index entry has no file list: {entry.get('name')}")
        for relative in files:
            if not (index_path.parent / relative).is_file():
                raise RuntimeError(f"license index names a file that is not bundled: {relative}")
    return (
        f"offline data ok ({len(encodings)} tiktoken encodings, "
        f"{report.get('litellmModelCostEntries')} LiteLLM cost entries, "
        f"{len(report.get('hfPins', {}))} HF pins, {len(distributions)} license entries, "
        f"{missing} without a license file)"
    )


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: smoke-headroom-runtime.py <runtime executable or onedir>", file=sys.stderr)
        return 2
    runtime = runtime_path(sys.argv[1])
    # The runtime must not change a file in its own folder. The release file
    # manifest would not match after such a change.
    files_before = runtime_file_digests(runtime.parent)
    version = subprocess.run(
        [str(runtime), "--version"],
        check=True,
        capture_output=True,
        text=True,
        timeout=20,
    ).stdout.strip()
    if "+halv.g" not in version:
        raise RuntimeError(f"runtime does not report immutable fork metadata: {version!r}")
    build_info = json.loads(
        subprocess.run(
            [str(runtime), "--build-info"],
            check=True,
            capture_output=True,
            text=True,
            timeout=20,
        ).stdout
    )
    if build_info.get("headroomExtras") != ["proxy", "code"]:
        raise RuntimeError(
            f"runtime does not declare Code-Aware packaging: {build_info!r}"
        )

    offline_summary = check_offline_data(runtime)

    upstream_port = free_port()
    proxy_port = free_port()
    upstream = ThreadingHTTPServer(("127.0.0.1", upstream_port), MockResponsesHandler)
    upstream_thread = threading.Thread(target=upstream.serve_forever, daemon=True)
    upstream_thread.start()

    with tempfile.TemporaryDirectory(prefix="halv-frozen-headroom-smoke-") as temporary:
        temporary_path = Path(temporary)
        log_path = temporary_path / "proxy.log"
        environment = {
            **os.environ,
            "OPENAI_TARGET_API_URL": f"http://127.0.0.1:{upstream_port}",
            "HEADROOM_WORKSPACE_DIR": str(temporary_path / "workspace"),
            "HF_HOME": str(temporary_path / "huggingface"),
            "HEADROOM_TELEMETRY": "off",
            "HEADROOM_TELEMETRY_WARN": "off",
            "HEADROOM_CODE_AWARE_ENABLED": "1",
            "HEADROOM_COMPRESS_USER_MESSAGES": "1",
            "HEADROOM_COMPRESS_RESPONSES_MESSAGES": "1",
            "HEADROOM_PROTECT_RECENT": "0",
        }
        with log_path.open("wb") as log:
            proxy = subprocess.Popen(
                [
                    str(runtime),
                    "proxy",
                    "--host",
                    "127.0.0.1",
                    "--port",
                    str(proxy_port),
                    "--workers",
                    "1",
                ],
                env=environment,
                stdout=log,
                stderr=subprocess.STDOUT,
            )
            try:
                wait_ready(proxy_port, proxy)
                original_text = extensive_first_message()
                payload = {
                    "model": "gpt-5.5",
                    "stream": False,
                    "instructions": "Reply with ok only.",
                    "input": [
                        {
                            "type": "message",
                            "role": "user",
                            "content": [{"type": "input_text", "text": original_text}],
                        }
                    ],
                }
                request = urllib.request.Request(
                    f"http://127.0.0.1:{proxy_port}/v1/responses",
                    data=json.dumps(payload).encode(),
                    method="POST",
                    headers={
                        "authorization": "Bearer halv-smoke-test-token",
                        "content-type": "application/json",
                        "x-client": "codex",
                        "x-headroom-compress-messages": "true",
                        "x-headroom-protect-recent": "0",
                    },
                )
                with urllib.request.urlopen(request, timeout=120) as response:
                    if response.status != 200:
                        raise RuntimeError(f"Responses request returned HTTP {response.status}")
                    json.loads(response.read())

                received = MockResponsesHandler.received
                forwarded = received.get("body")
                if not isinstance(forwarded, dict):
                    raise RuntimeError("mock upstream did not receive a JSON request")
                original_size = len(json.dumps(payload["input"], separators=(",", ":")))
                forwarded_size = len(json.dumps(forwarded.get("input"), separators=(",", ":")))
                if forwarded_size >= original_size:
                    raise RuntimeError(
                        f"first Responses input was not compressed ({original_size} -> {forwarded_size} bytes)"
                    )
                headers = received.get("headers")
                if not isinstance(headers, dict):
                    raise RuntimeError("mock upstream did not record request headers")
                for internal_header in (
                    "x-headroom-compress-messages",
                    "x-headroom-protect-recent",
                ):
                    if internal_header in headers:
                        raise RuntimeError(f"internal header leaked upstream: {internal_header}")

                stats_deadline = time.monotonic() + 5
                requests: object = None
                tokens: object = None
                request_count: object = 0
                saved: object = 0
                while time.monotonic() < stats_deadline:
                    stats = get_json(f"http://127.0.0.1:{proxy_port}/stats", timeout=3)
                    requests = stats.get("requests")
                    tokens = stats.get("tokens")
                    request_count = requests.get("total", 0) if isinstance(requests, dict) else 0
                    saved = tokens.get("saved", 0) if isinstance(tokens, dict) else 0
                    if (
                        isinstance(request_count, (int, float))
                        and request_count >= 1
                        and isinstance(saved, (int, float))
                        and saved > 0
                    ):
                        break
                    time.sleep(0.2)
                if not isinstance(request_count, (int, float)) or request_count < 1:
                    raise RuntimeError(f"/stats did not record the Responses request: {requests!r}")
                if not isinstance(saved, (int, float)) or saved <= 0:
                    raise RuntimeError(f"/stats did not record compression savings: {tokens!r}")

                warmup = get_json(
                    f"http://127.0.0.1:{proxy_port}/debug/warmup", timeout=3
                )
                code_aware = warmup.get("code_aware")
                if not isinstance(code_aware, dict) or code_aware.get("status") != "loaded":
                    raise RuntimeError(
                        "Code-Aware dependencies did not load in the frozen runtime: "
                        f"{code_aware!r}"
                    )

                result_line = (
                    f"PASS {version}: first /v1/responses input {original_size} -> "
                    f"{forwarded_size} bytes; /stats saved={int(saved)}; Code-Aware loaded; "
                    f"{offline_summary}"
                )
            except Exception:
                log.flush()
                print(log_path.read_text(errors="replace")[-8000:], file=sys.stderr)
                raise
            finally:
                proxy.terminate()
                try:
                    proxy.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    proxy.kill()
                    proxy.wait(timeout=5)
                upstream.shutdown()
                upstream.server_close()
                upstream_thread.join(timeout=5)
    unchanged = check_runtime_unchanged(runtime.parent, files_before)
    print(f"{result_line}; {unchanged} runtime files unchanged")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
