# 00 — Glossary

Every technical term this repository uses, explained from scratch, with the
number this lab actually uses and the reason it exists.

If you are starting here, read the first three sections in order. The rest works
as reference.

**Contents**

- [The core idea](#the-core-idea)
- [Rollback: the pieces](#rollback-the-pieces)
- [Networks: what breaks the conversation](#networks-what-breaks-the-conversation)
- [The five network profiles](#the-five-network-profiles)
- [The metrics this lab reports](#the-metrics-this-lab-reports)
- [Emulation and libretro](#emulation-and-libretro)
- [Determinism](#determinism)
- [Protocol and security](#protocol-and-security)
- [Infrastructure](#infrastructure)

## The core idea

### Frame

One step of simulation. This lab runs at **60 frames per second**, so one frame
every **16.7 ms**. That is the budget: everything that happens in a frame, read
the controller, send it, simulate, draw, has to fit in 16.7 ms or the game
stutters.

The frame is also the unit of everything else. Inputs are stamped by frame,
states are saved by frame, checksums are compared by frame.

### Input

What the player is holding **on that frame**. Here it is a single `u16`: sixteen
bits, one per button. It fits in a register, copies without allocating, and is
the unit that crosses the network.

### Simulation

The function that takes the current state plus both players' inputs and produces
the next state. In this project it is a trait with four methods: `save_state`,
`load_state`, `advance_frame`, `checksum`.

Two implementations: the **arena**, a tiny 2D fighting game written here, and
**libretro**, an entire arcade emulator. The rollback engine cannot tell the
difference, which is the point of the lab.

### Netcode

The set of decisions about *how two computers keep the same game in sync over a
network*. Not "network code" in the sockets sense: it is the policy for what to
do when information arrives late. Three families:

| Family | How it handles delay | Price |
|---|---|---|
| **Lockstep** | waits for the opponent's input before simulating | the game hitches when the network does |
| **Delay-based** | adds fixed delay to local input to hide the network | your own commands respond late, always |
| **Rollback** | guesses the opponent's input and corrects afterwards | extra CPU, and occasional visible corrections |

## Rollback: the pieces

### Rollback

In one sentence: **do not wait for the opponent. Guess what they did, keep
playing, and go back to fix it when the real input arrives.**

The name comes from the going back. On learning the guess for frame 100 was
wrong, the game restores the saved state from frame 100 and re-simulates 100,
101, 102 and onward to the present, this time with the correct input. All of
that inside a single display frame, with the player seeing none of it.

The gain is that **your own commands respond immediately**. The price is CPU and
the occasional visible correction.

### Prediction

The guess about what the opponent did on a frame whose input has not arrived.

The rule here is the simplest available: **repeat their last confirmed input**.
Not naivety. Fighting-game inputs are held for many frames; you walk forward
holding a direction, you crouch-block for a second at a time. Measured in this
lab: right about 93% of the frames it had to guess, in the arena, in The Last
Blade 2, and over the real internet.

### Confirmed frame

A frame for which **both** players' inputs have arrived. It is the past that
cannot change. Everything after it is speculation.

### Rollback depth

How many frames the game had to re-simulate in one correction. Depth 1 is
invisible; depth 8 in the middle of a trade is noticeable.

### Input delay

**Deliberate** delay between pressing a button and the game acting on it. The
default here is **1 frame** (16.7 ms).

It sounds counterproductive and it buys something. With one frame of delay your
input goes out a frame before it is needed, giving the opponent an extra window
to receive it. Each frame of input delay is one less frame of prediction; you
trade responsiveness for accuracy. Zero is possible and makes rollback work
harder.

### Prediction limit

How many frames the game is willing to guess before giving up and stopping. Here:
**8 frames**, about 133 ms.

It exists because re-simulation costs CPU. Correcting eight frames at once is
reasonable; correcting sixty would not fit in 16.7 ms.

### Stall

What happens when the prediction window fills: the simulation **stops** and
waits. The occasional stall is the limit doing its job. Continuous stalls mean
the peer stopped talking.

In the client's overlay a stall shows as a grey band.

### State history

How many saved states the engine keeps so it can go back. Here: **16**.

It has to be **strictly greater** than the prediction limit. If a rollback can
reach back 8 frames, the state from 8 frames ago has to still exist.
`SessionConfig::validate` refuses any configuration that violates that, and a
property test once produced exactly this error (`HistoryExhausted`) while
finding a real bug in the prediction bookkeeping.

### Snapshot, or savestate

A copy of the simulation's entire state, so it can be returned to.

Size is the most brutal difference between this lab's two simulations:

| | Arena | The Last Blade 2 |
|---|---|---|
| snapshot | **204 bytes** | **415 155 bytes** |

Saving 204 bytes is a negligible `memcpy`. Saving 405 KB **sixty times a
second**, plus once per re-simulated frame, is most of an emulated session's CPU
cost.

### Re-simulation

Running the frames between the rollback point and the present again. It is the
"extra work": if in one second the game presented 60 frames but re-simulated
another 30, the overhead is 50%.

### `OutputMode`

The distinction between "this frame goes to the screen" (`Present`) and "this
frame is re-simulation, throw the video and audio away" (`Resimulate`).

Without it, a depth-8 rollback would flash eight frames past the player and play
a burst of audio. The rule is absolute: **`OutputMode` may change output, never
state.** If it changed state, the two peers would diverge.

### Desync

The two machines stopped running the same game. From that point the two players
are watching different matches and nothing else means anything.

Rollback does **not** tolerate desync better than lockstep. It *depends* on
re-simulation reproducing exactly what would have happened.

### Checksum

A short number summarising the simulation's state, exchanged periodically
between peers to detect desync. Here it is 64-bit FNV-1a over the snapshot,
compared every **60 frames** and only on confirmed frames.

One detail that cost a lot of debugging: on the emulated core the checksum
**ignores the first 2 048 bytes** of the savestate, because FBNeo recomputes
rather than restores about 20 bytes of sound and timer bookkeeping. Without that
exclusion the detector reported a desync on the first rollback of every session.
Full reasoning in [05 — Determinism](05-determinism.md).

## Networks: what breaks the conversation

### Datagram

A UDP message. "Packet" in casual use. Each one here is at most **1 200 bytes**,
chosen to fit inside a typical internet MTU without fragmenting.

### UDP, and why not TCP

TCP guarantees delivery and order, and it does that by **waiting**. Lose a
packet and everything behind it sits in a queue until the retransmission arrives
(head-of-line blocking). For a 60 Hz game that is the worst possible behaviour:
an input from 200 ms ago is no longer interesting, and waiting for it freezes
the present.

UDP guarantees nothing and waits for nothing. The game would rather lose an
input than receive it late.

### Latency

How long a datagram takes to get from one side to the other. **One-way** is A to
B; **RTT** is there and back.

This lab reports **no one-way latency**, for an honest reason: measuring it
requires both clocks to be synchronised, and they are not. RTT is the only
latency measurement a peer can make without trusting someone else's clock.

### RTT (round-trip time)

There and back: I send something, you reply, how long did that take. The only
latency measurement a peer can make alone.

In the measurements, `natural` on loopback gives an RTT of **16.6 ms**. That is
not the network, it is the frame loop: the peer only replies on its next frame,
so 16.7 ms is the floor imposed by 60 Hz.

### SRTT and RTTVAR

Raw RTT bounces around. The protocol uses **RFC 6298** (the same formulas as
TCP) to smooth it:

- **SRTT** (smoothed RTT): a moving average.
- **RTTVAR** (RTT variation): how much RTT is moving, which is jitter from the
  measurer's point of view.

### Jitter

**Variation** in latency. Not the delay itself, but how much it changes from
packet to packet.

A link at a constant 50 ms is predictable and can be compensated for. A link
bouncing between 20 and 80 ms has the *same average* and is far worse, because
you never know when the next input arrives.

Where it comes from in practice: router queues filling and draining, Wi-Fi
contending for the medium, routes changing, CPU scheduling on the host.

Why jitter matters less to rollback than to lockstep:

- **Lockstep** has to wait for the **worst case** or it hitches. High jitter
  forces a wide margin, which is delay for everyone all the time.
- **Rollback** already corrects every frame. A datagram 15 ms late is fixed by
  the same mechanism that fixes one 20 ms late. It only needs the worst case to
  **fit inside the prediction window**.

Measured here: `jitter30` (30 ± 15 ms) and `delay20` (20 ms flat) produce
practically the same number of rollbacks, though RTTVAR rises from 0.5 ms to
about 18 ms.

The finer point, which only came out of per-event analysis: jitter does not
raise the *mean* depth, it **widens the distribution**, and the wider tail is
what eventually reaches the prediction limit. See
[15 — Elastic](15-elastic.md).

### Packet loss

Datagrams that simply do not arrive. A router with a full queue drops them;
Wi-Fi with interference loses them; a bad cable corrupts them and the network
checksum discards them.

Expressed as a percentage. 2% means one in fifty datagrams disappears.

**Burst loss** is the realistic case and the worst one: instead of one lost
packet in fifty, you lose five in a row and then none for a long time. The real
internet loses in bursts. This lab's emulator loses independently, which is
gentler, and that is recorded as a limitation.

### Input redundancy

The defence against loss, and the reason this protocol has **no
retransmission**.

Every datagram carries the **last eight inputs**, not just the newest. A lost
input arrives again in the next datagram, 16.7 ms later, long before it is
needed.

It costs almost nothing (an input is two bytes) and the arithmetic is
convincing: at 2% loss, the chance of losing all eight datagrams carrying one
input is 0.02⁸ ≈ 2.6 × 10⁻¹⁴.

Measured effect: `loss2` (2% loss) produced **4 rollbacks in 240 seconds**
against `delay20`'s 1 006, and every one of them was depth 1, the shallowest
correction possible. Loss barely becomes rollback. Loss and latency are
different problems, and rollback is only sensitive to the second.

### Reordering

Packets arriving out of order because they took different routes. The protocol
absorbs it silently: every input carries the frame number it belongs to, so
arriving out of order is irrelevant.

### Duplication

The same datagram delivered twice. Also absorbed silently; a repeated identical
input changes nothing.

What is **not** absorbed is the same frame with *different* values. That is
`PeerContradiction`, and it means a buggy peer or a forged datagram.

### Sequence number

A rising counter on every datagram. It serves two purposes: measuring RTT (by
matching a reply to a send) and **inferring loss** from gaps in the sequence.

"Inferring" is the right word. A delayed datagram looks lost until it arrives.

## The five network profiles

A **profile** is synthetic impairment the lab injects into each peer's
**outgoing** datagrams. It is the independent variable: the real network
underneath is loopback, so what you measure is the profile and nothing else.

> **Why measured RTT is double the configured delay:** delay is applied on
> *each* side's egress. A datagram takes `delay_ms` leaving A, and the reply
> takes `delay_ms` again leaving B. Round trip = 2 × `delay_ms`. Deliberate, and
> what happens on a real link, where both directions have delay.

| Profile | Delay | Jitter | Loss | Reordering | Measured RTT |
|---|---|---|---|---|---|
| `natural` | — | — | — | — | 16.6 ms |
| `delay20` | 20 ms | — | — | — | 70 ms |
| `jitter30` | 30 ms | ±15 ms | — | — | 84–88 ms |
| `loss2` | — | — | 2% | — | 27 ms |
| `combined` | 40 ms | ±20 ms | 2% | 0.5% | 97–105 ms |

### `natural`, the control

No impairment. Measures what the lab does when the network is perfect, and gives
everything else a baseline.

**Imitates:** two players on the same LAN, or in the same house.

**Result:** zero rollbacks. Inputs arrive before they are needed, so rollback
never engages. Useful precisely for that: it proved the desyncs appearing in
other profiles came from the *rollback*, not the simulation.

### `delay20`, clean latency

20 ms constant each way. No variation, no loss.

**Imitates:** a good fibre link between nearby cities. Madrid to Barcelona, São
Paulo to Rio.

**Isolates:** the pure effect of **distance**. With no jitter and no loss,
everything that happens is a consequence of delay, so it shows how much work
rollback does purely because the opponent is far away.

**Result:** 1 006 rollbacks in 240 s, mean depth 6.5, 46% extra CPU work. And
crucially, in only one of the two peers (see
[asymmetry](#why-rollbacks-are-asymmetric)).

### `jitter30`, unstable latency

30 ms delay with uniform ±15 ms variation, so each datagram takes between 15 and
45 ms.

**Imitates:** Wi-Fi, mobile, or a congested link. The closest profile to real
domestic internet.

**Isolates:** the effect of **variation**. Comparing against `delay20` answers
"is jitter worse than delay?".

**Result:** practically identical to `delay20` in count and mean. RTTVAR rises
from 0.5 ms to about 18 ms as expected, but maximum depth barely moves. For
rollback, jitter is not worse than delay. For lockstep it would be.

### `loss2`, loss without latency

2% of datagrams discarded, no added delay.

**Imitates:** Wi-Fi with interference, or a link with errors: the packet goes
missing but the path is short.

**Isolates:** the effect of **loss**, separated from delay. It exists to test
the eight-input redundancy specifically.

**Result:** 4 and 19 rollbacks in 240 s, practically nothing, all at depth 1.
The redundancy works: the lost input arrives in the next datagram, before it
matters. This profile is the evidence that **no retransmission is needed**.

### `combined`, everything at once

40 ms delay, ±20 ms jitter, 2% loss, 0.5% reordering.

**Imitates:** a genuinely bad connection. Intercontinental, or mobile while
moving. The worst case the lab sets out to survive.

**Used for:** sizing. This is where you find out whether a prediction limit of 8
and a 16-state buffer have headroom.

**Result:** 1 004 rollbacks, maximum depth 7, 39 stalls, 34% extra work, and
zero desyncs. The stalls show the prediction limit being touched, which means
the sizing is being exercised for real rather than with comfortable margin.

### Why rollbacks are asymmetric

The most counter-intuitive result in the experiments: under `delay20`, one peer
did **1 006** rollbacks and the other did **zero**.

Not a bug. The two peers run independent frame clocks, so there is a fixed phase
difference between them. Whoever is **ahead** reaches frame N before the
opponent's input for frame N exists, so they predict, and sometimes correct.
Whoever is **behind** receives the input before needing it and never guesses.

The practical consequence: **one of the two players pays essentially all of
rollback's CPU cost**, and which one is decided by a few milliseconds at the
start. Comparing "how many rollbacks does my game do" between two clients says
nothing about network quality; it says who started first.

The real session made that vivid. Over the same Madrid–Frankfurt link, fifteen
minutes apart, Madrid paid 1 280 rollbacks to Frankfurt's 31 on one run, then
Frankfurt paid 601 to Madrid's 19 on the next.

The sign that both peers are playing the same game is not symmetric rollbacks.
It is `checksums_compared` rising on both sides with `desync = false`.

### Choosing a profile

```bash
just bench 180 arena                                   # all five, 180 s each
DURATION=60 PROFILES=combined ./ops/scripts/bench.sh   # just one
```

The profiles are **seeded**: the impairment's random generator has a fixed seed,
so repeating an experiment with the same seed produces exactly the same sequence
of drops and delays. That is what makes `just bench` an experiment rather than
an anecdote.

## The metrics this lab reports

All of these appear in `artifacts/report/summary.csv` and in the Prometheus
exporter.

| Metric | What it is | How to read it |
|---|---|---|
| `effective_fps` | screen frames per second | should sit at 60. Below that, the peer is not keeping up |
| `rollbacks` | how many corrections happened | asymmetric by construction, see above |
| `mean_rollback_depth` | average frames re-simulated per correction | 1 is invisible, 8 is noticeable |
| `max_rollback_depth` | the worst case | if it touches the prediction limit, the sizing is at its edge |
| `prediction_accuracy` | fraction of guesses that were right | ~93% is expected, and it is the number that justifies rollback existing |
| `resimulation_overhead` | extra work ÷ useful work | 46% means nearly half an extra frame simulated per frame |
| `stalls` | times the prediction window filled | occasional is normal, continuous means a dead peer |
| `checksums_compared` | state comparisons that **agreed** | has to rise on both sides |
| `desync` | the simulations diverged | `true` invalidates the whole session |
| `srtt_ms` / `rttvar_ms` | smoothed RTT and its variation | see [SRTT](#srtt-and-rttvar) |
| `loss_pct` | loss **inferred** from sequence gaps | a delayed datagram counts as lost until it arrives |
| `state_bytes` | size of one snapshot | 204 in the arena, 415 155 emulated |
| `cpu_seconds` | CPU used by the session | 116 s for a 300 s session is about 39% of a core |

## Emulation and libretro

### Emulator

A program that pretends to be other hardware. FBNeo pretends to be an arcade
board (a 68000, a Z80, a sound chip, video memory) running the game's original
code instruction by instruction.

For rollback that is both excellent and awful. Excellent because an emulator is
naturally a state machine. Awful because the state is hundreds of kilobytes and
saving it sixty times a second is expensive.

### libretro

An **API** that separates the emulator from the interface. The emulator becomes
a library (a *core*) with fixed-name functions; the program using it is the
*frontend*.

This lab is a frontend. It loads the core with `dlopen` and calls:

| libretro function | For |
|---|---|
| `retro_run` | advance one frame |
| `retro_serialize` | save state (rollback's snapshot) |
| `retro_unserialize` | restore state (the "going back") |
| `retro_serialize_size` | how big the state is |

Those four are *exactly* what the `Simulation` trait asks for. Which is why an
arcade emulator drops into the same rollback engine as the toy arena without the
engine knowing.

### Core

The emulator library. Here: `fbneo_libretro.so`, about 90 MB, built in a
container from a pinned commit.

libretro cores are **singletons per process**: FBNeo keeps machine state in
globals. So the host refuses to load a second core in the same process.

### ROM and romset

The contents of the original board's chips. An arcade game is not one file but a
**set** of them: The Last Blade 2 is 14 files.

FBNeo validates each one by **CRC32**. One missing file, or one wrong CRC, and
the game does not run.

**Nothing ROM-related is in this repository**, and no step of the lab copies a
ROM into the source tree.

### BIOS

The board's firmware, separate from the game. The Neo Geo has one: `neogeo.zip`,
holding the boot routine, the Z80 BIOS, the text tiles and the zoom table.

Without it, a perfect game zip does not run. And for this lab the BIOS is **half
the code that executes**, which is why it goes into the hash compared at
handshake time alongside the ROM.

### NVRAM, or memory card

Memory that survives power-off. On the Neo Geo it holds settings and, crucially,
**credits**.

Why that matters here: FBNeo writes `<system>/fbneo/<game>.fs` on unload. A peer
that has run before starts with credits, reaches the title screen on a different
frame, and the boot script presses Start at the wrong moment. Two peers in
different menus, which is a desync. The lab deletes those files before every
session.

### Boot script

The timed button sequence that takes the machine from reset into a match: wait
out the logos, insert a coin, press Start, pick a character, confirm.

It is purely **temporal**, hold this button for N frames and wait M, and it
never reads the game's memory. Reading RAM would need offsets specific to a ROM
revision, and would make the lab depend on knowing the game's internals, which
would defeat the demonstration that rollback works on an **opaque** simulation.

This is only possible because the emulator is deterministic: the same script,
from the same reset, always lands on the same screen at the same frame.

## Determinism

### Determinism

The property that the same input **always** produces exactly the same output. On
every machine, every run, in debug and in release.

It underpins everything: rollback is re-simulation, and re-simulation only
converges if the simulation is a function of its inputs alone.

### Determinism across processes vs rollback safety

Two different properties, and this lab had to learn the difference in practice:

1. **Determinism across processes.** Two processes, same ROM, same inputs, same
   state. Verified by `just check-determinism`.
2. **Rollback safety.** `retro_unserialize` restores *everything* `retro_run`
   will go on to read. Verified by `just check-rollback-safety`.

A core can have the first and not the second: reproducible from a cold boot and
still keeping state outside its savestate. That was exactly the case here.

### Fixed point

Representing fractions with integers. The arena uses **Q23.8**: an `i32` where
the low eight bits are the fractional part, so the unit is 1/256 of a pixel.

It exists because **floating point is not trustworthy across machines**: the
compiler can fuse `a*b+c` into an FMA and change the rounding, x87 carries
excess precision in registers, `-ffast-math` permits reassociation, and maths
libraries differ between platforms on transcendental functions. A one-ULP
difference on one peer is a desync.

### ULP

*Unit in the Last Place*: the smallest representable difference between two
neighbouring floating-point numbers. The smallest possible disagreement, and
enough to desync two simulations.

### `overflow-checks`

In Rust, `debug` panics on integer overflow while `release` wraps by default. So
the same code would produce **different** values under the two profiles.

This project turns the check on in release too. A peer built in debug and one
built in release then behave alike, and an overflow becomes a loud failure
rather than quietly wrong state.

### ASLR

*Address Space Layout Randomization*: the operating system places a program's
memory at different addresses each run. Nothing in the simulation may depend on
an address, because it differs between peers and between runs.

## Protocol and security

### Handshake

The opening exchange where both peers verify they are compatible, **before** the
match starts. It compares, in this order: protocol version, simulation,
application commit, configuration hash, seed, core hash, ROM and BIOS hash, and
which player slot each wants.

Order matters: the first difference found becomes the error message, so the
reported reason is the most fundamental one.

It is a **compatibility** check, not a security one. It stops an incompatible
peer from producing a confusing desync twenty seconds later.

### HMAC

*Hash-based Message Authentication Code*: a cryptographic stamp proving a
message came from whoever holds the key and was not altered in transit.

Here it is HMAC-SHA256, 32 bytes, over **every** datagram. A datagram failing
the HMAC is discarded before becoming a message. It is not refused, it is
ignored, so the symptom of a wrong key is **silence** rather than an error.

### Constant-time comparison

Comparing two HMACs byte by byte with early exit leaks information: how long the
comparison took tells you how many bytes matched. Verification uses a
constant-time comparison so it does not.

### Session key

The shared secret feeding the HMAC. Ephemeral: generated per run, kept in SSM
SecureString, never in Terraform state, never on a command line (arguments are
visible in `ps` to every user on the machine).

### `PeerTimeout`

The peer has sent no authenticated datagram for more than **3 seconds**. The
session ends rather than waiting forever.

A subtlety that became a bug: the *stalled* peer is precisely the one that most
needs to notice the silence, and the check sat after the stall path's early
return. One peer stayed alive accumulating 20 735 stalls on a frame that was
never coming.

## Infrastructure

### Prometheus

A time-series database that scrapes metrics periodically. Each peer exposes its
metrics as text on `127.0.0.1:9898/metrics`, and Prometheus reads them.

Loopback on purpose: the metrics do not go on the network.

### Grafana

The interface that draws graphs on top of Prometheus.

### Elasticsearch and Kibana

A search index and its interface. Prometheus answers "what is happening now";
Elastic answers "what exactly happened around frame 2399". Different question,
different tool. See [15 — Elastic](15-elastic.md).

### JSONL

*JSON Lines*: one JSON object per line. The session log's format, chosen because
it can be written incrementally, so a session that dies mid-run still leaves a
usable file, and because `jq` reads it.

### Terraform

Describes infrastructure in version-controlled files rather than console clicks.
`terraform apply` creates, `terraform destroy` removes.

Here it describes the VPC, the EC2 instance in Frankfurt, the security group
opening UDP/7000 to **a single /32**, and a temporary S3 bucket.

### SSM

*AWS Systems Manager*. Two roles here: **Parameter Store** holds the encrypted
session key, and **Session Manager** gives a shell on the instance without
opening SSH to the internet.
