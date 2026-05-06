#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path


MARKER = "<!-- game-coverage-report -->"


def write_output(name: str, value: str) -> None:
    output_path = os.environ.get("GITHUB_OUTPUT")
    if not output_path:
        return
    with open(output_path, "a", encoding="utf-8") as output:
        output.write(f"{name}={value}\n")


def read_totals(summary_path: Path) -> dict | None:
    if not summary_path.exists():
        return None

    data = json.loads(summary_path.read_text())
    if "data" in data and data["data"]:
        return data["data"][0].get("totals")
    return data.get("totals")


def percent(value: float | int | None) -> str:
    if value is None:
        return "n/a"
    return f"{float(value):.2f}%"


def metric_row(label: str, totals: dict, key: str) -> str:
    metric = totals.get(key, {})
    covered = metric.get("covered", "n/a")
    count = metric.get("count", "n/a")
    value = percent(metric.get("percent"))
    return f"| {label} | {value} | {covered} / {count} |"


def build_comment(totals: dict | None, threshold: float, summary_text: str, failed: bool) -> tuple[str, bool]:
    repo = os.environ.get("GITHUB_REPOSITORY", "")
    run_id = os.environ.get("GITHUB_RUN_ID", "")
    run_url = f"https://github.com/{repo}/actions/runs/{run_id}" if repo and run_id else ""

    if totals is None:
        body = [
            MARKER,
            "### Coverage Gate",
            "",
            "Coverage could not be calculated because the coverage run did not produce a summary.",
        ]
        if run_url:
            body.extend(["", f"Workflow run: {run_url}"])
        body.append("")
        return "\n".join(body), False

    line_percent = float(totals.get("lines", {}).get("percent", 0.0))
    meets_threshold = line_percent >= threshold and not failed
    status = "Passed" if meets_threshold else "Failed"

    body = [
        MARKER,
        "### Coverage Gate",
        "",
        f"Status: **{status}**",
        f"Line coverage: **{line_percent:.2f}%**",
        f"Required line coverage: **{threshold:.2f}%**",
        "",
        "| Metric | Coverage | Covered / Total |",
        "| --- | ---: | ---: |",
        metric_row("Lines", totals, "lines"),
        metric_row("Functions", totals, "functions"),
        metric_row("Regions", totals, "regions"),
    ]

    if run_url:
        body.extend(
            [
                "",
                f"Full HTML, LCOV, JSON, and text reports are attached to the workflow run: {run_url}",
            ]
        )

    if summary_text and len(summary_text) <= 4000:
        body.extend(
            [
                "",
                "<details>",
                "<summary>Text summary</summary>",
                "",
                "```text",
                summary_text.strip(),
                "```",
                "",
                "</details>",
            ]
        )

    body.append("")
    return "\n".join(body), meets_threshold


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--summary-json", required=True)
    parser.add_argument("--summary-text", required=True)
    parser.add_argument("--comment", required=True)
    parser.add_argument("--threshold", type=float, default=50.0)
    parser.add_argument("--coverage-failed", action="store_true")
    args = parser.parse_args()

    summary_path = Path(args.summary_json)
    text_path = Path(args.summary_text)
    comment_path = Path(args.comment)
    comment_path.parent.mkdir(parents=True, exist_ok=True)

    totals = read_totals(summary_path)
    summary_text = text_path.read_text() if text_path.exists() else ""
    comment, meets_threshold = build_comment(totals, args.threshold, summary_text, args.coverage_failed)
    comment_path.write_text(comment)

    summary_path = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary_path:
        with open(summary_path, "a", encoding="utf-8") as summary:
            summary.write(comment.replace(MARKER + "\n", ""))

    line_percent = 0.0
    if totals is not None:
        line_percent = float(totals.get("lines", {}).get("percent", 0.0))

    write_output("line_coverage", f"{line_percent:.2f}")
    write_output("meets_threshold", "true" if meets_threshold else "false")


if __name__ == "__main__":
    main()
