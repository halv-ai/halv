"""Build-time helpers for Halv's frozen Headroom runtime.

The build script runs this file with the build environment's Python:

    python -I halv_runtime_build.py probe
    python -I halv_runtime_build.py licenses <output-dir>
    python -I halv_runtime_build.py tiktoken <cache-dir>

The PyInstaller spec imports ``normalize_metadata_toc`` from this file.
Nothing here ships in the runtime.
"""

from __future__ import annotations

import base64
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import sys
import sysconfig
import time
import uuid
from importlib import metadata

# Build-only distributions. They run PyInstaller and never ship.
BUILD_TOOL_DISTRIBUTIONS = frozenset(
    {
        "altgraph",
        "macholib",
        "pefile",
        "pyinstaller",
        "pyinstaller-hooks-contrib",
        "pywin32-ctypes",
    }
)

# The encodings that Headroom's tokenizers map models to
# (headroom/tokenizers/tiktoken_counter.py and headroom/providers/openai.py).
TIKTOKEN_ENCODINGS = ("cl100k_base", "o200k_base", "p50k_base", "r50k_base")
TIKTOKEN_ATTEMPTS = 4

LICENSE_NAME = re.compile(r"^(LICEN[CS]E|COPYING|NOTICE|AUTHORS|COPYRIGHT|PATENTS)", re.IGNORECASE)

# uv writes this file with the wall-clock time of the install.
VOLATILE_METADATA = frozenset({"uv_cache.json"})
SOURCE_PLACEHOLDER = "/headroom-source"


def _canonical(name: str) -> str:
    return re.sub(r"[-_.]+", "-", name).lower()


def _site_packages() -> list[str]:
    paths = {sysconfig.get_paths()["purelib"], sysconfig.get_paths()["platlib"]}
    return sorted(path for path in paths if os.path.isdir(path))


def _distributions() -> list[metadata.Distribution]:
    seen: dict[str, metadata.Distribution] = {}
    for distribution in metadata.distributions(path=_site_packages()):
        name = distribution.metadata.get("Name")
        if name:
            seen.setdefault(_canonical(name), distribution)
    return [seen[key] for key in sorted(seen)]


def _version(name: str) -> str | None:
    try:
        return metadata.version(name)
    except metadata.PackageNotFoundError:
        return None


def _wheel_generator(name: str) -> str | None:
    try:
        wheel = metadata.distribution(name).read_text("WHEEL") or ""
    except metadata.PackageNotFoundError:
        return None
    for line in wheel.splitlines():
        if line.startswith("Generator:"):
            return line.split(":", 1)[1].strip()
    return None


def probe() -> dict[str, object]:
    generator = _wheel_generator("headroom-ai")
    maturin_version = None
    if generator:
        match = re.match(r"maturin \(([^)]+)\)", generator)
        maturin_version = match.group(1) if match else None
    base_executable = getattr(sys, "_base_executable", sys.executable)
    return {
        "python": {
            "version": sys.version,
            "implementation": platform.python_implementation(),
            "build": list(platform.python_build()),
            "compiler": platform.python_compiler(),
            "executable": sys.executable,
            "baseExecutable": base_executable,
            "basePrefix": sys.base_prefix,
            "realBasePrefix": os.path.realpath(sys.base_prefix),
        },
        "packages": {
            name: _version(name)
            for name in (
                "pyinstaller",
                "pyinstaller-hooks-contrib",
                "altgraph",
                "macholib",
                "pefile",
                "pywin32-ctypes",
                "setuptools",
                "packaging",
            )
        },
        "maturin": {"version": maturin_version, "wheelGenerator": generator},
    }


def _license_files(distribution: metadata.Distribution) -> list[tuple[Path, str]]:
    found: dict[str, Path] = {}
    for item in distribution.files or []:
        parts = Path(str(item)).parts
        if not parts or not parts[0].endswith(".dist-info"):
            continue
        inner = parts[1:]
        if not inner:
            continue
        is_license_dir = inner[0] == "licenses" and len(inner) > 1
        if is_license_dir or (len(inner) == 1 and LICENSE_NAME.match(inner[0])):
            relative = "/".join(inner[1:] if is_license_dir else inner)
            located = Path(str(distribution.locate_file(item)))
            if located.is_file():
                found.setdefault(relative, located)
    return [(found[key], key) for key in sorted(found)]


def collect_licenses(output: Path) -> dict[str, object]:
    if output.exists():
        shutil.rmtree(output)
    output.mkdir(parents=True)
    entries = []
    for distribution in _distributions():
        name = distribution.metadata["Name"]
        if _canonical(name) in BUILD_TOOL_DISTRIBUTIONS:
            continue
        version = distribution.version
        folder = f"{_canonical(name)}-{version}"
        copied = []
        for source, relative in _license_files(distribution):
            destination = output / folder / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, destination)
            copied.append(f"{folder}/{relative}")
        classifiers = sorted(
            value
            for value in distribution.metadata.get_all("Classifier") or []
            if value.startswith("License ::")
        )
        license_text = (distribution.metadata.get("License") or "").strip()
        entries.append(
            {
                "name": _canonical(name),
                "version": version,
                "licenseExpression": distribution.metadata.get("License-Expression"),
                "license": license_text.splitlines()[0][:200] if license_text else None,
                "classifiers": classifiers,
                # "missing": the distribution has no license or notice file.
                "files": copied or "missing",
            }
        )
    index = {
        "schemaVersion": 1,
        "scope": "Python distributions installed in the runtime build environment, "
        "without the build-only PyInstaller tools",
        "distributions": entries,
    }
    (output / "INDEX.json").write_text(
        json.dumps(index, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    missing = [entry["name"] for entry in entries if entry["files"] == "missing"]
    return {"distributions": len(entries), "withoutFiles": len(missing), "withoutFilesNames": missing}


def cache_tiktoken(cache_dir: Path) -> dict[str, object]:
    if cache_dir.exists():
        shutil.rmtree(cache_dir)
    cache_dir.mkdir(parents=True)
    os.environ["TIKTOKEN_CACHE_DIR"] = str(cache_dir)
    import tiktoken  # noqa: PLC0415 - needs TIKTOKEN_CACHE_DIR first

    # tiktoken checks each download against the expected_hash in tiktoken_ext.
    # A broken connection stops one download; the next attempt starts again.
    for name in TIKTOKEN_ENCODINGS:
        for attempt in range(1, TIKTOKEN_ATTEMPTS + 1):
            try:
                encoding = tiktoken.get_encoding(name)
                break
            except Exception as error:  # noqa: BLE001 - retried, then raised
                if attempt == TIKTOKEN_ATTEMPTS:
                    raise
                print(
                    f"tiktoken {name}: attempt {attempt} failed ({type(error).__name__}); retrying",
                    file=sys.stderr,
                )
                time.sleep(2 * attempt)
        if not encoding.encode("halv offline check"):
            raise RuntimeError(f"tiktoken encoding {name} produced no tokens")
    files = sorted(path.name for path in cache_dir.iterdir() if path.is_file())
    stray = [name for name in files if name.endswith(".tmp")]
    if stray:
        raise RuntimeError(f"tiktoken left temporary files: {stray}")
    return {"encodings": list(TIKTOKEN_ENCODINGS), "files": files}


# --- metadata normalization (used by headroom-runtime.spec) ----------------


def _record_hash(data: bytes) -> str:
    digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
    return f"sha256={digest}"


def _normalize_text(text: str, source_dir: Path) -> str:
    variants = {str(source_dir), source_dir.as_posix()}
    for variant in sorted(variants, key=len, reverse=True):
        text = text.replace(variant, SOURCE_PLACEHOLDER)
    return text


def _normalized_file(
    path: Path,
    relative: str,
    source_dir: Path,
    repository: str,
    commit: str,
    epoch: int,
    *,
    built_locally: bool,
) -> bytes | None:
    """Return replacement bytes for a volatile metadata file, else None.

    Wheels from PyPI keep their files: the uv.lock hashes fix their bytes.
    Only a distribution that the build made from a local checkout (it has a
    ``direct_url.json``) gets a new SBOM serial number and timestamp.
    """
    name = path.name
    if name == "direct_url.json":
        return _normalize_text(path.read_text(encoding="utf-8"), source_dir).encode()
    if built_locally and relative.startswith("sboms/") and name.endswith(".json"):
        text = _normalize_text(path.read_text(encoding="utf-8"), source_dir)
        document = json.loads(text)
        if "serialNumber" in document:
            document["serialNumber"] = (
                f"urn:uuid:{uuid.uuid5(uuid.NAMESPACE_URL, f'{repository}@{commit}/{name}')}"
            )
        if isinstance(document.get("metadata"), dict) and "timestamp" in document["metadata"]:
            from datetime import datetime, timezone  # noqa: PLC0415

            document["metadata"]["timestamp"] = datetime.fromtimestamp(
                epoch, tz=timezone.utc
            ).strftime("%Y-%m-%dT%H:%M:%SZ")
        return (json.dumps(document, indent=2, ensure_ascii=False) + "\n").encode()
    return None


def _normalized_record(
    record: Path, replacements: dict[str, bytes], *, built_locally: bool
) -> bytes:
    lines = []
    prefix = record.parent.name
    for line in record.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        path = line.rsplit(",", 2)[0]
        # Console scripts live outside site-packages and embed the build path.
        if path.startswith("../"):
            continue
        # The .pth file of an editable install holds the checkout path.
        if built_locally and "/" not in path and path.endswith(".pth"):
            continue
        inner = path[len(prefix) + 1 :] if path.startswith(prefix + "/") else None
        if inner is not None and inner in VOLATILE_METADATA:
            continue
        if inner is not None and inner in replacements:
            data = replacements[inner]
            lines.append(f"{path},{_record_hash(data)},{len(data)}")
            continue
        lines.append(line)
    return ("\n".join(lines) + "\n").encode()


def _dist_info_parts(destination: str) -> tuple[str, str] | None:
    parts = destination.replace("\\", "/").split("/")
    for index, part in enumerate(parts[:-1]):
        if part.endswith(".dist-info"):
            return "/".join(parts[: index + 1]), "/".join(parts[index + 1 :])
    return None


def normalize_metadata_toc(
    toc: list[tuple[str, str, str]],
    work_dir: Path,
    source_dir: Path,
    repository: str,
    commit: str,
    epoch: int,
) -> list[tuple[str, str, str]]:
    """Replace build-specific files in the dist-info entries of a data TOC.

    The TOC is PyInstaller's list of ``(destination, source, typecode)``.

    * ``uv_cache.json`` holds the install time; it is dropped.
    * ``direct_url.json`` holds the source checkout path; the path becomes
      ``/headroom-source``. The SBOM of a locally built distribution (maturin
      writes one for headroom-ai) gets the same path change, a fixed serial
      number and the Headroom commit time as its timestamp.
    * Each rewritten file keeps the file mode of its source.
    * ``RECORD`` lists hashes of console scripts that embed the build path,
      and of the editable ``.pth`` file that holds the checkout path. Those
      lines and the dropped files are removed; rewritten files are rehashed.
    * A file that the rules do not change keeps its original source.
    """
    if work_dir.exists():
        shutil.rmtree(work_dir)
    built_locally = {
        located[0]
        for destination, _source, _typecode in toc
        if (located := _dist_info_parts(destination)) and located[1] == "direct_url.json"
    }
    replacements: dict[str, dict[str, bytes]] = {}
    for destination, source, _typecode in toc:
        located = _dist_info_parts(destination)
        if not located or located[1] in VOLATILE_METADATA:
            continue
        data = _normalized_file(
            Path(source),
            located[1],
            source_dir,
            repository,
            commit,
            epoch,
            built_locally=located[0] in built_locally,
        )
        if data is not None:
            replacements.setdefault(located[0], {})[located[1]] = data

    output: list[tuple[str, str, str]] = []
    for destination, source, typecode in toc:
        located = _dist_info_parts(destination)
        if not located:
            output.append((destination, source, typecode))
            continue
        dist_info, relative = located
        if relative in VOLATILE_METADATA:
            continue
        if relative == "RECORD":
            data = _normalized_record(
                Path(source),
                replacements.get(dist_info, {}),
                built_locally=dist_info in built_locally,
            )
        elif relative in replacements.get(dist_info, {}):
            data = replacements[dist_info][relative]
        else:
            data = None
        if data is None or data == Path(source).read_bytes():
            output.append((destination, source, typecode))
            continue
        written = work_dir / dist_info / relative
        written.parent.mkdir(parents=True, exist_ok=True)
        written.write_bytes(data)
        shutil.copymode(source, written)
        output.append((destination, str(written), typecode))
    return output


# python-build-standalone writes "/install" as the prefix; uv replaces it with
# the install folder when it installs a Python. The frozen runtime gets the
# original value back, so the uv Python folder does not change the runtime.
SYSCONFIG_PREFIX = "/install"


def normalize_sysconfig_toc(
    toc: list[tuple[str, str, str]], work_dir: Path, prefixes: list[str]
) -> list[tuple[str, str, str]]:
    """Replace the Python install folder in ``_sysconfigdata_*`` modules."""
    if work_dir.exists():
        shutil.rmtree(work_dir)
    variants = sorted({value for value in prefixes if value and value != "/"}, key=len, reverse=True)
    output: list[tuple[str, str, str]] = []
    for name, source, typecode in toc:
        if not name.startswith("_sysconfigdata_") or not source or not source.endswith(".py"):
            output.append((name, source, typecode))
            continue
        text = Path(source).read_text(encoding="utf-8")
        for variant in variants:
            text = text.replace(variant, SYSCONFIG_PREFIX)
        written = work_dir / Path(source).name
        written.parent.mkdir(parents=True, exist_ok=True)
        written.write_text(text, encoding="utf-8")
        output.append((name, str(written), typecode))
    return output


def main(arguments: list[str]) -> int:
    if arguments == ["probe"]:
        print(json.dumps(probe(), sort_keys=True))
        return 0
    if len(arguments) == 2 and arguments[0] == "licenses":
        print(json.dumps(collect_licenses(Path(arguments[1])), sort_keys=True))
        return 0
    if len(arguments) == 2 and arguments[0] == "tiktoken":
        print(json.dumps(cache_tiktoken(Path(arguments[1])), sort_keys=True))
        return 0
    print(
        "usage: halv_runtime_build.py {probe | licenses <dir> | tiktoken <dir>}",
        file=sys.stderr,
    )
    return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
