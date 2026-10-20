#!/usr/bin/env python3
"""Exercise frozen first-request Responses compression through a local upstream."""

from __future__ import annotations

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


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: smoke-headroom-runtime.py <runtime executable or onedir>", file=sys.stderr)
        return 2
    runtime = runtime_path(sys.argv[1])
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

                print(
                    f"PASS {version}: first /v1/responses input {original_size} -> "
                    f"{forwarded_size} bytes; /stats saved={int(saved)}; Code-Aware loaded"
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
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
