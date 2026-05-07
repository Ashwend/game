#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from pathlib import Path


ASSETS = [
    ("Linux Intel", "game-x86_64-unknown-linux-gnu.tar.gz"),
    ("Linux ARM", "game-aarch64-unknown-linux-gnu.tar.gz"),
    ("macOS ARM", "game-aarch64-apple-darwin.tar.gz"),
    ("Windows Intel", "game-x86_64-pc-windows-msvc.zip"),
]


def build_asset_section(release_json: Path, *, require_all: bool) -> list[str]:
    release = json.loads(release_json.read_text())
    urls = {
        asset["name"]: asset["browser_download_url"]
        for asset in release.get("assets", [])
        if asset.get("name") and asset.get("browser_download_url")
    }
    missing = [asset for _label, asset in ASSETS if asset not in urls]
    if require_all and missing:
        raise SystemExit(f"release is missing expected assets: {', '.join(missing)}")

    lines = ["### Release Assets"]
    for label, asset in ASSETS:
        if asset in urls:
            lines.append(f"- {label}: [{asset}]({urls[asset]})")
        else:
            lines.append(f"- {label}: {asset} (not uploaded)")
    return lines


def replace_asset_section(notes: str, asset_section: list[str]) -> str:
    lines = notes.splitlines()
    start = None
    end = None

    for index, line in enumerate(lines):
        if line == "### Release Assets":
            start = index
            continue
        if start is not None and index > start and line.startswith("### "):
            end = index
            break

    if start is None:
        insert_at = len(lines)
        lines.extend(["", *asset_section])
        return "\n".join(lines).rstrip() + "\n"

    if end is None:
        end = len(lines)

    replacement = asset_section
    if end < len(lines) and replacement[-1] != "":
        replacement = [*replacement, ""]

    return "\n".join([*lines[:start], *replacement, *lines[end:]]).rstrip() + "\n"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--notes", required=True)
    parser.add_argument("--release-json", required=True)
    parser.add_argument("--require-all", action="store_true")
    args = parser.parse_args()

    notes_path = Path(args.notes)
    asset_section = build_asset_section(Path(args.release_json), require_all=args.require_all)
    notes_path.write_text(replace_asset_section(notes_path.read_text(), asset_section))


if __name__ == "__main__":
    main()
