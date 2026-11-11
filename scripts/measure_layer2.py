#!/usr/bin/env python3
"""Run the small Layer 2 golden-corpus measurement against a real Cairn server."""

from __future__ import annotations

import json
import os
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
from typing import Any
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CORPUS = ROOT / "fixtures/phase-0/golden-corpus/v0.1/layer2-cases.json"
PORT = "3120"
BASE = f"http://127.0.0.1:{PORT}"


def request(path: str, payload: dict) -> Any:
    body = json.dumps(payload).encode()
    req = urllib.request.Request(
        BASE + path,
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=10) as response:
        return json.loads(response.read())


def wait_for_server() -> None:
    for _ in range(600):
        try:
            with urllib.request.urlopen(BASE + "/cairn-memory/health", timeout=1):
                return
        except (urllib.error.URLError, TimeoutError):
            time.sleep(0.1)
    raise RuntimeError("Layer 2 measurement server did not become ready")


def main() -> int:
    corpus = json.loads(CORPUS.read_text(encoding="utf-8"))
    file_descriptor, store_path = tempfile.mkstemp(prefix="cairn-layer2-measure-", suffix=".json")
    os.close(file_descriptor)
    os.unlink(store_path)
    try:
        process = subprocess.Popen(
            [
                "cargo",
                "run",
                "-q",
                "-p",
                "cairn-server",
                "--",
                "--store",
                store_path,
                "--bind",
                f"127.0.0.1:{PORT}",
            ],
            cwd=ROOT,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        try:
            wait_for_server()
            project_context = {
                "repository": "cairn-demo",
                "files": ["src/cache.rs"],
                "symbols": ["Cache::get"],
                "commands": ["cargo test cache"],
                "tests": ["cache_reads_are_consistent"],
                "services": ["cache"],
            }
            branch_context = {
                "repository": "cairn-demo",
                "branch": "experiment/redis-cache",
                "worktree": "wt-cache",
                "commit": "abc1234",
            }
            current_context = {
                "repository": "cairn-demo",
                "branch": "experiment/redis-cache",
                "worktree": "wt-cache",
                "commit": "def5678",
            }
            project = request(
                "/cairn-memory/remember",
                {
                    "content": "The project uses Redis as the default cache because reads are fast.",
                    "context": project_context,
                },
            )
            competing = request(
                "/cairn-memory/remember",
                {
                    "content": "The project does not use Redis as the default cache because consistency is preferred.",
                    "context": {"repository": "cairn-demo"},
                },
            )
            branch = request(
                "/cairn-memory/remember",
                {
                    "content": "The Redis default cache experiment is branch-local.",
                    "context": branch_context,
                },
            )
            stale = request(
                "/cairn-memory/remember",
                {
                    "content": "The old cache benchmark remains useful because it passed.",
                    "context": branch_context,
                    "lifecycle": {"freshness": 40},
                },
            )

            why = request(
                "/cairn-memory/why-changed",
                {"query": "Redis default cache", "limit": 10, "context": current_context},
            )
            why_results = why["results"]
            project_result = next(result for result in why_results if result["memory_id"] == project["memory_id"])
            competing_result = next(result for result in why_results if result["memory_id"] == competing["memory_id"])
            stale_result = next(result for result in why_results if result["memory_id"] == stale["memory_id"])

            main_results = request(
                "/cairn-memory/search",
                {
                    "query": "Redis default cache",
                    "limit": 10,
                    "context": {"repository": "cairn-demo", "branch": "main", "worktree": "wt-main"},
                },
            )
            matching_results = request(
                "/cairn-memory/search",
                {"query": "Redis default cache", "limit": 10, "context": current_context},
            )
            repeated = request(
                "/cairn-memory/why-changed",
                {"query": "Redis default cache", "limit": 10, "context": current_context},
            )
            first_ids = [result["memory_id"] for result in why_results]
            repeated_ids = [result["memory_id"] for result in repeated["results"]]

            checks = {
                "why-cache-change": bool(
                    project_result["decision_reason"]
                    and project_result["source_event_ids"]
                    and any(item["relation"] == "file_contains_symbol" for item in project_result["relationships"])
                ),
                "branch-hypothesis-isolation": (
                    branch["memory_id"] not in {result["memory_id"] for result in main_results}
                    and branch["memory_id"] in {result["memory_id"] for result in matching_results}
                ),
                "stale-commit-signal": stale_result["stale"] is True,
                "contradictory-cache-decisions": (
                    competing_result["contradictory"] is True
                    and project["memory_id"] in competing_result["contradiction_ids"]
                ),
            }
            report = {
                "corpusVersion": corpus["corpusVersion"],
                "cases": len(corpus["cases"]),
                "passed": sum(checks.values()),
                "checks": checks,
                "firstInformedAction": checks["why-cache-change"],
                "repeatedInvestigation": {
                    "stable": first_ids == repeated_ids,
                    "duplicateIds": len(repeated_ids) - len(set(repeated_ids)),
                },
                "scopePrecision": checks["branch-hypothesis-isolation"],
                "staleAndContradictionSignals": checks["stale-commit-signal"] and checks["contradictory-cache-decisions"],
            }
            print(json.dumps(report, indent=2, sort_keys=True))
            if not all(checks.values()) or not report["repeatedInvestigation"]["stable"]:
                return 1
            return 0
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
    finally:
        try:
            os.unlink(store_path)
        except FileNotFoundError:
            pass


if __name__ == "__main__":
    raise SystemExit(main())
