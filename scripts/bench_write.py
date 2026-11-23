#!/usr/bin/env python3
"""Exercise write-side session guarantees against a disposable Postgres server."""
import argparse
import json
import os
import shutil
import subprocess
import tempfile
import time
import urllib.error
from pathlib import Path

from bench import Http, IMAGE, PASSWORD, ROOT, free_port, run


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--out", required=True)
    args = parser.parse_args()
    started = time.monotonic()
    fixture = json.loads((ROOT / "bench/fixtures/write-sessions.json").read_text())
    if not args.skip_build:
        run(["cargo", "build", "-q", "-p", "cairn-server", "-p", "cairn-client"], cwd=ROOT)
    target = json.loads(run(["cargo", "metadata", "--format-version", "1", "--no-deps"], cwd=ROOT).stdout)["target_directory"]
    server_exe, cairn = f"{target}/debug/cairn-server", f"{target}/debug/cairn"
    work = Path(tempfile.mkdtemp(prefix="cairn-write-bench."))
    container = server = None
    try:
        container = run(["docker", "run", "-d", "--rm", "-e", "POSTGRES_PASSWORD=bench", "-e", "POSTGRES_USER=bench", "-e", "POSTGRES_DB=bench", "-p", "127.0.0.1::5432", IMAGE]).stdout.strip()
        port = run(["docker", "port", container, "5432/tcp"]).stdout.split(":")[-1].strip()
        ready = 0
        for _ in range(300):
            probe = subprocess.run(["docker", "exec", container, "psql", "-U", "bench", "-d", "bench", "-tAc", "select 1"], capture_output=True)
            ready = ready + 1 if probe.returncode == 0 else 0
            if ready >= 3:
                break
            time.sleep(0.5)
        else:
            raise RuntimeError("postgres did not become ready")
        env = {k: v for k, v in os.environ.items() if not k.startswith(("CAIRN_", "ORCA_"))}
        env.update(HOME=str(work / "home"), XDG_CONFIG_HOME=str(work / "config"), CAIRN_NO_UPDATE_CHECK="1", GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_SYSTEM="/dev/null", GIT_AUTHOR_NAME="bench", GIT_AUTHOR_EMAIL="bench@example.test", GIT_COMMITTER_NAME="bench", GIT_COMMITTER_EMAIL="bench@example.test")
        (work / "home").mkdir()
        server_port = free_port()
        base = f"http://127.0.0.1:{server_port}"
        with (work / "server.log").open("w") as log:
            server = subprocess.Popen([server_exe, "--database-url", f"postgres://bench:bench@127.0.0.1:{port}/bench", "--bind", f"127.0.0.1:{server_port}"], env=env, stdout=log, stderr=subprocess.STDOUT)
        http = Http(base + "/cairn-memory")
        for _ in range(600):
            try:
                http.call("/health")
                break
            except (urllib.error.URLError, OSError):
                if server.poll() is not None:
                    raise RuntimeError("server exited: " + (work / "server.log").read_text())
                time.sleep(0.1)
        else:
            raise RuntimeError("server did not become ready")
        account = http.call("/auth/bootstrap", {"email": "bench@example.test", "password": PASSWORD})["token"]
        device = http.call("/devices/register", {"name": "bench"}, account)["token"]
        subprocess.run([cairn, "setup", base], input=device + "\n", text=True, env=env, check=True, capture_output=True)
        config = work / "config/cairn/config.json"
        repo = work / "repo"
        repo.mkdir()
        run(["git", "init", "-q", "-b", "main"], cwd=repo, env=env)
        (repo / "src").mkdir()
        (repo / "src/a.ts").write_text("a\n")
        (repo / "src/old.ts").write_text("old\n")
        (repo / "src/sub.ts").write_text("a\n")
        run(["git", "add", "."], cwd=repo, env=env)
        run(["git", "commit", "-qm", "init"], cwd=repo, env=env)
        run([cairn, "init", "Write bench"], cwd=repo, env=env)
        project = json.loads((repo / ".cairn/project.json").read_text())["project_id"]
        plain = work / "plain"
        plain.mkdir()
        run([cairn, "init", "Plain bench"], cwd=plain, env=env)
        plain_project = json.loads((plain / ".cairn/project.json").read_text())["project_id"]

        def settle():
            deadline = time.monotonic() + 60
            while time.monotonic() < deadline:
                jobs = http.call("/admin/jobs", token=account)
                if jobs["quarantined"]:
                    raise RuntimeError(f"quarantined jobs: {jobs['quarantine']}")
                if jobs["pending"] == jobs["running"] == 0:
                    return
                time.sleep(0.2)
            raise RuntimeError("jobs did not settle in 60 seconds")

        def rows(project_id):
            return http.call(f"/memories?project_id={project_id}&limit=100", token=device)

        def session(name, events, cwd, automated=False):
            outputs = []
            for item in events:
                item = dict(item)
                event = item.pop("event")
                if automated and event == "Stop":
                    item["agent_id"] = "bench-agent"
                if name == "work" and item.get("tool_input", {}).get("command") == "rm src/old.ts":
                    (repo / "src/old.ts").unlink()
                payload = dict(item, hook_event_name=event, session_id="write-" + name, cwd=str(cwd))
                event_env = dict(env, CAIRN_SESSION_ORIGIN="automation") if automated else env
                result = subprocess.run([cairn, "hook", event, "--agent", "claude", "--config", str(config)], input=json.dumps(payload), text=True, capture_output=True, env=event_env, timeout=30, check=True)
                reply = json.loads(result.stdout) if result.stdout.strip() else {}
                output = reply.get("reason", "") if event == "Stop" else reply.get("hookSpecificOutput", {}).get("additionalContext", "")
                outputs.append((event, output))
            settle()
            return outputs

        work_out = session("work", fixture["work"], repo)
        work_rows = rows(project)
        handoffs = [m for m in work_rows if m["kind"] == "handoff"]
        handoff = handoffs[0] if len(handoffs) == 1 else {}
        notes = [m for m in work_rows if m["kind"] in ("decision", "gotcha")]
        digest = [m for m in work_rows if m["kind"] == "note" and m["title"].startswith("Session ")]
        work_stops = [out for event, out in work_out if event == "Stop"]
        chat_out = session("chat", fixture["chat"], repo)
        chat_handoffs = [m for m in rows(project) if m["kind"] == "handoff" and m["status"] == "active"]
        sub_out = session("subagent", fixture["subagent"], repo, automated=True)
        sub_rows = rows(project)
        sub_handoffs = [m for m in sub_rows if m["kind"] == "handoff" and m["status"] == "active"]
        work_ids = {m["id"] for m in work_rows}
        sub_notes = [m for m in sub_rows if m["id"] not in work_ids and m["kind"] in ("decision", "gotcha")]
        sub_reports = http.call(f"/projects/{project}/sessions", token=device)
        sub_report = next((r for r in sub_reports if r["session_id"] == "write-subagent"), {})
        session("no-git", fixture["no-git"], plain)
        plain_rows = rows(plain_project)
        reports = http.call(f"/projects/{plain_project}/sessions", token=device)
        injected_bytes = sum(len(out.encode("utf-8")) for _, out in work_out)
        checks = {
            "W1": bool(any(m["kind"] == "decision" for m in notes) and any(m["kind"] == "gotcha" for m in notes) and all(m["scope"] == "project" and m["rank"] == "candidate" for m in notes)),
            "W2": len(digest) == 1 and all(s in digest[0]["summary"] for s in ("src/a.ts", "src/old.ts", "bun test pass")),
            "W3": handoff.get("fields", {}).get("next_steps") == ["next: wire the web report"] and "src/old.ts" in handoff.get("fields", {}).get("files", []),
            "W4": len(work_stops) == 2 and "rule" in work_stops[0].lower() and "never edit migrations" in work_stops[0].lower() and not work_stops[1],
            "W5": not any(out for event, out in chat_out if event == "Stop") and len(chat_handoffs) == 1 and chat_handoffs[0]["id"] == handoff.get("id"),
            "W6": (not any(out for event, out in sub_out if event == "Stop")
                   and len(sub_handoffs) == 1 and sub_handoffs[0]["id"] == handoff.get("id")
                   and sub_report.get("notes_written", 0) >= 1 and sub_report.get("digest_written") is True
                   and bool(sub_notes) and all(m["scope"] == "project" and m["rank"] == "candidate" for m in sub_notes)
                   and {m["id"] for m in sub_rows if m["id"] in {n["id"] for n in notes} and m["rank"] == "candidate"}
                   == {n["id"] for n in notes}),
            "W7": any(r["session_id"] == "write-no-git" and r["captures"] >= 2 for r in reports) and any(m["kind"] == "handoff" and "(1 outside the project)" in m["summary"] for m in plain_rows),
            "W8": injected_bytes <= 1621 + 250,
        }
        result = {"checks": checks, "counts": {"work_notes": len(notes), "work_digest": len(digest), "injected_bytes": injected_bytes, "subagent_notes_written": sub_report.get("notes_written", 0)}, "runtime_seconds": round(time.monotonic() - started, 2)}
        Path(args.out).write_text(json.dumps(result, indent=2) + "\n")
        for name, passed in checks.items():
            print(f"{name}: {'PASS' if passed else 'FAIL'}")
        print(f"hook bytes: {injected_bytes}; runtime: {result['runtime_seconds']}s")
        return 0 if all(checks.values()) else 1
    finally:
        if server:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()  # still clean up the container and temp dir below
        if container:
            subprocess.run(["docker", "rm", "-f", container], capture_output=True)
        shutil.rmtree(work)


if __name__ == "__main__":
    raise SystemExit(main())
