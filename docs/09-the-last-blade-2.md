# 09 - The Last Blade 2 under FBNeo

> *libretro*, *core*, *romset*, *BIOS*, *NVRAM*, *boot script*: all defined in
> [00 - Glossary](00-glossary.md#emulation-and-libretro).

## Why a real game

The arena proves the rollback engine works. It does not prove the engine works
on a simulation nobody wrote for it.

An arcade game under FBNeo is opaque. No field is known, the state is 415 155
bytes, and the only way to save it is `retro_serialize`. If the same
`RollbackSession` drives both, the `Simulation` trait boundary is in the right
place.

| | |
|---|---|
| Game | The Last Blade 2 (NGM-2430 ~ NGH-2430) |
| Hardware | SNK Neo Geo MVS |
| Romset | `lastbld2`, 14 files |
| BIOS | `neogeo.zip`, required |
| Native rate | 59.18 Hz |
| Snapshot | 415 155 bytes |
| Geometry | 320×224 |

## The core

### About the pinned commit

The lab specification pins
`finalburnneo/FBNeo@f1c3545fcdfca4dd5fcf9c1baaac6bba143785f8`.

**That tree contains no libretro port.** Checked directly: the commit exists,
dated 17/08/2026, and its `src/burner/` holds `sdl`, `sdl2`, `win32`, `qt`,
`pi`, `psp` and `macos`, but no `libretro`. Upstream FBNeo does not host the
libretro port; it lives in the `libretro/FBNeo` fork, whose master at that
moment (`0332bb983c8f8a3e9b61cb79ade30f97a5032535`, 16/08/2026) had not
incorporated the named commit.

Building the libretro core at exactly that commit is impossible.

The resolution keeps the intent, an official core pinned reproducibly, without
pretending the specified pin worked. The container builds
`libretro/FBNeo@0332bb983c8f8a3e9b61cb79ade30f97a5032535`, and **both** SHAs go
into the exported provenance file so the discrepancy stays auditable:

```
libretro_fork_commit=0332bb983c8f8a3e9b61cb79ade30f97a5032535
spec_commit=f1c3545fcdfca4dd5fcf9c1baaac6bba143785f8
patches=kNetGame=1
```

If the specified commit is eventually merged into the libretro fork, change
`FBNEO_LIBRETRO_COMMIT` in `docker/fbneo/Dockerfile` and
`ops/scripts/build-fbneo.sh` to the corresponding merge.

### That third line

`patches=kNetGame=1` is not cosmetic. FBNeo as shipped seeds its RNG and its
emulated calendar clock from the host clock, which makes it non-deterministic
across processes and therefore unusable for rollback. The build patches it and
fails loudly if the line it edits ever moves. Measurement and reasoning in
`docker/fbneo/determinism.md` and [05 - Determinism](05-determinism.md).

### Building

```bash
just build-core
```

Ubuntu 24.04 container, `git fetch --depth 1` of the exact commit, then
`make -C src/burner/libretro`. Output lands in `cores/`:

```
cores/fbneo_libretro.so           ~90 MB
cores/fbneo_libretro.so.sha256
cores/fbneo-commit.txt
```

The SHA-256 is what the two peers compare at handshake time.

## The ROM and the BIOS

**You supply both.** Nothing ROM-related is committed, redistributed or included
in any artefact of this repository.

Both peers need **identical** files. The handshake compares SHA-256 and refuses
with "ROM hash mismatch" before the match starts. A different ROM revision is a
different game and would desync within seconds.

### Neo Geo games need the BIOS

Neo Geo drivers are declared `STDROMPICKEXT(game, game, neogeo)`, which appends
the **`neogeo`** romset, the BIOS, to the list of files FBNeo requires.

`neogeoRomDesc[]` has 40 entries, but nearly all carry `BRF_OPT` (regional
BIOSes, Universe BIOS, and so on). Four are mandatory:

| File | CRC | What it is |
|---|---|---|
| `sp-s3.sp1` | `91b64be3` | MVS Asia/Europe v6 BIOS, the default |
| `sm1.sm1` | `94416d67` | Z80 BIOS |
| `sfix.sfix` | `c2ea0cfd` | text layer tiles |
| `000-lo.lo` | `5a86cff2` | zoom table |

A perfect game zip, every CRC matching, fails exactly like a keyless CPS-2 set
if `neogeo.zip` is not alongside. FBNeo's `locate_archive()` searches each
romset in four places, in order:

```
<system>/fbneo/patched/<name>     (only with patched romsets enabled)
<rom directory>/<name>
<system>/fbneo/<name>
<system>/<name>
```

So `neogeo.zip` next to the game, or in `artifacts/system/`, works. This lab
puts it in the system directory, because that is also where the handshake hash
expects it.

### How the reason surfaces

The channel where FBNeo names a missing file is the **libretro log interface**,
not `SET_MESSAGE`. That matters, because `retro_log_printf_t` is a C-variadic
function, and stable Rust can *call* one but cannot *define* one. A pure-Rust
frontend has to refuse `GET_LOG_INTERFACE` and is then blind.

The way out is a C shim (`crates/rollback-libretro/src/log_shim.c`, compiled by
`build.rs`) that does nothing but format: it takes the `va_list`, runs
`vsnprintf`, and hands the finished string to Rust. It is the project's only C
dependency and exists for that one reason.

With it, a failure that used to be nothing but "serialize size of zero" now
names the files:

```
core error: [FBNeo] ROM at index 128 with name sp-s3.sp1  and CRC 0x91b64be3 is required
core error: [FBNeo] ROM at index 165 with name sm1.sm1    and CRC 0x94416d67 is required
core error: [FBNeo] ROM at index 166 with name sfix.sfix  and CRC 0xc2ea0cfd is required
core error: [FBNeo] ROM at index 167 with name 000-lo.lo  and CRC 0x5a86cff2 is required
```

The lab's error message shows only the `error` level. `just inspect-core` shows
every level, including the four romset searches per game.

## The boot script

Neither the machine nor the game starts on its own. Somebody has to insert a
coin, press Start, pick a character and wait out the round intro. The lab does
that with a **timed script**: hold button X for N frames, wait M, move on.

### Why timed macros and not memory reads

The obvious way to automate a boot is to read the game's RAM: watch a "mode"
byte, wait for the value that means character select, then press. It is fast and
robust, and it is refused here on purpose.

Memory offsets are version- and region-specific, so the automation would break
silently on a different ROM revision **while the handshake still says the hashes
match**. And reading RAM through `retro_get_memory_data` is not something
rollback needs; building that dependency would mean the lab no longer
demonstrates that rollback works on an *opaque* simulation.

So the script is purely temporal. That is only sound because the emulator is
deterministic: the same script from the same reset lands on the same screen at
the same frame, on both peers, which is the lab's whole premise.

### Where the numbers came from

Guessing them produces a script that presses Start during a logo and ends up one
screen behind, silently, because the script is blind. So: run the machine, dump
what it draws, read the numbers off the screenshots.

```bash
just probe-boot /path/lastbld2.zip lastblade2
# artifacts/probe/contact-sheet.png
```

`probe-boot` writes a PPM every N frames with the frame number in the filename,
and builds a contact sheet. Every constant in `script.rs` was set by looking at
one of those sheets.

### The timeline

```
frames    0- 400   BIOS memory check (ignores everything)
frames  400- 600   "1864 A.D. The Bakumatsu" / INSERT COIN
frames  600- 669   coins
frames  700- 820   Start HELD                      <- see below
frames  900- 960   character cursor (P2 walks two right)
frames  960-1080   confirm character (HELD)
frames 1140-1260   confirm the Power/Speed prompt (HELD)
frames 1260-1950   portraits, VS card, stage title, round intro
frame  1980        control passes to the players, round live
```

P1 stays on slot 0 and P2 walks two positions right, so the two sides pick
different characters. Both peers run **both** macros at the same absolute
frames, so they make identical menu inputs even though each only controls one
player. During the boot the human's inputs are ignored, so a stray press cannot
change the selection on one peer only.

### Two things only measurement revealed

This was by far the most stubborn part of the lab, and both causes were wrong
assumptions rather than bugs.

**Neo Geo menu buttons want to be HELD, not tapped.**

A twelve-frame tap, ample for the coin slot and ample on CPS-2, **does not start
the match at any frame**. That was measured, not guessed:
`examples/sweep-start.rs` tried a tap at every frame from 640 to 2400, with a
freshly booted machine per candidate, covering four full passes of the attract
loop. All of them stayed in the demo.

Sweeping the **duration** instead found the actual rule. Starting at frame 700:

| Hold length | Result |
|---|---|
| 45 frames | attract demo |
| 70 frames | attract demo |
| 75 frames | Player Select, both players in |
| 640 frames | Player Select |

The board acts on a Start held for roughly 1.2 seconds. The script holds 120
frames, about 1.7× the measured minimum, and
`start_is_held_across_the_measured_accept_point` fails if anyone shortens it.

**FBNeo's classic layout transposes the Neo Geo buttons.**

The button this repository calls `Confirm` was the only one that did not
confirm. From `retro_input.cpp`:

| Neo Geo | RetroPad | logical button here |
|---|---|---|
| A (weak slash) | B | `Block` |
| B (strong slash) | A | `Confirm` |
| C (kick) | Y | `Attack` |
| D (repel) | X | `Special` |

Neo Geo menus accept **A**, which is called `Block` here. The logical names are
the arena's, and the arena named them first; renaming to suit one emulated game
would only make the arena read strangely. So the script has a constant,
`LB2_CONFIRM_BUTTON`, carrying that table in its comment.

### Tight windows and determinism

Worth recording the intuition, because it changes what counts as reasonable
engineering here. Since the emulator is deterministic after the patch, a window
of a few frames **is not fragile**. The press either always lands or never does.
The risk is a mis-measured constant, not jitter, which is what the tools
(`probe-boot`, `sweep-start`) and the tests guarding those numbers are for.

Determinism does not reduce variability. It removes it.

## The opponent

After the boot, P2 plays a fixed move list chosen by a seeded RNG: walk in and
poke, a weak-strong-kick chain combo, crouching sweep, jump-in attack,
quarter-circle forward, quarter-circle back, dragon punch, standing guard,
crouching guard, high and low repel, backdash, throw, and standing still.

It **reads nothing from the game**. It cannot, without ROM offsets. Unlike the
arena bot, which reacts to state, this one plays blind.

Two details that matter to the measurement:

*Forward is toward the opponent*, which is Right for P1 and Left for P2. A bot
that ignored this would throw its specials backwards and hold guard the wrong
way, turning every "block" into "walk into the attack".

*Standing still is in the list on purpose.* Neutral play is real, and a bot that
never stops moving makes prediction look harder than it is. The measured
accuracy came out at 93.0%, matching the arena's 93.5% and the real link's
92.9%.

## Environment determinism

The emulator is deterministic. What surrounds it is not automatically.

### NVRAM

FBNeo reads per-game NVRAM from the system directory and **writes** it on
unload. On Neo Geo the file is `<system>/fbneo/<romset>.fs`, the memory card,
and it holds leftover **credits**.

That is not theoretical. A peer that has run before boots with credits already
inserted, reaches the title screen at a different frame than a peer that has
not, and the two boot scripts then press Start at different moments in the
attract loop. The result is two machines in different menus, which the rollback
correctly reports as a desync.

`rollback_libretro::clear_persistent_state` deletes `.fs`, `.nv` and `.hi`
before loading, on both sides, every session.

### Core options

Pinned in `PINNED_CORE_OPTIONS`; the table and the reason for each is in
[05 - Determinism](05-determinism.md). The important one is
`fbneo-frameskip=0`: frameskip would make `retro_run` advance a variable number
of frames, and rollback assumes exactly one.

## Running it

Locally, two processes. Each peer needs its own system directory, with
`neogeo.zip` in both:

```bash
export ROLLBACK_SESSION_KEY=$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')

cargo run --release -p rollback-bot -- --sim lastblade2 --player p2 \
  --bind 127.0.0.1:7000 --profile combined --duration 240 \
  --core cores/fbneo_libretro.so --rom /path/lastbld2.zip \
  --system-dir artifacts/sys-p2 --metrics 127.0.0.1:9899

cargo run --release -p rollback-client -- --sim lastblade2 \
  --peer 127.0.0.1:7000 --profile combined \
  --core cores/fbneo_libretro.so --rom /path/lastbld2.zip \
  --system-dir artifacts/sys-p1
```

Or `just e2e 240 lastblade2 /path/lastbld2.zip`, which handles the directories.

Against AWS:

```bash
just aws-up lastblade2 /path/lastbld2.zip
just play lastblade2 /path/lastbld2.zip
just collect
just aws-down
```

## Acceptance criteria, and where each was met

1. **Deterministic boot.** Both peers reach the same screen on the same frame
   and select the same characters. Verified by 2 389 agreeing checksums across
   five profiles.
2. **Human on P1.** Control passes to the player at frame 1980. The path works;
   no human session has been played yet ([13](13-coverage.md)).
3. **P2 acting.** The scripted bot produces continuous input, including combos
   and motion inputs.
4. **Observable rollback.** Under `combined`, the overlay shows yellow and red
   frames and the counters climb. [14 - Video](14-video.md) has recordings.
5. **Converging checksums.** `checksums_compared` rises on both peers and
   `desync` stays at 0, including over the real Madrid-Frankfurt link.

## CI without a ROM

`fake-libretro-core` is a real Rust `cdylib` implementing the same libretro ABI:
a trivial "game" (two dots on a 64-pixel line, 32 bytes of state) that is
order-sensitive, so one `retro_run` too many or too few shows up in the
checksum.

`crates/rollback-libretro/tests/fake_core_ffi.rs` loads it with `dlopen` and
exercises the whole FFI path: environment callback, `retro_run`,
`retro_serialize`/`retro_unserialize`, video, audio, output suppression during
re-simulation, the C log shim with real varargs, and a complete rollback session
that converges to the same state as a clean run.

CI therefore tests everything about the libretro host except the emulator
itself, on a machine that has never seen a protected ROM.

## CPU cost

`retro_serialize` produces 415 155 bytes per call. The host caches the checksum
of the last snapshot (`cached_checksum`), because the session calls `save_state`
and `checksum` in sequence and serialising twice per frame would eat most of the
budget.

Measured per presented frame, on the real session:

| | µs |
|---|---|
| `advance` | 3 948 |
| `save_state` | 2 271 |
| `load_state` | 17 |
| **budget at 60 Hz** | **16 667** |

37% of the frame, before any rollback happens. If `advance + save_state` climbs
past about 8 ms, the worst-case rollback of eight re-simulations no longer fits,
and it is time for `t3.medium`.
