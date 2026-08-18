#!/usr/bin/env python3
"""Index session logs into Elasticsearch, frame by frame.

`summary.csv` reduces a session to one row. That is the right shape for
comparing five profiles and the wrong shape for the question that actually
comes up: *what happened around frame 2399?* -- when the rollbacks clustered,
whether a stall followed a burst of loss, whether the depth crept up before the
prediction window filled.

The JSONL already holds all of it. This puts it somewhere queryable.

    ./ops/scripts/elastic-load.py                      # artifacts/logs
    ./ops/scripts/elastic-load.py --logs artifacts/e2e/logs --all

Two indices, because the two questions want different shapes:

  rollback-metrics   one document per second per peer, every counter flattened.
                     This is what you chart.
  rollback-events    one document per session event -- advanced, rolled back,
                     stalled, checksum matched, desync. This is what you scrub
                     through when a chart shows something odd.

Datagram records (`sent`, `received`, `local_input`, `remote_inputs`) are the
bulk of a log -- tens of thousands of lines each -- and are skipped unless you
pass `--all`. They belong in rollback-events too when you want them.

Every document carries the session's identity (simulation, profile, player,
seed, commit) denormalised onto it, so nothing needs a join to be filtered.
"""

from __future__ import annotations

import argparse
import json
import sys
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

METRICS_INDEX = "rollback-metrics"
EVENTS_INDEX = "rollback-events"

BULKY = {"sent", "received", "local_input", "remote_inputs"}

# Explicit mappings, because letting Elasticsearch guess gets `depth` right and
# `loss_pct` wrong the first time a session has none.
TEMPLATE = {
    "index_patterns": [f"{METRICS_INDEX}*", f"{EVENTS_INDEX}*"],
    "template": {
        "settings": {"number_of_shards": 1, "number_of_replicas": 0},
        "mappings": {
            "dynamic_templates": [
                {
                    "counters": {
                        "match_mapping_type": "long",
                        "mapping": {"type": "long"},
                    }
                },
                {
                    "rates": {
                        "match_mapping_type": "double",
                        "mapping": {"type": "double"},
                    }
                },
                {
                    "keywords": {
                        "match_mapping_type": "string",
                        "mapping": {"type": "keyword"},
                    }
                },
            ],
            "properties": {
                "@timestamp": {"type": "date"},
                "session": {"type": "keyword"},
                "simulation": {"type": "keyword"},
                "profile": {"type": "keyword"},
                "player": {"type": "keyword"},
                "record": {"type": "keyword"},
                "event": {"type": "keyword"},
                "frame": {"type": "long"},
                "elapsed_ms": {"type": "long"},
                # Checksums are 64-bit *unsigned*, which does not fit
                # Elasticsearch's signed long, and they are identifiers rather
                # than quantities -- nobody averages a checksum. Stored as hex
                # keywords, which is also how they read in an error message.
                "checksum": {"type": "keyword"},
                "local_checksum": {"type": "keyword"},
                "remote_checksum": {"type": "keyword"},
            },
        },
    },
}


def request(method: str, url: str, body: bytes | None = None, ctype: str = "application/json"):
    req = urllib.request.Request(url, data=body, method=method)
    req.add_header("Content-Type", ctype)
    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            return resp.status, json.loads(resp.read() or b"{}")
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"{}")
    except urllib.error.URLError as e:
        raise SystemExit(
            f"cannot reach Elasticsearch at {url}: {e.reason}\n"
            f"Start it with `just elastic-up`."
        ) from e


I64_MAX = 2**63 - 1


def representable(value):
    """Render integers Elasticsearch cannot hold as hex strings.

    FNV-1a checksums are u64 and routinely exceed signed-long range. Truncating
    or wrapping them would be worse than useless -- two different states could
    collide -- so they become keywords.
    """
    if isinstance(value, bool):
        return value
    if isinstance(value, int) and not (-I64_MAX - 1 <= value <= I64_MAX):
        return f"{value:016x}"
    return value


def flatten(prefix: str, value, out: dict) -> None:
    """Flatten nested metric groups into dotted keys.

    `local.rollbacks` reads better in Kibana than a nested object, and the
    groups here (local / remote / link / process) are flat structs anyway.
    """
    if isinstance(value, dict):
        for k, v in value.items():
            flatten(f"{prefix}.{k}" if prefix else k, v, out)
    elif value is not None:
        out[prefix] = representable(value)


def documents(path: Path, include_all: bool):
    """Yield (index, doc) for one session log."""
    session = path.stem
    started_ms: int | None = None
    info: dict = {}

    for line in path.open():
        line = line.strip()
        if not line:
            continue
        try:
            rec = json.loads(line)
        except json.JSONDecodeError:
            # A session killed mid-write leaves exactly one short final line.
            continue

        kind = rec.get("record")
        if kind == "session_start":
            started_ms = rec.get("started_unix_ms")
            info = rec.get("info", {}) or {}

        if started_ms is None:
            # Nothing before session_start can be placed on a timeline.
            continue
        if kind in BULKY and not include_all:
            continue

        base = {
            "@timestamp": started_ms + rec.get("t_ms", 0),
            "session": session,
            "simulation": info.get("simulation"),
            "profile": info.get("profile"),
            "player": info.get("player"),
            "seed": info.get("seed"),
            "app_commit": info.get("app_commit"),
            "input_delay": info.get("input_delay"),
            "prediction_limit": info.get("prediction_limit"),
            "record": kind,
        }

        if kind == "metrics":
            doc = dict(base)
            for group in ("local", "remote", "link", "process"):
                flatten(group, rec.get(group), doc)
            for k in ("frame", "confirmed_frame", "prediction_depth", "elapsed_ms", "desync"):
                if rec.get(k) is not None:
                    doc[k] = rec[k]
            # Derived here rather than in Kibana, so every consumer of this
            # index agrees with summary.csv without re-deriving anything.
            presented = doc.get("local.frames_presented", 0)
            resimulated = doc.get("local.frames_resimulated", 0)
            elapsed = doc.get("elapsed_ms", 0)
            if elapsed:
                doc["derived.effective_fps"] = round(presented / (elapsed / 1000.0), 3)
            if presented:
                doc["derived.resimulation_overhead"] = round(resimulated / presented, 5)
            highest = doc.get("link.highest_sequence")
            unique = doc.get("link.unique_received")
            if highest is not None and unique is not None and highest >= 0:
                expected = highest + 1
                doc["derived.loss_pct"] = round(
                    max(0.0, (expected - unique) / expected * 100.0), 4
                )
            for micros, ms in (("link.srtt_micros", "derived.srtt_ms"),
                               ("link.rttvar_micros", "derived.rttvar_ms")):
                if doc.get(micros) is not None:
                    doc[ms] = round(doc[micros] / 1000.0, 3)
            yield METRICS_INDEX, doc
        else:
            doc = dict(base)
            for k, v in rec.items():
                if k in ("record", "t_ms", "info"):
                    continue
                # A desync event calls the two checksums `local` and `remote`,
                # which in the metrics index name whole metric groups. Rename
                # them here so a single Kibana field list is not ambiguous.
                if rec.get("event") == "desync" and k in ("local", "remote"):
                    k = f"{k}_checksum"
                flatten(k, v, doc)
            yield EVENTS_INDEX, doc


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--logs", default=str(ROOT / "artifacts" / "logs"))
    ap.add_argument("--url", default="http://127.0.0.1:9200")
    ap.add_argument("--kibana", default="http://127.0.0.1:5601")
    ap.add_argument("--all", action="store_true", help="also index datagram records")
    ap.add_argument("--reset", action="store_true", help="delete the indices first")
    args = ap.parse_args()

    logs = sorted(Path(args.logs).glob("*.jsonl"))
    if not logs:
        print(f"no .jsonl under {args.logs}", file=sys.stderr)
        return 1

    status, body = request("GET", f"{args.url}/_cluster/health")
    if status != 200:
        print(f"Elasticsearch is not healthy: {body}", file=sys.stderr)
        return 1
    print(f"    cluster {body.get('cluster_name')} is {body.get('status')}")

    if args.reset:
        for index in (METRICS_INDEX, EVENTS_INDEX):
            request("DELETE", f"{args.url}/{index}")
        print("    indices dropped")

    request(
        "PUT",
        f"{args.url}/_index_template/rollback",
        json.dumps(TEMPLATE).encode(),
    )

    total = 0
    for path in logs:
        batch: list[bytes] = []
        count = 0

        def flush() -> int:
            if not batch:
                return 0
            status, body = request(
                "POST",
                f"{args.url}/_bulk",
                b"".join(batch),
                ctype="application/x-ndjson",
            )
            if status >= 300 or body.get("errors"):
                first = next(
                    (
                        item
                        for item in body.get("items", [])
                        if item.get("index", {}).get("error")
                    ),
                    None,
                )
                print(f"bulk index failed: {first or body}", file=sys.stderr)
                raise SystemExit(1)
            n = len(body.get("items", []))
            batch.clear()
            return n

        for index, doc in documents(path, args.all):
            batch.append(json.dumps({"index": {"_index": index}}).encode() + b"\n")
            batch.append(json.dumps(doc).encode() + b"\n")
            # 5 000 documents is a few MB of NDJSON: big enough to be fast,
            # small enough that a failure names a manageable range.
            if len(batch) >= 10_000:
                count += flush()
        count += flush()

        print(f"    {path.name}  {count} documents")
        total += count

    request("POST", f"{args.url}/{METRICS_INDEX}/_refresh")
    request("POST", f"{args.url}/{EVENTS_INDEX}/_refresh")
    print(f"\n    {total} documents from {len(logs)} session(s)")

    create_data_views(args.kibana)
    print(f"    Kibana: {args.kibana}")
    return 0


def create_data_views(kibana: str) -> None:
    """Give Kibana the two data views, so it opens ready to query.

    Without these the first thing Kibana asks for is a data view, which is a
    dull five-click detour every time the stack is recreated. Creating one that
    already exists returns 409, which is success as far as this is concerned.
    """
    for index, time_field in ((METRICS_INDEX, "@timestamp"), (EVENTS_INDEX, "@timestamp")):
        body = json.dumps(
            {"data_view": {"title": index, "name": index, "timeFieldName": time_field}}
        ).encode()
        req = urllib.request.Request(
            f"{kibana}/api/data_views/data_view", data=body, method="POST"
        )
        req.add_header("Content-Type", "application/json")
        req.add_header("kbn-xsrf", "true")
        try:
            with urllib.request.urlopen(req, timeout=30):
                print(f"    data view '{index}' created")
        except urllib.error.HTTPError as e:
            # Kibana answers 400 (not 409) for a duplicate data view name, so
            # the status code alone cannot distinguish "already there" from a
            # real problem. Read the body and say which it was.
            body = (e.read() or b"").decode(errors="replace")
            if e.code in (400, 409) and "uplicate" in body:
                print(f"    data view '{index}' already there")
            else:
                print(f"    could not create data view '{index}': HTTP {e.code} {body[:160]}")
        except urllib.error.URLError:
            print(f"    Kibana not reachable at {kibana}; create the data views by hand")


if __name__ == "__main__":
    sys.exit(main())
