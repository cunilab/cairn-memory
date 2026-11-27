"""Coding-memory suite for scripts/bench.py --coding.

Loads bench/fixtures/coding-memory.json into the disposable database the main harness already
started, then scores explicit memory_search and automatic hook recall. A plain Postgres FTS
baseline and an in-process grep baseline score the same explicit queries. Results use fixture
keys, not server ids.

Hit@5, MRR@5 and strict recall-all@5 ignore weak queries and unauthorized cases (those have no
gold). A forbid_win id that ranks above gold, or a never id in the delivered list, scores 0.
Weak-query empty rate is the share of weak queries that delivered nothing. Bytes are the text
the agent actually received. Hook p95 is the hook process. FTS p95 includes docker exec psql and
is not a server latency.
"""
import json
import math
import re
import subprocess
import time
from pathlib import Path

import bench
import bench_distractors

ROOT = bench.ROOT
FIXTURE = bench.FIXTURES / "coding-memory.json"
DISTRACTORS = 15
KEY = re.compile(r"^[a-z0-9-]+$")
IDENT = re.compile(r"^[A-Za-z0-9_-]+$")


def self_check():
    """Fixture words, plus the scorer, before any server work."""
    spec = json.loads(FIXTURE.read_text())
    by_key = {m["key"]: m for m in spec["memories"]}
    assert len(by_key) == len(spec["memories"])
    for memory in spec["memories"]:
        assert KEY.match(memory["key"]), memory["key"]
        assert len(memory["title"]) <= 40, memory["key"]
    for question in spec["questions"]:
        for key in question.get("gold", []) + question.get("forbid_win", []) + question.get("never", []) + question.get("sibling", []):
            assert key in by_key, f"{question['id']}: unknown {key}"
        if question["kind"] == "paraphrase":
            for key in question["gold"]:
                shared = bench.words(question["query"]) & bench.words(_text(by_key[key]))
                assert not shared, f"{question['id']} shares words with {key}: {sorted(shared)}"
        if question["kind"] == "unauthorized":
            assert not question.get("gold")
            assert question.get("never")
    ranked = ["new", "old", "other"]
    hit = score_case(ranked, ["new"], ["old"], [])
    assert hit["hit"] == 1.0 and hit["mrr"] == 1.0 and hit["recall_all"] == 1.0, hit
    lost = score_case(["old", "new"], ["new"], ["old"], [])
    assert lost["hit"] == 0.0 and lost["old_wins"], lost
    leaked = score_case(["secret"], ["new"], [], ["secret"])
    assert leaked["hit"] == 0.0 and leaked["leaked"] == ["secret"], leaked
    partial = score_case(["a"], ["a", "b"], [], [])
    assert partial["hit"] == 1.0 and partial["recall_all"] == 0.0, partial
    safety = score_case(["other"], [], [], ["secret"])
    assert safety["safety"] and not safety["leaked"], safety
    text = "Cairn notes for a.rs:\n- Amber claims rows in batches: ten\n- Birch claims rows one by one: one"
    assert titles_in_order(text, ["Birch claims rows one by one", "Amber claims rows in batches"]) == [
        "Amber claims rows in batches",
        "Birch claims rows one by one",
    ]


def _text(memory):
    return " ".join(str(memory.get(name) or "") for name in ("title", "summary", "reason", "fields"))


def score_case(ranked, gold, forbid_win, never, limit=5):
    """One delivered list. Safety-only cases (no gold) stay out of the hit averages."""
    top = list(ranked)[:limit]
    gold, forbid_win, never = set(gold), set(forbid_win), set(never)
    leaked = [item for item in top if item in never]
    gold_at = [index + 1 for index, item in enumerate(top) if item in gold]
    forbid_at = [index + 1 for index, item in enumerate(top) if item in forbid_win]
    best_gold = gold_at[0] if gold_at else None
    best_forbid = forbid_at[0] if forbid_at else None
    old_wins = best_forbid is not None and (best_gold is None or best_forbid < best_gold)
    ok = not leaked and not old_wins
    if not gold:
        return {"safety": True, "leaked": leaked, "old_wins": old_wins, "hit": None,
                "mrr": None, "recall_all": None, "best_gold": None}
    return {
        "safety": False,
        "leaked": leaked,
        "old_wins": old_wins,
        "hit": 1.0 if ok and best_gold else 0.0,
        "mrr": (1.0 / best_gold) if ok and best_gold else 0.0,
        "recall_all": 1.0 if ok and gold and all(item in top for item in gold) else 0.0,
        "best_gold": best_gold,
    }


def titles_in_order(text, titles):
    found = []
    for title in titles:
        at = text.find(title)
        if at >= 0:
            found.append((at, title))
    found.sort()
    return [title for _, title in found]


def percentile(values, fraction):
    if not values:
        return None
    ordered = sorted(values)
    return round(ordered[math.ceil(fraction * len(ordered)) - 1], 1)


def aggregate(rows):
    scored = [row for row in rows if row.get("hit") is not None]
    weak = [row for row in rows if row.get("weak")]
    empty = sum(1 for row in weak if row["hits"] == 0)
    n = len(scored)

    def avg(name):
        return round(sum(row[name] for row in scored) / n, 4) if n else None

    by_kind = {}
    for row in scored:
        by_kind.setdefault(row["kind"], []).append(row)
    return {
        "questions": n,
        "hit_at_5": avg("hit"),
        "mrr_at_5": avg("mrr"),
        "recall_all_at_5": avg("recall_all"),
        "weak_queries": len(weak),
        "weak_empty": empty,
        "weak_empty_rate": round(empty / len(weak), 4) if weak else None,
        "bytes_delivered": sum(row["bytes"] for row in rows),
        "p95_ms": percentile([row["ms"] for row in rows if row.get("ms") is not None], 0.95),
        "unauthorized_shown": sum(1 for row in rows if row.get("leaked")),
        "old_wins": sum(1 for row in rows if row.get("old_wins")),
        "by_kind": {kind: {"n": len(items), "hit_at_5": round(sum(item["hit"] for item in items) / len(items), 4)}
                    for kind, items in sorted(by_kind.items())},
    }


def _sql_ident(value, label):
    if not IDENT.match(value or ""):
        raise RuntimeError(f"unexpected {label}")
    return value


def run(http, session, device, repo, env, git_env, exe, config, sql, embedded, semantic, label, out):
    self_check()
    spec = json.loads(FIXTURE.read_text())
    by_key = {m["key"]: m for m in spec["memories"]}
    titles = {m["key"]: m["title"] for m in spec["memories"]}
    harbor = json.loads((repo / ".cairn" / "project.json").read_text())["project_id"]
    harbor = _sql_ident(harbor, "project id")
    person = sql("SELECT person_id FROM cairn.accounts WHERE email = 'bench-owner@example.test'")
    person = _sql_ident(person, "person id")

    wharf = http.call("/projects/init", {"repository": "example.test/bench/wharf", "name": "Wharf"}, device)["project_id"]
    http.call("/auth/register", {"email": "bench-outsider@example.test", "password": bench.PASSWORD}, session)
    outsider = http.call("/auth/login", {"email": "bench-outsider@example.test", "password": bench.PASSWORD})["token"]
    outsider_device = http.call("/devices/register", {"name": "bench-outsider"}, outsider)["token"]
    quarry = http.call("/projects/init", {"repository": "example.test/bench/quarry", "name": "Quarry"}, outsider_device)["project_id"]
    places = {"harbor": (device, harbor), "wharf": (device, wharf), "outsider": (outsider_device, quarry)}

    for memory in spec["memories"]:
        for name in memory.get("files") or []:
            path = repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            if not path.exists():
                path.write_text("\n")
    (repo / "src").mkdir(parents=True, exist_ok=True)
    (repo / "src" / "unrelated.rs").write_text("fn unused() {}\n")
    for name in ("feature/amber", "feature/birch"):
        if subprocess.run(["git", "show-ref", "--verify", "--quiet", f"refs/heads/{name}"], cwd=repo, env=git_env).returncode != 0:
            bench.run(["git", "branch", name], cwd=repo, env=git_env)

    ids = {}
    for memory in spec["memories"]:
        if memory.get("supersedes"):
            continue
        ids[memory["key"]] = _save(http, memory, places, ids)
    for memory in spec["memories"]:
        if memory.get("supersedes"):
            ids[memory["key"]] = _save(http, memory, places, ids)
    assert len(set(ids.values())) == len(spec["memories"]), "a coding memory was merged into another"
    distractors = bench_distractors.make(DISTRACTORS)
    for memory in distractors:
        body = {k: v for k, v in memory.items() if k != "key"}
        body.update(scope="project", context={"project_id": harbor, "repository": "example.test/bench/harbor"})
        ids[memory["key"]] = http.call("/remember", body, device)["id"]

    for memory in spec["memories"]:
        when = memory.get("recorded_at")
        if not when:
            continue
        memory_id = _sql_ident(ids[memory["key"]], "memory id")
        updated = sql(f"UPDATE cairn.memories SET created_at = '{when}' WHERE memory_id = '{memory_id}'")
        if updated != "UPDATE 1":
            raise RuntimeError(f"could not backdate {memory['key']}: {updated}")
    _settle(sql, embedded, len(ids))
    statuses = {}
    for memory in spec["memories"]:
        memory_id = _sql_ident(ids[memory["key"]], "memory id")
        statuses[memory["key"]] = sql(f"SELECT status FROM cairn.memories WHERE memory_id = '{memory_id}'")
    expected = {key: ("superseded" if key == "old-lock" else "active") for key in by_key}
    wrong = {key: statuses[key] for key in expected if statuses[key] != expected[key]}
    if wrong:
        raise RuntimeError(f"coding memories changed status: {wrong}")

    key_of = {value: key for key, value in ids.items()}
    visible = _visible_rows(sql, person, harbor)
    mcp = bench.Mcp(exe, repo, env)
    mcp.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "bench", "version": "0"}})
    try:
        explicit = [_explicit(mcp, repo, git_env, question, ids, key_of, False) for question in spec["questions"]]
        explicit += [_explicit(mcp, repo, git_env, question, ids, key_of, True) for question in spec["weak_queries"]]
        hooks = [_hook_case(exe, config, repo, env, git_env, question, ids, titles, False) for question in spec["questions"]]
        hooks += [_hook_case(exe, config, repo, env, git_env, question, ids, titles, True) for question in spec["weak_queries"]]
        fts = [_fts_case(sql, person, harbor, question, ids, key_of, visible, False) for question in spec["questions"]]
        fts += [_fts_case(sql, person, harbor, question, ids, key_of, visible, True) for question in spec["weak_queries"]]
        grep_rows = [_grep_case(question, visible, ids, key_of, False) for question in spec["questions"]]
        grep_rows += [_grep_case(question, visible, ids, key_of, True) for question in spec["weak_queries"]]
    finally:
        mcp.close()
        subprocess.run(["git", "checkout", "-q", "main"], cwd=repo, env=git_env)

    revision = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True, capture_output=True, check=True).stdout.strip()
    result = {
        "label": label,
        "suite": "coding-memory",
        "revision": revision,
        "definitions": {
            "hit_at_5": "gold in the delivered top 5, and no never-id, and no forbid_win id above gold",
            "mrr_at_5": "1/rank of the first gold inside that acceptable top 5, else 0",
            "recall_all_at_5": "every gold id is in that acceptable top 5",
            "weak_empty_rate": "share of weak queries that delivered no card",
            "bytes_delivered": "explicit and hook: text the agent received; FTS and grep: title and summary bytes of the top 5",
            "p95_ms": "explicit: MCP call; hook: cairn hook process; FTS: docker exec psql, not server latency; grep: in-process",
            "unauthorized_shown": "delivered lists that contained a never-id",
            "old_wins": "delivered lists where a forbid_win id ranked above gold",
            "sibling_outranks": "the other branch's memory ranked above the checkout branch; not part of hit@5",
        },
        "corpus": {"coding_memories": len(spec["memories"]), "distractors": len(distractors),
                   "questions": len(spec["questions"]), "weak_queries": len(spec["weak_queries"])},
        "semantic_channel": {"server_health": semantic, "embedding_model": "pinned all-MiniLM-L6-v2" if embedded else None},
        "explicit_search": aggregate(explicit),
        "hook_recall": aggregate(hooks),
        "baseline_fts": aggregate(fts),
        "baseline_grep": aggregate(grep_rows),
        "explicit_questions": explicit,
        "hook_questions": hooks,
        "fts_questions": fts,
        "grep_questions": grep_rows,
        "memory_status": statuses,
    }
    path = Path(out)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    _print(result)
    print(f"\nwrote {path}")


def _save(http, memory, places, ids):
    token, project = places[memory["place"]]
    context = {"project_id": project, "repository": "example.test/bench/" + ("quarry" if memory["place"] == "outsider" else memory["place"])}
    if memory.get("branch"):
        context["branch"] = memory["branch"]
    for name, key in (("files", "files"), ("symbols", "symbols"), ("commands", "commands")):
        if memory.get(key):
            context[name] = memory[key]
    body = {"kind": memory["kind"], "title": memory["title"], "summary": memory["summary"],
            "reason": memory["reason"], "scope": "project", "context": context}
    if memory.get("fields"):
        body["fields"] = memory["fields"]
    if memory.get("supersedes"):
        body["supersedes"] = ids[memory["supersedes"]]
    reply = http.call("/remember", body, token)
    if reply.get("deduplicated"):
        raise RuntimeError(f"{memory['key']} was merged on save")
    return reply["id"]


def _settle(sql, embedded, count):
    missing = "(SELECT count(*) FROM cairn.memories WHERE embedding IS NULL)" if embedded else "0"
    busy = "(SELECT count(*) FROM cairn.jobs WHERE kind IN ('embed','consolidate','link') AND status IN ('pending','running'))"
    for _ in range(240 + 4 * count):
        if int(sql(f"SELECT {missing} + {busy}") or 0) == 0:
            return
        time.sleep(0.5)
    raise RuntimeError("coding embeddings and consolidation did not settle")


def _visible_rows(sql, person, harbor):
    raw = sql(
        "SELECT coalesce(json_agg(json_build_object("
        "'id', m.memory_id, 'title', m.title, 'summary', m.summary, 'reason', m.reason, "
        "'fields', m.fields, 'detail', m.detail)), '[]'::json) "
        "FROM cairn.memories m "
        f"WHERE m.status IN ('active','stale','superseded') "
        f"AND (m.owner_id = '{person}' OR m.scope = 'global' "
        f"OR (m.scope = 'project' AND m.project_id IN (SELECT project_id FROM cairn.project_members WHERE person_id = '{person}'))) "
        f"AND (m.scope <> 'project' OR m.project_id = '{harbor}')"
    )
    rows = json.loads(raw or "[]")
    for row in rows:
        row["text"] = " ".join(str(row.get(name) or "") for name in ("title", "summary", "reason", "fields", "detail"))
    return rows


def _checkout(repo, git_env, branch):
    subprocess.run(["git", "checkout", "-q", branch or "main"], cwd=repo, env=git_env, check=True)


def _explicit(mcp, repo, git_env, question, ids, key_of, weak):
    _checkout(repo, git_env, question.get("branch"))
    started = time.perf_counter()
    selectors = {
        key: question[key]
        for key in ("recorded_from", "recorded_to", "branch", "version")
        if question.get(key)
    }
    shaped, _wire, text_bytes = mcp.tool("memory_search", query=question["query"], limit=5, **selectors)
    elapsed = (time.perf_counter() - started) * 1000
    ranked = [key_of.get(memory_id, "?") for memory_id in bench.result_ids(shaped)]
    return _row(question, ranked, text_bytes, elapsed, weak)


def _hook_case(exe, config, repo, env, git_env, question, ids, titles, weak):
    hook = question.get("hook") or {"event": "UserPromptSubmit", "prompt": question["query"]}
    _checkout(repo, git_env, question.get("branch"))
    payload = {"hook_event_name": hook["event"], "session_id": f"coding-{question['id']}", "cwd": str(repo)}
    payload.update({key: value for key, value in hook.items() if key != "event"})
    started = time.perf_counter()
    done = subprocess.run([exe, "hook", hook["event"], "--agent", "claude", "--config", str(config)],
                          input=json.dumps(payload), text=True, capture_output=True, env=env, cwd=repo, timeout=30)
    elapsed = (time.perf_counter() - started) * 1000
    if done.returncode != 0:
        raise RuntimeError(f"hook {question['id']} failed: {done.stderr}")
    injected = ""
    if done.stdout.strip():
        try:
            injected = json.loads(done.stdout)["hookSpecificOutput"]["additionalContext"]
        except (ValueError, KeyError, TypeError):
            injected = ""
    order = titles_in_order(injected, list(titles.values()))
    title_of = {title: key for key, title in titles.items()}
    ranked = [title_of[title] for title in order if title in title_of]
    row = _row(question, ranked, bench.blen(injected), elapsed, weak)
    row["injected_titles"] = order
    return row


def _baseline_case(question, ranked_ids, ids, key_of, visible, weak, elapsed):
    ranked = [key_of.get(memory_id, "?") for memory_id in ranked_ids]
    text = _delivered_text(ranked, visible, ids)
    return _row(question, ranked, bench.blen(text), elapsed, weak)


def _fts_case(sql, person, harbor, question, ids, key_of, visible, weak):
    started = time.perf_counter()
    found = _fts(sql, person, harbor, question["query"])
    return _baseline_case(question, found, ids, key_of, visible, weak, (time.perf_counter() - started) * 1000)


def _grep_case(question, visible, ids, key_of, weak):
    started = time.perf_counter()
    found = _grep(question["query"], visible)
    return _baseline_case(question, found, ids, key_of, visible, weak, (time.perf_counter() - started) * 1000)


def _delivered_text(ranked, visible, ids):
    by_id = {row["id"]: row for row in visible}
    lines = []
    for key in ranked:
        row = by_id.get(ids.get(key, ""))
        if row:
            lines.append(f"{row['title']}: {row['summary']}")
    return "\n".join(lines)


def _row(question, ranked, nbytes, elapsed, weak):
    judged = score_case(ranked, question.get("gold", []), question.get("forbid_win", []), question.get("never", []))
    sibling = question.get("sibling") or []
    gold_rank = judged["best_gold"]
    sibling_rank = next((index + 1 for index, key in enumerate(ranked[:5]) if key in sibling), None)
    row = {
        "id": question["id"],
        "kind": question.get("kind", "weak"),
        "weak": weak,
        "gold": question.get("gold", []),
        "top5": ranked[:5],
        "bytes": nbytes,
        "ms": round(elapsed, 1) if elapsed is not None else None,
        "hits": len(ranked),
        "hit": judged["hit"],
        "mrr": judged["mrr"],
        "recall_all": judged["recall_all"],
        "leaked": judged["leaked"],
        "old_wins": judged["old_wins"],
    }
    if sibling:
        row["sibling_outranks"] = sibling_rank is not None and (gold_rank is None or sibling_rank < gold_rank)
    return row


def _fts(sql, person, harbor, query):
    if "$q$" in query:
        raise RuntimeError("query cannot contain the dollar quote")
    raw = sql(
        "SELECT coalesce(json_agg(memory_id ORDER BY ord), '[]'::json) FROM ("
        "SELECT m.memory_id, row_number() OVER (ORDER BY ts_rank_cd((m.search_tsv || to_tsvector('english', m.detail)), "
        f"websearch_to_tsquery('english', $q${query}$q$), 32) DESC, m.created_at DESC, m.memory_id) AS ord "
        "FROM cairn.memories m "
        f"WHERE m.status IN ('active','stale','superseded') "
        f"AND (m.owner_id = '{person}' OR m.scope = 'global' "
        f"OR (m.scope = 'project' AND m.project_id IN (SELECT project_id FROM cairn.project_members WHERE person_id = '{person}'))) "
        f"AND (m.scope <> 'project' OR m.project_id = '{harbor}') "
        f"AND (m.search_tsv || to_tsvector('english', m.detail)) @@ websearch_to_tsquery('english', $q${query}$q$) "
        "ORDER BY ord LIMIT 5) s"
    )
    return json.loads(raw or "[]")


def _grep(query, rows):
    terms = set(re.findall(r"[a-z0-9_./:-]{3,}", query.lower()))
    terms.update(re.findall(r"[a-z0-9_]{3,}", query.lower()))
    ranked = []
    for row in rows:
        hay = row["text"].lower()
        hits = sum(1 for term in terms if term in hay)
        if hits:
            ranked.append((-hits, row["id"]))
    ranked.sort()
    return [memory_id for _hits, memory_id in ranked[:5]]


def _print(result):
    print(f"coding-memory {result['label']}  revision {result['revision'][:12]}  semantic {result['semantic_channel']['server_health']}")
    print(f"{'path':<16} {'hit@5':>8} {'MRR@5':>8} {'all@5':>8} {'weak empty':>12} {'bytes':>8} {'p95 ms':>8} {'leaks':>6} {'old wins':>9}")
    for name in ("explicit_search", "hook_recall", "baseline_fts", "baseline_grep"):
        row = result[name]
        print(f"{name:<16} {row['hit_at_5']!s:>8} {row['mrr_at_5']!s:>8} {row['recall_all_at_5']!s:>8} "
              f"{row['weak_empty']}/{row['weak_queries']!s:<8} {row['bytes_delivered']!s:>8} {row['p95_ms']!s:>8} "
              f"{row['unauthorized_shown']!s:>6} {row['old_wins']!s:>9}")


if __name__ == "__main__":
    self_check()
    print("coding fixture ok")
