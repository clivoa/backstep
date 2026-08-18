# 16 — The algorithm, in detail

[01 — Theory](01-theory.md) argues why rollback exists. This is how it is
implemented, at the level of data structures, invariants and code paths. It
follows `crates/rollback-core/src/session.rs` closely enough that you can read
the two side by side.

## State the session carries

```rust
pub struct RollbackSession<S: Simulation> {
    sim: S,
    local: PlayerHandle,
    current_frame: Frame,                        // next frame to simulate

    local_inputs:  BTreeMap<Frame, PlayerInput>, // ours, authoritative
    remote_inputs: BTreeMap<Frame, PlayerInput>, // theirs, as confirmed
    used_remote:   BTreeMap<Frame, UsedInput>,   // what we actually fed the sim

    remote_confirmed_through: Frame,             // contiguous frontier
    first_unverified: Frame,                     // audit cursor

    states: VecDeque<SavedState>,                // ring of snapshots
    frame_checksums: BTreeMap<Frame, u64>,
    pending_peer_checksums: BTreeMap<Frame, u64>,
}
```

Five choices in there are load-bearing.

**`BTreeMap`, not `HashMap`.** Rust's `HashMap` iteration order depends on a
per-process random seed. Anything iterated inside the simulation's causal path
has to be ordered deterministically or the two peers diverge. `BTreeMap` also
gives ordered range queries for free, which `settle_peer_checksums` uses.

**`used_remote` is separate from `remote_inputs`.** The first records what was
actually fed to `advance_frame`, guess or not. The second records what the peer
really sent. Rollback is the comparison of those two maps, and collapsing them
would destroy the only evidence that a prediction was wrong.

**`remote_confirmed_through` is a contiguous frontier, not a maximum.** If
frames 10, 11 and 13 have arrived, the frontier is 11, not 13. A maximum would
claim frame 12 is settled when it is not, and every downstream calculation
(prediction depth, stall condition, checksum finality) would be wrong in the
same direction: too optimistic. This distinction is not academic; getting it
wrong is exactly the bug the property tests found, described at the end.

**`states` is a `VecDeque` used as a ring**, bounded by `state_history`. Push at
the back, pop from the front when full.

**`first_unverified` is a cursor, not a scan.** Verification is O(newly
confirmed frames) per call, not O(session length).

## Two clocks and the gap between them

```
        remote_confirmed_through          current_frame
                  |                             |
   ...  confirmed | speculative speculative ... |  (not yet simulated)
                  |<------ prediction_depth --->|
```

```rust
pub fn confirmed_frame(&self) -> Frame {
    self.remote_confirmed_through.min(self.current_frame - 1)
}

pub fn prediction_depth(&self) -> u32 {
    (self.current_frame - self.remote_confirmed_through - 1).max(0) as u32
}
```

Prediction depth is the distance between the two clocks, and it is the number
that governs everything: how deep a rollback can be, whether the session stalls,
and how much CPU a correction costs.

Note the `min` in `confirmed_frame`. Inputs can arrive for frames not yet
simulated, and a confirmed frame you have not simulated is not a frame you can
report as done.

## The frame lifecycle

Every frame passes through four stages. Only the first is guaranteed to happen
once.

| Stage | Happens | Where |
|---|---|---|
| **Speculated** | once, when `advance()` reaches it | `advance` → `step(Present)` |
| **Confirmed** | when the peer's input for it arrives | `add_remote_inputs` |
| **Verified** | when guess and reality are compared | `reconcile` |
| **Corrected** | zero or more times, if the guess was wrong | `rollback_to` |

A frame can be re-simulated many times. It is presented exactly once.

## `advance()`: one speculative frame

```
advance():
    if would_stall():                       # window full
        record stall; return Stalled

    predicted := remote_inputs has no entry for current_frame
    step(Present)
    settle_peer_checksums()                 # advancing may unpark a comparison
    return Advanced { frame, predicted }
```

The stall check comes first and does no work at all when it trips. That matters
for a reason easy to miss: if the session queued a local input and *then*
discovered it had to stall, the next tick would file a second input for the same
frame. If the first had already gone out on the wire, the peer would see two
different inputs for one frame and refuse the session. That failure has a name
in this codebase, `LocalInputRefiled`, and the ordering here is what prevents
it.

### `step()`: the actual simulation call

```
step(mode):
    local  := local_inputs[frame]                    # error if absent
    remote := remote_inputs[frame] or predict_remote()
    record into used_remote[frame] whether it was a guess

    save_state()            -> push onto the ring
    checksum()              -> frame_checksums[frame]
    advance_frame([p1, p2], mode)
    current_frame += 1
```

Three details worth pulling out.

**The snapshot is taken *before* the frame runs.** `states[f]` is the state at
the *start* of frame `f`, so restoring it and replaying from `f` reproduces
frame `f` itself. Saving after would make the buffer off by one and every
rollback land a frame late.

**`checksum()` is called right after `save_state()`.** In `LibretroSimulation`
that pair costs one `retro_serialize`, not two, because the checksum of the
snapshot is cached as it is taken. On a 415 KB state that halves the per-frame
cost of the most expensive operation in the loop.

**The mode is passed through but never branched on for state.** `Present` and
`Resimulate` differ only in whether video and audio escape. See
[02](02-architecture.md).

### The prediction rule

```rust
fn predict_remote(&self) -> PlayerInput {
    self.remote_inputs
        .get(&self.remote_confirmed_through)
        .copied()
        .unwrap_or(PlayerInput::NEUTRAL)
}
```

Repeat the last confirmed input. Not the last *received* input, the last
*confirmed* one, which is the frontier value. Using a later out-of-order arrival
would predict from a frame we have not settled and would make the guess depend
on network delivery order rather than on game state.

The fallback to neutral only applies before the first remote input has ever
arrived.

Measured accuracy: 92.4% to 93.7% across every configuration in this
repository, including over the real internet. [08](08-experiments.md).

## Receiving inputs

```
add_remote_inputs(start_frame, inputs[]):
    for each (frame, input):
        if frame <= remote_confirmed_through:      # already settled
            if stored != input: error PeerContradiction
            continue                                # idempotent no-op
        insert into remote_inputs
    recompute_remote_confirmed()                    # walk the frontier forward
    reconcile()                                     # audit and roll back
```

`recompute_remote_confirmed` walks one frame at a time while the next frame
exists, which is what makes the frontier contiguous:

```rust
while self.remote_inputs.contains_key(&(self.remote_confirmed_through + 1)) {
    self.remote_confirmed_through += 1;
}
```

Idempotence is structural, not a special case. Every `InputBatch` repeats the
last eight inputs ([03](03-protocol.md)), so most arriving inputs are already
known. Re-filing a known value with the same content does nothing; re-filing it
with a *different* value is `PeerContradiction`, which cannot happen over a
network and can only mean a buggy or forged peer.

## `reconcile()`: finding the earliest lie

This is the heart of the algorithm.

```
reconcile():
    loop:
        frame := first_unverified
        divergence := none

        while frame < current_frame:
            actual := remote_inputs[frame]
            if actual is absent:  break        # unverifiable from here on
            if used_remote[frame] != actual:
                divergence := frame; break
            frame += 1

        first_unverified := frame

        if divergence is none: return
        rollback_to(divergence)                # and loop again
```

Two things about this loop are deliberate and neither is obvious.

**It rolls back to the earliest divergence, not the latest.** One batch can
confirm eight frames at once and contain several mispredictions. Replaying from
the earliest fixes all the later ones in the same pass, because the replay uses
confirmed inputs wherever they exist.

**It loops until the frontier stops moving.** Re-simulation itself calls
`step()`, which makes *fresh* predictions for frames past the confirmed
frontier. Those new guesses are unverified, so the audit has to run again. In
practice it converges immediately; the loop exists because "immediately" is an
observation about typical traffic, not a guarantee.

`first_unverified` never moves backwards past a confirmed frame, so the total
verification work over a session is linear in frames, not quadratic.

## `rollback_to()`

```
rollback_to(target):
    state := states.find(frame == target)          # else HistoryExhausted
    resume_at := current_frame
    depth := resume_at - target

    load_state(state.data)
    current_frame := target
    states.retain(frame < target)                  # drop the poisoned tail

    while current_frame < resume_at:
        step(Resimulate)

    record rollback of `depth`
```

The `states.retain` line is the subtle one. Every snapshot from `target` onward
was produced by simulating with a wrong input, so keeping them would let a later,
deeper rollback restore a state that was built on a guess already known to be
false. Dropping them means the ring refills as the replay runs, with corrected
states.

`HistoryExhausted` is raised rather than papered over. It means a rollback
needed to reach further back than the ring goes, which should be impossible:

```rust
if u16::from(state_history) <= u16::from(prediction_limit) {
    return Err(ConfigError::Invalid("state_history must exceed prediction_limit"));
}
```

If it happens anyway, the prediction-depth accounting is wrong, and continuing
would silently desync. It is better to stop.

## The stall condition

```rust
pub fn would_stall(&self) -> bool {
    self.prediction_depth() >= u32::from(self.config.prediction_limit)
}
```

One line, and it was wrong once in a way worth recording.

The original version exempted a frame whose remote input had already arrived out
of order, on the reasoning that a frame with a known input is not speculation.
It is not that simple. Depth has to be measured from the **contiguous frontier**,
because a rollback triggered by a later-arriving hole must be able to reach back
to that hole. Counting from a maximum let the session run further ahead than the
state ring could cover, and the next correction failed with
`HistoryExhausted { frame: 5, oldest: 7 }`.

A property test in `crates/rollback-core/tests/property_delivery.rs` found it by
generating arbitrary UDP delivery orders. The regression test is
`rolling_back_past_the_state_buffer_is_reported_not_silently_wrong`.

### What the limit is actually sizing

`prediction_limit` frames is `prediction_limit / 60` seconds of speculation. The
default 8 is **133 ms**, and that number is the whole configuration decision:

| One-way delay | Against a 133 ms window | Measured |
|---|---|---|
| 25 ms (Frankfurt) | fits nearly five times over | depth 1–3, no stalls |
| 133 ms (São Paulo, Tokyo) | fits exactly once | depth pinned at 8, stalls |

At 133 ms one way, an input physically cannot arrive before the window it would
have to fill has already filled. The session then stalls every frame it cannot
confirm, which is correct behaviour and also unplayable-adjacent: 56 fps with
1 283 freezes over five minutes.

Two knobs move it, and they charge different people:

**Raise `prediction_limit`** (and `state_history` with it). Speculate further,
stall less, pay in CPU: depth rose to 18 and re-simulation to 1.17×, meaning the
machine simulates more than twice the frames it displays. Free for a 204-byte
arena; not free for a 415 KB emulator state where `save_state` alone is 2.27 ms
of a 16.7 ms frame.

**Raise `input_delay`.** File local inputs further ahead so less has to be
guessed. Stalls and re-simulation both drop, and the player pays directly: 8
frames of delay is 133 ms of input lag.

Measured both ways in
[08](08-experiments.md#fixing-a-link-that-is-too-long-the-tuning-sweep). There
is no setting that makes distance free; the window only decides whether the CPU
or the player absorbs it.

## Why the two peers do unequal amounts of work

Rollback counts are routinely lopsided — 919 against 188 on one link, 590
against 5 on another over the same route minutes later. It looks like a bug and
it is not.

Nothing in the algorithm is asymmetric. What differs is **when each frame clock
started**. The host completes the handshake as soon as it receives the client's
hello; the client completes it one one-way delay later, when the reply arrives.
So the host's frame *f* happens slightly earlier in wall-clock time than the
client's frame *f*.

Whoever is ahead is the one speculating. Their opponent's inputs are still in
flight when they simulate, so they predict and correct. The peer behind receives
inputs that are already due and predicts almost nothing:

```
        P2 (ahead)     frame f simulated ─── needs P1's input for f ─── still in flight → predict
        P1 (behind)    frame f simulated ─── P2's input for f arrived 8 frames ago → known
```

This is measurable to the point of being reproducible on loopback. In the tuning
sweep the trailing peer recorded **1 rollback** across 90 seconds while the
leading peer recorded 362, on the same link, in the same session.

Two consequences worth carrying:

**"How many rollbacks does my client do" measures starting phase, not connection
quality.** Comparing rollback counts between two peers says nothing about the
network; comparing them across sessions on one peer says even less unless the
handshake order was the same.

**The asymmetry is a short-link phenomenon.** It requires slack in the window.
Once both peers saturate, both stall, and stalling is what re-couples the clocks
— at 267 ms the counts landed within 4% of each other. The mechanism that limits
the damage also distributes it.

## Desync detection

The state at the start of frame `f` is final when every input before `f` is
confirmed, which is exactly `remote_confirmed_through >= f - 1`:

```rust
let final_through = (self.remote_confirmed_through + 1).min(self.current_frame);
```

Checksums are emitted on the configured interval for frames below that line, and
**incoming** peer checksums are parked in `pending_peer_checksums` until two
conditions hold locally: we have simulated that frame, and our own state there
is final.

Both conditions are necessary and neither is guaranteed on arrival. A peer sends
its checksum as soon as the frame is final *for it*, and the two peers run
independent frame clocks. Comparing early would produce a false desync against a
state a pending rollback is about to rewrite. Discarding what arrives early
makes detection work in one direction only, which is what the E2E test caught:
one peer compared ten checksums and the other compared zero.

## Complexity

Per presented frame, ignoring the simulation itself:

| Operation | Cost |
|---|---|
| `save_state` + `checksum` | one snapshot; `O(state size)` |
| ring push | amortised `O(1)` |
| input insert | `O(log n)` over a bounded map |
| `reconcile` | `O(frames newly confirmed)`, amortised `O(1)` |

Per rollback of depth *d*: one `load_state`, *d* × `advance_frame`, *d* ×
`save_state`. The dominant term is `d × save_state`, which is why state size
governs rollback's cost far more than the depth does.

Memory is bounded by `state_history × state size` for the ring, plus the input
maps, which are trimmed by `prune()` with a slack window so late datagrams
referring to old frames stay diagnosable rather than crashing.

## Where the numbers come from

Everything above has a measurable consequence. On The Last Blade 2 with a
415 155-byte state, per presented frame:

| | µs | share of a 16 667 µs frame |
|---|---|---|
| `advance_frame` | 3 948 | 24% |
| `save_state` | 2 271 | 14% |
| `load_state` | 17 | 0.1% |

`load_state` is nearly free and `save_state` is not, because saving happens on
every frame and loading only on a correction. That asymmetry is the reason
rollback is affordable at all: you pay the snapshot constantly and the restore
rarely.

Full breakdown, including how to query it per frame, in
[15 — Elastic](15-elastic.md).

## Reading the code

In dependency order, shallowest first:

| File | What to look for |
|---|---|
| `rollback-core/src/simulation.rs` | the four-method trait, and `OutputMode` |
| `rollback-core/src/config.rs` | `validate()`, and `compatibility_hash()` |
| `rollback-core/src/session.rs` | everything above |
| `rollback-core/tests/property_delivery.rs` | convergence under arbitrary delivery |
| `rollback-runner/src/runner.rs` | the frame loop that drives all of it |
