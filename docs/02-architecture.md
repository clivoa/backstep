# 02 — Architecture

## The crates

```
rollback-core          the engine: prediction, history, re-simulation, desync
 ├─ rollback-net       versioned UDP protocol, HMAC, network emulator, link stats
 ├─ rollback-arena     deterministic 2D simulation (integers only) + FSM bot
 ├─ rollback-libretro  libretro host: retro_serialize/unserialize, boot scripts
 ├─ rollback-telemetry Prometheus exporter, JSONL log, /proc sampling
 └─ rollback-runner    the glue: handshake, shared frame loop, video recording
     ├─ rollback-client  SDL2, human on P1, overlay
     └─ rollback-bot     headless, FSM on P2, runs on EC2

rollback-report        reads the JSONL, writes summary.csv + report.html
fake-libretro-core     a real libretro cdylib, so CI can test the FFI with no ROM
```

Dependencies only ever point downwards. `rollback-core` knows nothing about
networks, nothing about libretro, nothing about telemetry. It knows the
`Simulation` trait and stops there.

## The boundary that matters

```rust
pub trait Simulation {
    fn save_state(&self) -> Vec<u8>;
    fn load_state(&mut self, data: &[u8]) -> Result<(), SimulationError>;
    fn advance_frame(&mut self, inputs: [PlayerInput; 2], output_mode: OutputMode);
    fn checksum(&self) -> u64;
}
```

Four methods. That is the entire surface between the rollback engine and
whatever is being simulated.

This boundary is the project's argument. A 204-byte toy arena and a 90 MB arcade
emulator implement the same trait, and `RollbackSession` is *literally the same
code* driving both. If the engine needed to know anything about the game,
positions or hitboxes or anything at all, it could not drive FBNeo, whose state
is opaque by construction.

### `OutputMode`, and why it must not touch state

`advance_frame` is told whether the frame is `Present` (the player will see it)
or `Resimulate` (a correction replay). The contract is strict:

> `OutputMode` may change **output** (video, audio, rumble, logging) and
> **never** simulation state.

A rollback of depth 8 runs eight `advance_frame` calls inside one display frame.
Without discarding, the player would see a blur and hear eight frames of audio
at once. But if `OutputMode` changed state the two peers would diverge outright,
because they re-simulate different frames at different times.

Three tests hold that line, one per layer:
`output_mode_does_not_touch_simulation_state` (core),
`output_mode_cannot_touch_simulation_state` (arena),
`output_mode_does_not_change_the_machine_state` (libretro, over the real FFI).

## One frame

The loop lives in `rollback-runner/src/runner.rs`. The order is deliberate.

```
1. receive        drain the socket; remote inputs land first, so a rollback
                  happens BEFORE we build anything else on top
2. would_stall?   if the prediction window is full, do no local work at all
3. read input     human controller or bot FSM
4. send           the batch goes out BEFORE simulating, to buy the peer a frame
5. advance        simulate and present
6. checksums      any frame that just became final has its checksum exchanged
7. telemetry      publish, log, check the peer is still alive
```

Each ordering has a reason:

**1 before 5.** Applying old inputs after already speculating the current frame
would only deepen the rollback.

**2 before 3.** Queueing a local input during a stall would refile the same
frame on the next tick. If the first value had already gone out on the wire, the
peer would see two different inputs for one frame and refuse the session. That
is the `LocalInputRefiled` error, and it exists because this ordering is easy to
get wrong.

**4 before 5.** Simulating takes time, and that time is pure latency for the
peer.

Step 7 is also where the liveness check lives, and it has a scar: it used to sit
only at the end of the function, after the stalled branch returned early. A peer
whose partner died was therefore the one peer that never looked. One sat at
20 735 stalls waiting for a frame that was never coming. See
[05 — Determinism](05-determinism.md).

## Where each concern lives

| Concern | Where | Why there |
|---|---|---|
| Prediction and rollback | `rollback-core::session` | Depends on neither network nor game |
| The prediction rule | `session::predict_remote` | One function, swappable |
| Datagram format | `rollback-net::wire` | Testable byte for byte without a socket |
| Authentication | `rollback-net::auth` | Deliberately separate from the format |
| Synthetic delay and loss | `rollback-net::emulator` | Applied on **egress** (see 03) |
| RTT, loss, bitrate | `rollback-net::link` | Measurement, not transport |
| Arena physics | `rollback-arena::arena` | Integers only, no RNG |
| Arena bot | `rollback-arena::bot` | It is a **player**, not part of the simulation |
| libretro FFI | `rollback-libretro::{ffi,host,core}` | The only place with `unsafe` |
| Boot scripts | `rollback-libretro::script` | Timed macros, no ROM offsets |
| Video recording | `rollback-runner::record` | Presented frames only, piped to ffmpeg |
| Exporter and JSONL | `rollback-telemetry` | One source, three consumers |
| Handshake | `rollback-runner::handshake` | Compatibility, not security |

## Why the bots are not part of the simulation

Both `ArenaBot` and `ScriptedBot` produce one `PlayerInput` per frame, exactly
as a controller would. That input travels the wire like any other.

This matters. If a bot were part of the simulation, both peers would have to run
it identically, and its random number generator would become one more way to
desync. Because it is a *player*, its randomness is irrelevant to
synchronisation. It only has to be reproducible from the seed, so that
`just bench` is an experiment rather than an anecdote.

`ScriptedBot` carries an extra restriction: it **reads nothing from the game**.
It cannot, without ROM memory offsets, which this lab deliberately refuses (the
reasoning is in [09 — The Last Blade 2](09-the-last-blade-2.md)). It plays a
fixed move list (chain combos, quarter-circles, dragon punch, guard, parry,
throw) and knows which way it is facing, and that is all.

## `unsafe`: where, and why

Every crate declares `#![forbid(unsafe_code)]`, with two exceptions.

**`rollback-libretro`** loads a C library through `dlopen` and calls into it.
There is no safe way to do that in Rust. The mitigation is the handshake: both
peers compare the core's SHA-256 before a session starts, so a swapped or
corrupted core becomes a refused connection rather than undefined behaviour.

**`fake-libretro-core`** implements the same C ABI, so the host can load it in
tests.

There is also one isolated `unsafe` in `rollback-client/src/render.rs`, to
reinterpret the `u32` framebuffer as bytes when updating an SDL texture. It is
documented at the point of use, and the alternative was a whole dependency
(`bytemuck`) for a three-line function.

There is one C file, `rollback-libretro/src/log_shim.c`, and it is not `unsafe`
Rust. It exists because `retro_log_printf_t` is a C-variadic function and
stable Rust cannot define one. Without it, FBNeo's diagnostics never reach
the frontend and a missing ROM file surfaces as a bare zero. See
[09](09-the-last-blade-2.md).

## What each layer of tests proves

| Layer | Test | What it guarantees |
|---|---|---|
| `rollback-core` | unit + `property_delivery.rs` | Convergence under arbitrary UDP delivery |
| `rollback-arena` | `replay_100k.rs` | Same checksum in debug and release, 100 000 frames |
| `rollback-net` | `golden_protocol.rs` | Exact bytes of the format and the HMAC |
| `rollback-libretro` | `fake_core_ffi.rs` | The real FFI path, no ROM required |
| `rollback-runner` | paired tests | Two real peers over a real socket |
| System | `ops/scripts/e2e-local.sh` | Two **processes**, five profiles, no desync |
| Docs | `ops/scripts/check-docs.py` | Every link and anchor resolves |

The property tests and the E2E are not decoration. Each found a real bug during
implementation, and both are named in the commit history.
