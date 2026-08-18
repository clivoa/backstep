# 05 - Determinism

> *Fixed point*, *ULP*, *ASLR*, *savestate*, *NVRAM*: defined in
> [00 - Glossary](00-glossary.md).

## The rule

> Starting from the same state and receiving the same inputs, the two machines
> must produce **bit-identical** states. On every machine, every run, in debug
> and in release.

One bit apart and the two simulations have diverged. Everything after that is
fiction: the two players are watching different games. That is a *desync*.

Rollback does not tolerate desync better than lockstep. It **depends** on the
replay reproducing exactly what the original run would have produced.

## The rules the arena obeys

The arena is native code and has to impose each rule by hand. The emulator does
not get determinism for free either, which is the section after this one.

### 1. No floating point

Not because integers are faster, they are not at this scale, but because `f32`
addition is only *nearly always* reproducible:

- the compiler may contract `a * b + c` into an FMA, changing the rounding;
- x87 carries excess precision in registers;
- `-ffast-math`-style reassociation is legal under some LLVM flags;
- maths libraries differ between platforms on transcendental functions.

A one-ULP difference on one peer is a desync. Integers have none of those
degrees of freedom.

The arena uses Q23.8 fixed point: an `i32` with eight fractional bits, so the
unit is 1/256 of a pixel. Multiplication goes through `i64` so the intermediate
product cannot overflow, and the shift back is arithmetic, rounding towards
negative infinity consistently on every platform. See
`crates/rollback-arena/src/fixed.rs`.

### 2. No hashing, no `HashMap` iteration

Rust's `HashMap` iteration order depends on a per-process random seed, so two
peers would iterate differently. Anything that has to be deterministic uses
`BTreeMap`, a fixed-size `Vec`, or an array.

For the same reason, hashes that cross the network use the repository's own
FNV-1a (`Fnv1a`) rather than `DefaultHasher`, which guarantees stability neither
across processes nor across Rust versions.

### 3. Nothing derived from a pointer

Memory addresses move with ASLR. Nothing in the simulation may depend on one.

### 4. No clock, no threads, no randomness

The simulation does not read the clock, does not observe thread scheduling, and
does not roll dice. The repository's `DeterministicRng` exists for the **bots**
and for the network emulator, which are players and infrastructure rather than
simulation.

### 5. Every loop has a fixed count and a fixed order

The projectile pool has four slots, always walked in the same order. The two
fighters are always processed as 0 then 1. Body separation always gives the odd
fixed-point unit to fighter 0: arbitrary, but **fixed**, so both peers make the
same choice.

### 6. `overflow-checks` in release too

```toml
[profile.release]
overflow-checks = true
```

By default Rust panics on integer overflow in debug and wraps in release. An
overflow that only happened in release would produce different values from
debug, and worse, different values between a peer built one way and a peer built
the other. Turning the check on in both makes them behave alike and turns an
overflow into a loud failure rather than quietly wrong state.

### 7. `OutputMode` does not touch state

Detailed in [02 - Architecture](02-architecture.md). Video, audio and
presentation counters stay **outside** the snapshot and **outside** the
checksum. `presented_frames` exists on `Arena` but is neither serialised nor
hashed, because it counts *screen* frames, which legitimately differ between
peers.

## The rules the emulator forces

### The emulator was not deterministic

This section began as a confident sentence: the emulator is deterministic by
construction, the problem is only the environment around it. That sentence was
wrong, and how it fell over is the most instructive result in the lab.

The symptom appeared while calibrating The Last Blade 2's boot script: the same
screens arrived on different frames each run. Since the script is purely
temporal, that broke it irreproducibly. The test that settled it:

```
# two runs, different seconds
checksum f000300 e5467290974af991
checksum f000300 eaf4e5314c60aeed     <- diverged

# two runs launched in the SAME wall-clock second
checksum f000300 59c82db8a4071bd1
checksum f000300 59c82db8a4071bd1     <- identical
```

`time(NULL)` has one-second granularity. Two runs agreeing inside the same
second and disagreeing across seconds is an unambiguous signature of a
dependency on the host clock.

There were two, both in FBNeo's `src/burn/burn.cpp`:

1. `BurnRandomInit()` seeds the driver RNG with `time(NULL)`.
2. `BurnGetLocalTime()` returns the host's calendar, and the Neo Geo has a
   calendar clock chip, the µPD4990A, which the BIOS reads during boot
   (`src/burn/drv/neogeo/neo_upd4990a.cpp:54`).

The fix is in `docker/fbneo/determinism.md`: FBNeo already handles both when
`kNetGame` is set, and the lab's build sets it. The `Dockerfile` fails loudly if
the line it edits stops existing.

**The lesson is not "FBNeo has a bug".** It is that "the emulator is
deterministic" is a claim about the emulator *and about how it is built and
configured*, and the only way to know is to measure. Hence:

```bash
just check-determinism /path/lastbld2.zip
```

It runs the core twice, in separate processes, with a deliberate `sleep` between
them, and compares checksums. The sleep is the test: without it both runs land
in the same second and a broken core passes.

### NVRAM and per-game settings

FBNeo reads NVRAM and settings from the system directory, and **writes** them on
unload. On Neo Geo the file is `<system>/fbneo/<romset>.fs`, the memory card,
and it holds credits between runs.

Not theoretical: a peer that has run before starts with credits inserted,
reaches the title screen on a different frame than a peer that has not, and the
two boot scripts then press Start at different points in the attract loop. Two
machines in different menus, which the rollback correctly reports as a desync.

So `rollback_libretro::clear_persistent_state` deletes `.fs`, `.nv` and `.hi`
before loading, on both sides, every session.

### The checksum was measuring the wrong thing

After all of the above, the emulator became reproducible **across processes**,
and sessions kept desyncing anyway, always on the first rollback. Zero desyncs
under `natural`, which does no rollback at all, and a guaranteed desync under
`delay20`, which does.

Rollback needs a property that "deterministic" does not cover: that
`retro_unserialize` restores **everything** `retro_run` will go on to read. A
core can be perfectly reproducible from a cold boot and still keep state outside
its savestate.

The tool that answers it is `just check-rollback-safety`:

```
save the state at frame N
run K frames with inputs I     -> checksum A     (the peer that did not roll back)
restore, run the same K        -> checksum B     (the peer that did)
A == B ?
```

The result, on The Last Blade 2:

```
save -> load -> save disagrees on 16 to 21 bytes out of 415 155,
always in four four-byte fields at offsets 537, 829, 1413 and 1705.
```

And the question that decides everything: does it spread? It does not.

```
probe at frame 2100, 300 re-simulated frames of a live match
  -> 18 bytes differ, highest offset 1761
probe at frame 2500 -> 23 bytes, highest offset 1757
probe at frame 2900 -> 17 bytes, highest offset 1499
```

Five seconds of re-simulated fighting and the difference is still a couple of
dozen bytes below offset 1800. It **never reaches** the 413 KB of work RAM,
video RAM and palette where the game actually lives. This is sound-chip and
timer bookkeeping, which the 68000 does not read back.

So the machine *was* rollback-safe and the checksum was not. Hashing the whole
blob reported a desync on the first rollback of every session, a false alarm
that makes the detector worthless on precisely the core where it matters most.

`CHECKSUM_SKIP_BYTES = 2048` is the fix: the checksum ignores the unstable
prefix. The price, stated plainly, is that a genuine divergence confined to
those 2 KB would go unnoticed. Worth taking, because the alternative is a
detector that fires every time, and because everything the players can observe
lives past that boundary. `check-rollback-safety` **fails** if instability ever
reaches the limit, so the claim is checkable rather than hopeful.

The skip is opt-in, not a default. The first version applied it unconditionally
and swallowed the test core's 32-byte state whole: every checksum equal, desync
detection silently a no-op that could never fire. `with_checksum_skip` now
refuses a skip that leaves less than half the state, and the FFI suite holds
that line.

### The stalled peer never noticed the other had died

Found at the bench rather than by reasoning: during a lossy run, one peer stayed
alive for minutes accumulating 20 735 stalls on a frame that was never coming.

The cause was in `SessionRunner::step`. The liveness check sat at the end of the
function, and the "I am stalled" path returned before reaching it. The peer that
most needed to notice the silence was the only one not looking.

Fixed by extracting `check_peer_liveness` and calling it on both paths, with a
regression test in `a_peer_that_dies_mid_session_times_out_while_stalled`, which
kills the peer *without* sending Disconnect, because the polite path already
worked.

### The BIOS is in the hash

A Neo Geo game is half the code that runs; `neogeo.zip` is the other half. Two
peers with different BIOS revisions would pass the handshake and diverge during
boot, before any input.

`app::hash_rom_set` hashes ROM **and** BIOS, domain-separated, into the same
`rom_hash` field. A different BIOS is now refused at the door with "ROM hash
mismatch", which is a dull message but an honest one.

### Core options

Some FBNeo options change how many machine cycles a frame runs for. Those are
pinned explicitly in `PINNED_CORE_OPTIONS` rather than left to whatever the core
defaults to on that machine:

| Option | Value | Why |
|---|---|---|
| `fbneo-frameskip` | `0` | Frameskip would make `retro_run` advance a variable number of frames; rollback assumes exactly one |
| `fbneo-cpu-speed-adjust` | `100` | Changes the cycle budget per frame |
| `fbneo-neogeo-mode` | `DIPSWITCH` | Fixes the emulated region, which changes the frame rate |
| `fbneo-diagnostic-input` | `Disabled` | The service menu cannot be opened by accident |

That last line used to read `Hold Start`, and changing it was an attempt to
explain why the boot script could not start a match. **It was not the cause**;
the real one is in [09](09-the-last-blade-2.md), where the board wants Start
*held* for about 75 frames. `Disabled` is still the right value: the script
holds Start for 120 frames, and pointing a diagnostic gesture at exactly that is
asking for trouble.

### Core and ROM hashes

Different emulators simulate differently. Different ROM revisions are different
games. Both are compared at handshake time by SHA-256, and the session is
refused with a readable reason before it starts.

## How all of this is verified

### A 100 000-frame replay, in debug and release

`crates/rollback-arena/tests/replay_100k.rs` runs 100 000 frames of a
deterministic input script and asserts a checksum **hard-coded as a constant**:

```rust
const GOLDEN_SCRIPTED: u64 = 0xf594_92aa_1a1b_d8cf;
const GOLDEN_BOTS: u64 = 0x15fd_05bb_8237_0920;
```

The constant is the point. Without it the test would only prove the arena agrees
with itself in that process. With it, a change that silently alters simulation
behaviour has to be **acknowledged** by updating a number, and that is the
signal that every peer needs rebuilding before playing together.

`just test` runs the file in debug **and** release. If the two disagree,
something in the arena is sensitive to optimisation level, which is exactly the
class of bug that kills a session between two differently-built peers.

The same file also checks that saving and restoring mid-replay changes nothing,
which is rollback's central premise, tested every 997 frames. A prime, so the
interruption lands on every phase of the simulation's internal cycles.

### Property tests

`crates/rollback-core/tests/property_delivery.rs` generates arbitrary UDP
delivery, reordered, duplicated and delayed, and asserts the session converges
to the same state as one that received everything in order. Loss is modelled as
delay, because that is what the protocol turns it into: eight-input redundancy
makes a lost datagram mean "arrives later" rather than "never arrives".

> This file found a real bug. The stall condition exempted a frame whose remote
> input had arrived out of order, letting the session advance past a hole and
> then need to roll back further than the state buffer reaches. Depth is now
> measured from the contiguous frontier only.

### Checksum comparison at runtime

Every 60 confirmed frames the peers exchange a state checksum. A disagreement
ends the session immediately and is logged in the JSONL as a `desync` event with
both values.

That is the safety net, not the defence. When a desync shows up, the work is
finding which of the rules above was broken.

### Determinism across machines

The one check none of the above performs. Two processes of one binary on one CPU
would agree even if every rule in this document were wrong.

Closed by the Madrid-Frankfurt session: 449 agreeing checksum comparisons
between an Intel i7-10750H running Arch and an EC2 `t3.small` running Ubuntu
24.04, in both the arena and the emulated game, with no desyncs.
See [13 - Coverage](13-coverage.md).

## Diagnosing a desync

1. **Which frame?** The `desync` event in the JSONL gives the exact number.
2. **Same commit on both peers?** The handshake guarantees it, but check
   `app_commit` in both `session_start` records.
3. **Reproducible?** Run `just bench` at the same seed and profile. If it
   reproduces, the problem is in the simulation. If not, look for something
   dependent on timing or scheduling.
4. **Debug against release.** Run the two peers at different optimisation
   levels. If it only desyncs that way, it is optimisation: floating point or
   overflow.
5. **Isolate in the arena.** If the desync is in the emulated game, try to
   reproduce it in the arena. If the arena is clean, the problem is in the
   core's environment: NVRAM, options, or the hash. Then run
   `just check-determinism` and `just check-rollback-safety`, which cover the
   two failures this project actually hit.
6. **Reduce it to a test.** The 100 000-frame replay and the property tests are
   where a reproduction should end up living.
