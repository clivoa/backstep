# 07 — Dashboard

> What each metric means and how to read it is in
> [00 — Glossary: metrics](00-glossary.md#the-metrics-this-lab-reports).

## Bringing it up

```bash
just local-up
```

- Grafana: <http://127.0.0.1:3000> (anonymous access, dashboard provisioned)
- Prometheus: <http://127.0.0.1:9090>
- Exporter: <http://127.0.0.1:9898/metrics> (only exists while a session runs)

On a local bench with two peers, the second exports on `127.0.0.1:9899` and
Prometheus scrapes both, labelled `instance="local"` and
`instance="local-peer2"`.

Everything listens on loopback. That is deliberate, and the reasoning is in
[06 — AWS](06-aws.md).

For picking a single session apart after the fact rather than watching one live,
see [15 — Elastic](15-elastic.md). Different tool, different question.

## How the remote peer shows up

Prometheus does **not** scrape the EC2 instance. There is no metrics port open
there.

Each peer sends the other a `TelemetrySummary` every 60 frames over the session's
own link. The local exporter republishes those numbers labelled
`peer="remote"`, alongside its own labelled `peer="local"`.

Practical consequence: `rollback_rollbacks_total{peer="remote"}` is **what the
remote peer says about itself**, up to a second stale. That is the right
information for comparing the two sides, and it is not the same thing as
scraping the instance.

## What each panel means

### Session

| Panel | Reading |
|---|---|
| **Desync** | 0 = fine. 1 = two confirmed-frame checksums disagreed and the session ended. There is no middle state. |
| **Prediction depth** | How many frames ahead of the peer we are speculating. The limit is 8. Touching it means a stall. |
| **Prediction accuracy** | Fraction of guesses that held. Below ~0.85 under moderate latency means the opponent is moving a lot, or the link got worse. |
| **Smoothed RTT** | RFC 6298 SRTT. There is no one-way latency here — see [03](03-protocol.md). |
| **State size** | 204 bytes in the arena; 415 155 in The Last Blade 2. |

### Rollback

| Panel | Reading |
|---|---|
| **Rollbacks per second** | Each one is a prediction that did not hold. Compare the two peers: whoever is ahead corrects more. |
| **Rollback depth** | Mean (re-simulated frames ÷ rollbacks) and the highest seen. The maximum cannot exceed the prediction limit. |
| **Extra simulation work** | Re-simulated frames per presented frame. 0 = no rollbacks; 1 = the CPU simulated everything twice. This is rollback's CPU cost, directly. |
| **Stalls** | Frames where the window filled and the simulation stopped. Non-zero means the peer is not keeping up. |

### Network

| Panel | Reading |
|---|---|
| **RTT and variation** | Under `jitter30`, the variation is the interesting number. |
| **Loss, duplication, reordering** | Loss is **inferred** from sequence gaps; a delayed datagram looks lost until it arrives, then the estimate corrects itself. |
| **Bitrate** | Only inputs travel. An `InputBatch` at 60 Hz with eight repeated inputs is ~35 kbit/s. Much above that and something is sending more than it should. |
| **Rejected datagrams** | HMAC failures and malformed packets. Non-zero outside a test means someone is sending rubbish at the port. |

### Cost of running

| Panel | Reading |
|---|---|
| **Time per frame** | How much of each frame goes into `advance_frame`, `save_state` and `load_state`. The budget at 60 Hz is 16 667 µs; you want to stay under about half of it so the worst-case rollback fits. |
| **CPU and memory** | Read from `/proc/self/stat` and `/proc/self/statm` every 30 frames. |

On The Last Blade 2 those two panels are the whole cost argument: `advance` runs
about 3 948 µs and `save_state` about 2 271 µs per presented frame, which is 37%
of the budget before any rollback happens at all.

## Visual signatures

**A healthy session under `delay20`:** prediction depth steady at 2–3, accuracy
above 0.9, rollbacks constant but shallow, stalls at zero, loss at zero.

**`loss2` behaving as designed:** inferred loss hovering near 2%, and rollbacks
**not** following it. That is the eight-input redundancy doing its job: the lost
input arrives in the next datagram, before it is needed. Every rollback under
this profile is depth 1.

**The peer is not keeping up:** prediction depth pinned at 8, stalls climbing,
`effective_fps` below 60. Either the network got worse or the instance is too
small for the simulation.

**Desync:** the panel goes red and every counter stops. The session is over; the
JSONL has the event with both checksums and the frame number.

## Useful queries

```promql
# rollback's CPU cost
rate(rollback_frames_resimulated_total[30s])
  / clamp_min(rate(rollback_frames_presented_total[30s]), 0.001)

# both peers side by side
rollback_rollbacks_total

# how much of the frame budget is in use
rate(rollback_advance_seconds_total[15s])
  / clamp_min(rate(rollback_frames_presented_total[15s])
            + rate(rollback_frames_resimulated_total[15s]), 0.001)

# is the session actually at 60 Hz?
rate(rollback_frames_presented_total{peer="local"}[30s])
```

## The dashboard is version controlled

`ops/grafana/dashboards/rollback.json` is mounted read-only, with
`allowUiUpdates: false` and `disableDeletion: true`. The dashboard is part of the
experiment; it should not drift in someone's browser and then fail to reproduce.

To change it: edit the JSON, `just local-down && just local-up`, commit.

## The HTML report

The dashboard shows *now*. The report shows what **happened**:

```bash
just report   # from whatever logs are already on disk
```

It writes `artifacts/report/report.html`, self-contained: no CDN, no script, no
external font, charts as inline SVG. It has to be readable from a laptop with
the Wi-Fi off, months after the AWS account was torn down.

It carries an overview per profile, a table with both peers side by side for
each run, time series, and a section of caveats about how to read the numbers.
