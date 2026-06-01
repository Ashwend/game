#!/usr/bin/env python3
"""Package a release artifact.

Every platform now ships two binaries: the game (`ashwend`) and the self-update
helper (`ashwend-updater`). The game finds the helper as a sibling of its own
executable, so they must travel together in the archive.

- macOS: assemble a proper `Ashwend.app` bundle (Info.plist + both binaries
  under `Contents/MacOS/`) and zip it with `ditto` so bundle metadata and the
  executable bits survive. The self-updater replaces `Contents/MacOS/ashwend`
  in place. The bundle is intentionally *not* code-signed here — see
  docs/updates.md.
- Linux: a `.tar.gz` of both bare binaries with the executable bit set.
- Windows: a `.zip` of both `.exe`s.
"""

from __future__ import annotations

import argparse
import plistlib
import shutil
import stat
import subprocess
import tarfile
import zipfile
from pathlib import Path

GAME_BASE = "ashwend"
UPDATER_BASE = "ashwend-updater"

BUNDLE_NAME = "Ashwend.app"
BUNDLE_IDENTIFIER = "com.Ashwend.Ashwend"


def binary_name(target: str, base_name: str) -> str:
    if "windows" in target:
        return f"{base_name}.exe"
    return base_name


def release_binary(target: str, base_name: str) -> Path:
    path = Path("target") / target / "release" / binary_name(target, base_name)
    if not path.exists():
        raise SystemExit(f"release binary not found: {path}")
    return path


def create_tarball(binaries: list[Path], output: Path) -> None:
    with tarfile.open(output, "w:gz") as archive:
        for source in binaries:
            info = archive.gettarinfo(str(source), arcname=source.name)
            info.mode |= stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH
            with source.open("rb") as binary:
                archive.addfile(info, binary)


def create_zip(binaries: list[Path], output: Path) -> None:
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for source in binaries:
            archive.write(source, source.name)


def info_plist(version: str) -> bytes:
    plist = {
        "CFBundleName": "Ashwend",
        "CFBundleDisplayName": "Ashwend",
        "CFBundleIdentifier": BUNDLE_IDENTIFIER,
        "CFBundleExecutable": GAME_BASE,
        "CFBundleVersion": version,
        "CFBundleShortVersionString": version,
        "CFBundlePackageType": "APPL",
        "CFBundleInfoDictionaryVersion": "6.0",
        "LSMinimumSystemVersion": "11.0",
        "NSHighResolutionCapable": True,
    }
    return plistlib.dumps(plist)


def build_app_bundle(game: Path, updater: Path, version: str) -> Path:
    app = Path(BUNDLE_NAME)
    if app.exists():
        shutil.rmtree(app)
    macos = app / "Contents" / "MacOS"
    macos.mkdir(parents=True)

    for source, base in ((game, GAME_BASE), (updater, UPDATER_BASE)):
        dest = macos / base
        shutil.copy2(source, dest)
        dest.chmod(dest.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)

    (app / "Contents" / "Info.plist").write_bytes(info_plist(version))
    (app / "Contents" / "PkgInfo").write_text("APPL????")
    return app


def zip_app_bundle(app: Path, output: Path) -> None:
    # `ditto` is the canonical way to zip a macOS bundle: it preserves the
    # bundle layout and executable bits, and `--keepParent` keeps `Ashwend.app/`
    # as the top-level entry (matches what the in-game extractor looks for).
    # It also writes AppleDouble `._<name>` metadata entries — those are
    # expected: macOS Archive Utility folds them back in (preserving perms) when
    # a user double-clicks the zip, and the self-updater matches the real
    # `Contents/MacOS/ashwend` entry exactly, ignoring the sidecars.
    subprocess.run(
        ["ditto", "-c", "-k", "--keepParent", str(app), str(output)],
        check=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True)
    parser.add_argument("--asset", required=True)
    parser.add_argument(
        "--version",
        required=True,
        help="release version (MAJOR.MINOR.PATCH) for the macOS Info.plist",
    )
    args = parser.parse_args()

    game = release_binary(args.target, GAME_BASE)
    updater = release_binary(args.target, UPDATER_BASE)
    output = Path(args.asset)

    if "apple-darwin" in args.target:
        app = build_app_bundle(game, updater, args.version)
        zip_app_bundle(app, output)
    elif output.suffix == ".zip":
        create_zip([game, updater], output)
    else:
        create_tarball([game, updater], output)

    print(f"packaged {output}")


if __name__ == "__main__":
    main()
