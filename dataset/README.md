# The dataset

Every session this lab recorded, so the analysis can be checked or taken
somewhere else without running anything.

```bash
tar xf dataset/rollback-sessions.tar.zst -C artifacts/
just elastic-up && just elastic-load
./ops/scripts/elastic-dashboards.py
```

5.3 MB compressed, 100 MB expanded. 24 session logs in JSONL, one per peer per
run.

## What is in it

| Runs | Sessions | What it is |
|---|---|---|
| `*-frankfurt.jsonl` | 4 | Madrid to `eu-central-1`, 50 ms. Both simulations, both peers |
| `*-saopaulo.jsonl` | 4 | Madrid to `sa-east-1`, 267 ms |
| `*-tokyo.jsonl` | 4 | Madrid to `ap-northeast-1`, 267 ms |
| `*-play.jsonl` | 2 | Madrid to Frankfurt, recorded, the footage in docs |
| `*-human.jsonl` | 2 | A person on P1 against the scripted bot, recorded |
| `tuning/logs/*-tune-*.jsonl` | 8 | Four configurations against a synthetic 267 ms link |

Filenames read `<unix-seconds>-<simulation>-<profile>-<player>-<mode>.jsonl`.
`mode` is the run's label and is what every dashboard groups by.

## The format

One JSON object per line. `record` says which kind:

| `record` | Written | Carries |
|---|---|---|
| `session_start` | once, first line | `started_unix_ms` and the full configuration |
| `metrics` | every 60 frames | a snapshot of every counter, nested under `local`, `remote`, `link`, `process` |
| `session` | on interesting events | `event`: `advanced`, `rolled_back`, `stalled`, `checksum_matched`, `desync` |
| `local_input` | every frame | the input this peer filed, and for which frame |
| `remote_inputs` | on arrival | what the peer sent, and from which frame |
| `sent` / `received` | every datagram | sequence, kind, ack |
| `session_end` | once, last line | the final totals and why the session ended |

Counters inside `metrics` and `session_end` are **cumulative**, so a session's
final value is the maximum, not the mean. Rates (`srtt_micros`, and anything
`elastic-load.py` derives) are instantaneous.

A session killed mid-write leaves exactly one truncated final line. Every tool
here skips it rather than failing.

## Reading it without Elasticsearch

The logs are line-delimited JSON, so they need no tooling at all:

```python
import json, pathlib

for path in pathlib.Path("artifacts/logs").glob("*-tokyo.jsonl"):
    for line in path.open():
        try:
            record = json.loads(line)
        except json.JSONDecodeError:
            continue                      # the truncated last line
        if record.get("record") == "session_end":
            local = record["local"]
            print(path.name,
                  local["rollbacks"], "rollbacks,",
                  local["stalls"], "stalls,",
                  local["max_rollback_depth"], "max depth")
```

```bash
# every correction deeper than 6 frames, and when it happened
jq -c 'select(.record=="session" and .event=="rolled_back" and .depth>6)' \
  artifacts/logs/*-tokyo.jsonl | head
```

## What is not in it

**No ROM or BIOS data.** The logs contain SHA-256 hashes of the ROM set, which
identify it without reproducing any of it.

**No video.** The recordings are hundreds of megabytes; the GIFs in
`docs/media/` are the excerpts worth keeping.

**No keys.** Session keys are ephemeral, live only in SSM and a mode-0600 local
file, and are deleted at teardown. Nothing derived from one appears here.

## Provenance

Every log records the commit that produced it (`info.app_commit`), the SHA-256
of the emulator core and of the ROM set, the seed, and the full session
configuration. Two logs claiming to be the same experiment can be checked
rather than trusted.

The runs are real: a desktop in Madrid against EC2 instances in three AWS
regions, over the public internet, on the dates in the filenames. Nothing here
is synthetic except the eight `tune-*` sessions, which use the emulator's
`transcontinental` profile on loopback and say so in their `profile` field.
