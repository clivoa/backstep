# 13 - Coverage: what was validated, and what was not

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

The five network profiles in [08 - Experiments](08-experiments.md) were measured
this way.

### B. Real sessions: Madrid to three continents

| | P1 | P2 |
|---|---|---|
| Where | Madrid, Spain | Frankfurt `eu-central-1`, São Paulo `sa-east-1`, Tokyo `ap-northeast-1` |
| Machine | Arch Linux, Intel Core i7-10750H | Ubuntu 24.04, EC2 `t3.small` |
| Network | the internet, no synthetic impairment | same |

Both simulations against each region: The Last Blade 2 for 300 s, then the arena
for 150 s. Twelve session logs, six sessions, 2 697 checksum comparisons. Full
results in
[08](08-experiments.md#three-continents-where-the-prediction-window-runs-out).

**A note on evidence, and how it was repaired.** An earlier pair of
Madrid-Frankfurt sessions had their logs overwritten by a later recorded run,
so the numbers first reported here could not be re-derived from disk. That gap
is now closed: the three-region runs above were collected under region-labelled
filenames, and **every one of them is on disk and re-checkable**.

| Run | Logs on disk |
|---|---|
| Five-profile bench, 240 s each, loopback | `artifacts/e2e/logs` |
| Five-profile recorded bench, 90 s each | `artifacts/video/raw/*/` |
| Madrid-Frankfurt, 150 s, recorded | `artifacts/logs/*-play.jsonl` |
| Madrid-Frankfurt, game + arena | `artifacts/logs/*-frankfurt.jsonl` |
| Madrid-São Paulo, game + arena | `artifacts/logs/*-saopaulo.jsonl` |
| Madrid-Tokyo, game + arena | `artifacts/logs/*-tokyo.jsonl` |
| Tuning sweep, four configurations | `artifacts/tuning/logs` |
| Original Madrid-Frankfurt, 300 s + arena 150 s | **overwritten, superseded** |

Two changes make the loss structural rather than a thing to remember:

- every log carries its region in the filename (`--mode`), so runs cannot
  overwrite each other or become ambiguous on disk;
- `region-run.sh` **verifies** that both peers' logs and both recordings arrived
  before it tears anything down, and refuses to destroy the instance if
  something is missing. It says so loudly, including that the instance is still
  costing money.

That check earned itself immediately. A Tokyo run aborted mid-script and never
reached teardown, leaving an instance running with its logs and a 171 MB peer
recording still on it. Both were collected before anything was destroyed.

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
  profiles, after the detector itself was fixed, plus 2 697 more across three
  continents.
- **Emulator determinism across processes.** `just check-determinism` runs the
  core in two separate processes, in different wall-clock seconds.
- **Savestate safety for rollback.** `just check-rollback-safety` shows that 300
  re-simulated frames change nothing the game can observe.
- **Determinism across different machines.** 2 997 checksum comparisons agreeing
  between an i7-10750H running Arch and EC2 instances running Ubuntu 24.04, in
  both simulations, across three continents, with no desyncs.
- **Sessions over the real internet**, at 50 ms and at 267 ms, with the latency,
  stability and loss those paths actually have.
- **A session with a person on P1**, over the real internet, which is the
  only kind of run that can say anything about how a human's inputs behave.
- **Behaviour at the edge of the prediction window.** The eight long-distance
  runs pinned depth at the limit and produced the project's first stalls, so the
  stall path and the recovery from it are exercised by measurement rather than
  only by tests.

## What was not validated

In order of importance.

### ~~1. Determinism across different machines~~ - CLOSED

This was the serious one. Fixed point Q23.8, no `HashMap` in the simulation, the
hand-rolled FNV-1a, `overflow-checks` in release, and nothing derived from an
address: all of it exists so two different hosts agree bit for bit. Two
processes of one binary on one CPU would have agreed even if every one of those
rules were wrong.

Closed by the Madrid-Frankfurt session: 449 agreeing checksum comparisons across
different CPUs, operating systems and libcs, in both simulations. The arena
counts separately, because the arena is the code we wrote.

The three-region runs since then bring the total to **2 997 agreeing comparisons
and no desyncs**, including 1 800 at 267 ms where the session was stalling and
re-simulating constantly. Determinism holding while the engine is under that
much correction pressure is a stronger result than determinism on a quiet link.

### ~~2. No session between different locations~~ - CLOSED

Six sessions across three regions, run, collected and destroyed. What was
learned beyond the obvious is in
[08](08-experiments.md#what-that-proves-that-loopback-could-not); the short
version is that the synthetic profiles were pessimistic in every dimension, and
that part of the cost measured on loopback was the bench rather than the
rollback.

### ~~3. Only one distance~~ - CLOSED

Frankfurt at 50 ms said rollback works over the internet. It could not say what
happens when the opponent is far away, and the honest answer turned out to be
that the default configuration stops being adequate: at 267 ms the 8-frame
window saturates, depth pins at the limit, and effective FPS falls below 60 for
the first time in this project.

São Paulo and Tokyo both measured 267 ms from Madrid, in opposite directions,
which made the second region an independent repeat of the first rather than a
new data point. The
[tuning sweep](08-experiments.md#fixing-a-link-that-is-too-long-the-tuning-sweep)
then measured what it costs to fix, on a loopback profile calibrated against the
real link.

Still unmeasured, because six sessions on good links do not produce them:

- burst loss (the worst link here lost 0.03%)
- routes changing mid-session
- congestion at peak hours
- domestic NAT on both ends; here one end was always an EC2 instance with a
  public IP

### 4. The synthetic network is not the internet

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

This stopped being theory after the real sessions. Measured, not estimated:

| | Madrid↔Frankfurt | Madrid↔Tokyo | `delay20` | `jitter30` | `loss2` |
|---|---|---|---|---|---|
| RTT | 50 ms | 267 ms | 70 ms | 86 ms | 27 ms |
| RTT variation | 0.37 ms | 1.1 ms | 0.5 ms | ~18 ms | 0.5 ms |
| Loss | 0.000% (0 of 18 602) | 0.03% | 0% | 0% | 1.88% |

The profiles are pessimistic in every dimension, which is the safe direction to
be wrong in. Worth knowing that `jitter30` describes bad Wi-Fi rather than a
link between datacentres - and that this holds at 9 700 km as much as at 1 400.
Intercontinental fibre is *slow*, not *unstable*: Tokyo added 217 ms of latency
over Frankfurt and less than a millisecond of variance.

That distinction matters for rollback specifically, because rollback is
sensitive to variance rather than to latency. A steady 267 ms is a configuration
problem, solvable by tuning. A 267 ms link that jittered like `jitter30` would
be a much harder one.

What remains unmeasured is the *shape* of loss: none of these links lost enough
to see, so the hypothesis that bursts defeat the redundancy window is still
untested.

The model earned its keep in one respect the real links could not. Because the
emulator holds delayed datagrams in a local queue, it exposed a deadlock in the
stalled path that a real network structurally cannot produce - see
[08](08-experiments.md#a-deadlock-this-found-that-the-cloud-runs-could-not).
Simplifications make a model wrong in known ways; they also make it able to
break things reality is too forgiving to break.

### ~~5. A human on P1~~ - CLOSED, and it moved a result

A person played The Last Blade 2 on P1 against the scripted bot in Frankfurt,
over the real internet: 480 seconds, recorded on both ends, 8 303 frames.

It did not merely tick a box. **It contradicted an assumption written into this
file.** The reasoning had been that bots change input on a regular cadence while
people hold directions much longer, so bot-measured accuracy was "probably a
floor". Measured:

| Predicting | Accuracy |
|---|---|
| a human | **89.9%** |
| the bot, same session | 93.7% |
| the bot, bot-vs-bot sessions | 91.5% to 92.4% |

Predicting the person was harder than predicting the bot, not easier, and lower
than every bot-versus-bot figure in the dataset. The bot plays a fixed move list
with held inputs by construction; the person mashed, changed direction
erratically, and left gaps.

Treat it as one sample: one player, one session, one game, with video recording
on. It is enough to retire the "floor" claim and not enough to replace it with
anything. Reproducing it across several players is the obvious next experiment,
and `ops/scripts/region-run.sh` plus `just play` make it cheap.

Getting there also exposed three real defects, each documented in
[12 - Troubleshooting](12-troubleshooting.md): the remote peer waited only 120
seconds for a handshake, `just play` passed none of the configuration the
handshake checks, and a refused handshake killed the host. All three were
invisible while every session was launched by one script.

### 6. Perception

Still open, and now the only thing rollback exists for that this lab cannot
measure. The logs can say a correction was 8 frames deep. Nothing here says
whether the player noticed it, or would have preferred 100 ms of input delay
instead. Answering that needs several players, blind comparisons, and a
protocol this repository does not have.

### 7. Street Fighter Alpha 3

Blocked by an incomplete romset: the available set is missing `sfa3.key`, the
CPS-2 decryption key, and none of FBNeo's eleven SFA3 variants does without one.
See [09](09-the-last-blade-2.md).

The libretro path is validated, with The Last Blade 2 in its place.

### 8. One run per profile

There is no confidence interval on any number in this repository. Each cell in
each table is one sample. For spread, vary the seed:

```bash
for s in 1 2 3 4 5; do SEED=$s DURATION=60 ./ops/scripts/bench.sh; done
```

## Next steps, by value

1. **A human on P1.** The only gap no metric here can close, and the question
   rollback exists to answer. It matters more now than it did: the tuning sweep
   measures input lag and correction depth separately but cannot say what they
   are worth against each other, and that exchange rate is the whole point.
2. **A long session on a genuinely bad link**, mobile or congested, to see burst
   loss. Distance turned out not to supply it - the intercontinental links were
   slow but almost perfectly clean. It remains the one case where the
   eight-input redundancy could genuinely fail.
3. **A tuned configuration against a real long link.** The sweep was run on a
   loopback profile calibrated to the measured 267 ms. Confirming that
   `prediction_limit=16` behaves the same against the real São Paulo route
   would close the last gap between the model and the thing it models.
4. **Spread.** Vary the seed and aggregate, so the tables carry confidence
   intervals rather than single samples.
5. **SFA3**, if a set with `sfa3.key` turns up.
