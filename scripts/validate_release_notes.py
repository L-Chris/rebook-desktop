#!/usr/bin/env python3
"""Validate the categorized release notes for a tag build."""

from __future__ import annotations

import re
import sys
from pathlib import Path


EXPECTED_SECTIONS = ("Feature", "Improvement", "Fix")
RELEASE_NOTES = Path(__file__).resolve().parents[1] / "RELEASE_NOTES.md"


def fail(message: str) -> None:
    raise SystemExit(f"Invalid release notes: {message}")


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: validate_release_notes.py <tag>")

    tag = sys.argv[1]
    text = RELEASE_NOTES.read_text(encoding="utf-8")
    lines = text.splitlines()
    expected_title = f"# Torto {tag}"
    if not lines or lines[0] != expected_title:
        fail(f"first line must be {expected_title!r}")

    matches = list(re.finditer(r"^## ([^\r\n]+)$", text, flags=re.MULTILINE))
    sections = tuple(match.group(1) for match in matches)
    if sections != EXPECTED_SECTIONS:
        fail(
            "sections must appear exactly in this order: "
            + " -> ".join(EXPECTED_SECTIONS)
        )

    for index, match in enumerate(matches):
        end = matches[index + 1].start() if index + 1 < len(matches) else len(text)
        body = text[match.end() : end]
        content_lines = [line for line in body.splitlines() if line.strip()]
        bullets = [line for line in content_lines if re.fullmatch(r"- \S.*", line)]
        if not bullets:
            fail(f"section {match.group(1)!r} must contain at least one bullet")
        non_bullets = [
            line
            for line in content_lines
            if not re.fullmatch(r"- \S.*", line)
            and not line.startswith("**Full Changelog**:")
        ]
        if non_bullets:
            fail(f"section {match.group(1)!r} contains non-bullet content")

    compare_pattern = re.compile(
        r"^\*\*Full Changelog\*\*: "
        r"https://github\.com/L-Chris/rebook-desktop/compare/"
        r"v\d+\.\d+\.\d+\.\.\."
        + re.escape(tag)
        + r"$",
        flags=re.MULTILINE,
    )
    if compare_pattern.search(text) is None:
        fail("missing a Full Changelog link from the previous version to this tag")

    print(f"Validated {RELEASE_NOTES.name} for {tag}")


if __name__ == "__main__":
    main()
