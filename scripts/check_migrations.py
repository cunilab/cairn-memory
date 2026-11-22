#!/usr/bin/env python3
"""Fail when a migration drops, deletes, or truncates data or schema.

See docs/plan/0.1.9/00-guard.md. Dollar-quoted bodies are scanned with the rest
of the file; they are not skipped.
"""

from __future__ import annotations

import argparse
import re
from pathlib import Path

FLAG_PATTERNS = (
    re.compile(
        r"\bDROP\s+(?:TABLE|COLUMN|SCHEMA|DATABASE|VIEW|MATERIALIZED\s+VIEW|TYPE|INDEX|FUNCTION|TRIGGER|EXTENSION)\b",
        re.IGNORECASE,
    ),
    re.compile(r"\bALTER\s+TABLE\b.*\bDROP\b", re.IGNORECASE | re.DOTALL),
    re.compile(r"\bDELETE\s+FROM\b", re.IGNORECASE),
    re.compile(r"\bTRUNCATE\b", re.IGNORECASE),
)
BLOCK_COMMENT = re.compile(r"/\*.*?\*/", re.DOTALL)
LINE_COMMENT = re.compile(r"--[^\n]*")
WHITESPACE = re.compile(r"\s+")
SPEC = "docs/plan/0.1.9/00-guard.md"


def normalise(text: str) -> str:
    return WHITESPACE.sub(" ", text).strip()


def statements(sql: str) -> list[str]:
    stripped = LINE_COMMENT.sub(" ", BLOCK_COMMENT.sub(" ", sql))
    found = []
    for part in stripped.split(";"):
        statement = normalise(part)
        if statement:
            found.append(statement)
    return found


def flagged(statement: str) -> bool:
    return any(pattern.search(statement) for pattern in FLAG_PATTERNS)


def allowlist(path: Path) -> list[tuple[str, str]]:
    if not path.is_file():
        return []
    entries: list[tuple[str, str]] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        raw = line.strip()
        if not raw or raw.startswith("#"):
            continue
        filename, prefix, _reason = raw.split("|", 2)
        entries.append((filename.strip(), normalise(prefix)))
    return entries


def check(root: Path) -> tuple[int, list[str]]:
    migrations = root / "migrations"
    files = sorted(migrations.glob("*.sql"))
    entries = allowlist(migrations / "destructive-allowlist.txt")
    found: list[tuple[str, str]] = []
    for path in files:
        found.extend(
            (path.name, statement)
            for statement in statements(path.read_text(encoding="utf-8"))
            if flagged(statement)
        )

    used = [False] * len(entries)
    violations: list[str] = []
    allowlisted = 0
    for name, statement in found:
        folded = statement.casefold()
        matched = False
        for index, (filename, prefix) in enumerate(entries):
            if filename == name and folded.startswith(prefix.casefold()):
                used[index] = True
                matched = True
        if matched:
            allowlisted += 1
        else:
            violations.append(f"migrations/{name}: {statement[:80]}")

    for consumed, (filename, prefix) in zip(used, entries):
        if not consumed:
            violations.append(f"stale allowlist entry: {filename}|{prefix}")

    if violations:
        violations.append(f"Blocked by {SPEC}. An allowlist line needs user approval.")
        return 1, violations
    return 0, [f"migrations: {len(files)} files, {allowlisted} allowlisted"]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parents[1],
        help="Cairn workspace root (defaults to the parent of scripts/)",
    )
    args = parser.parse_args(argv)
    code, lines = check(args.root.resolve())
    print("\n".join(lines))
    return code


if __name__ == "__main__":
    raise SystemExit(main())
