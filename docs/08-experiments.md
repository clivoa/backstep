# 08 — Experiments

## The question

What happens to a 60 Hz fighting game under rollback as the network gets worse?
Specifically: how much extra CPU it costs, how often the player sees a
correction, and where the simulation starts to stall.

## Method

Five network profiles, fixed seed (4242), 1 frame of input delay, a prediction
limit of 8 frames, and a 16-state buffer.

| Profile | Delay | Jitter | Loss | Reordering | Measured RTT |
|---|---|---|---|---|---|
| `natural` | — | — | — | — | 16.6 ms |
| `delay20` | 20 ms | — | — | — | 70 ms |
| `jitter30` | 30 ms | ±15 ms | — | — | 84–88 ms |
| `loss2` | — | — | 2% | — | 27 ms |
| `combined` | 40 ms | ±20 ms | 2% | 0.5% | 97–105 ms |

What each profile imitates, what it isolates, and what jitter even is:
[00 — Glossary: the five profiles](00-glossary.md#the-five-network-profiles).

Briefly: `natural` is the control, `delay20` isolates distance, `jitter30`
isolates the *variation* in latency, `loss2` isolates loss to test the
redundancy, and `combined` is the worst case the lab sets out to survive.

Impairment is applied to each peer's **outgoing** datagrams, so the observed RTT
is roughly twice the configured one-way delay. Deliberate, and explained in
[03 — Protocol](03-protocol.md).

```bash
just bench                  # five profiles, 180 s each, ~15 min
just bench 30               # the quick version
```

Both peers are bots with a fixed seed, and the network emulator is seeded too,
so the only thing that varies between runs is the real network underneath, which
on a local bench is close to nothing.

## How to read the numbers

**Presented frames differ between peers** by construction: each side draws its
own present. What has to match are the checksums of confirmed frames.

**Rollbacks are asymmetric.** Whoever is a few milliseconds ahead corrects more,
because their predictions cover a wider window. Different numbers on P1 and P2
are expected; what cannot differ is the confirmed state.

**Loss is inferred** from sequence gaps. A delayed datagram counts as lost until
it arrives.

**There is no one-way latency.** Only RTT.

## The arena

180 s per profile, seed 4242, two bots on loopback, commit `ee5ca9422d88`.
Reproduce with `just bench 180 arena`; these particular logs were later
overwritten, see [13](13-coverage.md).

| Profile | Peer | FPS | Rollbacks | Mean depth | Max | Accuracy | Extra work | Stalls | Checksums | Desync | RTT | RTT var | Loss | Bitrate | CPU |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| natural | P1 | 60.01 | 1 | 1.00 | 1 | — | 0.01% | 0 | 179 | no | 16.7 ms | 0.5 ms | 0.00% | 34.9 kbit/s | 2.0 s |
| natural | P2 | 60.01 | 1 | 1.00 | 1 | — | 0.01% | 0 | 180 | no | 16.7 ms | 0.4 ms | 0.00% | 34.9 kbit/s | 2.2 s |
| delay20 | P1 | 60.01 | 1 | 1.00 | 1 | — | 0.01% | 0 | 179 | no | 77.4 ms | 11.2 ms | 0.00% | 34.9 kbit/s | 2.2 s |
| delay20 | P2 | 60.01 | 727 | 4.00 | 5 | 93.3% | 26.9% | 0 | 180 | no | 83.4 ms | 0.5 ms | 0.00% | 34.9 kbit/s | 2.2 s |
| jitter30 | P1 | 60.01 | 1 | 1.00 | 1 | — | 0.01% | 0 | 179 | no | 84.0 ms | 19.2 ms | 0.00% | 34.9 kbit/s | 2.1 s |
| jitter30 | P2 | 60.01 | 683 | 4.25 | 5 | 93.7% | 26.9% | 0 | 180 | no | 86.9 ms | 17.9 ms | 0.00% | 34.9 kbit/s | 2.1 s |
| loss2 | P1 | 60.01 | 1 | 1.00 | 1 | 87.5% | 0.01% | 0 | 176 | no | 16.5 ms | 0.5 ms | 1.89% | 34.9 kbit/s | 2.0 s |
| loss2 | P2 | 60.01 | 12 | 1.00 | 1 | 94.2% | 0.11% | 0 | 177 | no | 16.7 ms | 0.4 ms | 1.89% | 34.9 kbit/s | 2.1 s |
| combined | P1 | 60.01 | 122 | 1.00 | 1 | 93.3% | 1.1% | 0 | 177 | no | 99.3 ms | 19.5 ms | 1.95% | 34.9 kbit/s | 2.0 s |
| combined | P2 | 60.01 | 700 | 4.83 | 6 | 93.5% | 31.3% | 1 | 178 | no | 102.7 ms | 19.8 ms | 1.95% | 34.9 kbit/s | 2.0 s |

No desyncs across 1 800 seconds of session and 1 786 checksum comparisons.

## The same engine, on a real emulator

The arena measures the rollback engine. It does not measure what happens when
the simulation is opaque and the state is large. So the same five profiles ran
with **The Last Blade 2** under FBNeo: same `RollbackSession`, same protocol,
same runner, only the `Simulation` implementation changes.

```bash
just e2e 240 lastblade2 /path/lastbld2.zip
```

### The difference that matters: state size

| | Arena | The Last Blade 2 |
|---|---|---|
| `state_bytes` | 204 | 415 155 |
| ratio | 1× | 2 036× |
| CPU per 240 s session | ~2 s | ~90 s |
| fraction of a core | ~1% | ~38% |

That is the number the arena could not show. Saving state in the arena is
copying 204 bytes. In the emulator it is a `retro_serialize` of 405 KB, and
rollback does that **once per frame** plus once per re-simulated frame.

`LibretroSimulation` caches the last snapshot's checksum in a `Cell` for exactly
this reason. Without it, the `save_state` + `checksum` pair the session performs
every frame would cost two `retro_serialize` calls instead of one. At 60 Hz with
405 KB that is the difference between fitting in the budget and not.

### The reference run

240 s per profile, seed 4242, both bots playing the full move list (chain
combos, motion inputs, high and low guard, repel, throw), real fights with
complete rounds.

| Profile | Peer | FPS | Rollbacks | Mean depth | Max | Accuracy | Extra work | Stalls | Checksums | Desync | RTT | Loss | CPU |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| natural | P1 | 60.01 | 0 | 0.00 | 0 | — | 0.0% | 0 | 240 | no | 16.6 ms | 0.00% | 85.6 s |
| natural | P2 | 60.01 | 0 | 0.00 | 0 | — | 0.0% | 0 | 240 | no | 15.7 ms | 0.00% | 85.1 s |
| delay20 | P1 | 60.00 | 0 | 0.00 | 0 | — | 0.0% | 0 | 240 | no | 69.9 ms | 0.00% | 69.3 s |
| delay20 | P2 | 59.85 | 1006 | 6.53 | 7 | 93.0% | 45.7% | 37 | 240 | no | 70.1 ms | 0.00% | 88.8 s |
| jitter30 | P1 | 60.00 | 0 | 0.00 | 0 | — | 0.0% | 0 | 240 | no | 88.5 ms | 0.00% | 73.4 s |
| jitter30 | P2 | 59.85 | 1006 | 6.90 | 8 | 93.0% | 48.2% | 37 | 240 | no | 84.2 ms | 0.00% | 93.7 s |
| loss2 | P1 | 60.01 | 19 | 1.00 | 1 | 93.0% | 0.1% | 0 | 237 | no | 28.2 ms | 1.88% | 88.0 s |
| loss2 | P2 | 60.01 | 4 | 1.00 | 1 | 95.1% | 0.0% | 0 | 237 | no | 27.5 ms | 1.88% | 87.7 s |
| combined | P1 | 60.00 | 125 | 1.11 | 2 | 93.1% | 1.0% | 0 | 239 | no | 97.6 ms | 2.00% | 78.0 s |
| combined | P2 | 59.85 | 1004 | 4.84 | 7 | 93.0% | 33.7% | 39 | 236 | no | 105.6 ms | 2.00% | 92.1 s |

Twenty minutes of emulated fighting under rollback, 2 389 checksums compared,
zero desyncs.

### What changes against the arena

**60 Hz survives 48% extra work.** Under `jitter30` the busy peer re-simulated
almost half an extra frame per frame, on top of a 405 KB state, and still
delivered 59.85 fps. The cost lands on CPU (93.7 s for a 240 s session, about
39% of a core) rather than on frame rate.

**Depth reaches the limit.** In the arena the deepest correction was 6 against a
limit of 8. Here it was 8, with a mean of 6.9 and 37 stalls. The larger state
makes `save_state` and `load_state` more expensive, the leading peer gains more
phase, and the prediction window fills. The sizing is being exercised for real
rather than with comfortable headroom.

**Prediction accuracy did not move: 93.0%.** This is the result most worth
looking at. The arena, with simple bots, gave 93.5%. A real fighting game, with
a bot doing three-hit chain combos, quarter-circles with four directions in
twelve frames, and guard held for nearly a second, gives 93.0%.

"Repeat the last confirmed input" is not clever. It works because fighting-game
inputs are **held**, and that is as true of a toy arena as of The Last Blade 2.
It is rollback's central hypothesis, measured twice on simulations that share
nothing but the genre.

**Loss stays nearly free.** `loss2` produced 19 and 4 rollbacks in 240 s against
`delay20`'s 1 006. The eight-input redundancy delivers the lost input in the
next datagram, well before it is needed. Loss and latency are different
problems, and rollback is only sensitive to the second, exactly as in the arena,
now with an emulator in the middle.

## The real session: Madrid to Frankfurt

Everything above is loopback with synthetic impairment. This section is the
internet.

| | P1 | P2 |
|---|---|---|
| Where | Madrid, Spain | Frankfurt, `eu-central-1` |
| Machine | Arch Linux, Intel Core i7-10750H | Ubuntu 24.04, EC2 `t3.small` |
| Role | dials | listens on UDP/7000 |

Profile `natural`, with no synthetic impairment. Injecting delay on top of a
real link would only muddy the measurement; the point here is to measure what is
there.

### The Last Blade 2, 300 seconds

| Metric | P1 (Madrid) | P2 (Frankfurt) |
|---|---|---|
| `effective_fps` | 60.01 | 60.01 |
| Rollbacks | 1 280 | 31 |
| Mean depth | 2.04 | 1.03 |
| Max depth | 4 | 2 |
| Prediction accuracy | 92.9% | 92.4% |
| Extra work | 14.5% | 0.2% |
| Stalls | 0 | 0 |
| Checksums compared | 300 | 300 |
| Desync | no | no |
| SRTT | 49.9 ms | 51.6 ms |
| RTT variation | 0.37 ms | 3.22 ms |
| Loss | 0.000% | 0.000% |
| CPU | 116 s | 46 s |

### The arena, 150 seconds

Run immediately afterwards over the same link, changing only the simulation.

| Metric | P1 (Madrid) | P2 (Frankfurt) |
|---|---|---|
| Rollbacks | 19 | 601 |
| Mean / max depth | 1.00 / 1 | 2.02 / 3 |
| Prediction accuracy | 91.7% | 93.3% |
| Checksums compared | 149 | 150 |
| Desync | no | no |
| SRTT | 51.8 ms | 50.0 ms |
| CPU | 1.95 s | 1.54 s |

### A third session, recorded

Run later over the same route to produce the footage in [14](14-video.md), with
both peers recording. 150 s, The Last Blade 2, profile `natural`.

| Metric | P1 (Madrid) | P2 (Frankfurt) |
|---|---|---|
| Rollbacks | 544 | 13 |
| Max depth | **8** | 2 |
| Prediction accuracy | 93.0% | 99.0% |
| Stalls | 133 | 0 |
| Checksums compared | 150 | 150 |
| Desync | no | no |
| SRTT | 33.9 ms | 37.8 ms |

Two things differ from the un-recorded run, and both have the same cause. The
route was better on this occasion (34 ms rather than 50), and yet depth reached
the prediction limit of 8 with 133 stalls, where the earlier session peaked at 4
with none. The local peer was encoding video at the same time, which slows the
frame loop and widens the phase difference.

That is the recording caveat below, visible in a table. This run is good
footage; the un-recorded one is the measurement.

Its logs are the ones still in `artifacts/logs`; see [13](13-coverage.md) for
what survives on disk.

### What that proves that loopback could not

**Determinism across different machines.** This was the project's most serious
gap ([13 — Coverage](13-coverage.md)). 449 checksum comparisons agreed between
an Arch desktop with an i7-10750H and an Ubuntu 24.04 EC2 instance: different
CPU, different OS, different libc. No desyncs.

The arena matters separately here. It is the code **we** wrote, and Q23.8 fixed
point, no `HashMap`, the hand-rolled FNV-1a and `overflow-checks` in release all
exist for this scenario. Two processes of one binary on one CPU would have
agreed even if every one of those rules were wrong. They would not have now.

**The synthetic profiles were pessimistic, in every dimension.**

| | Real Madrid↔Frankfurt | `delay20` | `jitter30` | `loss2` |
|---|---|---|---|---|
| RTT | 50 ms | 70 ms | 86 ms | 27 ms |
| RTT variation | 0.37 ms | 0.5 ms | ~18 ms | 0.5 ms |
| Loss | 0.000% | 0% | 0% | 1.88% |

Zero datagrams lost out of **18 602 sent** over five minutes. And an RTT
variation of 0.37 ms means fibre between European cities is far steadier than
`jitter30` assumes.

Erring pessimistic is the right direction to err, but it is worth recording that
`jitter30` and `loss2` describe bad Wi-Fi rather than a link between
datacentres.

**Prediction lands at ~93% again.** Third independent measurement: arena on
loopback 93.5%, The Last Blade 2 on loopback 93.0%, The Last Blade 2 over the
real internet 92.9%. Rollback's central hypothesis, that fighting-game inputs
are held, depends on neither the game nor the network.

**Real depth is far shallower than simulated depth.** At comparable RTT, 50 ms
real against 70 ms under `delay20`, mean depth fell from 6.53 to 2.04, the
maximum from 7 to 4, and stalls from 37 to zero.

Two reasons, and the second is a lesson about method:

- the real link's RTT variation is about 50× lower;
- on loopback the **two peers shared one CPU**, and scheduling contention adds
  phase drift that does not exist when each peer has its own machine.

Part of the cost the loopback experiments measured was the lab, not the
rollback.

**The asymmetry is real, and it swapped sides between runs.** On The Last Blade
2, Madrid paid 1 280 rollbacks against Frankfurt's 31. On the arena, fifteen
minutes later over the same link, **Frankfurt paid 601 against Madrid's 19**.

Nothing about the network changed. What changed was who completed the handshake
first. It is as clean a demonstration as you could ask for that "how many
rollbacks does my client do" measures starting phase, not connection quality.

**The CPU cost holds up off the bench.** 116 s of CPU for a 300 s session in
Madrid, about 39% of a core, matching loopback exactly, against 46 s in
Frankfurt, which did almost no rollback work. The peer that pays the asymmetry
pays it in CPU too.

### Reproducing it

```bash
just check-determinism /path/lastbld2.zip     # before spending anything
just aws-up lastblade2 /path/lastbld2.zip
# in another terminal, the local peer:
export ROLLBACK_SESSION_KEY=$(cat artifacts/session.key)
./target/release/rollback-bot --sim lastblade2 --player p1 \
  --peer "$(terraform -chdir=terraform output -raw peer_address)" --bind 0.0.0.0:0 \
  --profile natural --seed 4242 --duration 300 --mode play \
  --core cores/fbneo_libretro.so --rom /path/lastbld2.zip \
  --system-dir artifacts/system --log-dir artifacts/logs
just collect      # ALWAYS first
just aws-down
```

Both sessions together, including bring-up and teardown, cost under US$ 0.05.
See [10 — Costs](10-costs.md).

## The numbers here come from un-recorded sessions

Worth stating outright, because it is easy to conflate: the videos in
[14 — Video](14-video.md) are not the source of these numbers.

Recording costs about 65% of a core per peer, and that shifts the measurement.
Under `natural` on loopback, RTT p50 rises from **16.6 ms without recording to
38.0 ms with**. That is not the network; it is the frame loop slowing down on a
busy machine.

The videos illustrate the behaviour faithfully. The numbers come from here.

## What these experiments do not measure

> First and foremost, for the loopback tables: one machine, two processes. No
> number in this document's loopback sections came from two different computers.
> The full inventory of what was and was not validated is in
> [13 — Coverage](13-coverage.md).

**Perception.** Nothing here says whether the game *feels* good. A depth-2
rollback is invisible; a depth-8 one during a trade is quite noticeable, and a
mean cannot tell them apart. Nor has anyone played a session with a human on P1.

**Bots do not play like people.** They change input on a regular cadence. A
human holds directions much longer, which makes prediction easier, so the
accuracy measured here is probably a **floor**.

**Loopback is not the internet**, and neither is the synthetic model. Real loss
comes in bursts, and bursts are the worst case for an eight-input redundancy
window. The real link lost nothing at all, so that hypothesis remains untested.

**One run per profile.** No confidence intervals. For spread, vary the seed and
aggregate.

## Natural extensions

```bash
# vary input delay: what does it buy in accuracy?
for d in 0 1 2 3; do
  DURATION=60 SEED=4242 PROFILES=combined \
    ./ops/scripts/bench.sh --input-delay $d
done

# vary the seed, for dispersion
for s in 1 2 3 4 5; do SEED=$s DURATION=60 ./ops/scripts/bench.sh; done

# the same, with the emulated game
just bench 180 lastblade2 /path/lastbld2.zip
```

The most interesting question this lab is set up to answer and has not:
**which input delay minimises the sum of perceived latency and visible
corrections, for a given network profile?** Every number needed is already in
`summary.csv`, and [15 — Elastic](15-elastic.md) has the per-event detail to go
with it.
