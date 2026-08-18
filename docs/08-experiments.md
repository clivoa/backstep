# 08 - Experiments

## The question

What happens to a 60 Hz fighting game under rollback as the network gets worse?
Specifically: how much extra CPU it costs, how often the player sees a
correction, and where the simulation starts to stall.

## Method

Five network profiles, fixed seed (4242), 1 frame of input delay, a prediction
limit of 8 frames, and a 16-state buffer. A sixth profile was added later, for
an experiment those five could not reach.

| Profile | Delay | Jitter | Loss | Reordering | Measured RTT |
|---|---|---|---|---|---|
| `natural` | - | - | - | - | 16.6 ms |
| `delay20` | 20 ms | - | - | - | 70 ms |
| `jitter30` | 30 ms | ±15 ms | - | - | 84-88 ms |
| `loss2` | - | - | 2% | - | 27 ms |
| `combined` | 40 ms | ±20 ms | 2% | 0.5% | 97-105 ms |
| `transcontinental` | 133 ms | ±5 ms | - | - | 267 ms |

`transcontinental` is not one of the five and does not appear in `just bench`.
It was added afterwards to reproduce the measured Madrid-São Paulo and
Madrid-Tokyo link on loopback, and it is the only profile whose one-way delay
exceeds the default prediction window. See
[the tuning sweep](#fixing-a-link-that-is-too-long-the-tuning-sweep).

What each profile imitates, what it isolates, and what jitter even is:
[00 - Glossary: the network profiles](00-glossary.md#the-network-profiles).

Briefly: `natural` is the control, `delay20` isolates distance, `jitter30`
isolates the *variation* in latency, `loss2` isolates loss to test the
redundancy, and `combined` is the worst case the lab sets out to survive.

Impairment is applied to each peer's **outgoing** datagrams, so the observed RTT
is roughly twice the configured one-way delay. Deliberate, and explained in
[03 - Protocol](03-protocol.md).

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
| natural | P1 | 60.01 | 1 | 1.00 | 1 | - | 0.01% | 0 | 179 | no | 16.7 ms | 0.5 ms | 0.00% | 34.9 kbit/s | 2.0 s |
| natural | P2 | 60.01 | 1 | 1.00 | 1 | - | 0.01% | 0 | 180 | no | 16.7 ms | 0.4 ms | 0.00% | 34.9 kbit/s | 2.2 s |
| delay20 | P1 | 60.01 | 1 | 1.00 | 1 | - | 0.01% | 0 | 179 | no | 77.4 ms | 11.2 ms | 0.00% | 34.9 kbit/s | 2.2 s |
| delay20 | P2 | 60.01 | 727 | 4.00 | 5 | 93.3% | 26.9% | 0 | 180 | no | 83.4 ms | 0.5 ms | 0.00% | 34.9 kbit/s | 2.2 s |
| jitter30 | P1 | 60.01 | 1 | 1.00 | 1 | - | 0.01% | 0 | 179 | no | 84.0 ms | 19.2 ms | 0.00% | 34.9 kbit/s | 2.1 s |
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
| natural | P1 | 60.01 | 0 | 0.00 | 0 | - | 0.0% | 0 | 240 | no | 16.6 ms | 0.00% | 85.6 s |
| natural | P2 | 60.01 | 0 | 0.00 | 0 | - | 0.0% | 0 | 240 | no | 15.7 ms | 0.00% | 85.1 s |
| delay20 | P1 | 60.00 | 0 | 0.00 | 0 | - | 0.0% | 0 | 240 | no | 69.9 ms | 0.00% | 69.3 s |
| delay20 | P2 | 59.85 | 1006 | 6.53 | 7 | 93.0% | 45.7% | 37 | 240 | no | 70.1 ms | 0.00% | 88.8 s |
| jitter30 | P1 | 60.00 | 0 | 0.00 | 0 | - | 0.0% | 0 | 240 | no | 88.5 ms | 0.00% | 73.4 s |
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
gap ([13 - Coverage](13-coverage.md)). 449 checksum comparisons agreed between
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
See [10 - Costs](10-costs.md).

## Three continents: where the prediction window runs out

Frankfurt answers "does this work over the internet". It cannot answer "what
happens when the opponent is far away", because 50 ms is close. So the same two
simulations ran again from the same desk in Madrid against **São Paulo**
(`sa-east-1`) and **Tokyo** (`ap-northeast-1`).

Distance is the only variable. Same binaries, same seed, same `natural` profile,
same 8-frame prediction limit, same 300 s for the game and 150 s for the arena.

```bash
./ops/scripts/region-run.sh eu-central-1   frankfurt /path/lastbld2.zip
./ops/scripts/region-run.sh sa-east-1      saopaulo  /path/lastbld2.zip
./ops/scripts/region-run.sh ap-northeast-1 tokyo     /path/lastbld2.zip
```

| Region | Sim | Peer | SRTT | FPS | Rollbacks | Max depth | Stalls | Accuracy | Checksums | Desync |
|---|---|---|---|---|---|---|---|---|---|---|
| Frankfurt | arena | P1 | 50.2 ms | 60.01 | 5 | 1 | 0 | 95.5% | 149 | no |
| Frankfurt | arena | P2 | 50.0 ms | 60.01 | 590 | 3 | 0 | 93.4% | 150 | no |
| Frankfurt | lastblade2 | P1 | 49.8 ms | 60.01 | 919 | 3 | 0 | 92.4% | 300 | no |
| Frankfurt | lastblade2 | P2 | 50.9 ms | 60.00 | 188 | 2 | 0 | 91.5% | 300 | no |
| São Paulo | arena | P1 | 267.9 ms | 59.60 | 634 | **8** | 63 | 92.9% | 149 | no |
| São Paulo | arena | P2 | 266.7 ms | 59.54 | 642 | **8** | 70 | 92.9% | 150 | no |
| São Paulo | lastblade2 | P1 | 271.6 ms | **57.37** | 1 316 | **8** | 827 | 92.7% | 300 | no |
| São Paulo | lastblade2 | P2 | 271.1 ms | **57.35** | 1 345 | **8** | 443 | 92.4% | 300 | no |
| Tokyo | arena | P1 | 268.2 ms | 59.56 | 658 | **8** | 68 | 92.7% | 149 | no |
| Tokyo | arena | P2 | 266.6 ms | 59.51 | 658 | **8** | 75 | 92.7% | 150 | no |
| Tokyo | lastblade2 | P1 | 267.3 ms | **56.01** | 1 315 | **8** | 1 283 | 92.7% | 300 | no |
| Tokyo | lastblade2 | P2 | 266.9 ms | **55.99** | 1 366 | **8** | 437 | 92.2% | 300 | no |

2 697 checksums compared across three continents. Zero desyncs.

Loss was 0.00% everywhere except Tokyo, which lost 0.02% on the arena and 0.03%
on the game: a handful of datagrams in nine thousand, all covered by the
eight-input redundancy.

### Madrid to São Paulo and Madrid to Tokyo are the same distance

8 500 km westward and 9 700 km eastward both measured **267 ms**. The two
regions produced results so close that they read as a repeat of one experiment,
which is what makes them useful: the same behaviour twice, from opposite
directions, on unrelated undersea cable.

Latency follows the cable route and the number of hops, not the great-circle
distance. Madrid has direct fibre to both.

### At 267 ms the window is simply too small

The prediction limit is 8 frames. At 60 Hz that is 133 ms - **exactly one way**
at this distance. An input cannot make the round trip before the window fills,
so the session hits the limit on essentially every frame:

- max depth pinned at **8** in all eight long-distance runs, against 1-3 at
  Frankfurt;
- stalls appear for the first time: 63-1 283, against **zero** at Frankfurt;
- effective FPS drops below 60 for the first time anywhere in this project.

This is not a failure. It is the design working as specified: the session
refuses to speculate further than it can undo, and waits. The player sees a
brief freeze instead of a desync. But it does mean the default configuration is
tuned for a continent, not a planet.

### The heavier simulation degrades first

Same link, same distance, two simulations:

| | Arena (204 B) | The Last Blade 2 (415 KB) |
|---|---|---|
| FPS, São Paulo | 59.60 | 57.37 |
| FPS, Tokyo | 59.56 | 56.01 |
| Stalls, Tokyo P1 | 68 | 1 283 |

The arena holds 59.5 fps and stalls 68 times. The game drops to 56 and stalls
1 283 times over the same route. The network is identical; what differs is that
recovering from a stall means re-simulating up to eight frames, and eight frames
of The Last Blade 2 is eight `retro_serialize` calls over 415 KB.

**Latency and state size compound.** Each is survivable alone: Frankfurt showed
a large state at short distance is free, and the arena showed long distance with
a small state costs almost nothing. Together they are the only configuration
here that misses 60 Hz.

### The asymmetry disappears with distance

At Frankfurt the split is stark: 919 rollbacks against 188 on the game, 590
against 5 on the arena. At São Paulo and Tokyo the peers land within 4% of each
other - 1 316 against 1 345, 1 315 against 1 366, 658 against 658.

The asymmetry comes from starting phase: whoever completes the handshake first
runs slightly ahead and does all the predicting ([above](#how-to-read-the-numbers)).
That advantage only survives while there is slack in the window. Once both peers
are saturated, both stall, and stalling is what re-couples their clocks. The
lead is erased by the same mechanism that limits the damage.

So the asymmetry is a **short-link phenomenon**. On a bad connection the cost is
shared, whether you want it shared or not.

### Prediction accuracy does not care about any of this

92.2% to 95.5%, across 50 ms and 267 ms, two simulations, three continents.

The one number that has stayed flat through every experiment in this project is
the one rollback's entire premise rests on. Distance changes how *often* you
must correct and how *deep* the correction goes. It does not change how often
the guess was right, because the guess depends on the player's hands, not on the
network.

## Fixing a link that is too long: the tuning sweep

The three-region runs prove the 8-frame window saturates at 267 ms. They do not
show how to fix it, and finding out by trial would mean another instance-hour on
another continent per configuration.

So the measured link was brought home. The `transcontinental` profile is
133 ms one-way with ±5 ms of jitter, which reproduces the 267 ms round trip and
the few milliseconds of variance seen from both São Paulo and Tokyo:

```bash
./ops/scripts/tuning-sweep.sh              # arena, four configurations, 90 s each
DURATION=180 ./ops/scripts/tuning-sweep.sh
```

Arena rather than the emulated game, deliberately. Its state is 204 bytes, so
re-simulation is nearly free and what the numbers show is the **algorithm**
saturating rather than the CPU running out. The Last Blade 2 suffers both at
once, which is realistic but confounds the two causes.

Results are given for the peer that runs *ahead* - the one that does the
predicting, and therefore the one the tuning has to protect:

| Configuration | Input delay | Limit | History | FPS | Stalls | Rollbacks | Max depth | Re-simulation | Accuracy | Input lag |
|---|---|---|---|---|---|---|---|---|---|---|
| baseline | 1 | 8 | 16 | 57.16 | 269 | 394 | 8 | 0.55× | 92.7% | 17 ms |
| wide-window | 1 | 20 | 28 | 60.02 | 0 | 362 | 18 | 1.17× | 93.3% | 17 ms |
| both | 6 | 16 | 24 | 60.02 | 0 | 365 | 13 | 0.84× | 93.2% | 100 ms |
| input-delay-8 | 8 | 8 | 16 | 59.99 | 3 | 363 | 8 | 0.50× | 93.3% | 133 ms |

"Re-simulation" is frames the CPU ran that the player never saw, as a multiple of
the frames they did see. "Input lag" is `input_delay` converted to milliseconds
at 60 Hz - what the player actually feels.

**The baseline reproduces the cloud result.** 57.16 fps and 269 stalls on
loopback, against 57.35 and 443 measured from São Paulo. Close enough that the
profile can stand in for the region.

**Widening the window removes every stall, and the player pays nothing.** FPS
back to 60.02, stalls to zero, input lag unchanged at 17 ms. It looks free.

It is not free. Depth rises from 8 to 18 and re-simulation rises to **1.17×**:
the peer now simulates more than twice the frames it displays. On a 204-byte
arena that is affordable. On a 415 KB state, where `save_state` alone costs
2.27 ms of a 16.7 ms frame, 1.17× re-simulation does not fit - which is exactly
why The Last Blade 2 fell to 56 fps at Tokyo while the arena held 59.5.

**Buying the same result with input delay costs the player instead.** Eight
frames of delay also removes the stalls, but caps depth at 8 and halves the
re-simulation to 0.50×. The CPU is comfortable. The price is 133 ms of input
lag, which in a fighting game is roughly the difference between a reactable
move and an unreactable one.

**The middle is the honest answer.** `both` - 6 frames of delay with a 16-frame
window - holds 60 fps with no stalls, at 0.84× re-simulation and 100 ms of lag.

The trade is not between "good" and "bad" configurations. It is a choice of
**who pays for the distance**: the CPU, through deeper speculation and more
re-simulation, or the player, through input lag. The window sets the split.

There is no configuration that makes 267 ms feel like 50 ms, and it is worth
being clear that none of these is a fix in that sense. What tuning buys is a
smooth 60 Hz with honest input lag, instead of 56 Hz with unpredictable freezes.
The second is worse to play even though its average latency is lower.

### A deadlock this found that the cloud runs could not

The sweep did not run the first time. Both peers connected, both filled the
prediction window, both stalled, and both sat at `depth=8` forever.

The frame loop skipped `transport.pump()` on the stalled path, on the reasoning
that a stalled peer should "do no local work at all". But `pump` is what moves
the network emulator's delay queue onto the socket. A datagram already handed to
`send` is **in flight**, and a real network delivers it whether or not the
sender sends anything more. Modelling flight as a queue that only advances when
the sender acts is fine until the sender stops - and the sender stops precisely
when it is starving for what is in that queue. Each peer held the other's
inputs hostage.

It needs one-way delay to exceed the prediction window before it can bite, which
is why no profile up to `combined` (40 ms against a 133 ms window) ever showed
it in eighteen months of benchmarks. And the AWS runs could not have found it:
there the delay is the real network, which needs no pumping.

Fixed by pumping on the stalled path too, and guarded by
`two_peers_that_stall_at_once_still_deliver_what_is_already_in_flight`, which
deadlocks against the old code and passes against the new.

A synthetic model found a bug that the real thing structurally could not. That
is the argument for keeping both.

## The numbers here come from un-recorded sessions

Worth stating outright, because it is easy to conflate: the videos in
[14 - Video](14-video.md) are not the source of these numbers.

Recording costs about 65% of a core per peer, and that shifts the measurement.
Under `natural` on loopback, RTT p50 rises from **16.6 ms without recording to
38.0 ms with**. That is not the network; it is the frame loop slowing down on a
busy machine.

The videos illustrate the behaviour faithfully. The numbers come from here.

## What these experiments do not measure

> First and foremost, for the loopback tables: one machine, two processes. No
> number in this document's loopback sections came from two different computers.
> The full inventory of what was and was not validated is in
> [13 - Coverage](13-coverage.md).

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

```bash
# the tuning sweep on a different link, or with your own configurations
PROFILE=combined ./ops/scripts/tuning-sweep.sh
CONFIGS="a:0:8:16 b:2:12:20 c:4:16:24" ./ops/scripts/tuning-sweep.sh
```

The question this lab is set up to answer and has only half answered: **which
input delay minimises the sum of perceived latency and visible corrections, for
a given network profile?** The sweep above measures both halves of that sum
separately - input lag in milliseconds, corrections as rollback depth and
re-simulation ratio - for one link and four configurations. What it cannot
supply is the exchange rate between them, because that is a question about
perception, and nobody has played a session on this lab yet.

Every number needed for the measurable half is in `summary.csv`, and
[15 - Elastic](15-elastic.md) has the per-event detail to go with it.
