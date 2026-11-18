#!/usr/bin/env python3
"""Validate Cairn's Phase 0 documentation and contract fixtures."""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path


LINK_PATTERN = re.compile(r"\[[^\]]+\]\(([^)]+)\)")
ID_PATTERN = re.compile(r"^id:\s*(\S+)", re.MULTILINE)
TITLE_PATTERN = re.compile(r'^title:\s*"(.*)"\s*$', re.MULTILINE)
H1_PATTERN = re.compile(r"^# (.+)$", re.MULTILINE)


def validate_markdown(root: Path) -> list[str]:
    errors: list[str] = []
    files = [root / "README.md", *sorted((root / "docs").rglob("*.md"))]
    ids: dict[str, str] = {}

    for path in files:
        text = path.read_text(encoding="utf-8")
        relative = path.relative_to(root)

        if path != root / "README.md":
            parts = text.split("---\n", 2)
            if not text.startswith("---\n") or len(parts) < 3:
                errors.append(f"{relative}: malformed YAML header")
            else:
                header = parts[1]
                for key in ("id:", "title:", "status:"):
                    if key not in header:
                        errors.append(f"{relative}: missing {key[:-1]}")
                match = ID_PATTERN.search(header)
                if match:
                    document_id = match.group(1)
                    if document_id in ids:
                        errors.append(
                            f"{relative}: duplicate id {document_id} also in {ids[document_id]}"
                        )
                    ids[document_id] = str(relative)
                title = TITLE_PATTERN.search(header)
                heading = H1_PATTERN.search(parts[2])
                if title is None or heading is None or title.group(1) != heading.group(1).strip():
                    errors.append(f"{relative}: title does not match the H1")

        for target in LINK_PATTERN.findall(text):
            target = target.split("#", 1)[0]
            if not target or "://" in target or target.startswith("mailto:"):
                continue
            if not (path.parent / target).resolve().exists():
                errors.append(f"{relative}: broken link {target}")

    print(f"Markdown: {len(files)} files, {len(ids)} document IDs")
    return errors


def validate_fixtures(root: Path) -> list[str]:
    errors: list[str] = []
    fixture_dir = root / "fixtures" / "phase-0"
    count = 0

    for path in sorted(fixture_dir.rglob("*.jsonl")):
        count += 1
        for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if not line.strip():
                continue
            try:
                json.loads(line)
            except json.JSONDecodeError as exc:
                errors.append(f"{path.relative_to(root)}:{line_number}: invalid JSONL: {exc}")

    for path in sorted(fixture_dir.rglob("*.json")):
        count += 1
        try:
            json.loads(path.read_text(encoding="utf-8"))
        except json.JSONDecodeError as exc:
            errors.append(f"{path.relative_to(root)}: invalid JSON: {exc}")

    print(f"Fixtures: {count} JSON/JSONL files")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parents[1],
        help="Cairn workspace root (defaults to the parent of scripts/)",
    )
    args = parser.parse_args()
    root = args.root.resolve()

    errors = validate_markdown(root) + validate_fixtures(root)
    if errors:
        print("Phase 0 validation failed:", file=sys.stderr)
        print("\n".join(errors), file=sys.stderr)
        return 1

    print("Phase 0 validation passed: headers, IDs, links, JSON, and JSONL are valid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
