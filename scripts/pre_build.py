#!/usr/bin/env python3
"""Download sona + sona-diarize sidecars into src-tauri/binaries/ (Tauri externalBin).

Requires Python 3.10+ (stdlib only — no pip/uv).

Run from repo root:
  python3 scripts/pre_build.py [--target <rust-target-triple>]
"""

from __future__ import annotations

import io
import os
import platform
import ssl
import subprocess
import sys
import tarfile
import zipfile
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

SONA_ASSET_MAP = {
    "aarch64-apple-darwin": ("sona-darwin-arm64-with-ffmpeg.tar.gz", "sona", "ffmpeg"),
    "x86_64-apple-darwin": ("sona-darwin-amd64-with-ffmpeg.tar.gz", "sona", "ffmpeg"),
    "x86_64-unknown-linux-gnu": ("sona-linux-amd64", None, None),
    "aarch64-unknown-linux-gnu": ("sona-linux-arm64", None, None),
    "x86_64-pc-windows-msvc": ("sona-windows-amd64-with-ffmpeg.zip", "sona.exe", "ffmpeg.exe"),
}

DIARIZE_ASSET_MAP = {
    "aarch64-apple-darwin": "sona-diarize-darwin-arm64",
    "x86_64-unknown-linux-gnu": "sona-diarize-linux-amd64",
    "aarch64-unknown-linux-gnu": "sona-diarize-linux-arm64",
    "x86_64-pc-windows-msvc": "sona-diarize-windows-amd64.exe",
}

HOST_TRIPLE_MAP = {
    ("Darwin", "arm64"): "aarch64-apple-darwin",
    ("Darwin", "x86_64"): "x86_64-apple-darwin",
    ("Linux", "x86_64"): "x86_64-unknown-linux-gnu",
    ("Linux", "aarch64"): "aarch64-unknown-linux-gnu",
    ("Windows", "AMD64"): "x86_64-pc-windows-msvc",
}

USER_AGENT = "dory-prebuild/1 (+https://github.com/your-username/dory)"


def parse_target_arg(args: list[str]) -> str | None:
    for i, arg in enumerate(args):
        if arg == "--target" and i + 1 < len(args):
            return args[i + 1]
    return None


def detect_host_target() -> str | None:
    return HOST_TRIPLE_MAP.get((platform.system(), platform.machine()))


def download_with_progress(url: str, label: str, timeout: int = 300) -> bytes:
    req = Request(url, headers={"User-Agent": USER_AGENT})
    ctx = ssl.create_default_context()
    try:
        resp = urlopen(req, timeout=timeout, context=ctx)
    except (HTTPError, URLError) as e:
        raise RuntimeError(f"Download failed ({url}): {e}") from e

    try:
        total = int(resp.headers.get("Content-Length", 0))
        downloaded = 0
        chunks: list[bytes] = []
        chunk_size = 256 * 1024
        while True:
            chunk = resp.read(chunk_size)
            if not chunk:
                break
            chunks.append(chunk)
            downloaded += len(chunk)
            if total:
                pct = downloaded * 100 // total
                print(
                    f"\r  {label}: {pct}% ({downloaded // 1024 // 1024}MB / {total // 1024 // 1024}MB)",
                    end="",
                    flush=True,
                )
        if total:
            print()
        return b"".join(chunks)
    finally:
        resp.close()


def download_sona(repo_root: Path, target_triple: str | None) -> None:
    resolved_target = target_triple or detect_host_target()
    if not resolved_target:
        print(
            "Warning: Could not detect host Rust target triple. "
            "Pass --target <rust-target-triple> or place binaries manually in src-tauri/binaries/."
        )
        return

    asset_entry = SONA_ASSET_MAP.get(resolved_target)
    if not asset_entry:
        print(
            f"Warning: Unsupported target triple '{resolved_target}' for automatic sona download. "
            "Place the binary manually in src-tauri/binaries/."
        )
        return

    version_file = repo_root / ".sona-version"
    try:
        tag = version_file.read_text(encoding="utf-8").strip()
    except OSError as exc:
        print(f"Warning: Unable to read {version_file}: {exc}")
        return

    if not tag:
        print(f"Warning: {version_file} is empty; skipping sona download.")
        return

    is_windows = resolved_target.endswith("windows-msvc")
    sona_sidecar = f"sona-{resolved_target}" + (".exe" if is_windows else "")
    binaries_dir = repo_root / "src-tauri" / "binaries"
    sona_dest = binaries_dir / sona_sidecar

    binaries_dir.mkdir(parents=True, exist_ok=True)
    asset_name, sona_member, ffmpeg_member = asset_entry
    ffmpeg_dest: Path | None = None
    if ffmpeg_member is not None:
        ffmpeg_sidecar = f"ffmpeg-{resolved_target}" + (".exe" if is_windows else "")
        ffmpeg_dest = binaries_dir / ffmpeg_sidecar

    if sona_dest.exists() and (ffmpeg_dest is None or ffmpeg_dest.exists()):
        print(f"Sona sidecar already exists at {sona_dest}; skipping download.")
        return

    url = f"https://github.com/thewh1teagle/sona/releases/download/{tag}/{asset_name}"

    try:
        data = download_with_progress(url, "sona")
    except Exception as exc:
        print(f"Warning: Failed to download sona from {url}: {exc}")
        print(f"Warning: You can manually place the sidecar at {sona_dest}")
        return

    if sona_member is None:
        sona_dest.write_bytes(data)
        sona_dest.chmod(sona_dest.stat().st_mode | 0o111)
        print(f"Downloaded sona sidecar to {sona_dest}")
        return

    if ffmpeg_dest is None:
        raise RuntimeError(f"Expected ffmpeg sidecar path for target {resolved_target}")

    if asset_name.endswith(".zip"):
        with zipfile.ZipFile(io.BytesIO(data)) as zf:
            for name in zf.namelist():
                basename = Path(name).name
                if basename == sona_member:
                    sona_dest.write_bytes(zf.read(name))
                elif basename == ffmpeg_member:
                    ffmpeg_dest.write_bytes(zf.read(name))
    else:
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tf:
            for member in tf.getmembers():
                basename = Path(member.name).name
                if basename == sona_member:
                    sona_dest.write_bytes(tf.extractfile(member).read())
                elif basename == ffmpeg_member:
                    ffmpeg_dest.write_bytes(tf.extractfile(member).read())

    for path in (sona_dest, ffmpeg_dest):
        if path.exists() and not path.name.endswith(".exe"):
            path.chmod(path.stat().st_mode | 0o111)

    print(f"Extracted sona sidecar to {sona_dest}")
    print(f"Extracted ffmpeg sidecar to {ffmpeg_dest}")


def download_diarize(repo_root: Path, target_triple: str | None) -> None:
    resolved_target = target_triple or detect_host_target()
    if not resolved_target:
        return

    binaries_dir = repo_root / "src-tauri" / "binaries"
    is_windows = resolved_target.endswith("windows-msvc")
    sidecar = f"sona-diarize-{resolved_target}" + (".exe" if is_windows else "")
    dest = binaries_dir / sidecar

    asset_name = DIARIZE_ASSET_MAP.get(resolved_target)
    if not asset_name:
        binaries_dir.mkdir(parents=True, exist_ok=True)
        if not dest.exists():
            if is_windows:
                dest.write_text(
                    "@echo off\necho sona-diarize is not supported on this platform\nexit /b 1\n"
                )
            else:
                c_src = binaries_dir / "_diarize_stub.c"
                c_src.write_text(
                    '#include <stdio.h>\n'
                    'int main(void) {\n'
                    '    fprintf(stderr, "sona-diarize is not supported on this platform\\n");\n'
                    '    return 1;\n'
                    '}\n'
                )
                subprocess.run(["cc", "-o", str(dest), str(c_src)], check=True)
                c_src.unlink(missing_ok=True)
            print(f"Created sona-diarize stub at {dest} (not available for '{resolved_target}')")
        return

    version_file = repo_root / ".sona-version"
    try:
        tag = version_file.read_text(encoding="utf-8").strip()
    except OSError:
        return

    if not tag:
        return

    if dest.exists():
        print(f"sona-diarize sidecar already exists at {dest}; skipping download.")
        return

    binaries_dir.mkdir(parents=True, exist_ok=True)
    url = f"https://github.com/thewh1teagle/sona/releases/download/{tag}/{asset_name}"

    try:
        data = download_with_progress(url, "sona-diarize")
        dest.write_bytes(data)
        if not is_windows:
            dest.chmod(dest.stat().st_mode | 0o111)
        print(f"Downloaded sona-diarize sidecar to {dest}")
    except Exception as exc:
        print(f"Warning: Failed to download sona-diarize from {url}: {exc}")
        print("Diarization support will not be available in this build.")


def main() -> int:
    script_root = Path(__file__).resolve().parent
    repo_root = script_root.parent
    tauri_dir = repo_root / "src-tauri"
    os.chdir(tauri_dir)

    argv = sys.argv[1:]
    target_triple = parse_target_arg(argv)
    download_sona(repo_root, target_triple)
    download_diarize(repo_root, target_triple)

    print("\nNext: pnpm dev  or  pnpm build")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
