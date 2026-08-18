# 01 — Why rollback exists

> New to *lockstep*, *prediction*, *confirmed frame* or *jitter*?
> [00 — Glossary](00-glossary.md) defines all of them from scratch.

## The problem

A fighting game at 60 Hz settles an exchange inside two or three frames: 33 to
50 milliseconds. Madrid to Frankfurt is a 50 ms round trip, measured. The time
information needs to cross the wire is the same order of magnitude as the
decision the game has to make.

That leaves a designer with a problem that has no clean answer: **on frame *N*,
the local machine does not know what the opponent pressed on frame *N*.** It
will not know for a while yet. Every netcode architecture is an answer to that
one sentence.

## The three possible answers

### Lockstep: wait

Simulate frame *N* only once both inputs for frame *N* have arrived.

Trivially correct: the two machines cannot diverge, because neither moves
without the other. The cost is that the network's latency becomes the game's
latency. Every frame waits half a round trip, so at 50 ms every button responds
two or three frames late, and any variation in latency turns into a visible
hitch. Fine on a LAN. Unplayable between countries.

### Input delay: wait, but on a schedule

Deliberately hold your own input back by *k* frames so it reaches the opponent
in time. If *k* × 16.7 ms ≥ RTT/2, nobody ever waits.

The game stops hitching, and the latency becomes constant and predictable. You
have also paid the network's latency in *input* latency, permanently, including
during the stretches when the network was perfectly fine. And it trades against
human perception the wrong way round: a player feels three frames of input delay
far more sharply than three frames of visual correction.

### Rollback: guess, then fix

Simulate frame *N* **now**, guessing the opponent's input. When the real input
arrives, do nothing if the guess was right. If it was wrong, restore the last
known-good state and re-simulate from there with the truth.

Your own input responds on the frame you pressed it. Always. Network latency
stops being a control problem and becomes a *visual* one. The opponent
occasionally snaps a few pixels sideways.

Rollback won because it trades the right currency. Fighting players detect input
latency with brutal precision and tolerate the opponent shifting position rather
well, mostly because the guess is usually right.

## Why guessing works

The prediction in this lab is the simplest one available: **assume the opponent
is still doing whatever they were doing** (`predict_remote`, in
`crates/rollback-core/src/session.rs`).

That sounds naive until you look at what fighting-game input actually is.
Directions stay held for entire walks. Charge buttons are held for dozens of
frames. There are long stretches of neutral. Inputs are *held*, not tapped, and
a rule that repeats the last one is right far more often than it has any
business being.

Measured here, three separate times on three different setups: **93%**. The
number barely moves. [08 — Experiments](08-experiments.md) has the details.

The point is not that prediction is clever. It is that prediction does not need
to be right every time, only often enough that corrections stay rare and
shallow.

## What rollback costs

### The state has to be saved and restored, every frame

The engine snapshots the whole simulation each frame. Going back six frames
means loading the snapshot from six frames ago and re-simulating six frames.

That puts a hard requirement on the simulation: **everything that matters must
be inside the snapshot, and restoring it must be exact.** Not an animation
counter, not a random seed, not a "already played this sound" flag living
outside it.

The arena's snapshot is 204 bytes. The Last Blade 2 under FBNeo is **415 155**.
Both fit in the 16.7 ms frame budget, but only one of them fits comfortably.
`save_state` alone eats 14% of every frame on the emulated side, rollback or no
rollback. [15 — Elastic](15-elastic.md) has that broken down.

### CPU

A rollback of depth 6 means simulating seven frames in the time of one. The
frame budget needs enough headroom for the worst case, which is why there is a
**prediction limit**, 8 frames here. Without one, a momentary disconnection
would have the machine trying to re-simulate hundreds of frames at once.

The report measures this directly as `resimulation_overhead`: re-simulated
frames per presented frame. It reached 48% under jitter and still held 60 Hz.

### Determinism, which is the one that kills projects

If two machines start from the same state, receive the same inputs, and produce
different states, by a single bit, the simulations have diverged and
**everything after that is fiction**. The two players are watching different
games.

That is a *desync*, and the only honest defence is to catch it early and stop.
This lab compares state checksums every 60 confirmed frames and ends the session
at the first disagreement.

Getting there was not free. The emulator turned out not to be deterministic as
shipped, and the checksum turned out to be measuring the wrong bytes. Both
stories are in [05 — Determinism](05-determinism.md), and both are more
instructive than the parts that worked.

## Confirmed frames

Worth pinning the vocabulary down, because nearly all the logic turns on it.

- **Current frame** (`current_frame`): the next one to simulate.
- **Confirmed frame** (`confirmed_frame`): the highest frame for which **both**
  players' inputs are known. Nothing before it can change.
- **Prediction depth** (`prediction_depth`): how far past the confirmed frame
  the simulation has already run on speculation.

A rollback can only reach back as far as `prediction_depth`. So the state buffer
must be **strictly deeper** than the prediction limit: 16 states for a limit of
8. `SessionConfig::validate` enforces it, and
`rolling_back_past_the_state_buffer_is_reported_not_silently_wrong` exists
because a property test once produced exactly that failure from a real bug.

## What this lab adds

Rollback works. That has not been in question since GGPO. This lab is trying to
do something else: **make the mechanism visible and measurable.**

- The arena exists so you can open the state, count the bytes, and see which
  field diverged.
- The client's overlay colours every frame by how it was produced: confirmed,
  predicted, corrected, stalled.
- The network emulator injects delay, jitter, loss, duplication and reordering
  from a fixed seed, so an experiment is *repeatable* rather than anecdotal.
- The Last Blade 2 exists to prove none of it depends on the simulation being a
  toy: the same engine drives an opaque 415 KB arcade machine, and
  [14 — Video](14-video.md) shows it happening.

## Going deeper

This document argues why. [16 — The algorithm](16-algorithm.md) covers how: the
data structures, the frame lifecycle, `reconcile()` and `rollback_to()` line by
line, the invariants that hold it together, and the complexity of each step.

## Further reading

- GGPO (Tony Cannon), the implementation that set the standard.
- "Fight the Lag!", the classic rollback explainer, by GGPO's author.
- Fightcade, where this runs in anger, on FBNeo, and has for years. A reference
  point here, not a dependency.
