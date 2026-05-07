#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path


ASSETS = [
    ("Linux Intel", "game-x86_64-unknown-linux-gnu.tar.gz"),
    ("Linux ARM", "game-aarch64-unknown-linux-gnu.tar.gz"),
    ("macOS Intel", "game-x86_64-apple-darwin.tar.gz"),
    ("macOS ARM", "game-aarch64-apple-darwin.tar.gz"),
    ("Windows Intel", "game-x86_64-pc-windows-msvc.zip"),
]

CATEGORY_ORDER = [
    ("breaking", "Breaking Change"),
    ("feat", "Feature"),
    ("fix", "Fix"),
    ("perf", "Performance"),
    ("refactor", "Refactor"),
    ("docs", "Documentation"),
    ("test", "Test"),
    ("build", "Build"),
    ("ci", "CI"),
    ("chore", "Chore"),
    ("revert", "Revert"),
    ("other", "Other Change"),
]

TYPE_TO_CATEGORY = {
    "feat": "feat",
    "fix": "fix",
    "perf": "perf",
    "refactor": "refactor",
    "docs": "docs",
    "doc": "docs",
    "test": "test",
    "tests": "test",
    "build": "build",
    "ci": "ci",
    "chore": "chore",
    "revert": "revert",
}

CONVENTIONAL_COMMIT = re.compile(
    r"^(?P<type>[A-Za-z]+)(?:\((?P<scope>[^)]+)\))?(?P<breaking>!)?:\s*(?P<description>.+)$"
)
SEMVER = re.compile(r"^(?P<major>0|[1-9]\d*)\.(?P<minor>0|[1-9]\d*)\.(?P<patch>0|[1-9]\d*)$")
TAG_SEMVER = re.compile(r"^v(?P<major>0|[1-9]\d*)\.(?P<minor>0|[1-9]\d*)\.(?P<patch>0|[1-9]\d*)$")


def run_git(args: list[str], *, check: bool = True) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        ["git", *args],
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if check and result.returncode != 0:
        sys.stderr.write(result.stderr)
        raise SystemExit(result.returncode)
    return result


def read_manifest_version(path: Path) -> str:
    in_package = False
    for line in path.read_text().splitlines():
        stripped = line.strip()
        if stripped == "[package]":
            in_package = True
            continue
        if stripped.startswith("[") and stripped.endswith("]"):
            in_package = False
        if in_package:
            match = re.match(r'^version\s*=\s*"([^"]+)"$', stripped)
            if match:
                return match.group(1)
    raise SystemExit(f"could not find [package] version in {path}")


def write_manifest_version(path: Path, version: str) -> None:
    in_package = False
    updated = False
    lines: list[str] = []

    for line in path.read_text().splitlines(keepends=True):
        stripped = line.strip()
        if stripped == "[package]":
            in_package = True
        elif stripped.startswith("[") and stripped.endswith("]"):
            in_package = False

        if in_package and not updated:
            match = re.match(r'^(\s*version\s*=\s*)"[^"]+"(.*)$', line.rstrip("\n"))
            if match:
                newline = "\n" if line.endswith("\n") else ""
                line = f'{match.group(1)}"{version}"{match.group(2)}{newline}'
                updated = True

        lines.append(line)

    if not updated:
        raise SystemExit(f"could not update [package] version in {path}")

    path.write_text("".join(lines))


def parse_version(version: str) -> tuple[int, int, int]:
    match = SEMVER.match(version)
    if not match:
        raise SystemExit(f"only plain SemVer versions are supported, got {version!r}")
    return tuple(int(match.group(name)) for name in ("major", "minor", "patch"))


def bump_version(version: str, bump: str) -> str:
    major, minor, patch = parse_version(version)
    if bump == "major":
        return f"{major + 1}.0.0"
    if bump == "minor":
        return f"{major}.{minor + 1}.0"
    if bump == "patch":
        return f"{major}.{minor}.{patch + 1}"
    raise SystemExit(f"unsupported bump {bump!r}")


def release_tags() -> list[tuple[tuple[int, int, int], str]]:
    tags: list[tuple[tuple[int, int, int], str]] = []
    for tag in run_git(["tag", "--list", "v[0-9]*"]).stdout.splitlines():
        match = TAG_SEMVER.match(tag.strip())
        if match:
            version = tuple(int(match.group(name)) for name in ("major", "minor", "patch"))
            tags.append((version, tag.strip()))
    return sorted(tags, reverse=True)


def latest_release_tag() -> str | None:
    tags = release_tags()
    if not tags:
        return None
    return tags[0][1]


def latest_release_tag_before(version: tuple[int, int, int]) -> str | None:
    for tag_version, tag in release_tags():
        if tag_version < version:
            return tag
    return None


def assert_tag_is_available(tag: str, *, check_remote: bool) -> None:
    local = run_git(["rev-parse", "-q", "--verify", f"refs/tags/{tag}"], check=False)
    if local.returncode == 0:
        raise SystemExit(f"tag {tag} already exists locally")

    if not check_remote:
        return

    remote = run_git(["ls-remote", "--tags", "origin", f"refs/tags/{tag}"], check=False)
    if remote.returncode == 0 and remote.stdout.strip():
        raise SystemExit(f"tag {tag} already exists on origin")


def read_commits(since_tag: str | None, end_ref: str) -> list[dict[str, str]]:
    args = ["log", "--no-merges", "--reverse", "--format=%H%x1f%h%x1f%s%x1f%b%x1e"]
    if since_tag:
        args.append(f"{since_tag}..{end_ref}")
    else:
        args.append(end_ref)
    output = run_git(args).stdout
    commits: list[dict[str, str]] = []

    for record in output.split("\x1e"):
        record = record.strip("\n")
        if not record:
            continue
        parts = record.split("\x1f", 3)
        if len(parts) != 4:
            continue
        full_hash, short_hash, subject, body = parts
        commits.append(
            {
                "full_hash": full_hash,
                "short_hash": short_hash,
                "subject": subject.strip(),
                "body": body.strip(),
            }
        )

    return commits


def classify_commit(subject: str, body: str) -> tuple[str, str]:
    match = CONVENTIONAL_COMMIT.match(subject)
    if not match:
        if subject.startswith("Revert "):
            return "revert", subject
        return "other", subject

    commit_type = match.group("type").lower()
    description = match.group("description").strip()
    scope = match.group("scope")
    is_breaking = bool(match.group("breaking")) or "BREAKING CHANGE" in body or "BREAKING-CHANGE" in body

    if scope:
        description = f"{scope}: {description}"

    if is_breaking:
        return "breaking", description

    return TYPE_TO_CATEGORY.get(commit_type, "other"), description


def build_release_notes(version: str, tag: str, since_tag: str | None, end_ref: str) -> str:
    since_label = since_tag or "the first commit"
    grouped: dict[str, list[str]] = defaultdict(list)

    for commit in read_commits(since_tag, end_ref):
        category, description = classify_commit(commit["subject"], commit["body"])
        grouped[category].append(f"- {description} (`{commit['short_hash']}`)")

    lines = [
        f"## game {tag}",
        "",
        f"Changes since {since_label}.",
        "",
        "### Release Assets",
    ]

    for label, asset in ASSETS:
        lines.append(f"- {label}: {asset}")

    lines.extend(["", "### Changelog"])

    if not any(grouped.values()):
        lines.append("- No commits found since the previous release tag.")
    else:
        for key, label in CATEGORY_ORDER:
            entries = grouped.get(key, [])
            if not entries:
                continue
            lines.extend(["", f"#### {label}", *entries])

    lines.append("")
    return "\n".join(lines)


def write_github_output(name: str, value: str) -> None:
    output_path = os.environ.get("GITHUB_OUTPUT")
    if not output_path:
        return
    with open(output_path, "a", encoding="utf-8") as output:
        output.write(f"{name}={value}\n")


def parse_tag_version(tag: str) -> tuple[int, int, int]:
    match = TAG_SEMVER.match(tag)
    if not match:
        raise SystemExit(f"existing tag must look like v1.2.3, got {tag!r}")
    return tuple(int(match.group(name)) for name in ("major", "minor", "patch"))


def tag_commit(tag: str) -> str:
    result = run_git(["rev-list", "-n", "1", tag], check=False)
    if result.returncode != 0 or not result.stdout.strip():
        raise SystemExit(f"could not resolve existing tag {tag}")
    return result.stdout.strip()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--bump", choices=["patch", "minor", "major"])
    parser.add_argument("--existing-tag")
    parser.add_argument("--manifest", default="Cargo.toml")
    parser.add_argument("--notes", default="release-notes.txt")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    manifest = Path(args.manifest)
    patch_required = not bool(args.existing_tag)

    if args.existing_tag:
        tag = args.existing_tag.strip()
        version_tuple = parse_tag_version(tag)
        next_version = tag.removeprefix("v")
        base_sha = tag_commit(tag)
        since_tag = latest_release_tag_before(version_tuple)
        notes = build_release_notes(next_version, tag, since_tag, tag)
    else:
        if not args.bump:
            raise SystemExit("--bump is required unless --existing-tag is provided")
        current_version = read_manifest_version(manifest)
        next_version = bump_version(current_version, args.bump)
        tag = f"v{next_version}"
        base_sha = run_git(["rev-parse", "HEAD"]).stdout.strip()
        since_tag = latest_release_tag()

        assert_tag_is_available(tag, check_remote=not args.dry_run)

        notes = build_release_notes(next_version, tag, since_tag, "HEAD")

    if args.dry_run:
        print(notes)
    else:
        if patch_required:
            write_manifest_version(manifest, next_version)
        Path(args.notes).write_text(notes)

    write_github_output("version", next_version)
    write_github_output("tag", tag)
    write_github_output("base_sha", base_sha)
    write_github_output("release_notes", args.notes)
    write_github_output("previous_tag", since_tag or "")
    write_github_output("patch_required", "true" if patch_required else "false")


if __name__ == "__main__":
    main()
