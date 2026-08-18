#!/usr/bin/env python3
"""The analyses that summary.csv cannot do, run from the command line.

Kibana is where you go to look around. This is for the questions worth asking
every time, phrased once and answered the same way -- so a result can be quoted
in the documentation and reproduced by whoever reads it.

Each one exists because a single-row summary hides it:

  distribution   summary.csv says "mean depth 2.04". It does not say that 94%
                 of rollbacks were depth 2 and exactly four reached depth 4 --
                 which is what tells you the prediction limit of 8 is nowhere
                 near being tested.
  latency        "srtt 49.9 ms" is a smoothed average. The p99 and the maximum
                 are what a player would actually feel.
  clustering     Rollbacks per second over the session: steady, or in bursts?
                 A mean cannot tell you, and the answer changes what you would
                 tune.
  work           Where the frame budget goes: advance, save_state, load_state.
                 On a 415 KB state this is the whole cost argument.

    ./ops/scripts/elastic-analyze.py
    ./ops/scripts/elastic-analyze.py --session 1787044688-lastblade2-natural-p1-play
"""

from __future__ import annotations

import argparse
import json
import sys
import urllib.error
import urllib.request

METRICS = "rollback-metrics"
EVENTS = "rollback-events"


def search(url: str, index: str, body: dict) -> dict:
    req = urllib.request.Request(
        f"{url}/{index}/_search?size=0", data=json.dumps(body).encode(), method="POST"
    )
    req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            return json.loads(resp.read())
    except urllib.error.URLError as e:
        raise SystemExit(
            f"cannot reach Elasticsearch at {url}: {e}\nStart it with `just elastic-up`."
        ) from e


def filters(session: str | None, extra: list[dict] | None = None) -> dict:
    clauses = list(extra or [])
    if session:
        clauses.append({"term": {"session": session}})
    return {"bool": {"filter": clauses}} if clauses else {"match_all": {}}


def rule(title: str) -> None:
    print(f"\n{title}\n{'-' * len(title)}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--url", default="http://127.0.0.1:9200")
    ap.add_argument("--session", help="restrict to one session (default: all)")
    args = ap.parse_args()
    url, session = args.url, args.session

    # --- what is even in here --------------------------------------------
    rule("sessions indexed")
    body = {
        "query": filters(session),
        "aggs": {
            "s": {
                "terms": {"field": "session", "size": 50, "order": {"_key": "asc"}},
                "aggs": {"docs": {"value_count": {"field": "record"}}},
            }
        },
    }
    for b in search(url, METRICS, body)["aggregations"]["s"]["buckets"]:
        print(f"  {b['key']:<48} {b['doc_count']:>5} metric samples")

    # --- rollback depth distribution -------------------------------------
    rule("rollback depth: how deep did corrections actually go")
    body = {
        "query": filters(session, [{"term": {"event": "rolled_back"}}]),
        "aggs": {
            "by_session": {
                "terms": {"field": "session", "size": 50, "order": {"_key": "asc"}},
                "aggs": {"depth": {"terms": {"field": "depth", "size": 20, "order": {"_key": "asc"}}}},
            }
        },
    }
    for s in search(url, EVENTS, body)["aggregations"]["by_session"]["buckets"]:
        total = s["doc_count"]
        print(f"  {s['key']}  ({total} rollbacks)")
        for d in s["depth"]["buckets"]:
            share = d["doc_count"] / total * 100 if total else 0
            bar = "#" * max(1, round(share / 3))
            print(f"      depth {d['key']:>2}  {d['doc_count']:>6}  {share:5.1f}%  {bar}")

    # --- latency, beyond the average -------------------------------------
    rule("round-trip time: the average hides the tail")
    body = {
        "query": filters(session),
        "aggs": {
            "by_session": {
                "terms": {"field": "session", "size": 50, "order": {"_key": "asc"}},
                "aggs": {
                    "rtt": {
                        "percentiles": {
                            "field": "derived.srtt_ms",
                            "percents": [50, 90, 99, 100],
                        }
                    },
                    "jitter": {"max": {"field": "derived.rttvar_ms"}},
                },
            }
        },
    }
    print(f"  {'session':<48} {'p50':>7} {'p90':>7} {'p99':>7} {'max':>7} {'jitter':>8}")
    for s in search(url, METRICS, body)["aggregations"]["by_session"]["buckets"]:
        v = s["rtt"]["values"]
        j = s["jitter"]["value"] or 0.0
        print(
            f"  {s['key']:<48} {v['50.0'] or 0:>7.2f} {v['90.0'] or 0:>7.2f} "
            f"{v['99.0'] or 0:>7.2f} {v['100.0'] or 0:>7.2f} {j:>8.2f}"
        )

    # --- are rollbacks steady or bursty ----------------------------------
    rule("rollbacks per 10 s: steady, or in bursts")
    body = {
        "query": filters(session, [{"term": {"event": "rolled_back"}}]),
        "aggs": {
            "by_session": {
                "terms": {"field": "session", "size": 50, "order": {"_key": "asc"}},
                "aggs": {
                    "over_time": {
                        "date_histogram": {"field": "@timestamp", "fixed_interval": "10s"}
                    }
                },
            }
        },
    }
    for s in search(url, EVENTS, body)["aggregations"]["by_session"]["buckets"]:
        counts = [b["doc_count"] for b in s["over_time"]["buckets"]]
        if not counts:
            continue
        peak = max(counts) or 1
        spark = "".join(" ▁▂▃▄▅▆▇█"[min(8, round(c / peak * 8))] for c in counts)
        mean = sum(counts) / len(counts)
        print(f"  {s['key']}")
        print(f"      {spark}")
        print(f"      mean {mean:.1f} per 10 s, peak {max(counts)}, quiet buckets "
              f"{sum(1 for c in counts if c == 0)}/{len(counts)}")

    # --- where the frame budget goes -------------------------------------
    rule("frame budget: advance vs save_state vs load_state")
    body = {
        "query": filters(session),
        "aggs": {
            "by_session": {
                "terms": {"field": "session", "size": 50, "order": {"_key": "asc"}},
                "aggs": {
                    "advance": {"max": {"field": "local.advance_nanos"}},
                    "save": {"max": {"field": "local.save_state_nanos"}},
                    "load": {"max": {"field": "local.load_state_nanos"}},
                    "presented": {"max": {"field": "local.frames_presented"}},
                    "resim": {"max": {"field": "local.frames_resimulated"}},
                },
            }
        },
    }
    print(f"  {'session':<48} {'advance':>9} {'save':>9} {'load':>9}   (µs per presented frame)")
    for s in search(url, METRICS, body)["aggregations"]["by_session"]["buckets"]:
        presented = s["presented"]["value"] or 1
        adv = (s["advance"]["value"] or 0) / presented / 1000
        sav = (s["save"]["value"] or 0) / presented / 1000
        loa = (s["load"]["value"] or 0) / presented / 1000
        print(f"  {s['key']:<48} {adv:>9.1f} {sav:>9.1f} {loa:>9.1f}")
    print("\n  A frame at 60 Hz has 16 667 µs. Anything approaching that is the ceiling.")

    # --- the thing that must never happen --------------------------------
    rule("desyncs")
    body = {"query": filters(session, [{"term": {"event": "desync"}}])}
    total = search(url, EVENTS, {**body, "aggs": {}})["hits"]["total"]["value"]
    print(f"  {total} desync event(s) across the indexed sessions")
    return 0


if __name__ == "__main__":
    sys.exit(main())
