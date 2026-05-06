#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path


MARKER = "<!-- game-coverage-report -->"


def github_request(method: str, path: str, token: str, payload: dict | None = None):
    repo = os.environ["GITHUB_REPOSITORY"]
    url = f"https://api.github.com/repos/{repo}{path}"
    data = None
    if payload is not None:
        data = json.dumps(payload).encode()

    request = urllib.request.Request(url, data=data, method=method)
    request.add_header("Accept", "application/vnd.github+json")
    request.add_header("Authorization", f"Bearer {token}")
    request.add_header("X-GitHub-Api-Version", "2022-11-28")
    if payload is not None:
        request.add_header("Content-Type", "application/json")

    try:
        with urllib.request.urlopen(request) as response:
            body = response.read().decode()
    except urllib.error.HTTPError as error:
        sys.stderr.write(error.read().decode())
        raise

    if not body:
        return None
    return json.loads(body)


def find_existing_comment(pr_number: str, token: str) -> int | None:
    page = 1
    while True:
        comments = github_request(
            "GET",
            f"/issues/{pr_number}/comments?per_page=100&page={page}",
            token,
        )
        if not comments:
            return None
        for comment in comments:
            if MARKER in comment.get("body", ""):
                return int(comment["id"])
        if len(comments) < 100:
            return None
        page += 1


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--comment", required=True)
    parser.add_argument("--pr-number", required=True)
    args = parser.parse_args()

    token = os.environ.get("GITHUB_TOKEN")
    if not token:
        raise SystemExit("GITHUB_TOKEN is required")

    pr_number = Path(args.pr_number).read_text().strip()
    body = Path(args.comment).read_text()
    if MARKER not in body:
        body = f"{MARKER}\n{body}"

    existing_id = find_existing_comment(pr_number, token)
    if existing_id is None:
        github_request("POST", f"/issues/{pr_number}/comments", token, {"body": body})
        print(f"created coverage comment on PR #{pr_number}")
    else:
        github_request("PATCH", f"/issues/comments/{existing_id}", token, {"body": body})
        print(f"updated coverage comment on PR #{pr_number}")


if __name__ == "__main__":
    main()
