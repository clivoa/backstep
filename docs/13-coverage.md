# 13 — Coverage: what was validated, and what was not

This file exists so nobody, the author included, confuses "the lab runs" with
"the lab was proven". It lists the exact environment of every measurement and
what still has no evidence behind it.

Updated after each round of experiments.

## The two measurement environments

### A. Local bench: one machine, two processes, `127.0.0.1`

| | |
|---|---|
| Topology | two `rollback-bot` on the same host; P2 listens on `127.0.0.1:7100`, P1 dials |
| Real network | loopback, negligible latency and loss |
| Impairment | synthetic, injected on each peer's outgoing datagrams |
| Binary | the same executable on both sides |
| CPU / OS / compiler | identical on both sides, by construction |

The five network profiles in [08 — Experiments](08-experiments.md) were measured
this way.

### B. Real session: Madrid to Frankfurt

| | P1 | P2 |
|---|---|---|
| Where | Madrid, Spain | Frankfurt, `eu-central-1` |
| Machine | Arch Linux, Intel Core i7-10750H | Ubuntu 24.04, EC2 `t3.small` |
| Network | the internet, no synthetic impairment | same |

Two sessions: The Last Blade 2 for 300 s, then the arena for 150 s. Full results
in [08](08-experiments.md#the-real-session-madrid-to-frankfurt).

A third session followed later, 150 s of The Last Blade 2 with both peers
recording video, which is where the footage in [14](14-video.md) comes from.

**A note on evidence.** The raw logs for the first two sessions were overwritten
by that third run, so `artifacts/logs` no longer holds them. The numbers were
recorded at the time and stand as reported, but they cannot be re-derived from
what is on disk now. What survives and can be re-checked:

| Run | Logs on disk |
|---|---|
| Five-profile bench, 240 s each, loopback | `artifacts/e2e/logs` |
| Five-profile recorded bench, 90 s each | `artifacts/video/raw/*/` |
| Madrid–Frankfurt, 150 s, recorded | `artifacts/logs` |
| Madrid–Frankfurt, 300 s + arena 150 s | **overwritten** |

The surviving real session corroborates the same claims independently: 150
checksum comparisons on each side, no desyncs, across the same two machines.
Anyone reproducing this should run `just collect` into a fresh directory rather
than the default.

## What that actually validates

This part is solid, and it is not a small list. It is also where four real bugs
turned up.

- **The rollback engine.** Prediction, re-simulation, prediction limit, state
  buffer, stalls. Exercised by 20 minutes of emulated fighting and by
  100 000-frame replays in the arena.
- **The protocol, end to end.** Real UDP sockets, real datagrams, real HMAC,
  real handshake. Nothing mocked.
- **Behaviour under loss, delay, jitter and reordering**, within the limits of
  the synthetic model below.
- **Desync detection.** 2 389 checksum comparisons agreeing across five
  profiles, after the detector itself was fixed.
- **Emulator determinism across processes.** `just check-determinism` runs the
  core in two separate processes, in different wall-clock seconds.
- **Savestate safety for rollback.** `just check-rollback-safety` shows that 300
  re-simulated frames change nothing the game can observe.
- **Determinism across different machines.** 449 checksum comparisons agreeing
  between an i7-10750H running Arch and an EC2 instance running Ubuntu 24.04, in
  both simulations, with no desyncs.
- **A session over the real internet**, with the latency, stability and loss the
  Madrid to Frankfurt path actually has.

## What was not validated

In order of importance.

### ~~1. Determinism across different machines~~ — CLOSED

This was the serious one. Fixed point Q23.8, no `HashMap` in the simulation, the
hand-rolled FNV-1a, `overflow-checks` in release, and nothing derived from an
address: all of it exists so two different hosts agree bit for bit. Two
processes of one binary on one CPU would have agreed even if every one of those
rules were wrong.

Closed by the Madrid–Frankfurt session: **449 agreeing checksum comparisons**
across different CPUs, operating systems and libcs, in both simulations. The
arena counts separately, because the arena is the code we wrote.

### ~~2. No session between different locations~~ — CLOSED

Two sessions run, collected and destroyed. What was learned beyond the obvious
is in
[08](08-experiments.md#what-that-proves-that-loopback-could-not); the short
version is that the synthetic profiles were pessimistic in every dimension, and
that part of the cost measured on loopback was the bench rather than the
rollback.

Still unmeasured, because five minutes on a good link does not produce them:

- burst loss (the real link lost **zero** of 18 602 datagrams)
- routes changing mid-session
- congestion at peak hours
- domestic NAT on both ends; here one end was an EC2 instance with a public IP

### 3. The synthetic network is not the internet

The network emulator is deliberately simple, and that has consequences:

| The model does | The internet does |
|---|---|
| independent per-datagram loss (Bernoulli) | **burst** loss: several in a row, then none |
| uniform jitter within ±N ms | a **long-tailed** distribution, rare large spikes |
| constant delay per profile | delay that moves with congestion and time of day |
| a fixed route | routes that change mid-session |

Bursts matter especially. This protocol's defence against loss is repeating the
last eight inputs in every datagram, and **a burst is exactly the worst case for
a redundancy window**. Losing eight datagrams in a row defeats it; losing eight
scattered ones does not come close.

This stopped being theory after the real session. Measured, not estimated:

| | Madrid↔Frankfurt, real | `delay20` | `jitter30` | `loss2` |
|---|---|---|---|---|
| RTT | 50 ms | 70 ms | 86 ms | 27 ms |
| RTT variation | 0.37 ms | 0.5 ms | ~18 ms | 0.5 ms |
| Loss | 0.000% (0 of 18 602) | 0% | 0% | 1.88% |

The profiles are pessimistic in every dimension, which is the safe direction to
be wrong in. Worth knowing that `jitter30` describes bad Wi-Fi rather than a
link between datacentres.

What remains unmeasured is the *shape* of loss: this link lost nothing, so the
hypothesis that bursts defeat the redundancy window is still untested.

### 4. A human on P1

Every session has been bot against bot. `rollback-client` (SDL2, keyboard and
gamepad, overlay) compiles and is exercised by tests, but no match with a person
on P1 has been played on this bench.

That leaves unanswered the very question rollback exists to answer: **what is it
like to play?** No metric in this repository measures perception.

### 5. Street Fighter Alpha 3

Blocked by an incomplete romset: the available set is missing `sfa3.key`, the
CPS-2 decryption key, and none of FBNeo's eleven SFA3 variants does without one.
See [09](09-the-last-blade-2.md).

The libretro path is validated, with The Last Blade 2 in its place.

### 6. One run per profile

There is no confidence interval on any number in this repository. Each cell in
each table is one sample. For spread, vary the seed:

```bash
for s in 1 2 3 4 5; do SEED=$s DURATION=60 ./ops/scripts/bench.sh; done
```

## Next steps, by value

1. **A human on P1.** The only gap no metric here can close, and the question
   rollback exists to answer.
2. **A long session on a bad link**, mobile or intercontinental, to see burst
   loss. It is the one case where the eight-input redundancy could genuinely
   fail.
3. **Spread.** Vary the seed and aggregate, so the tables carry confidence
   intervals rather than single samples.
4. **SFA3**, if a set with `sfa3.key` turns up.
