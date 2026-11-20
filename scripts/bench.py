#!/usr/bin/env python3
"""Cairn token and recall benchmark harness.

Starts a disposable pgvector Postgres in Docker and a real cairn-server on it, loads the
synthetic corpus in bench/fixtures, then measures the agent-facing surface through the real
`cairn mcp` binary and the real hook script. Prints a short table and writes JSON.

    python3 scripts/bench.py --out bench/result.json [--label NAME] [--lexical-only]

Token figures are estimates: bytes / 4. There is no tokenizer and no LLM. The run is
deterministic: fixed corpus, fixed query order, no clocks or random ids in the output.
"""
import argparse
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "bench" / "fixtures"
IMAGE = "pgvector/pgvector:pg17"
PASSWORD = "correct horse battery staple"
DEFAULT_MODEL_DIR = Path.home() / ".cache" / "cairn-bench" / "minilm"
# Ten distinct files for the scripted Edit events. A mix of topics the corpus covers and not.
EDIT_FILES = [
    "src/jobs/queue.rs", "src/billing/money.rs", "src/api/handlers/refund.rs",
    "src/orders/list.rs", "src/telemetry.rs", "templates/invoice.typ",
    "migrations/0042_add_tax_column.sql", "console/src/App.tsx",
    "src/payments/webhooks.rs", "docs/README.md",
]
STOPWORDS = set(
    "a an and are as at be by do does for from how in is it of on or the to we what when which "
    "who why with this that there their they them our can may must not no only any all".split()
)


def est_tokens(nbytes):
    return round(nbytes / 4)


def blen(text):
    return len(text.encode("utf-8"))


def compact(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def words(text):
    return {w for w in re.findall(r"[a-z0-9]+", text.lower()) if len(w) > 2 and w not in STOPWORDS}


def searchable(memory):
    """Every text the memory carries, `fields` included, except its fixture key."""
    parts = []
    for k, v in memory.items():
        if k == "key":
            continue
        parts.append(searchable(v) if isinstance(v, dict) else " ".join(map(str, v)) if isinstance(v, list) else str(v))
    return " ".join(parts)


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def run(cmd, **kwargs):
    return subprocess.run(cmd, check=True, text=True, capture_output=True, **kwargs)


class Http:
    def __init__(self, base):
        self.base = base

    def call(self, path, payload=None, token=None):
        data = None if payload is None else json.dumps(payload).encode()
        headers = {"Content-Type": "application/json"} if data is not None else {}
        if token:
            headers["Authorization"] = "Bearer " + token
        request = urllib.request.Request(self.base + path, data=data, headers=headers,
                                         method="POST" if data is not None else "GET")
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        with opener.open(request, timeout=30) as response:
            body = response.read()
            return json.loads(body) if body else None


class Mcp:
    """Newline-delimited JSON-RPC to a `cairn mcp` child, as an agent host would drive it."""

    def __init__(self, exe, cwd, env):
        self.proc = subprocess.Popen([exe, "mcp"], cwd=cwd, env=env, text=True, bufsize=1,
                                     stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=subprocess.DEVNULL)
        self.next_id = 0

    def rpc(self, method, params=None):
        self.next_id += 1
        message = {"jsonrpc": "2.0", "id": self.next_id, "method": method}
        if params is not None:
            message["params"] = params
        self.proc.stdin.write(json.dumps(message) + "\n")
        self.proc.stdin.flush()
        line = self.proc.stdout.readline()
        if not line:
            raise RuntimeError("cairn mcp closed its output")
        reply = json.loads(line)
        if "error" in reply:
            raise RuntimeError(f"{method}: {reply['error']}")
        return reply["result"]

    def tool(self, name, **arguments):
        """Returns (structured result, wire bytes of the MCP result, bytes of the text channel)."""
        result = self.rpc("tools/call", {"name": name, "arguments": arguments})
        if result.get("isError"):
            raise RuntimeError(f"{name}: {result['content'][0]['text']}")
        text = result["content"][0]["text"]
        return result.get("structuredContent"), blen(compact(result)), blen(text)

    def close(self):
        self.proc.stdin.close()
        self.proc.wait(timeout=10)


def stats(values):
    if not values:
        return {"calls": 0, "mean_bytes": 0, "min_bytes": 0, "max_bytes": 0, "mean_tokens_est": 0}
    mean = round(sum(values) / len(values))
    return {"calls": len(values), "mean_bytes": mean, "min_bytes": min(values),
            "max_bytes": max(values), "mean_tokens_est": est_tokens(mean)}


def result_ids(structured):
    rows = (structured or {}).get("results") or []
    return [row["id"] for row in rows if isinstance(row, dict) and "id" in row]


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--out", required=True, help="JSON result path")
    parser.add_argument("--label", default="unlabelled")
    parser.add_argument("--model-dir", help="pinned MiniLM directory (default: CAIRN_BENCH_EMBEDDING_MODEL_DIR, "
                        "CAIRN_TEST_EMBEDDING_MODEL_DIR, then ~/.cache/cairn-bench/minilm)")
    parser.add_argument("--lexical-only", action="store_true", help="do not load the embedding model")
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--min-hit5", type=float, help="exit 1 when memory_search hit@5 is below this (a CI gate)")
    parser.add_argument("--dump-injected", help="also write the text each hook event injected to this file (contains random memory ids)")
    args = parser.parse_args()

    corpus = json.loads((FIXTURES / "corpus.json").read_text())["memories"]
    spec = json.loads((FIXTURES / "questions.json").read_text())
    questions, weak = spec["questions"], spec["weak_queries"]
    by_key = {m["key"]: m for m in corpus}

    # Fixture self-check: paraphrase questions share no content word with any gold memory.
    for q in questions:
        for key in q["gold"]:
            assert key in by_key, f"{q['id']}: unknown gold {key}"
        if q["kind"] == "paraphrase":
            for key in q["gold"]:
                shared = words(q["query"]) & words(searchable(by_key[key]))
                assert not shared, f"{q['id']} shares words with {key}: {sorted(shared)}"

    model_dir = None
    if not args.lexical_only:
        for candidate in (args.model_dir, os.environ.get("CAIRN_BENCH_EMBEDDING_MODEL_DIR"),
                          os.environ.get("CAIRN_TEST_EMBEDDING_MODEL_DIR"), str(DEFAULT_MODEL_DIR)):
            if candidate and (Path(candidate) / "model.onnx").is_file():
                model_dir = Path(candidate)
                break

    if not args.skip_build:
        run(["cargo", "build", "-q", "-p", "cairn-server", "-p", "cairn-client"], cwd=ROOT)
    target = json.loads(run(["cargo", "metadata", "--format-version", "1", "--no-deps"], cwd=ROOT).stdout)["target_directory"]
    server_exe, cairn_exe = f"{target}/debug/cairn-server", f"{target}/debug/cairn"

    work = Path(tempfile.mkdtemp(prefix="cairn-bench."))
    container = None
    server = None
    mcp = None
    try:
        # Disposable Postgres + pgvector.
        container = run(["docker", "run", "-d", "--rm", "-e", "POSTGRES_PASSWORD=bench",
                         "-e", "POSTGRES_USER=bench", "-e", "POSTGRES_DB=bench",
                         "-p", "127.0.0.1::5432", IMAGE]).stdout.strip()
        db_port = run(["docker", "port", container, "5432/tcp"]).stdout.split(":")[-1].strip()
        # The image restarts once after init, so wait for two consecutive successful queries.
        ok = 0
        for _ in range(300):
            done = subprocess.run(["docker", "exec", container, "psql", "-U", "bench", "-d", "bench", "-tAc", "select 1"],
                                  capture_output=True, text=True)
            ok = ok + 1 if done.returncode == 0 else 0
            if ok >= 3:
                break
            time.sleep(0.5)
        else:
            raise RuntimeError("postgres did not become ready")

        def sql(query):
            return run(["docker", "exec", container, "psql", "-U", "bench", "-d", "bench", "-tAc", query]).stdout.strip()

        clean = {k: v for k, v in os.environ.items() if not k.startswith("CAIRN_") and not k.startswith("ORCA_")}
        env = dict(clean, HOME=str(work / "home"), XDG_CONFIG_HOME=str(work / "config"),
                   CAIRN_NO_UPDATE_CHECK="1", GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_SYSTEM="/dev/null")
        (work / "home").mkdir()

        port = free_port()
        base = f"http://127.0.0.1:{port}"
        server_cmd = [server_exe, "--database-url", f"postgres://bench:bench@127.0.0.1:{db_port}/bench", "--bind", f"127.0.0.1:{port}"]
        if model_dir:
            server_cmd += ["--embedding-model-dir", str(model_dir)]
        server = subprocess.Popen(server_cmd, env=env, stdout=open(work / "server.log", "w"), stderr=subprocess.STDOUT, start_new_session=True)
        http = Http(base + "/cairn-memory")
        for _ in range(600):
            try:
                health = http.call("/health")
                break
            except (urllib.error.URLError, OSError):
                if server.poll() is not None:
                    raise RuntimeError("server exited:\n" + (work / "server.log").read_text())
                time.sleep(0.1)
        else:
            raise RuntimeError("server did not become ready")
        semantic = health.get("semantic_search")

        session = http.call("/auth/bootstrap", {"email": "bench-owner@example.test", "password": PASSWORD})["token"]
        device = http.call("/devices/register", {"name": "bench-agent"}, session)["token"]
        subprocess.run([cairn_exe, "setup", base], input=device + "\n", text=True, env=env, check=True, capture_output=True)
        config_file = work / "config" / "cairn" / "config.json"

        # A git checkout with a committed .cairn/project.json, as a real repository would have.
        repo = work / "harbor"
        repo.mkdir()
        git_env = dict(env, GIT_AUTHOR_NAME="bench", GIT_AUTHOR_EMAIL="bench@example.test",
                       GIT_COMMITTER_NAME="bench", GIT_COMMITTER_EMAIL="bench@example.test",
                       GIT_AUTHOR_DATE="2026-01-01T00:00:00Z", GIT_COMMITTER_DATE="2026-01-01T00:00:00Z")
        run(["git", "init", "-q", "-b", "main"], cwd=repo, env=git_env)
        run(["git", "remote", "add", "origin", "https://example.test/bench/harbor.git"], cwd=repo, env=git_env)
        (repo / "README.md").write_text("harbor\n")
        run(["git", "add", "."], cwd=repo, env=git_env)
        run(["git", "commit", "-q", "-m", "init"], cwd=repo, env=git_env)
        run([cairn_exe, "init", "Harbor"], cwd=repo, env=env)
        project_id = json.loads((repo / ".cairn" / "project.json").read_text())["project_id"]
        context = {"project_id": project_id, "repository": "example.test/bench/harbor"}

        # Load the corpus through the same /remember route the save tools use.
        ids = {}
        for memory in corpus:
            body = {k: v for k, v in memory.items() if k != "key"}
            body.update(scope="project", context=context)
            if memory["kind"] in ("rule", "decision"):
                # created_at has one-second resolution and the session brief ranks rules and decisions by it
                # (ties fall back to id). Without a boundary between saves, which ones the brief shows would
                # depend on where the wall-clock second changes during the load.
                time.sleep(1.0 - (time.time() % 1.0) + 0.02)
            reply = http.call("/remember", body, device)
            ids[memory["key"]] = reply["id"]
        assert len(set(ids.values())) == len(corpus), "a corpus memory was merged into another on save"

        def settle():
            # Search embeds the query with try_lock and silently drops to lexical-only while the background
            # embedding job holds the model, so wait until every memory has its vector before measuring.
            if not model_dir:
                return
            for _ in range(240):
                if int(sql("select count(*) from cairn.memories m left join cairn.memory_embeddings e using (memory_id) where e.memory_id is null") or 0) == 0:
                    return
                time.sleep(0.25)
            raise RuntimeError("embeddings were not produced")

        settle()
        key_of = {v: k for k, v in ids.items()}

        # (a) MCP surface.
        mcp = Mcp(cairn_exe, repo, env)
        init = mcp.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "bench", "version": "0"}})
        tools_list = mcp.rpc("tools/list", {})
        instructions = init.get("instructions", "")
        surface = {
            "tool_count": len(tools_list["tools"]),
            "tools_list_bytes": blen(compact(tools_list)),
            "tools_list_tokens_est": est_tokens(blen(compact(tools_list))),
            "tool_names_bytes": sum(blen(t["name"]) + 1 for t in tools_list["tools"]),
            "initialize_instructions_bytes": blen(instructions),
            "initialize_instructions_tokens_est": est_tokens(blen(instructions)),
            "largest_tools": sorted(((blen(compact(t)), t["name"]) for t in tools_list["tools"]), reverse=True)[:3],
        }

        # (c) recall quality, (d) bytes per search call, (e) zero-hit behaviour.
        # Query order is fixed. Recall touches access counts, which feed ranking, so this order is part
        # of the benchmark definition.
        recall = {}
        per_question = []
        search_wire, search_text = [], []
        for tool in ("memory_search", "memory_recall"):
            wire_all, text_all = [], []
            scores = {"recall5": [], "hit5": [], "rr": []}
            by_kind = {}
            for q in questions:
                gold = {ids[k] for k in q["gold"]}
                # limit 5 is what an agent sees by default; the bytes are measured on it.
                shaped, wire, text = mcp.tool(tool, query=q["query"], limit=5)
                wire_all.append(wire)
                text_all.append(text)
                top5 = result_ids(shaped)
                deep, _, _ = mcp.tool(tool, query=q["query"], limit=10)
                ranked = result_ids(deep)
                rank = next((i + 1 for i, mid in enumerate(ranked) if mid in gold), None)
                values = {
                    "recall5": len(gold & set(top5)) / len(gold),
                    "hit5": 1.0 if gold & set(top5) else 0.0,
                    "rr": 1.0 / rank if rank else 0.0,
                }
                for name, value in values.items():
                    scores[name].append(value)
                    by_kind.setdefault(q["kind"], {}).setdefault(name, []).append(value)
                if tool == "memory_search":
                    per_question.append({"id": q["id"], "kind": q["kind"], "gold": q["gold"], "first_gold_rank_in_10": rank,
                                         "gold_in_top5": sorted(key_of[m] for m in gold & set(top5)),
                                         "top5": [key_of.get(m, "?") for m in top5]})
            avg = lambda xs: round(sum(xs) / len(xs), 4)
            recall[tool] = {
                "questions": len(questions),
                "recall_at_5": avg(scores["recall5"]),
                "hit_at_5": avg(scores["hit5"]),
                "mrr_at_10": avg(scores["rr"]),
                "by_kind": {k: {n: avg(v) for n, v in d.items()} for k, d in sorted(by_kind.items())},
                "bytes_per_call": stats(wire_all),
                "text_channel_bytes_per_call": stats(text_all),
            }
            if tool == "memory_search":
                search_wire, search_text = wire_all, text_all

        zero = {}
        for tool in ("memory_search", "memory_recall"):
            rows = []
            for w in weak:
                shaped, wire, _ = mcp.tool(tool, query=w["query"], limit=5)
                rows.append({"id": w["id"], "hits": len(result_ids(shaped)), "bytes": wire, "top": [key_of.get(m, "?") for m in result_ids(shaped)]})
            zero[tool] = {"queries": len(weak), "correct_zero_hit": sum(1 for r in rows if r["hits"] == 0), "rows": rows}

        # (d) other call sizes, on corpus data, before the scripted session adds its own memories.
        calls = {"memory_search": stats(search_wire)}
        calls["memory_search"]["note"] = "limit 5, mean over the gold questions"
        shaped, wire, text = mcp.tool("memory_resume", limit=2)
        calls["memory_resume"] = {"limit": 2, "results": len((shaped or {}).get("results") or []), "bytes": wire, "tokens_est": est_tokens(wire), "text_channel_bytes": text}
        why = []
        for query in ("migration lock deploy", "money cents", "payment webhook duplicate"):
            shaped, wire, text = mcp.tool("memory_why_changed", query=query, limit=3)
            why.append(wire)
        calls["memory_why_changed"] = dict(stats(why), limit=3, note="3 queries")
        get = []
        for key in ("dec-queue-postgres", "got-migration-lock", "proc-release", "sess-typst"):
            shaped, wire, text = mcp.tool("memory_get", memoryId=ids[key])
            get.append(wire)
        calls["memory_get"] = dict(stats(get), note="decision, gotcha, procedure, note")
        shaped, wire, text = mcp.tool("memory_context", query="payments and idempotency", limit=5)
        calls["memory_context"] = {"limit": 5, "bytes": wire, "tokens_est": est_tokens(wire)}
        mcp.close()
        mcp = None

        # (b) one scripted Claude Code session through the real hook script.
        def hook(event, **extra):
            settle()
            payload = {"hook_event_name": event, "session_id": "bench-session-1", "cwd": str(repo)}
            payload.update(extra)
            done = subprocess.run([sys.executable, str(ROOT / "scripts" / "cairn_hook.py"), "--agent", "claude", "--config", str(config_file)],
                                  input=json.dumps(payload), text=True, capture_output=True, env=env, timeout=30)
            if done.returncode != 0:
                raise RuntimeError(f"hook {event} failed: {done.stderr}")
            out = done.stdout.strip()
            injected = ""
            if out:
                try:
                    injected = json.loads(out)["hookSpecificOutput"]["additionalContext"]
                except (ValueError, KeyError, TypeError):
                    injected = ""
            if args.dump_injected:
                with open(args.dump_injected, "a", encoding="utf-8") as dump:
                    dump.write(f"--- {event} {extra.get('tool_input', {}).get('file_path', '')}\n{injected}\n")
            return {"event": event, "stdout_bytes": blen(out), "injected_bytes": blen(injected), "injected_tokens_est": est_tokens(blen(injected))}

        events = [hook("SessionStart", source="startup")]
        for path in EDIT_FILES:
            events.append(dict(hook("PostToolUse", tool_name="Edit",
                                    tool_input={"file_path": path, "old_string": "a", "new_string": "b"},
                                    tool_response={"success": True}), file=path))
        events.append(hook("Stop", stop_hook_active=False))
        total = sum(e["injected_bytes"] for e in events)
        session_bytes = {
            "events": events,
            "total_injected_bytes": total,
            "total_injected_tokens_est": est_tokens(total),
            "session_start_bytes": events[0]["injected_bytes"],
            "post_tool_use_bytes": sum(e["injected_bytes"] for e in events[1:-1]),
            "note": "SessionStart + 10 Edit PostToolUse on distinct files + Stop, bytes of additionalContext",
        }

        # Fixed per-session cost for an agent that loads the whole MCP surface eagerly vs deferred.
        fixed = {
            "eager_hosts_bytes": surface["tools_list_bytes"] + surface["initialize_instructions_bytes"] + total,
            "deferred_hosts_bytes": surface["tool_names_bytes"] + surface["initialize_instructions_bytes"] + total,
        }
        fixed["eager_hosts_tokens_est"] = est_tokens(fixed["eager_hosts_bytes"])
        fixed["deferred_hosts_tokens_est"] = est_tokens(fixed["deferred_hosts_bytes"])
        fixed["note"] = "tools/list (or just names) + initialize.instructions + scripted-session hook injection; excludes retrieval calls"

        result = {
            "label": args.label,
            "token_estimate": "bytes / 4; an estimate, not a tokenizer count",
            "corpus": {"memories": len(corpus), "questions": len(questions), "weak_queries": len(weak)},
            "semantic_channel": {"server_health": semantic, "embedding_model": "pinned all-MiniLM-L6-v2" if model_dir else None,
                                 "mode": "hybrid (lexical + entity + semantic)" if model_dir else "lexical-only (no embedding model loaded)"},
            "mcp_surface": surface,
            "session_injection": session_bytes,
            "fixed_session_cost": fixed,
            "recall": recall,
            "zero_hit": zero,
            "call_bytes": calls,
            "per_question_memory_search": per_question,
        }
        out = Path(args.out)
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
        print_table(result)
        print(f"\nwrote {out}")
        hit5 = recall["memory_search"]["hit_at_5"]
        if args.min_hit5 is not None and hit5 < args.min_hit5:
            print(f"FAIL: memory_search hit@5 {hit5:.3f} is below --min-hit5 {args.min_hit5}", file=sys.stderr)
            sys.exit(1)
    finally:
        if mcp:
            try:
                mcp.close()
            except Exception:
                pass
        if server and server.poll() is None:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
        if container:
            subprocess.run(["docker", "rm", "-f", container], capture_output=True)
        shutil.rmtree(work, ignore_errors=True)


def print_table(r):
    s, f, rec = r["mcp_surface"], r["session_injection"], r["recall"]
    rows = [
        ("semantic channel", r["semantic_channel"]["mode"]),
        ("MCP tools", f"{s['tool_count']}"),
        ("tools/list", f"{s['tools_list_bytes']} B (~{s['tools_list_tokens_est']} tok est)"),
        ("initialize.instructions", f"{s['initialize_instructions_bytes']} B (~{s['initialize_instructions_tokens_est']} tok est)"),
        ("hook injection / scripted session", f"{f['total_injected_bytes']} B (~{f['total_injected_tokens_est']} tok est)"
         f"  [start {f['session_start_bytes']} B, 10 edits {f['post_tool_use_bytes']} B]"),
    ]
    for tool in ("memory_search", "memory_recall"):
        t = rec[tool]
        rows.append((f"{tool} recall@5 / hit@5 / MRR@10",
                     f"{t['recall_at_5']:.3f} / {t['hit_at_5']:.3f} / {t['mrr_at_10']:.3f}  (n={t['questions']}, mean {t['bytes_per_call']['mean_bytes']} B/call)"))
    for tool, z in r["zero_hit"].items():
        rows.append((f"{tool} zero-hit correct", f"{z['correct_zero_hit']}/{z['queries']}"))
    c = r["call_bytes"]
    rows.append(("bytes: search (mean, limit 5)", f"{c['memory_search']['mean_bytes']}"))
    rows.append(("bytes: resume (limit 2)", f"{c['memory_resume']['bytes']}  ({c['memory_resume']['results']} results)"))
    rows.append(("bytes: why_changed (mean, limit 3)", f"{c['memory_why_changed']['mean_bytes']}"))
    rows.append(("bytes: get (mean)", f"{c['memory_get']['mean_bytes']}"))
    width = max(len(k) for k, _ in rows)
    print(f"Cairn benchmark: {r['label']}  (tokens = bytes/4, estimate)")
    for key, value in rows:
        print(f"  {key.ljust(width)}  {value}")


if __name__ == "__main__":
    main()
