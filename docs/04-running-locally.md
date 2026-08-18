# 04 - Running it locally

> Not sure what a *network profile* is, or what `delay20` means? See
> [00 - Glossary: the network profiles](00-glossary.md#the-network-profiles).

## Controls

Keyboard and gamepad are **both** read every frame and OR'd together, so you can
hold a direction on the stick and press buttons on the keyboard.

| Action | Keyboard | Gamepad | The Last Blade 2 |
|---|---|---|---|
| Up | `W` / `↑` | D-pad ↑, left stick | jump |
| Down | `S` / `↓` | D-pad ↓ | crouch |
| Left | `A` / `←` | D-pad ← | - |
| Right | `D` / `→` | D-pad → | - |
| Attack | `J` | `X` | C - kick |
| Block | `K` | `A` | **A - weak slash** |
| Special | `L` | `Y` | D - repel |
| Confirm | `U` | `B` | B - strong slash |
| Start | `Enter` | `Start` | start |
| Coin | `Space` | `Back/Select` | insert coin |
| Quit | `Esc` | - | - |

That right-hand column is transposed on purpose, and it caught us out: under
FBNeo's classic pad layout, RetroPad **B** maps to Neo Geo button **A**. So the
button this repo calls `Block` is the one Neo Geo menus accept as "yes", and the
one called `Confirm` is the only one that does not confirm. The full table is in
[09 - The Last Blade 2](09-the-last-blade-2.md).

Three deliberate details:

**Held state, not events.** Rollback needs the input as it was at a specific
frame boundary. An event queue reports what happened somewhere between two
frames, which is not the same question.

**Opposing directions cancel** (SOCD → neutral). An arcade stick physically
cannot report left and right at once. Keeping that guarantee here means the
simulation never has to define what "both" means, and the two peers cannot
disagree about it.

**The analogue dead zone is large**, half the travel. A worn stick reporting a
phantom direction becomes a misprediction on the peer, which is a confusing way
to discover your hardware is dying.

## Commands

```
just                    list everything
just test               fmt + clippy + tests (debug and release) + shellcheck
                        + terraform + docs
just e2e                two processes, a real socket, all five profiles
just bench              180 s per profile, bot vs bot, writes the report
just local-up           Prometheus + Grafana on 127.0.0.1
just play <sim> [rom]   human on P1 against the remote peer
just report             rebuild the report from logs already on disk
just build-core         compile FBNeo in a reproducible container
just clean-logs         delete local logs and reports (does not touch AWS)
```

Recipe arguments are **positional**. `just play sim=lastblade2` does not set
`sim`. Just takes everything after the recipe name as a positional value, so
that passes the literal string `"sim=lastblade2"`. Use
`just play lastblade2 /path/rom.zip`. `just --list` shows each recipe's
parameters and defaults.

## A local session, end to end, no AWS

Useful while developing: two processes on one machine over loopback.

Terminal 1, the hosting peer, standing in for the EC2 instance:

```bash
export ROLLBACK_SESSION_KEY=$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')
cargo run --release -p rollback-bot -- \
    --sim arena --player p2 \
    --bind 127.0.0.1:7000 \
    --profile combined --duration 180 \
    --metrics 127.0.0.1:9899
```

Terminal 2, you:

```bash
export ROLLBACK_SESSION_KEY=<the same key as terminal 1>
cargo run --release -p rollback-client -- \
    --sim arena --peer 127.0.0.1:7000 \
    --profile combined
```

The key must be **identical** on both sides. Otherwise every datagram fails the
HMAC and the handshake times out. That is correct behaviour, but the symptom
(`no compatible peer answered`) is not obvious. See
[12 - Troubleshooting](12-troubleshooting.md).

For The Last Blade 2, add `--core cores/fbneo_libretro.so --rom /path/lastbld2.zip`
to both, give each peer its **own** `--system-dir`, and put `neogeo.zip` in each.
Two peers sharing one system directory race to clear and rewrite the same NVRAM
file.

## The overlay

The client draws what the netcode is doing, in the corner:

```
FRAME 1234 CONFIRMED 1230 AHEAD 3
PREDICTED 4021 WRONG 260 ACC 94%
ROLLBACKS 260 DEPTH 3 MAX 6
RESIM 812 STALLS 0 STATE 204B
RTT 41MS VAR 7MS LOSS 2%
SENT 1234 RECV 1210 DUP 4 REORD 11
PROFILE COMBINED
```

And along the bottom, a strip of the last 180 frames, one pixel each:

| Colour | Means |
|---|---|
| Green | **Confirmed** - both inputs were known, nothing was guessed |
| Yellow | **Predicted** - the remote input was a guess |
| Red | **Corrected** - a rollback happened on this frame |
| Grey | **Stalled** - the prediction window filled and the simulation waited |

One detail about the strip: a rollback is painted on the frame where it was
**noticed**, not on the frames it actually re-simulated. Those have already
scrolled away, and rewriting history there would hide *how late* the correction
was, which is the interesting part.

A healthy session is almost all green and yellow with sparse red. Continuous
grey means the peer stopped talking.

## What to expect from each profile

Running `just bench`, the profiles behave distinctly:

- **`natural`**: no impairment. On loopback almost nothing is predicted: the
  remote input arrives before the frame it belongs to.
- **`delay20`**: 20 ms each way. The peer sits roughly 2.5 frames behind, so
  nearly every frame is predicted and corrections show up.
- **`jitter30`**: 30 ± 15 ms. Prediction depth wanders. This is the profile
  that exercises RTT variation, and the one that widens the depth distribution
  without raising its mean.
- **`loss2`**: 2% loss, no delay. The eight-input redundancy absorbs nearly all
  of it; the interesting part is watching inferred loss climb while rollbacks do
  not follow.
- **`combined`**: 40 ± 20 ms, 2% loss, 0.5% reordering. The bad case.

## Where the artefacts go

```
artifacts/
├─ logs/          one .jsonl per session, per peer
├─ report/        summary.csv and report.html
├─ video/         recordings, when --record was used
├─ system/        FBNeo's NVRAM directory (must match between peers)
└─ session.key    ephemeral key, mode 0600, deleted by `just aws-down`
```

Nothing under `artifacts/` is version controlled.

## The JSONL log

One JSON object per line. JSONL rather than a single document because a session
that dies mid-write still leaves a file readable up to the last complete line,
which is exactly when the log matters.

Record kinds: `session_start`, `local_input`, `remote_inputs`, `sent`,
`received`, `session` (engine events: advanced, stalled, rolled_back,
checksum_matched, desync), `metrics` (a full snapshot every 60 frames) and
`session_end`.

For a quick look:

```bash
# how many rollbacks, and how deep
jq -r 'select(.event=="rolled_back") | .depth' artifacts/logs/*-p1-bench.jsonl \
  | sort -n | uniq -c

# the final line, with every counter
jq 'select(.record=="session_end")' artifacts/logs/*-p1-bench.jsonl
```

For anything more involved, load it into Elastic. See
[15 - Elastic](15-elastic.md).

## Wayland, SDL2 and the frame clock

The client uses SDL2 with vsync off: the **session** owns the frame clock, not
the monitor. A 144 Hz display cannot make the simulation run at 144 Hz.

If the window does not open, check `XDG_SESSION_TYPE`. On a bare TTY there is no
compositor and SDL has nowhere to draw. `rollback-bot` is headless and works
fine in those conditions, which is what the automated tests use.
