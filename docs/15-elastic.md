# 15 - Elastic: the questions `summary.csv` cannot answer

> Terms like *depth*, *SRTT*, *stall* and *profile* are defined in
> [00 - Glossary](00-glossary.md).

## Why another tool

The lab already had two ways of looking at a session:

| Tool | Granularity | Good for |
|---|---|---|
| Prometheus + Grafana ([07](07-dashboard.md)) | live, ~1 s | watching a session **while** it runs |
| `summary.csv` / `report.html` | one row per session | **comparing** sessions and profiles |
| Elastic | one document per event | **investigating** one session |

`summary.csv` says "mean depth 2.04". It cannot say that 94% of rollbacks were
depth 2 and exactly four reached depth 4, which is what tells you whether the
prediction limit of 8 is being tested or nowhere near it.

The JSONL logs always had that detail. What was missing was somewhere to query
it.

## Bringing it up

```bash
just elastic-up          # Elasticsearch :9200, Kibana :5601, both on loopback
just elastic-load        # index artifacts/logs
just elastic-analyze     # the standing analyses, in the terminal
just elastic-down
```

The loader creates the Kibana data views itself, so `http://127.0.0.1:5601`
opens ready to query.

Loopback only and no password, for the same reason as the metrics exporter: this
is an analysis tool on one machine, not a service. See [06 - AWS](06-aws.md) for
the same argument at length.

## Two indices

| Index | One document per | Use it for |
|---|---|---|
| `rollback-metrics` | second, per peer | charts, percentiles, trends |
| `rollback-events` | session event | frame-by-frame forensics |

Datagram records (`sent`, `received`, `local_input`, `remote_inputs`) are the
bulk of a log, tens of thousands of lines each, and are skipped by default.
`just elastic-load artifacts/logs 1` includes them.

Every document carries the session's identity (simulation, profile, player,
seed, commit) denormalised onto it, so nothing needs a join to be filtered.

### Two mapping decisions worth knowing

**Checksums are `keyword`, not numbers.** They are 64-bit *unsigned* FNV-1a and
do not fit Elasticsearch's signed long; the first load attempt failed with
`Numeric value out of range of long`. Storing them truncated would be worse than
useless, because two different states could collide. And nobody averages a
checksum, they compare it. Hex keywords are the right representation.

**Derived metrics are computed at load time**, not in Kibana:
`derived.effective_fps`, `derived.resimulation_overhead`, `derived.loss_pct`,
`derived.srtt_ms`. That way every consumer of the index agrees with
`summary.csv` without re-deriving anything, and two people cannot derive it
differently.

## What it turned up

All of this was in the logs already and invisible in the tables.

### Jitter does not raise rollback depth, it spreads it

The most useful thing to come out of this. Comparing profiles, on the peer doing
the work:

| Profile | Rollbacks | Depth distribution |
|---|---|---|
| `loss2` | 5 | `d1: 100%` |
| `natural` | 26 | `d1: 23%  d2: 76%` |
| `delay20` | 260 | `d4: 11%  d5: 88%` |
| `jitter30` | 260 | `d4: 13%  d5: 59%  d6: 27%` |
| `combined` | 259 | `d3: 1%  d4: 39%  d5: 47%  d6: 10%  d7: <1%` |

`delay20` and `jitter30` produce the **same count** and nearly identical means,
which is what [08](08-experiments.md) concluded and which still holds. The shape
is different: `delay20` is concentrated, with 88% at a single value, while
`jitter30` is spread, and it is the spread that pushes the **tail** from 5 to 6.

That is a precise statement about what jitter does to rollback, and the mean was
incapable of making it. Jitter does not cost more work. It costs worse cases.
And the worst case is what reaches the prediction limit and becomes a stall.

### Loss produces only the shallowest possible correction

`loss2`: **100% of rollbacks at depth 1**. Which makes sense. A lost input
arrives in the next datagram, 16.7 ms later, so the correction never needs to
reach back more than one frame.

It is about as direct a confirmation as possible that the eight-input redundancy
does what it was built for, and that loss and latency are different problems.

### The RTT tail the average hides

On the real Madrid-Frankfurt session:

```
p50  49.97 ms      p90  52.21 ms      p99  54.96 ms      max  57.11 ms
```

The reported SRTT was 49.9 ms. The worst case was 57.1, and on the other peer,
72.4 ms. A 22 ms spike above the median that no average would show.

### Where the frame budget actually goes

Accumulated nanoseconds divided by presented frames, so mean per frame:

| Session | `advance` | `save_state` | `load_state` |
|---|---|---|---|
| The Last Blade 2 (Madrid) | 3 948 µs | 2 271 µs | 17 µs |
| arena (Madrid) | 1.3 µs | 0.9 µs | 0.0 µs |

A frame at 60 Hz has 16 667 µs. The emulator spends 6.2 ms of it, 37%, just
advancing and saving. The arena spends 2.2 µs, three orders of magnitude less.

This is rollback's cost argument quantified: `save_state` alone, on a 415 KB
state, eats 14% of the budget on **every** frame, rollback or not.

## Useful queries

`just elastic-analyze` runs the four above. For your own, in Kibana's Dev Tools:

```json
// When did the rollbacks cluster?
GET rollback-events/_search
{
  "size": 0,
  "query": { "bool": { "filter": [
    { "term": { "event": "rolled_back" } },
    { "term": { "profile": "combined" } }
  ]}},
  "aggs": { "over_time": {
    "date_histogram": { "field": "@timestamp", "fixed_interval": "5s" },
    "aggs": { "mean_depth": { "avg": { "field": "depth" } } }
  }}
}
```

```json
// Did depth climb before the stall?
GET rollback-metrics/_search
{
  "size": 20,
  "query": { "range": { "local.stalls": { "gt": 0 } } },
  "sort": [{ "@timestamp": "asc" }],
  "_source": ["@timestamp", "frame", "prediction_depth",
              "local.stalls", "derived.srtt_ms"]
}
```

```json
// Did both peers agree on every checksum compared?
GET rollback-events/_search
{
  "size": 0,
  "query": { "term": { "event": "checksum_matched" } },
  "aggs": { "by_session": { "terms": { "field": "session", "size": 50 } } }
}
```

## Limits

**Post-mortem only.** Loading is manual, after the session. There is no live
shipping; Prometheus is for that.

**No retention policy.** The indices grow until you run `just elastic-reload`. A
five-minute session is about 20 000 documents without datagrams, 60 000 with.

**No version-controlled dashboard.** The data views are created automatically. A
saved Kibana dashboard would be a large JSON blob, fragile across versions, and
would age worse than the queries above.
