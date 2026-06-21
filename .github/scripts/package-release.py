#!/usr/bin/env python3
"""Package a release artifact.

Every platform now ships two binaries: the game (`ashwend`) and the self-update
helper (`ashwend-updater`). The game finds the helper as a sibling of its own
executable, so they must travel together in the archive.

- macOS: assemble a proper `Ashwend.app` bundle (Info.plist + both binaries
  under `Contents/MacOS/`) and zip it with `ditto` so bundle metadata and the
  executable bits survive. The self-updater replaces `Contents/MacOS/ashwend`
  in place. The bundle is intentionally *not* code-signed here, see
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
import tempfile
import zipfile
from pathlib import Path

# Installer specs live next to this script's parent (.github/installer/).
INSTALLER_DIR = Path(__file__).resolve().parents[1] / "installer"
DMG_SPEC = INSTALLER_DIR / "ashwend-dmg.json"
INNO_SCRIPT = INSTALLER_DIR / "ashwend.iss"

GAME_BASE = "ashwend"
UPDATER_BASE = "ashwend-updater"

BUNDLE_NAME = "Ashwend.app"
BUNDLE_IDENTIFIER = "com.Ashwend.Ashwend"

# Pre-rendered macOS app icon (committed). Generated from the website favicon
# with the native QuickLook renderer (ImageMagick can't render the SVG's
# gradient), see docs/updates.md. Regenerate only when the logo changes.
ICON_SRC = Path(__file__).resolve().parents[1] / "assets" / "AppIcon.icns"
ICON_NAME = "AppIcon"  # CFBundleIconFile (macOS appends .icns)


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
        "CFBundleIconFile": ICON_NAME,
        "LSMinimumSystemVersion": "11.0",
        "NSHighResolutionCapable": True,
        # Required for any bundled app that touches the microphone: without a
        # usage-description string macOS TCC kills the process the moment it
        # opens the mic. Ashwend captures voice for in-game chat.
        "NSMicrophoneUsageDescription": "Ashwend uses your microphone for in-game voice chat.",
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

    if not ICON_SRC.exists():
        raise SystemExit(f"app icon not found: {ICON_SRC}")
    resources = app / "Contents" / "Resources"
    resources.mkdir(parents=True)
    shutil.copy2(ICON_SRC, resources / f"{ICON_NAME}.icns")

    (app / "Contents" / "Info.plist").write_bytes(info_plist(version))
    (app / "Contents" / "PkgInfo").write_text("APPL????")
    return app


def adhoc_sign(app: Path) -> None:
    # Ad-hoc sign the assembled bundle. The Rust toolchain only applies a
    # *linker* ad-hoc signature to each bare binary, which is invalid as a
    # bundle's main executable (no sealed `_CodeSignature/CodeResources`), a
    # broken signature is what makes Gatekeeper say "damaged, move to Trash"
    # with no recourse. A proper ad-hoc bundle signature downgrades that to the
    # ordinary "Apple can't check it" prompt (which has an "Open Anyway"
    # button), and lets the curl installer's de-quarantined copy launch cleanly.
    #
    # `--deep` is fine here because nothing in the bundle is running at build
    # time (the in-app self-updater uses non-`--deep` re-signing precisely
    # because it *is* running from inside the bundle it re-signs).
    #
    # Not notarized, that needs a paid Developer ID. When that lands, swap the
    # `-` identity for the Developer ID and add an `xcrun notarytool` step here.
    subprocess.run(
        ["codesign", "--force", "--deep", "--sign", "-", str(app)],
        check=True,
    )
    subprocess.run(
        ["codesign", "--verify", "--deep", "--strict", str(app)],
        check=True,
    )


def zip_app_bundle(app: Path, output: Path) -> None:
    # `ditto` is the canonical way to zip a macOS bundle: it preserves the
    # bundle layout and executable bits, and `--keepParent` keeps `Ashwend.app/`
    # as the top-level entry (matches what the in-game extractor looks for).
    # It also writes AppleDouble `._<name>` metadata entries, those are
    # expected: macOS Archive Utility folds them back in (preserving perms) when
    # a user double-clicks the zip, and the self-updater matches the real
    # `Contents/MacOS/ashwend` entry exactly, ignoring the sidecars.
    subprocess.run(
        ["ditto", "-c", "-k", "--keepParent", str(app), str(output)],
        check=True,
    )


def build_dmg(output: Path) -> None:
    """Wrap the already-assembled, ad-hoc-signed `Ashwend.app` in a styled
    drag-to-Applications `.dmg` for the website download. The self-updater keeps
    consuming the `.zip` (it extracts with the `zip` crate and cannot read a
    dmg), so this is an *additional* asset, not a replacement.

    `appdmg` writes the volume's `.DS_Store` directly (no Finder/AppleScript), so
    unlike `create-dmg` it runs reliably headless on a CI macOS runner. It
    resolves the spec's relative paths (`../../Ashwend.app`, the background, the
    icon) against the spec's own directory, so the bundle must already exist at
    the repo root, which it does after `build_app_bundle`.
    """
    if not DMG_SPEC.exists():
        raise SystemExit(f"appdmg spec not found: {DMG_SPEC}")
    if output.exists():
        output.unlink()
    subprocess.run(["npx", "--yes", "appdmg", str(DMG_SPEC), str(output)], check=True)
    verify_dmg_app_signature(output)


def verify_dmg_app_signature(dmg: Path) -> None:
    """Fail the build if the `.app` inside the dmg lost its ad-hoc seal. A
    packaging tool that stripped extended attributes would silently break
    Gatekeeper; mount read-only, verify, always detach."""
    attached = subprocess.run(
        ["hdiutil", "attach", str(dmg), "-nobrowse", "-readonly", "-plist"],
        check=True,
        capture_output=True,
    )
    plist = plistlib.loads(attached.stdout)
    mount_point = next(
        (
            entity["mount-point"]
            for entity in plist.get("system-entities", [])
            if entity.get("mount-point")
        ),
        None,
    )
    if not mount_point:
        raise SystemExit("could not determine the dmg mount point for signature verification")
    try:
        subprocess.run(
            ["codesign", "--verify", "--deep", "--strict", str(Path(mount_point) / BUNDLE_NAME)],
            check=True,
        )
    finally:
        subprocess.run(["hdiutil", "detach", mount_point], check=False)


def find_iscc() -> Path:
    """Locate the Inno Setup compiler. `choco install innosetup` puts an `iscc`
    shim on PATH; fall back to the default install location."""
    found = shutil.which("iscc") or shutil.which("ISCC")
    if found:
        return Path(found)
    default = Path(r"C:\Program Files (x86)\Inno Setup 6\ISCC.exe")
    if default.exists():
        return default
    raise SystemExit("ISCC.exe (Inno Setup 6) not found on PATH or at its default location")


def build_windows_installer(target: str, output: Path, version: str) -> None:
    """Compile the per-user Inno Setup installer beside the existing `.zip`. The
    zip stays the self-update transport (the updater extracts a bare `.exe`); the
    installer is the nicer website first-install (Start Menu + optional desktop
    shortcut + uninstaller, the embedded icon). See `.github/installer/ashwend.iss`
    for why it installs per-user rather than to Program Files."""
    if not INNO_SCRIPT.exists():
        raise SystemExit(f"installer script not found: {INNO_SCRIPT}")
    iscc = find_iscc()
    game = release_binary(target, GAME_BASE)
    updater = release_binary(target, UPDATER_BASE)
    out = output.resolve()
    base_name = out.name[:-4] if out.name.lower().endswith(".exe") else out.name
    with tempfile.TemporaryDirectory() as staging:
        staging_dir = Path(staging)
        shutil.copy2(game, staging_dir / game.name)
        shutil.copy2(updater, staging_dir / updater.name)
        # ISCC resolves a relative OutputDir against the .iss directory, not the
        # cwd, so pass /O explicitly to land the setup.exe where the upload step
        # expects it. /F sets the output base name (the .iss adds `.exe`).
        subprocess.run(
            [
                str(iscc),
                "/Qp",
                f"/O{out.parent}",
                f"/F{base_name}",
                f"/DAppVersion={version}",
                f"/DStagingDir={staging_dir}",
                str(INNO_SCRIPT),
            ],
            check=True,
        )
    if not out.exists():
        raise SystemExit(f"installer compile did not produce {out}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True)
    parser.add_argument("--asset", required=True)
    parser.add_argument(
        "--version",
        required=True,
        help="release version (MAJOR.MINOR.PATCH) for the macOS Info.plist",
    )
    parser.add_argument(
        "--dmg-asset",
        help="optional macOS .dmg output, built in addition to the .zip (macOS targets only)",
    )
    parser.add_argument(
        "--installer-asset",
        help="optional Windows installer .exe output, built in addition to the .zip (Windows targets only)",
    )
    args = parser.parse_args()

    game = release_binary(args.target, GAME_BASE)
    updater = release_binary(args.target, UPDATER_BASE)
    output = Path(args.asset)

    if "apple-darwin" in args.target:
        app = build_app_bundle(game, updater, args.version)
        adhoc_sign(app)
        zip_app_bundle(app, output)
        if args.dmg_asset:
            build_dmg(Path(args.dmg_asset).resolve())
            print(f"packaged {args.dmg_asset}")
    elif output.suffix == ".zip":
        create_zip([game, updater], output)
        if args.installer_asset:
            build_windows_installer(args.target, Path(args.installer_asset), args.version)
            print(f"packaged {args.installer_asset}")
    else:
        create_tarball([game, updater], output)

    print(f"packaged {output}")


if __name__ == "__main__":
    main()
