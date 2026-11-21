#!/usr/bin/env python3
"""Cairn token and recall benchmark harness.

Starts a disposable pgvector Postgres in Docker and a real cairn-server on it, loads the
synthetic corpus in bench/fixtures, then measures the agent-facing surface through the real
`cairn mcp` and `cairn hook` binary. Prints a short table and writes JSON.

    python3 scripts/bench.py --out bench/result.json [--label NAME] [--lexical-only]
        [--questions questions-v2.json --distractors 2000]

Question sets (bench/fixtures): `questions.json` (v1, frozen history), `questions-v2.json` (the main
set from step 5c on: stemmed self-check, single-gold paraphrases, partial-overlap weak queries) and
`questions-holdout.json` (never used to tune; reported with every run). `--distractors N` adds N
generated, unrelated memories (scripts/bench_distractors.py) so the relevance gate and precision
are measured on a store that is not tiny.

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

sys.path.insert(0, str(Path(__file__).resolve().parent))
import bench_distractors  # noqa: E402

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
        try:
            with opener.open(request, timeout=30) as response:
                body = response.read()
                return json.loads(body) if body else None
        except urllib.error.HTTPError as err:
            raise RuntimeError(f"{path}: HTTP {err.code}: {err.read().decode(errors='replace')}") from None


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
        """Returns (parsed result, wire bytes of the MCP result, bytes of the text channel)."""
        result = self.rpc("tools/call", {"name": name, "arguments": arguments})
        if result.get("isError"):
            raise RuntimeError(f"{name}: {result['content'][0]['text']}")
        text = result["content"][0]["text"]
        return json.loads(text), blen(compact(result)), blen(text)

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
    parser.add_argument("--questions", default="questions.json", help="main question set in bench/fixtures (default: v1)")
    parser.add_argument("--distractors", type=int, default=0, help="add this many generated unrelated memories")
    parser.add_argument("--min-hit5", type=float, help="exit 1 when memory_search hit@5 is below this (a CI gate)")
    parser.add_argument("--max-weak-hits", type=int, help="exit 1 when the main-set weak queries return more than this many cards in total (a CI gate)")
    parser.add_argument("--dump-scores", help="write gold cosine and coverage per main question and the top candidate cosines per weak query "
                        "(calibration input; starts the server with CAIRN_DEBUG_SCORES=1)")
    parser.add_argument("--budget", type=int, help="pass this token budget to memory_search (default: the server default)")
    parser.add_argument("--dump-injected", help="also write the text each hook event injected to this file (contains random memory ids)")
    args = parser.parse_args()

    corpus = json.loads((FIXTURES / "corpus.json").read_text())["memories"]
    spec = json.loads((FIXTURES / args.questions).read_text())
    questions, weak = spec["questions"], spec["weak_queries"]
    strict = bool(spec.get("strict_coverage"))
    holdout = json.loads((FIXTURES / "questions-holdout.json").read_text())
    by_key = {m["key"]: m for m in corpus}
    distractors = bench_distractors.make(args.distractors) if args.distractors else []

    # Fixture self-check, unstemmed: paraphrase questions share no content word with any gold memory.
    # The stemmed check (what Postgres actually matches) runs after the load, below.
    for q in questions + holdout["questions"]:
        for key in q["gold"]:
            assert key in by_key, f"{q['id']}: unknown gold {key}"
        if q["kind"].startswith("paraphrase"):
            if q["kind"] == "paraphrase_stem":
                continue
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
        server_env = dict(env, CAIRN_DEBUG_SCORES="1") if args.dump_scores else env
        server = subprocess.Popen(server_cmd, env=server_env, stdout=open(work / "server.log", "w"), stderr=subprocess.STDOUT, start_new_session=True)
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
            reply = http.call("/remember", body, device)
            ids[memory["key"]] = reply["id"]
        assert len(set(ids.values())) == len(corpus), "a corpus memory was merged into another on save"
        # Distractors: half engineering notes in the project, then global and personal everyday notes,
        # so every search rule (project, global, own) has unrelated rows to rank.
        for i, memory in enumerate(distractors):
            body = {k: v for k, v in memory.items() if k != "key"}
            body["scope"] = "project" if memory["kind"] != "fact" or i % 4 == 0 else ("global" if i % 4 == 1 else "personal")
            if body["scope"] == "personal" and body["kind"] in ("rule", "decision", "gotcha"):
                body["scope"] = "project"
            body["context"] = context
            ids[memory["key"]] = http.call("/remember", body, device)["id"]
        assert len(set(ids.values())) == len(corpus) + len(distractors), "a distractor was merged into another on save"

        def settle():
            # Measure only once every memory has its vector and the embed and consolidate jobs are done:
            # consolidation supersedes and links near-duplicates, which changes what search returns, so a
            # search that races it makes two runs differ.
            missing = "(select count(*) from cairn.memories where embedding is null)" if model_dir else "0"
            busy = "(select count(*) from cairn.jobs where kind in ('embed','consolidate') and status in ('pending','running'))"
            for _ in range(240 + 4 * len(distractors)):
                if int(sql(f"select {missing} + {busy}") or 0) == 0:
                    return
                time.sleep(1.0 if distractors else 0.25)
            raise RuntimeError("embeddings and consolidation did not settle")

        settle()
        key_of = {v: k for k, v in ids.items()}

        def coverage(query, gold_keys):
            """Share of the query's stemmed lexemes each gold memory's `search_tsv` contains: the
            number the relevance gate reads (`load_candidates`)."""
            gold_ids = ",".join(f"'{ids[k]}'" for k in gold_keys)
            rows = sql(f"""with q as (select coalesce(array_agg(distinct lexeme), '{{}}') lx from unnest(to_tsvector('english', $q${query}$q$)))
                           select m.memory_id, case when cardinality(q.lx) = 0 then 0 else
                             (select count(*) from unnest(m.search_tsv) t where t.lexeme = any(q.lx))::float / cardinality(q.lx) end
                           from cairn.memories m, q where m.memory_id in ({gold_ids})""")
            by_id = dict(line.split("|") for line in rows.splitlines())
            return {k: round(float(by_id[ids[k]]), 3) for k in gold_keys}

        # Stemmed fixture self-check (the review's finding: unstemmed word checks let q17 and q20 pass
        # as paraphrases while Postgres matched page/paging and night/nightly). A strict set asserts
        # it: `paraphrase` has coverage 0 on every gold memory, `paraphrase_stem` has some. Frozen
        # sets only report it.
        gold_coverage = {}
        for q in questions + holdout["questions"]:
            if q["kind"].startswith("paraphrase"):
                gold_coverage[q["id"]] = coverage(q["query"], q["gold"])
        if strict:
            wrong = []
            for q in questions:
                cov = gold_coverage.get(q["id"])
                if q["kind"] == "paraphrase" and not all(v == 0 for v in cov.values()):
                    wrong.append(f"{q['id']} is labelled paraphrase but a gold memory shares stems: {cov}")
                elif q["kind"] == "paraphrase_stem" and not any(v > 0 for v in cov.values()):
                    wrong.append(f"{q['id']} is labelled paraphrase_stem but no gold shares a stem: {cov}")
            assert not wrong, "stemmed self-check failed:\n" + "\n".join(wrong)

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

        extra = {"budget": args.budget} if args.budget else {}

        # (c) recall quality, (d) bytes per search call, (e) zero-hit behaviour.
        # Query order is fixed. Recall touches access counts, which feed ranking, so this order is part
        # of the benchmark definition.
        recall = {}
        per_question = []
        search_wire, search_text = [], []
        for tool in ("memory_search",):
            wire_all, text_all = [], []
            scores = {"recall5": [], "hit5": [], "rr": [], "cards": [], "goldcards": []}
            by_kind = {}
            for q in questions:
                gold = {ids[k] for k in q["gold"]}
                # limit 5 is what an agent sees by default; the bytes are measured on it.
                shaped, wire, text = mcp.tool(tool, query=q["query"], limit=5, **extra)
                wire_all.append(wire)
                text_all.append(text)
                top5 = result_ids(shaped)
                deep, _, _ = mcp.tool(tool, query=q["query"], limit=10, **extra)
                ranked = result_ids(deep)
                rank = next((i + 1 for i, mid in enumerate(ranked) if mid in gold), None)
                values = {
                    "recall5": len(gold & set(top5)) / len(gold),
                    "hit5": 1.0 if gold & set(top5) else 0.0,
                    "rr": 1.0 / rank if rank else 0.0,
                    "cards": len(top5),
                    "goldcards": len(gold & set(top5)),
                }
                for name, value in values.items():
                    scores[name].append(value)
                    by_kind.setdefault(q["kind"], {}).setdefault(name, []).append(value)
                if tool == "memory_search":
                    per_question.append({"id": q["id"], "kind": q["kind"], "gold": q["gold"], "first_gold_rank_in_10": rank,
                                         "gold_coverage": gold_coverage.get(q["id"]),
                                         "cards": len(top5),
                                         "gold_in_top5": sorted(key_of[m] for m in gold & set(top5)),
                                         "top5": [key_of.get(m, "?") for m in top5]})
            avg = lambda xs: round(sum(xs) / len(xs), 4)
            precision = lambda d: round(sum(d["goldcards"]) / sum(d["cards"]), 4) if sum(d["cards"]) else None
            recall[tool] = {
                "questions": len(questions),
                "recall_at_5": avg(scores["recall5"]),
                "hit_at_5": avg(scores["hit5"]),
                "mrr_at_10": avg(scores["rr"]),
                "precision_at_5": precision(scores),
                "cards_returned": sum(scores["cards"]),
                "gold_cards_returned": sum(scores["goldcards"]),
                "by_kind": {k: dict({n: avg(d[n]) for n in ("recall5", "hit5", "rr")}, n=len(d["hit5"]), precision_at_5=precision(d))
                            for k, d in sorted(by_kind.items())},
                "bytes_per_call": stats(wire_all),
                "text_channel_bytes_per_call": stats(text_all),
            }
            if tool == "memory_search":
                search_wire, search_text = wire_all, text_all

        zero = {}
        for tool in ("memory_search",):
            rows = []
            for w in weak:
                shaped, wire, _ = mcp.tool(tool, query=w["query"], limit=5, **extra)
                rows.append({"id": w["id"], "hits": len(result_ids(shaped)), "bytes": wire, "top": [key_of.get(m, "?") for m in result_ids(shaped)]})
            zero[tool] = {"queries": len(weak), "correct_zero_hit": sum(1 for r in rows if r["hits"] == 0), "rows": rows}

        # Holdout set: same measurements, reported next to the main set and never used to tune it.
        recall_holdout = {"questions": len(holdout["questions"]), "per_question": []}
        hits, ranks, cards, goldcards = [], [], 0, 0
        for q in holdout["questions"]:
            gold = {ids[k] for k in q["gold"]}
            shaped, _, _ = mcp.tool("memory_search", query=q["query"], limit=5, **extra)
            top5 = result_ids(shaped)
            deep, _, _ = mcp.tool("memory_search", query=q["query"], limit=10, **extra)
            ranked = result_ids(deep)
            rank = next((i + 1 for i, mid in enumerate(ranked) if mid in gold), None)
            hits.append(1.0 if gold & set(top5) else 0.0)
            ranks.append(1.0 / rank if rank else 0.0)
            cards += len(top5)
            goldcards += len(gold & set(top5))
            recall_holdout["per_question"].append({"id": q["id"], "gold": q["gold"], "first_gold_rank_in_10": rank,
                                                   "gold_coverage": gold_coverage.get(q["id"]), "cards": len(top5),
                                                   "top5": [key_of.get(m, "?") for m in top5]})
        recall_holdout["hit_at_5"] = round(sum(hits) / len(hits), 4)
        recall_holdout["mrr_at_10"] = round(sum(ranks) / len(ranks), 4)
        recall_holdout["precision_at_5"] = round(goldcards / cards, 4) if cards else None
        recall_holdout["cards_returned"] = cards
        rows = []
        for w in holdout["weak_queries"]:
            shaped, wire, _ = mcp.tool("memory_search", query=w["query"], limit=5, **extra)
            rows.append({"id": w["id"], "hits": len(result_ids(shaped)), "bytes": wire, "top": [key_of.get(m, "?") for m in result_ids(shaped)]})
        zero_holdout = {"queries": len(rows), "correct_zero_hit": sum(1 for r in rows if r["hits"] == 0), "rows": rows}

        if args.dump_scores:
            dump_scores(http, device, context, questions, weak, holdout, ids, key_of, args.dump_scores)

        # (d) other call sizes, on corpus data, before the scripted session adds its own memories.
        calls = {"memory_search": stats(search_wire)}
        calls["memory_search"]["note"] = "limit 5, mean over the gold questions"
        decisions = []
        for query in ("migration lock deploy", "money cents", "payment webhook duplicate"):
            shaped, wire, text = mcp.tool("memory_search", query=query, kind="decision", limit=3)
            decisions.append(wire)
        calls["memory_search_kind_decision"] = dict(stats(decisions), limit=3, note="3 queries")
        shaped, wire, text = mcp.tool("memory_search", query="recent work handoff", kind="handoff", limit=2)
        calls["memory_search_kind_handoff"] = {"limit": 2, "results": len(result_ids(shaped)), "bytes": wire, "tokens_est": est_tokens(wire), "text_channel_bytes": text}
        get = []
        for key in ("dec-queue-postgres", "got-migration-lock", "proc-release", "sess-typst"):
            shaped, wire, text = mcp.tool("memory_get", ids=[ids[key]])
            get.append(wire)
        calls["memory_get"] = dict(stats(get), note="decision, gotcha, procedure, note; one id per call")
        mcp.close()
        mcp = None

        # (b) one scripted Claude Code session through the real `cairn hook`.
        def hook(event, **extra):
            settle()
            payload = {"hook_event_name": event, "session_id": "bench-session-1", "cwd": str(repo)}
            payload.update(extra)
            done = subprocess.run([cairn_exe, "hook", event, "--agent", "claude", "--config", str(config_file)],
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

        fetched_before = int(sql("select coalesce(sum(fetched), 0) from cairn.usage") or 0)
        events = [hook("SessionStart", source="startup")]
        for path in EDIT_FILES:
            events.append(dict(hook("PostToolUse", tool_name="Edit",
                                    tool_input={"file_path": path, "old_string": "a", "new_string": "b"},
                                    tool_response={"success": True}), file=path))
        events.append(hook("PreCompact", trigger="auto"))
        events.append(hook("Stop", stop_hook_active=False))
        total = sum(e["injected_bytes"] for e in events)
        usage_delta = int(sql("select coalesce(sum(fetched), 0) from cairn.usage") or 0) - fetched_before
        session_bytes = {
            "events": events,
            "total_injected_bytes": total,
            "total_injected_tokens_est": est_tokens(total),
            "session_start_bytes": events[0]["injected_bytes"],
            "post_tool_use_bytes": sum(e["injected_bytes"] for e in events[1:-1]),
            "note": "SessionStart + 10 Edit PostToolUse on distinct files + PreCompact + Stop, bytes of additionalContext",
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
            "corpus": {"memories": len(corpus), "distractors": len(distractors), "question_set": args.questions,
                       "questions": len(questions), "weak_queries": len(weak)},
            "semantic_channel": {"server_health": semantic, "embedding_model": "pinned all-MiniLM-L6-v2" if model_dir else None,
                                 "mode": "hybrid (lexical + entity + semantic)" if model_dir else "lexical-only (no embedding model loaded)"},
            "mcp_surface": surface,
            "session_injection": session_bytes,
            "usage_after_hooks_delta": usage_delta,
            "fixed_session_cost": fixed,
            "recall": recall,
            "zero_hit": zero,
            "recall_holdout": recall_holdout,
            "zero_hit_holdout": zero_holdout,
            "call_bytes": calls,
            "per_question_memory_search": per_question,
        }
        out = Path(args.out)
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
        print_table(result)
        print(f"\nwrote {out}")
        hit5 = recall["memory_search"]["hit_at_5"]
        weak_hits = sum(r["hits"] for r in zero["memory_search"]["rows"])
        if usage_delta != 0:
            print(f"FAIL: hooks counted {usage_delta} fetches (usage_after_hooks_delta must be 0)", file=sys.stderr)
            sys.exit(1)
        if args.max_weak_hits is not None and weak_hits > args.max_weak_hits:
            print(f"FAIL: weak queries returned {weak_hits} cards, above --max-weak-hits {args.max_weak_hits}", file=sys.stderr)
            sys.exit(1)
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


def dump_scores(http, device, context, questions, weak, holdout, ids, key_of, path):
    """Gate inputs: the gold memories' cosine and coverage per question, and the three highest
    candidate cosines per weak query. Main set first (calibration input, spec 4.3); the holdout is
    dumped under `holdout` to report where its misses fall, never to tune."""
    def trace(query):
        return http.call("/search", {"query": query, "context": context, "debug": True}, device)["trace"]

    out = {"questions": [], "weak": []}
    for q in questions:
        rows = {row["id"]: row for row in trace(q["query"])}
        gold = {}
        for key in q["gold"]:
            row = rows.get(ids[key])
            gold[key] = None if row is None else {k: row[k] for k in ("cosine", "coverage", "entity_hit", "kept")}
        out["questions"].append({"id": q["id"], "kind": q["kind"], "query": q["query"], "gold": gold})
    for w in weak:
        rows = sorted(trace(w["query"]), key=lambda r: -(r["cosine"] if r["cosine"] is not None else -1))[:3]
        out["weak"].append({"id": w["id"], "query": w["query"],
                            "top3": [{"memory": key_of.get(r["id"], "?"), **{k: r[k] for k in ("cosine", "coverage", "entity_hit", "kept")}} for r in rows]})
    best = sorted((max((g["cosine"] for g in q["gold"].values() if g and g["cosine"] is not None), default=0.0)
                   for q in out["questions"] if q["kind"].startswith("paraphrase")), reverse=True)
    weak_max = max((r["cosine"] for w in out["weak"] for r in w["top3"] if r["cosine"] is not None), default=0.0)
    out["holdout"] = {"questions": [], "weak": []}
    for q in holdout["questions"]:
        rows = {row["id"]: row for row in trace(q["query"])}
        out["holdout"]["questions"].append({"id": q["id"], "query": q["query"], "gold": {
            key: (None if ids[key] not in rows else {k: rows[ids[key]][k] for k in ("cosine", "coverage", "entity_hit", "kept")})
            for key in q["gold"]}})
    for w in holdout["weak_queries"]:
        rows = sorted(trace(w["query"]), key=lambda r: -(r["cosine"] if r["cosine"] is not None else -1))[:3]
        out["holdout"]["weak"].append({"id": w["id"], "query": w["query"],
                                       "top3": [{"memory": key_of.get(r["id"], "?"), **{k: r[k] for k in ("cosine", "coverage", "entity_hit", "kept")}} for r in rows]})
    out["summary"] = {"paraphrase_best_gold_cosines_desc": best, "fourth_paraphrase_cosine": best[3] if len(best) > 3 else None,
                      "max_weak_cosine": weak_max}
    Path(path).write_text(json.dumps(out, indent=2) + "\n")


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
    for tool in ("memory_search",):
        t = rec[tool]
        rows.append((f"{tool} recall@5 / hit@5 / MRR@10",
                     f"{t['recall_at_5']:.3f} / {t['hit_at_5']:.3f} / {t['mrr_at_10']:.3f}  (n={t['questions']}, mean {t['bytes_per_call']['mean_bytes']} B/call)"))
        rows.append(("precision@5 (gold cards / cards)", f"{t['precision_at_5']:.3f}  ({t['gold_cards_returned']} of {t['cards_returned']})"))
        for kind, k in t["by_kind"].items():
            rows.append((f"  hit@5 {kind}", f"{k['hit5']:.3f}  (n={k['n']}, precision {k['precision_at_5']})"))
    for tool, z in r["zero_hit"].items():
        rows.append((f"{tool} zero-hit correct", f"{z['correct_zero_hit']}/{z['queries']}"))
    h, hz = r["recall_holdout"], r["zero_hit_holdout"]
    rows.append(("holdout hit@5 / MRR@10", f"{h['hit_at_5']:.3f} / {h['mrr_at_10']:.3f}  (n={h['questions']}, precision {h['precision_at_5']})"))
    rows.append(("corpus", f"{r['corpus']['memories']} fixture + {r['corpus']['distractors']} distractor memories, set {r['corpus']['question_set']}"))
    rows.append(("holdout zero-hit correct", f"{hz['correct_zero_hit']}/{hz['queries']}"))
    c = r["call_bytes"]
    rows.append(("bytes: search (mean, limit 5)", f"{c['memory_search']['mean_bytes']}"))
    rows.append(("bytes: search kind=handoff (limit 2)", f"{c['memory_search_kind_handoff']['bytes']}  ({c['memory_search_kind_handoff']['results']} results)"))
    rows.append(("bytes: search kind=decision (mean, limit 3)", f"{c['memory_search_kind_decision']['mean_bytes']}"))
    rows.append(("bytes: get (mean)", f"{c['memory_get']['mean_bytes']}"))
    width = max(len(k) for k, _ in rows)
    print(f"Cairn benchmark: {r['label']}  (tokens = bytes/4, estimate)")
    for key, value in rows:
        print(f"  {key.ljust(width)}  {value}")


if __name__ == "__main__":
    main()
