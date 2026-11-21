"""Deterministic distractor memories for scripts/bench.py (`--distractors N`).

A real store holds thousands of memories, most unrelated to any one question. These are generated
from fixed word lists and a fixed seed, so a run is reproducible and nothing is committed but this
file. Two families, half each: engineering notes about a different product ("Meridian", an
analytics platform) and everyday notes (office life, travel, hobbies, health). Neither answers a
question about the Harbor corpus, and neither answers a weak (off-topic) query in the question
files: topics such as sourdough, guitar, running and telescopes are left out on purpose. Both
families do share ordinary words with the corpus (deploy, queue, release, report, plan), which is
what makes a lexical or semantic gate work for its answer.
"""
import random

COMPONENTS = [
    "ingest pipeline", "feature store", "model registry", "notebook server", "dashboard renderer",
    "alert router", "cohort builder", "query planner", "scheduler", "exporter", "auth proxy",
    "tenant manager", "audit trail", "usage meter", "search indexer", "cache layer", "message bus",
    "artifact store", "telemetry agent", "config service", "schema registry", "stream processor",
    "report builder", "sampling service", "lineage tracker", "access broker", "event collector",
    "metric aggregator", "anomaly detector", "forecast runner", "label service", "batch loader",
    "secrets sidecar", "gateway", "backfill tool", "snapshot service", "retention sweeper",
    "workspace sync", "sandbox runner", "api docs site",
]
SYMPTOMS = [
    "stalls for several minutes", "returns stale numbers", "drops events silently",
    "doubles the memory footprint", "fails with a timeout", "reorders rows between runs",
    "loses the last batch", "uses far too much disk", "rejects valid tokens",
    "restarts in a loop", "reports negative durations", "leaks file handles",
]
CAUSES = [
    "a lock held across a network call", "an unbounded in-memory buffer", "a clock that drifts",
    "a retry storm after a short outage", "an index that was never created",
    "a config value that is read only at boot", "a default that changed in the last upgrade",
    "a cache key that ignores the tenant", "a sort that is not stable",
    "a job that overlaps with its previous run",
]
FIXES = [
    "bound the buffer and apply backpressure", "add the missing index and analyze the table",
    "read the value on every request", "pin the version and add a canary check",
    "include the tenant in the key", "make the sort deterministic with a tiebreaker",
    "serialize the runs with an advisory lock", "cap retries with jitter",
]
RULES = [
    "Every {c} change needs a rollback note in the pull request.",
    "The {c} must never write to a table owned by another team.",
    "Alerts from the {c} page only when users are affected; everything else is a ticket.",
    "Keep the {c} configuration in version control and review it like code.",
    "A new {c} setting ships disabled and is switched on one tenant at a time.",
    "Document each {c} metric with its unit, owner and expected range.",
    "The {c} runs with the smallest permissions that let it do its job.",
]
DECISIONS = [
    ("Run the {c} as {x}", "{x} fits the team's size and the {c}'s load.", ["a managed service", "a single binary", "a sidecar", "a scheduled job"]),
    ("Keep the {c} state in {x}", "{x} gives the {c} durable state without another system to operate.", ["object storage", "a relational table", "an embedded database", "a log"]),
    ("Version the {c} contract with {x}", "{x} lets consumers upgrade at their own pace.", ["semantic versions", "dated releases", "feature negotiation"]),
]
UNITS = ["seconds", "minutes", "megabytes", "gigabytes", "requests per second", "percent", "workers", "partitions"]
TEAMS = ["platform", "data", "growth", "security", "finance ops", "support tooling"]

LIFE_TOPICS = [
    ("Berlin office lunch", "Lunch orders close at {n}:30 and the caterer delivers to the {f} floor kitchen."),
    ("Bike storage", "The bike room on level {n} needs a badge; spots are first come first served."),
    ("Parking permits", "Visitor parking permits are issued by reception and valid for {n} days."),
    ("Language classes", "The {l} class meets on {d} evenings and the first {n} sessions are free."),
    ("Garden plan", "Plant {v} in the {d} bed and water twice a week until {m}."),
    ("Travel booking", "The train to {city} leaves at {n}:15 and a seat reservation costs a few euros."),
    ("Dentist appointment", "The check-up is on {d} at {n}:00; bring the insurance card."),
    ("Book club", "This month's pick is a {g} novel; we meet at the {p} café on {d}."),
    ("Insurance renewal", "The {g} policy renews in {m}; compare two quotes before accepting."),
    ("Moving checklist", "Forward the post, read the meters and photograph the {f} room before handing over keys."),
    ("Recipe: {v} soup", "Simmer the {v} for {n} minutes, blend half, season at the end."),
    ("Board game night", "We play on {d} evenings at the {p} café; the {g} edition needs {n} players."),
    ("Phone plan", "The {g} plan renews in {m} and includes {n} gigabytes a month."),
    ("Gym locker", "Locker {n} on the {f} floor takes a padlock; the {d} class starts at {t}:00."),
    ("Conference notes", "The {g} talk argued that {n} small experiments beat one big launch."),
]
FLOORS = ["second", "third", "fourth", "ground", "top"]
LANGS = ["Spanish", "Japanese", "Portuguese", "Dutch", "Italian"]
DAYS = ["Tuesday", "Wednesday", "Thursday", "Saturday", "Sunday"]
PLACES = ["lake", "old town", "canal", "city park", "harbour front", "river", "ridge"]
VEG = ["leeks", "beetroot", "kale", "carrots", "pumpkin", "fennel", "chard", "celery"]
MONTHS = ["March", "May", "July", "September", "November", "January"]
CITIES = ["Hamburg", "Vienna", "Prague", "Lyon", "Porto", "Rotterdam", "Ghent"]
GENRES = ["crime", "science fiction", "historical", "travel", "romance", "classical", "jazz"]


def make(n, seed=20261121):
    """`n` distinct `/remember` bodies (no scope or context) with `key`s `dis-0001`...; every other
    one is an engineering note, the rest everyday notes."""
    rng = random.Random(seed)
    out, seen = [], set()
    attempts = 0
    while len(out) < n:
        attempts += 1
        assert attempts < n * 50, "distractor word lists are too small for this many"
        if len(out) % 2 == 0:
            body = engineering(rng)
        else:
            body = everyday(rng)
        sig = (body["title"], body["summary"])
        if sig in seen:
            continue
        seen.add(sig)
        body["key"] = f"dis-{len(out) + 1:04d}"
        out.append(body)
    return out


def engineering(rng):
    c = rng.choice(COMPONENTS)
    team = rng.choice(TEAMS)
    form = rng.choice(["gotcha", "rule", "decision", "fact", "note"])
    if form == "gotcha":
        s, cause, fix = rng.choice(SYMPTOMS), rng.choice(CAUSES), rng.choice(FIXES)
        return {"kind": "gotcha", "title": f"The {c} {s}",
                "summary": f"The {c} {s} because of {cause}. Fix: {fix}.",
                "reason": f"Found while on call for the {team} team.",
                "fields": {"symptom": f"The {c} {s}.", "cause": cause[0].upper() + cause[1:] + ".", "fix": fix[0].upper() + fix[1:] + "."}}
    if form == "rule":
        text = rng.choice(RULES).format(c=c)
        title = text.split(".")[0][:80].rstrip()
        # A one-sentence rule would repeat its title as the summary, which the save quality check rejects.
        summary = text if text.rstrip(". ") != title else f"{text} The {team} team agreed to this."
        return {"kind": "rule", "title": title, "summary": summary,
                "reason": f"Agreed with the {team} team.", "fields": {}}
    if form == "decision":
        title, why, options = rng.choice(DECISIONS)
        x = rng.choice(options)
        return {"kind": "decision", "title": title.format(c=c, x=x), "summary": why.format(c=c, x=x)[0].upper() + why.format(c=c, x=x)[1:],
                "reason": f"Keeps the {team} team's operating load low.",
                "fields": {"context": f"The {c} needed a durable choice.", "alternatives": [o for o in options if o != x][:2], "consequences": "Revisit when volume grows tenfold."}}
    n, unit = rng.randint(2, 900), rng.choice(UNITS)
    if form == "note":
        return {"kind": "note", "title": f"Session: {c} tuning",
                "summary": f"Raised the {c} limit to {n} {unit} and watched it for a day.",
                "reason": f"Next: compare with the {team} dashboard and write down the result.", "fields": {}}
    return {"kind": "fact", "title": f"{c[0].upper() + c[1:]} limit {n} {unit}",
            "summary": f"The {c} is configured for {n} {unit}. The {team} team owns the setting.",
            "reason": "Reference for reviews.", "fields": {"source": f"{team} runbook"}}


def everyday(rng):
    title, text = rng.choice(LIFE_TOPICS)
    slots = dict(n=rng.randint(2, 40), f=rng.choice(FLOORS), l=rng.choice(LANGS), d=rng.choice(DAYS),
                 p=rng.choice(PLACES), v=rng.choice(VEG), m=rng.choice(MONTHS), city=rng.choice(CITIES),
                 g=rng.choice(GENRES), t=rng.randint(16, 24))
    return {"kind": "fact", "title": f"{title.format(**slots)} {rng.randint(100, 999)}", "summary": text.format(**slots),
            "reason": "Personal note.", "fields": {}}
