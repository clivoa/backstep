# 17 - How emulation works

This lab runs a 1998 arcade game on a 2020 laptop and rewinds it sixty times a
second. That only makes sense once you know what an emulator actually is, so
this document builds it from nothing. No prior knowledge assumed.

If you already know what a cycle-accurate emulator is and why savestates exist,
skip to [09 - The Last Blade 2](09-the-last-blade-2.md), which is about this
particular game rather than emulation in general.

## The problem

The Last Blade 2 was not written for your computer. It was written for a **Neo
Geo MVS**, an arcade board Japanese company SNK sold in 1990. Its parts:

| Part | What it is | Rough modern equivalent |
|---|---|---|
| Motorola 68000 | the main processor, 12 MHz | the CPU |
| Zilog Z80 | a second, smaller processor, just for sound | a sound card with its own brain |
| LSPC2-A2 | custom video chip that draws sprites | the GPU |
| YM2610 | sound chip: FM synthesis plus samples | the audio hardware |
| 68000 machine code on a ROM chip | the game itself | the executable |

The game is machine code for a processor your laptop does not have, talking to
chips your laptop does not have. You cannot run it directly, in the same way you
cannot run a Windows `.exe` by wishing on a Mac.

There are two ways out. One is to **translate** the game, port it, rewrite it
for modern hardware. That needs the source code, which nobody has. The other is
to **build the arcade board in software**, and that is emulation.

## What an emulator is

An emulator is a program that pretends to be hardware.

Concretely, FBNeo holds, in ordinary memory:

```
a variable holding the 68000's program counter    "which instruction is next"
sixteen variables holding the 68000's registers   its working scratch space
an array of 64 KB standing in for the work RAM    where the game keeps its state
an array standing in for video RAM                what is on screen
counters standing in for the sound chip's state   where each voice is in its note
```

Then it runs a loop that is, in essence:

```
forever:
    instruction = rom[program_counter]     # fetch
    decode it                              # what does 0x4E71 mean?
    do what it says, to those variables    # execute
    advance program_counter
```

That is the whole idea. The 68000 instruction `MOVE.W D0, D1` means "copy
register D0 into register D1", so the emulator executes `d1 = d0`. The game
writes a sprite's coordinates to a particular address, and the emulator notices
the address belongs to the video chip and updates its own sprite table.

The game cannot tell. It reads and writes the addresses it always read and
wrote, and something answers correctly every time. There is no hardware behind
those addresses any more, only arrays and a switch statement.

## Why "cycle accurate" matters

A naive emulator gets instructions right and timing wrong, and timing is not a
detail on hardware from 1990.

The Neo Geo's two processors run **at the same time**. The 68000 runs game
logic; the Z80 drives sound. They communicate by leaving messages at shared
addresses. If the emulator runs a thousand 68000 instructions and only then
lets the Z80 catch up, the Z80 reads a message meant for a moment that has
already passed, and the music desynchronises from the action, or a sound effect
fires on the wrong frame.

Worse, the original game was **written against real timings**. Programmers in
1998 counted machine cycles and knew a routine would finish before the video
chip started drawing the next scanline. An emulator that is fast in the wrong
places breaks code that was correct on hardware.

So FBNeo tracks cycles. It runs the 68000 for a fixed number of cycles, then
the Z80 for its share, then the video chip, then back, keeping all of them
inside the same simulated instant. That interleaving is what "cycle accurate"
means, and it is why an emulator is expensive: it is not just running the
game's instructions, it is running a clock and several chips around them.

## The BIOS: half the code

A Neo Geo game cartridge does not contain a whole program. The board itself
carries a **BIOS**, a ROM chip on the motherboard with the code that boots the
machine, draws the SNK logo, runs the credit and coin logic, and shows the
"INSERT COIN" screen.

The game calls into it constantly. This is why the lab needs `neogeo.zip`
alongside `lastbld2.zip`, and why both are hashed into the handshake: two peers
running the same game with different BIOS revisions are running **different
code**, and would diverge. Half the software that executes in this lab is
software the game did not ship with.

## Savestates: the feature rollback is built on

Everything the emulated machine "is" at a given moment is those arrays and
variables. So you can copy them.

That copy is a **savestate**: registers, work RAM, video RAM, sound chip
counters, cycle counters, all of it. Restore the copy and the machine is exactly
where it was, down to the half-finished note the sound chip was playing.

libretro, the plugin API FBNeo implements, exposes exactly this:

```c
size_t retro_serialize_size(void);            /* how big is the state? */
bool   retro_serialize(void *data, size_t);   /* copy it out */
bool   retro_unserialize(const void *, size_t); /* put it back */
```

For The Last Blade 2 that state is **415 155 bytes**. Those three functions are
the entire reason this lab can put an arcade game under rollback: they are
`save_state` and `load_state` in the `Simulation` trait
([02 - Architecture](02-architecture.md)), and the rollback engine never needs
to know what any of those bytes mean.

That is worth sitting with. The engine rewinds a game it cannot read. It has no
idea where the players are, how much health they have, or whether a fireball is
in flight. It copies 415 155 opaque bytes, and that is sufficient.

## Why the emulator had to be patched

An emulator is deterministic **only if you keep it that way**. FBNeo, as
shipped, does two things that break it for this purpose:

- it seeds its random number generator from `time(NULL)`, the wall clock;
- it feeds the host's calendar into the Neo Geo's real-time clock chip, which
  the BIOS reads at boot.

Two machines starting in different wall-clock seconds therefore begin with
different state, and diverge before the first input. FBNeo already has the fix,
because Fightcade needs it too: a `kNetGame` flag that makes both deterministic.
The lab's build patches it on. Measured, with the evidence, in
[05 - Determinism](05-determinism.md).

The general lesson generalises past emulation: **determinism is a property you
maintain, not one you get.** Any clock, any address, any hash seed, any thread
scheduling decision that leaks into the simulation will eventually desync two
machines that are otherwise running identical code.

## Where the time goes

Emulation is not free, and this lab measured the bill. Per frame of The Last
Blade 2, on an i7-10750H:

| | microseconds | share of a 16 667 us frame |
|---|---|---|
| `advance_frame` - run every chip for 1/60 s | 3 948 | 24% |
| `save_state` - copy 415 KB out | 2 271 | 14% |
| `load_state` - copy 415 KB back | 17 | 0.1% |

Emulating the machine costs a quarter of the frame budget. Snapshotting it costs
another seventh, **every single frame**, because rollback needs a restore point
for every frame it might have to return to.

Restoring is nearly free, which is the asymmetry that makes rollback affordable:
you pay for the snapshot constantly and for the rewind rarely.

## libretro: the plug the emulator fits into

FBNeo could have been embedded directly, but every emulator would then need its
own integration. **libretro** is a small C interface that separates the two
concerns:

- a **core** implements the game machine (FBNeo, and hundreds of others);
- a **frontend** supplies the window, the pad, the audio device and the loop.

The contract is about a dozen functions. The frontend calls `retro_run()` once
per frame; the core simulates one frame and calls back with video and audio.

This lab is a frontend. That is why it can drive an arcade emulator and a
204-byte toy arena with the same rollback engine and change nothing in between:
both are just implementations of four methods.

The FFI details, the C shim needed for the core's variadic logging callback, and
the diagnosis of a ROM that would not boot are in
[09 - The Last Blade 2](09-the-last-blade-2.md).

## What this lab is not doing

**Not reading the game's memory.** No health bars, no positions, no character
state. The scripted opponent plays a fixed list of moves on a timer and knows
only which way it is facing. Reading RAM would need per-ROM offsets, would break
on any other game, and would make the bot part of the simulation rather than a
player. [02](02-architecture.md) explains why that distinction matters.

**Not distributing anything.** The ROM and BIOS are yours. This repository
contains no game data, and the build downloads none.

**Not rewriting the emulator.** FBNeo is used as-is from a pinned commit, with
one flag flipped, and both the upstream commit and the patch are recorded in the
built artefact's provenance.
