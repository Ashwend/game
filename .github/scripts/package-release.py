#!/usr/bin/env python3
from __future__ import annotations

import argparse
import stat
import tarfile
import zipfile
from pathlib import Path


def binary_name(target: str, base_name: str) -> str:
    if "windows" in target:
        return f"{base_name}.exe"
    return base_name


def create_tarball(source: Path, output: Path, archive_name: str) -> None:
    with tarfile.open(output, "w:gz") as archive:
        info = archive.gettarinfo(str(source), arcname=archive_name)
        info.mode |= stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH
        with source.open("rb") as binary:
            archive.addfile(info, binary)


def create_zip(source: Path, output: Path, archive_name: str) -> None:
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.write(source, archive_name)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True)
    parser.add_argument("--asset", required=True)
    parser.add_argument("--binary", default="ashwend")
    args = parser.parse_args()

    binary = binary_name(args.target, args.binary)
    source = Path("target") / args.target / "release" / binary
    output = Path(args.asset)

    if not source.exists():
        raise SystemExit(f"release binary not found: {source}")

    if output.suffix == ".zip":
        create_zip(source, output, binary)
    else:
        create_tarball(source, output, binary)

    print(f"packaged {output}")


if __name__ == "__main__":
    main()
