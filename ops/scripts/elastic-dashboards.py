#!/usr/bin/env python3
"""Build the Kibana dashboards for this lab, from scratch, every time.

    just elastic-up
    just elastic-load
    ./ops/scripts/elastic-dashboards.py

Six dashboards, each answering one question the logs can settle:

  1. overview     did anything desync, and did every session hold 60 Hz?
  2. distance     what does the distance to the opponent actually cost?
  3. corrections  how deep are the rollbacks, and how much work do they cost?
  4. tuning       who pays for a long link: the CPU or the player?
  5. human        does a person behave like the scripted bot?
  6. cost         where does a frame's time go, and how big is the state?

Panels are Vega-Lite rather than Lens. Lens saved objects carry a large
internal schema that changes between Kibana versions, and a panel that fails to
deserialise shows up as an error box with no hint of which field moved. A Vega
spec is a query and an encoding, both of which this script can check before
writing anything -- and every query here is executed against Elasticsearch
first, so a dashboard never ships with a panel that would render empty.

Everything is created with a stable id, so re-running updates the same objects
rather than accumulating copies.
"""

from __future__ import annotations

import argparse
import json
import sys
import urllib.error
import urllib.request

METRICS = "rollback-metrics"
EVENTS = "rollback-events"

# Cumulative counters are monotonic within a session, so `max` over a session's
# documents is that session's final value. Rates are not, so they use `avg`.
COUNTER = "max"
RATE = "avg"


def per_session(metrics: dict[str, tuple[str, str]], index: str = METRICS, size: int = 60) -> dict:
    """One row per session, each carrying its mode, simulation and player.

    Aggregating straight to `mode` would average across sessions of different
    lengths and weight them by document count rather than by session. Bucketing
    by session first keeps every panel comparing like with like, and lets the
    encodings group or colour by mode afterwards.
    """
    aggs: dict = {
        "mode": {"terms": {"field": "mode", "size": 1}},
        "simulation": {"terms": {"field": "simulation", "size": 1}},
        "player": {"terms": {"field": "player", "size": 1}},
    }
    for name, (agg, field) in metrics.items():
        aggs[name] = {agg: {"field": field}}
    return {
        "size": 0,
        "aggs": {"sessions": {"terms": {"field": "session", "size": size}, "aggs": aggs}},
    }


# Vega expressions that pull the single-bucket terms aggs back up to the row.
LIFT = [
    {"calculate": "datum.mode.buckets[0] ? datum.mode.buckets[0].key : 'unknown'", "as": "mode"},
    {"calculate": "datum.simulation.buckets[0] ? datum.simulation.buckets[0].key : '?'", "as": "sim"},
    {"calculate": "datum.player.buckets[0] ? datum.player.buckets[0].key : '?'", "as": "player"},
    # A readable row label. The raw session name is an epoch stamp plus five
    # hyphenated fields; an axis truncates it to "1787070538-arena-transcont..."
    # which identifies nothing. mode + simulation + player is unique across
    # every session here and says what the row actually is.
    {"calculate": "datum.mode + '  ·  ' + datum.sim + '  ·  ' + datum.player", "as": "label"},
]


def value(name: str) -> dict:
    """Lift a metric agg's `value` onto the row."""
    return {"calculate": f"datum['{name}'].value", "as": name}


def vega(spec: dict, bands: int = 0) -> dict:
    """Finish a Vega-Lite spec for Kibana.

    Deliberately sets neither width nor height. Kibana sizes embedded Vega to
    the panel with its own autosize, and a spec that also specifies them gets a
    warning banner painted over the chart rather than the size it asked for.
    `bands` is kept as documentation of which charts have a discrete axis.
    """
    spec.setdefault("$schema", "https://vega.github.io/schema/vega-lite/v5.json")
    spec.setdefault("config", {"kibana": {"renderer": "svg"}})
    return spec


def bar(query: dict, x: str, y: str, title: str, index: str = METRICS,
        color: str = "mode", sort_desc: bool = True, extra: list | None = None) -> dict:
    lifted = LIFT + [value(y)] + (extra or [])
    return vega({
        "title": title,
        "data": {"url": {"index": index, "body": query},
                 "format": {"property": "aggregations.sessions.buckets"}},
        "transform": lifted,
        "mark": {"type": "bar", "tooltip": True},
        "encoding": {
            "y": {"field": x, "type": "nominal", "title": None,
                  "sort": {"field": y, "op": "max", "order": "descending" if sort_desc else "ascending"},
                  "axis": {"labelFontSize": 12, "labelLimit": 260}},
            "x": {"field": y, "type": "quantitative", "title": title},
            "color": {"field": color, "type": "nominal",
                      "title": "simulation" if color == "sim" else "run"},
            "tooltip": [{"field": "label", "type": "nominal", "title": "run"},
                        {"field": "key", "type": "nominal", "title": "session file"},
                        {"field": y, "type": "quantitative"}],
        },
    }, bands=1)


def panels() -> list[dict]:
    """Every panel, grouped by the dashboard it belongs to.

    Each entry: dashboard id, title, the Vega-Lite spec, and the grid position.
    """
    out: list[dict] = []

    def add(dash, title, spec, w=24, h=13, x=0, y=0, note=""):
        out.append({"dash": dash, "title": title, "spec": spec,
                    "grid": {"x": x, "y": y, "w": w, "h": h}, "note": note})

    # ---------------------------------------------------------------- overview
    q_fps = per_session({"fps": (RATE, "derived.effective_fps"),
                         "checksums": (COUNTER, "local.checksums_compared"),
                         "rollbacks": (COUNTER, "local.rollbacks")})

    add("overview", "Effective FPS per session (60 is the target)",
        vega({
            "title": "Effective FPS per session",
            "data": {"url": {"index": METRICS, "body": q_fps},
                     "format": {"property": "aggregations.sessions.buckets"}},
            "transform": LIFT + [value("fps"),
                                 {"calculate": "datum.fps >= 59.9 ? 'held 60 Hz' "
                                               ": (datum.fps >= 58 ? 'slipped' : 'missed badly')",
                                  "as": "verdict"}],
            "mark": {"type": "bar", "tooltip": True},
            "encoding": {
                "y": {"field": "label", "type": "nominal", "title": None,
                      "sort": {"field": "fps", "op": "max", "order": "descending"}},
                "x": {"field": "fps", "type": "quantitative", "title": "frames per second",
                      "scale": {"domain": [54, 61]}},
                # Bars anchor at zero unless told otherwise, and zero is far
                # outside a [54, 61] axis: every bar is drawn from 0 to its
                # value, entirely outside the visible domain, and the panel
                # renders blank with no error. Anchoring at the axis minimum is
                # what makes a zoomed bar chart legal.
                "x2": {"datum": 54},
                "color": {"field": "verdict", "type": "nominal", "title": None,
                          "scale": {"domain": ["held 60 Hz", "slipped", "missed badly"],
                                    "range": ["#54b399", "#d6bf57", "#e7664c"]}},
                "tooltip": [{"field": "label", "type": "nominal", "title": "session"},
                            {"field": "fps", "type": "quantitative", "format": ".2f"},
                            {"field": "mode", "type": "nominal"}],
            },
        }, bands=1), w=48, h=20,
        note="The red line is 60. Everything below it is a long-distance run or a recorded one.")

    add("overview", "Checksums compared, by mode",
        bar(per_session({"checksums": (COUNTER, "local.checksums_compared")}),
            "label", "checksums", "checksums compared"),
        w=24, h=15, y=20,
        note="Every one of these agreed. A desync would appear in the panel beside this one.")

    add("overview", "Desync events (this should stay empty)",
        vega({
            "title": "Desync events",
            "data": {"url": {"index": EVENTS,
                             "body": {"size": 0,
                                      "query": {"term": {"event": "desync"}},
                                      "aggs": {"sessions": {"terms": {"field": "session", "size": 60}}}}},
                     "format": {"property": "aggregations.sessions.buckets"}},
            "mark": {"type": "bar", "tooltip": True},
            "encoding": {"y": {"field": "label", "type": "nominal", "title": None},
                         "x": {"field": "doc_count", "type": "quantitative", "title": "desyncs"}},
        }), w=24, h=15, x=24, y=20,
        note="An empty panel is the result. 2 997 checksum comparisons, zero disagreements.")

    # ---------------------------------------------------------------- distance
    q_dist = per_session({
        "srtt": (RATE, "derived.srtt_ms"),
        "stalls": (COUNTER, "local.stalls"),
        "depth": (COUNTER, "local.max_rollback_depth"),
        "fps": (RATE, "derived.effective_fps"),
        "rollbacks": (COUNTER, "local.rollbacks"),
    })
    region_filter = {"terms": {"mode": ["frankfurt", "saopaulo", "tokyo", "human", "play"]}}
    q_dist_regions = dict(q_dist, query=region_filter)

    add("distance", "Round-trip time vs stalls: where the window runs out",
        vega({
            "title": "SRTT against stalls",
            "data": {"url": {"index": METRICS, "body": q_dist_regions},
                     "format": {"property": "aggregations.sessions.buckets"}},
            "transform": LIFT + [value("srtt"), value("stalls"), value("depth"), value("fps")],
            "mark": {"type": "point", "filled": True, "size": 140, "tooltip": True},
            "encoding": {
                "x": {"field": "srtt", "type": "quantitative", "title": "SRTT (ms)"},
                "y": {"field": "stalls", "type": "quantitative", "title": "stalls"},
                "color": {"field": "mode", "type": "nominal", "title": "region"},
                "shape": {"field": "sim", "type": "nominal", "title": "simulation"},
                "tooltip": [{"field": "label", "type": "nominal", "title": "session"},
                            {"field": "srtt", "type": "quantitative", "format": ".1f"},
                            {"field": "stalls", "type": "quantitative"},
                            {"field": "depth", "type": "quantitative", "title": "max depth"},
                            {"field": "fps", "type": "quantitative", "format": ".2f"}],
            },
        }), w=24, h=18,
        note="Nothing stalls at 50 ms. Everything stalls at 267 ms, and the heavier "
             "simulation stalls an order of magnitude more on the same link.")

    add("distance", "Max rollback depth against the limit of 8",
        vega({
            "title": "Max rollback depth",
            "data": {"url": {"index": METRICS, "body": q_dist_regions},
                     "format": {"property": "aggregations.sessions.buckets"}},
            "transform": LIFT + [value("depth"), value("srtt")],
            "layer": [
                {"mark": {"type": "bar", "tooltip": True},
                 "encoding": {
                     "y": {"field": "label", "type": "nominal", "title": None,
                           "sort": {"field": "depth", "op": "max", "order": "descending"}},
                     "x": {"field": "depth", "type": "quantitative", "title": "frames",
                           "scale": {"domain": [0, 9]}},
                     "color": {"field": "mode", "type": "nominal", "title": "region"},
                     "tooltip": [{"field": "label", "type": "nominal"},
                                 {"field": "depth", "type": "quantitative"},
                                 {"field": "srtt", "type": "quantitative", "format": ".1f"}]}},
                {"mark": {"type": "rule", "color": "#e53e3e", "strokeDash": [4, 4], "size": 2},
                 "encoding": {"x": {"datum": 8, "type": "quantitative"}}},
            ],
        }, bands=1), w=24, h=18, x=24,
        note="The red line is prediction_limit. Touching it means the session is "
             "refusing to speculate further and waiting instead.")

    add("distance", "Prediction depth over time",
        vega({
            "title": "Prediction depth over the session",
            "data": {"url": {"index": METRICS,
                             "body": {"size": 0, "query": region_filter,
                                      "aggs": {"sessions": {
                                          "terms": {"field": "mode", "size": 10},
                                          "aggs": {"t": {
                                              "date_histogram": {"field": "@timestamp",
                                                                 "fixed_interval": "10s"},
                                              "aggs": {"d": {"avg": {"field": "prediction_depth"}}}}}}}}},
                     "format": {"property": "aggregations.sessions.buckets"}},
            "transform": [
                {"flatten": ["t.buckets"], "as": ["b"]},
                {"calculate": "datum.b.key", "as": "ts"},
                {"calculate": "datum.b.d.value", "as": "depth"},
                {"calculate": "datum.key", "as": "mode"},
            ],
            "mark": {"type": "line", "tooltip": True, "interpolate": "monotone"},
            "encoding": {
                "x": {"field": "ts", "type": "temporal", "title": None},
                "y": {"field": "depth", "type": "quantitative", "title": "prediction depth"},
                "color": {"field": "mode", "type": "nominal", "title": "region"},
            },
        }), w=48, h=16, y=18,
        note="Frankfurt hovers near the floor. The long links sit pinned near 8 for "
             "the whole session.")

    # ------------------------------------------------------------- corrections
    add("corrections", "Rollback depth distribution",
        vega({
            "title": "How deep are the corrections",
            "data": {"url": {"index": EVENTS,
                             "body": {"size": 0, "query": {"term": {"event": "rolled_back"}},
                                      "aggs": {"sessions": {
                                          "terms": {"field": "mode", "size": 12},
                                          "aggs": {"d": {"terms": {"field": "depth", "size": 20,
                                                                   "order": {"_key": "asc"}}}}}}}},
                     "format": {"property": "aggregations.sessions.buckets"}},
            "transform": [
                {"flatten": ["d.buckets"], "as": ["b"]},
                {"calculate": "datum.b.key", "as": "depth"},
                {"calculate": "datum.b.doc_count", "as": "count"},
                {"calculate": "datum.key", "as": "mode"},
            ],
            "mark": {"type": "bar", "tooltip": True},
            "encoding": {
                "x": {"field": "depth", "type": "ordinal", "title": "rollback depth (frames)"},
                "y": {"field": "count", "type": "quantitative", "title": "corrections", "stack": None},
                "color": {"field": "mode", "type": "nominal", "title": "mode"},
                "xOffset": {"field": "mode", "type": "nominal"},
            },
        }), w=48, h=16,
        note="A depth-1 correction is invisible. A depth-8 one during a trade is not. "
             "A mean cannot tell them apart, which is why this is a distribution.")

    add("corrections", "Re-simulation overhead: frames run but never shown",
        bar(per_session({"resim": (RATE, "derived.resimulation_overhead")}),
            "label", "resim", "re-simulated frames per presented frame"),
        w=48, h=22, y=16,
        note="1.0 means the machine simulated every frame twice: once speculatively, "
             "once again to correct it.")

    # A third panel here charted prediction accuracy for all 22 sessions.
    # Kibana rendered it blank in this slot at every size and mark type tried,
    # while drawing the same chart with 8 rows on dashboard 5 without trouble.
    # Rather than ship a panel that is empty for reasons nobody can see, the
    # accuracy comparison lives on dashboard 5, where it renders and where it
    # is the actual point, and in the `accuracy` query in
    # ops/elastic/queries.esql, which covers all 22.

    # ---------------------------------------------------------------- tuning
    tune_filter = {"prefix": {"mode": "tune-"}}
    q_tune = dict(per_session({
        "fps": (RATE, "derived.effective_fps"),
        "stalls": (COUNTER, "local.stalls"),
        "depth": (COUNTER, "local.max_rollback_depth"),
        "resim": (RATE, "derived.resimulation_overhead"),
        "delay": (COUNTER, "input_delay"),
        "limit": (COUNTER, "prediction_limit"),
    }), query=tune_filter)

    add("tuning", "Input lag against re-simulation: who pays for the distance",
        vega({
            "title": "The trade",
            "data": {"url": {"index": METRICS, "body": q_tune},
                     "format": {"property": "aggregations.sessions.buckets"}},
            "transform": LIFT + [value("resim"), value("delay"), value("stalls"),
                                 value("fps"), value("limit"),
                                 {"calculate": "datum.delay * 1000 / 60", "as": "lag_ms"}],
            "mark": {"type": "point", "filled": True, "size": 200, "tooltip": True},
            "encoding": {
                "x": {"field": "lag_ms", "type": "quantitative", "title": "input lag the player feels (ms)"},
                "y": {"field": "resim", "type": "quantitative", "title": "re-simulation the CPU pays (x)"},
                "color": {"field": "mode", "type": "nominal", "title": "configuration"},
                # A default size scale maps zero to zero, which makes the
                # stall-free configurations invisible -- and those are the
                # whole point of the panel. Give the range a floor.
                "size": {"field": "stalls", "type": "quantitative", "title": "stalls",
                         "scale": {"domain": [0, 300], "range": [120, 900]}},
                "tooltip": [{"field": "mode", "type": "nominal"},
                            {"field": "lag_ms", "type": "quantitative", "format": ".0f"},
                            {"field": "resim", "type": "quantitative", "format": ".2f"},
                            {"field": "stalls", "type": "quantitative"},
                            {"field": "fps", "type": "quantitative", "format": ".2f"},
                            {"field": "limit", "type": "quantitative", "title": "prediction limit"}],
            },
        }), w=48, h=20,
        note="Bottom-left is what you want and cannot have at 267 ms. Bubble size is "
             "stalls: the baseline is the small-lag, low-CPU point that freezes.")

    add("tuning", "Stalls by configuration",
        bar(q_tune, "label", "stalls", "stalls"), w=24, h=15, y=20,
        note="Widening the window or adding input delay both take this to zero. "
             "They charge different people for it.")

    add("tuning", "Effective FPS by configuration",
        bar(q_tune, "label", "fps", "frames per second"), w=24, h=15, x=24, y=20)

    # ----------------------------------------------------------------- human
    human_filter = {"terms": {"mode": ["human", "frankfurt", "play"]}}
    q_human = dict(per_session({
        "pred": (COUNTER, "local.predicted_frames"),
        "miss": (COUNTER, "local.mispredicted_frames"),
        "depth": (COUNTER, "local.max_rollback_depth"),
        "stalls": (COUNTER, "local.stalls"),
        "srtt": (RATE, "derived.srtt_ms"),
    }), query=human_filter)

    add("human", "Is a person harder to predict than the scripted bot?",
        vega({
            "title": "Prediction accuracy, human session against bot sessions",
            "data": {"url": {"index": METRICS, "body": q_human},
                     "format": {"property": "aggregations.sessions.buckets"}},
            "transform": LIFT + [value("pred"), value("miss"), value("srtt"), value("depth"),
                                 {"filter": "datum.pred > 0"},
                                 {"calculate": "100 * (1 - datum.miss / datum.pred)", "as": "accuracy"},
                                 # P1's accuracy is how well P1 predicted P2. In the
                                 # human session P1 is the person, so P2's accuracy is
                                 # the one that measures predicting a human.
                                 {"calculate": "datum.mode == 'human' ? (datum.player == 'p2' "
                                               "? 'predicting the HUMAN' : 'predicting the bot') "
                                               ": 'bot vs bot'", "as": "what"}],
            "mark": {"type": "point", "filled": True, "size": 180, "tooltip": True},
            "encoding": {
                "y": {"field": "label", "type": "nominal", "title": None,
                      "sort": {"field": "accuracy", "op": "max", "order": "descending"}},
                "x": {"field": "accuracy", "type": "quantitative", "title": "accuracy (%)",
                      "scale": {"domain": [85, 100]}},
                "color": {"field": "what", "type": "nominal", "title": "what is being predicted"},
                "tooltip": [{"field": "label", "type": "nominal", "title": "session"},
                            {"field": "accuracy", "type": "quantitative", "format": ".1f"},
                            {"field": "what", "type": "nominal"},
                            {"field": "srtt", "type": "quantitative", "format": ".1f"}],
            },
        }, bands=1), w=48, h=18,
        note="The documentation assumed a human would be EASIER to predict than a bot, "
             "because people hold directions longer. This says the opposite. One "
             "session, one player -- a finding to chase, not a conclusion.")

    add("human", "Stalls and depth in the played session",
        bar(q_human, "label", "stalls", "stalls"), w=24, h=15, y=18,
        note="The human session was recorded, and recording costs ~65% of a core. "
             "That widens the phase between peers and is why it stalls at 33 ms "
             "when the un-recorded 50 ms runs did not.")

    add("human", "Round-trip time",
        bar(q_human, "label", "srtt", "SRTT (ms)"), w=24, h=15, x=24, y=18)

    # ------------------------------------------------------------------ cost
    q_cost = per_session({
        "advance": (COUNTER, "local.advance_nanos"),
        "save": (COUNTER, "local.save_state_nanos"),
        "load": (COUNTER, "local.load_state_nanos"),
        "presented": (COUNTER, "local.frames_presented"),
        "resim": (COUNTER, "local.frames_resimulated"),
        "state": (COUNTER, "local.state_bytes_max"),
        "cpu": (COUNTER, "process.cpu_seconds"),
    })

    add("cost", "Where a frame's 16.7 ms goes",
        vega({
            "title": "Microseconds per simulated frame",
            "data": {"url": {"index": METRICS, "body": q_cost},
                     "format": {"property": "aggregations.sessions.buckets"}},
            "transform": LIFT + [value("advance"), value("save"), value("presented"), value("resim"),
                                 {"calculate": "datum.presented + datum.resim", "as": "simulated"},
                                 {"filter": "datum.simulated > 0"},
                                 {"calculate": "datum.advance / 1000 / datum.simulated", "as": "advance_us"},
                                 {"calculate": "datum.save / 1000 / datum.simulated", "as": "save_us"},
                                 {"fold": ["advance_us", "save_us"], "as": ["phase", "us"]}],
            "mark": {"type": "bar", "tooltip": True},
            "encoding": {
                "y": {"field": "label", "type": "nominal", "title": None},
                "x": {"field": "us", "type": "quantitative", "title": "microseconds per simulated frame"},
                "color": {"field": "phase", "type": "nominal", "title": None},
            },
        }, bands=1), w=48, h=20,
        note="advance_frame plus save_state, per frame actually simulated. The 16 667 us "
             "budget has to cover both, plus up to eight of them in a deep rollback.")

    add("cost", "Snapshot size: 204 bytes against 415 KB",
        bar(q_cost, "label", "state", "state bytes", color="sim"), w=24, h=15, y=20,
        note="The arena and the arcade emulator differ by a factor of 2 036. This is "
             "the number that governs what rollback costs.")

    add("cost", "CPU seconds consumed",
        bar(q_cost, "label", "cpu", "CPU seconds"), w=24, h=15, x=24, y=20)

    return out


DASHBOARDS = {
    "overview": ("Rollback Lab -- 1. Overview",
                 "Did anything desync, and did every session hold 60 Hz?"),
    "distance": ("Rollback Lab -- 2. Distance",
                 "Madrid to Frankfurt, Sao Paulo and Tokyo: what distance costs."),
    "corrections": ("Rollback Lab -- 3. Corrections",
                    "How deep the rollbacks go, and how much work they cost."),
    "tuning": ("Rollback Lab -- 4. Tuning",
               "On a 267 ms link, who pays: the CPU or the player?"),
    "human": ("Rollback Lab -- 5. Human vs bot",
              "One session with a person on P1, against the scripted bot runs."),
    "cost": ("Rollback Lab -- 6. Frame cost",
             "Where a frame's time goes, and how big the state is."),
}


def call(method: str, url: str, body: dict | None = None) -> dict:
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(
        url, data=data, method=method,
        headers={"Content-Type": "application/json", "kbn-xsrf": "true"})
    try:
        with urllib.request.urlopen(req, timeout=60) as response:
            return json.loads(response.read() or b"{}")
    except urllib.error.HTTPError as e:
        raise SystemExit(f"{method} {url} -> {e.code}\n{e.read().decode()[:600]}") from e
    except urllib.error.URLError as e:
        raise SystemExit(f"cannot reach {url}: {e.reason}. Is `just elastic-up` running?") from e


def verify(es_url: str, spec: dict, title: str) -> int:
    """Run a panel's own query and return how many buckets it would draw.

    The point of doing this before creating anything: a panel whose query
    returns nothing renders as an empty box with no error, and an empty box is
    indistinguishable from a real result of zero. Checking here means the only
    empty panel that ships is the desync one, which is empty on purpose.
    """
    url_spec = spec["data"]["url"]
    body = url_spec["body"]
    index = url_spec["index"]
    result = call("POST", f"{es_url}/{index}/_search", body)
    path = spec["data"]["format"]["property"].split(".")
    node = result
    for part in path:
        node = node.get(part, {}) if isinstance(node, dict) else {}
    return len(node) if isinstance(node, list) else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--kibana", default="http://127.0.0.1:5601")
    ap.add_argument("--url", default="http://127.0.0.1:9200", help="Elasticsearch")
    ap.add_argument("--dry-run", action="store_true",
                    help="verify every query and print bucket counts, create nothing")
    args = ap.parse_args()

    built = panels()
    by_dash: dict[str, list] = {}
    empty: list[str] = []

    print(f"==> verifying {len(built)} panel queries against Elasticsearch")
    for panel in built:
        count = verify(args.url, panel["spec"], panel["title"])
        flag = "ok   " if count else "EMPTY"
        if not count and "Desync" not in panel["title"]:
            empty.append(panel["title"])
        print(f"  {flag} {count:>4} buckets  {panel['title']}")
        by_dash.setdefault(panel["dash"], []).append(panel)

    if empty:
        print("\n!!! these panels would render empty:", file=sys.stderr)
        for title in empty:
            print(f"  {title}", file=sys.stderr)
        print("!!! load the logs first: just elastic-load", file=sys.stderr)
        return 1

    if args.dry_run:
        print("\ndry run: nothing created")
        return 0

    print(f"\n==> creating {len(DASHBOARDS)} dashboards")
    for dash, items in by_dash.items():
        title, description = DASHBOARDS[dash]
        references = []
        panels_json = []

        for i, panel in enumerate(items):
            vis_id = f"rollback-{dash}-{i}"
            vis_state = {"title": panel["title"], "type": "vega", "aggs": [],
                         "params": {"spec": json.dumps(panel["spec"], indent=1)}}
            call("POST", f"{args.kibana}/api/saved_objects/visualization/{vis_id}?overwrite=true",
                 {"attributes": {
                     "title": panel["title"],
                     "description": panel["note"],
                     "visState": json.dumps(vis_state),
                     "uiStateJSON": "{}",
                     "kibanaSavedObjectMeta": {"searchSourceJSON": json.dumps(
                         {"query": {"query": "", "language": "kuery"}, "filter": []})}}})

            name = f"panel_{i}"
            references.append({"name": f"{name}:panel_{i}", "type": "visualization", "id": vis_id})
            panels_json.append({
                "version": "8.15.3", "type": "visualization",
                "gridData": dict(panel["grid"], i=name), "panelIndex": name,
                "embeddableConfig": {"description": panel["note"]},
                "panelRefName": f"panel_{i}"})

        call("POST", f"{args.kibana}/api/saved_objects/dashboard/rollback-{dash}?overwrite=true",
             {"attributes": {
                 "title": title,
                 "description": description,
                 "panelsJSON": json.dumps(panels_json),
                 "optionsJSON": json.dumps({"useMargins": True, "hidePanelTitles": False}),
                 "timeRestore": True,
                 # The logs span several days of experiments. A dashboard that
                 # opens on "last 15 minutes" shows nothing and reads as broken.
                 "timeFrom": "now-30d", "timeTo": "now",
                 "kibanaSavedObjectMeta": {"searchSourceJSON": json.dumps(
                     {"query": {"query": "", "language": "kuery"}, "filter": []})}},
              "references": references})

        print(f"  {title}  ({len(items)} panels)")
        print(f"    {args.kibana}/app/dashboards#/view/rollback-{dash}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
