# 12 - Troubleshooting

## The handshake times out

```
Error: handshake failed
Caused by: no compatible peer answered within 60s
```

In order of likelihood:

1. **Different session keys.** Every datagram fails the HMAC and is discarded
   *before* becoming a message, so the symptom is silence rather than refusal.
   Check with `curl -s http://127.0.0.1:9898/metrics | grep auth_failures`. If
   that is climbing, someone is talking with the wrong key.
2. **`allowed_cidr` is not your current address.** Residential addresses move.
   Compare `curl -s https://checkip.amazonaws.com` with
   `terraform -chdir=terraform output allowed_cidr`.
3. **UDP blocked on the path.** Some ISPs and corporate networks drop UDP on
   high ports. Test with `nc -u <ip> 7000` from both ends.
4. **The remote peer is not running.** `aws ssm start-session --target <id>`,
   then `systemctl status rollback-bot`.

On that last one: `aws-up` now proves the peer is listening with
`ss -lun | grep -q :7000` before it claims success, so a silent dead peer should
be rarer than it was. If `aws-up` printed a failure and you continued anyway,
this is why.

## The handshake is refused, with a reason

```
Error: peer refused the session: ROM hash mismatch
```

That is the system working.

| Reason | Cause | Fix |
|---|---|---|
| `protocol version mismatch` | incompatible protocol builds | rebuild both sides |
| `peers chose different simulations` | one `--sim arena`, one `--sim lastblade2` | use the same |
| `peers run different application builds` | different commits | `just aws-up` again, from the same commit |
| `session configuration mismatch` | different `--input-delay` or `--prediction-limit` | use the same values |
| `session seed mismatch` | different `--seed` | use the same seed |
| `libretro core hash mismatch` | differently built cores | copy the same `.so` to both |
| `ROM hash mismatch` | different ROM or BIOS | use exactly the same files, including `neogeo.zip` |
| `both peers asked for the same player slot` | two `--player p1` | one must be `p2` |

The ROM hash covers the BIOS as well, so a mismatched `neogeo.zip` reports as a
ROM mismatch. That is deliberate: a Neo Geo game is only half the code that
runs.

## The session stalls, with a continuous grey band

Grey in the overlay means a stall: the prediction window filled and the
simulation stopped.

Occasional stalls under high jitter are normal. That is the prediction limit
doing its job, keeping a rollback from reaching further back than the state
buffer goes.

Continuous grey means the peer stopped talking. After three seconds with no
authenticated datagram, the session ends with `PeerTimeout`.

Look at `rollback_inferred_lost_total` and `rollback_srtt_seconds`. If RTT
spiked, the network got worse. If loss went to 100%, the path is down.

## Confirmed desync

```
Error: session ended in a confirmed desync
```

The two simulations diverged. Full diagnosis in
[05 - Determinism](05-determinism.md); in short:

```bash
# which frame, and the two checksums
jq 'select(.event=="desync")' artifacts/logs/*.jsonl

# were both peers on the same commit?
jq -r 'select(.record=="session_start") | .info.app_commit' artifacts/logs/*.jsonl
```

Then: reproduce with `just bench` at the same seed and profile, try debug
against release, and try the arena before the emulated game. If the arena is
clean and the emulator is not, read the next section but one.

## `effective_fps` well below 60

The peer cannot simulate at 60 Hz.

```bash
# how much of the 16 667 µs budget is in use
curl -s http://127.0.0.1:9898/metrics | grep -E "advance_seconds|save_state_seconds"
```

On the EC2 instance with The Last Blade 2, the usual cause is `retro_serialize`
on a `t3.small`. Switch to `t3.medium` in `terraform.tfvars` and run
`just aws-up` again. The measured split is in [15 - Elastic](15-elastic.md):
`advance` around 3 948 µs and `save_state` around 2 271 µs per frame.

Locally, check you are not running a debug build. The `justfile` uses release,
but a hand-typed `cargo run` without `--release` is about ten times slower.

## Prometheus is not collecting anything

```
http://127.0.0.1:9898/metrics  down
```

- **No session running.** The exporter only exists while a session does. That is
  the normal state between runs.
- **The stack is not on host networking.** The `docker-compose` uses
  `network_mode: host` on purpose, to reach a loopback exporter. On a bridge
  network it cannot; see [06 - AWS](06-aws.md) for why the exporter is not bound
  to `0.0.0.0`.
- **Not Linux.** Docker host networking is Linux-only.

## The SDL client does not open

```bash
echo $XDG_SESSION_TYPE     # expected: wayland or x11
```

On a bare TTY there is no compositor and SDL has nowhere to draw. `rollback-bot`
is headless and works there, which is what the automated tests use.

If no gamepad appears, that is not an error: the keyboard is a complete input
device on its own. The client prints `gamepad: <name>` when it finds one.

## `LocalInputRefiled`

```
Error: local input for frame 1234 was queued twice with different values
```

A bug in the calling loop: it queued a local input during a stall.
`SessionRunner` checks `would_stall()` before reading the controller precisely
to prevent this. If you see it, a custom loop skipped that check.

## `PeerContradiction`

```
Error: peer sent two different inputs for frame 1234
```

The peer sent two different values for the same frame. This is not a network
artefact; duplication and reordering are absorbed silently. It is either a buggy
peer or a forged datagram that passed the HMAC, which would mean the key leaked.

## `HistoryExhausted`

```
Error: cannot roll back to frame 1200: oldest saved state is 1208
```

A rollback needed to reach further back than the state buffer goes. It should
not happen: `SessionConfig::validate` requires
`state_history > prediction_limit`.

If it does, either the configuration was built around the validator, or there is
a bug in the prediction-depth bookkeeping. This is the exact error the property
tests produced when they found the stall-condition bug.

## `terraform apply` fails

- `InvalidClientTokenId`: wrong or expired AWS credentials.
  `aws sts get-caller-identity`.
- `UnauthorizedOperation`: missing IAM permission. The lab needs EC2, VPC, S3,
  IAM and SSM.
- `AddressLimitExceeded`: Elastic IP limit in the region. Probably orphaned EIPs
  from an earlier run; see [11 - Cleanup](11-cleanup.md).
- `Invalid rule description`: an apostrophe or another character AWS rejects in a
  security group description. `terraform validate` accepts it; only the API
  refuses.
- `terraform.tfvars is missing`: copy it from `example.tfvars`.

## The remote peer does not start

`aws-up` fails loudly now, and the message says the infrastructure is up and
costing money. The three failures seen in practice, all documented in
[06 - AWS](06-aws.md):

- `Illegal option -o pipefail` or `source: not found`: SSM runs `/bin/sh`, which
  is dash on Ubuntu.
- `Package 'awscli' has no installation candidate`: Ubuntu 24.04 dropped the
  package, which killed `user_data` on its first line.
- `cannot open /opt/rollback/env`: the command raced the bootstrap. `aws-up`
  waits for `/opt/rollback/BOOTSTRAP_COMPLETE` now.

To look at the instance directly:

```bash
aws ssm start-session --region eu-central-1 --target <instance-id>
sudo tail -50 /var/log/rollback-bootstrap.log
sudo journalctl -u rollback-bot -n 50
```

## The FBNeo build fails

```bash
just build-core
```

- `No rule to make target`: the pinned commit has no `src/burner/libretro`. That
  is the problem described in [09](09-the-last-blade-2.md): the libretro port
  lives in the `libretro/FBNeo` fork, not upstream.
- `kNetGame declaration not found`: the determinism patch could not apply. The
  build fails on purpose rather than producing a core that desyncs. See
  `docker/fbneo/determinism.md`.
- Out of space: the build needs about 5 GB. `docker system prune`.
- Slow: 20 to 40 minutes on a ten-core machine is expected. Tune with `JOBS=n`.

## `core reports a serialize size of zero`

```
Error: loading ROM "/path/lastbld2.zip"
Caused by: core reports a serialize size of zero, so no game is actually running.
```

The core loaded, the ROM was accepted, and no game is running. FBNeo returns
success from `retro_load_game` even with an unusable romset, so a zero state
size is how an incomplete set surfaces here.

The cause is almost always a missing file. Read the `core error:` lines just
below: FBNeo names every file it required and could not find.

```
core error: [FBNeo] ROM at index 128 with name sp-s3.sp1 and CRC 0x91b64be3 is required
```

Two cases produce a zip that looks complete and still will not run:

| Symptom | Missing | Where it goes |
|---|---|---|
| `sp-s3.sp1`, `sm1.sm1`, `sfix.sfix`, `000-lo.lo` | `neogeo.zip`, the Neo Geo BIOS | beside the game, or in `artifacts/system/` |

To see everything the core said, including which paths it searched for each
romset:

```bash
just inspect-core /path/to.zip
```

That prints the full core log, the environment commands it asked for, and the
state size. To check file by file, compare your zip's CRCs against the driver's
`RomDesc[]` in FBNeo: `src/burn/drv/neogeo/d_neogeo.cpp` for Neo Geo,
`src/burn/drv/capcom/d_cps2.cpp` for CPS-2.

## Every emulated session desyncs at the start

If the arena never desyncs and the emulated game always does, particularly
**before any player input**, the problem is not the rollback. It is the core.

```bash
just check-determinism /path/lastbld2.zip
```

That runs the core twice, in separate processes, with a deliberate pause between
them, and compares machine state. The pause is the test: `time(NULL)` has
one-second granularity, so two runs inside the same second agree even on a
clock-dependent core.

If it fails, the core was probably not built by `just build-core`:

```bash
grep patches cores/fbneo-commit.txt     # expected: patches=kNetGame=1
```

Background in `docker/fbneo/determinism.md` and
[05 - Determinism](05-determinism.md).

Other early-desync causes, in order of frequency:

| Symptom | Cause | Fix |
|---|---|---|
| desync at boot, core is patched | different NVRAM between peers | it is cleared automatically; check both peers printed `cleared stale machine state` |
| `ROM hash mismatch` on a Neo Geo game | different `neogeo.zip` | the BIOS is in the hash; use the same file on both sides |
| desync in the middle of a menu | the boot script fell outside its window | `just probe-boot` and compare against [09](09-the-last-blade-2.md) |

## The boot script ends up outside the match

The symptom is a session that runs, measures everything, does not desync, and
shows the attract loop instead of a fight. The script is blind: it presses
buttons on the frames it was told to and checks nothing.

```bash
just probe-boot /path/lastbld2.zip lastblade2
```

Open `artifacts/probe/contact-sheet.png` and see which screen each frame landed
on. The measured windows are in [09](09-the-last-blade-2.md). Note that the
board wants menu buttons *held*, not tapped: a twelve-frame tap starts nothing
at any frame.

## The core log is empty

```
-- core log --
(none -- the core did not ask for the log interface)
```

The host offers `GET_LOG_INTERFACE` through a C shim
(`crates/rollback-libretro/src/log_shim.c`), because `retro_log_printf_t` is
variadic and stable Rust cannot define a variadic function. If the log comes
back empty with FBNeo, the shim was not compiled: check that `build.rs` ran and
that a C compiler is on `PATH`.

## The report is empty

```
0 session(s) read from artifacts/logs
```

No `.jsonl` in the directory. Run `just bench` or `just collect` first.

If there are files but they show as incomplete, the session died before writing
`session_end`. The report still uses what arrived and marks `complete=false`;
look at the end of the file for the last recorded state.

## The handshake fails, or nothing appears on screen

Three defects sat here undiscovered for the whole project, because every session
until the first human one was launched by a single script that got all three
right by accident. They are worth listing together, because they compound: each
one makes the next harder to see.

### "peer refused the session: session configuration mismatch"

The handshake hashes **simulation, seed, tick rate, input delay, prediction
limit and state history** into one number and refuses the session if a single
field differs. The message names the category, not the field, because the hash
cannot say which one moved.

Both peers now print their own identity when refused, so the two lines can be
diffed:

```
  this peer: protocol v1 sim LastBlade2 player P1 seed 0x0000000000001092 config 0x45f2… commit 44141777
```

The usual culprit is **seed**. `aws-up` defaults to 4242; the client's built-in
default is `0x123456789ABCDEF0`. Before this was fixed, `just aws-up` followed
by `just play` - the documented path - always failed this way.

`just play` now reads `artifacts/session.env`, which `aws-up` writes with
exactly what the remote peer was started with, so the two agree by construction
rather than by both hard-coding the same numbers.

### The client hangs, then times out, and the peer is gone

The remote peer used to wait **120 seconds** for a handshake and then exit. That
is right for `bench`, where a script starts both peers seconds apart, and wrong
for a person, who has to read the output and sit down.

The failure is nasty because the instance stays up. It answers SSM, it bills,
and `systemctl is-active` reports `active` right up until it does not, so a
check a minute earlier tells you nothing. From the client it looks like a
handshake that hangs for no reason.

`aws-up` now passes `--handshake-timeout 900`. To confirm what a running peer
actually has, read the process rather than the log line:

```bash
aws ssm send-command --instance-ids i-… --document-name AWS-RunShellScript \
  --parameters 'commands=["ps -eo args | grep [r]ollback-bot"]'
```

### One bad connection attempt killed the host

A refused handshake used to end the host's process too, so a single wrong flag
cost a full redeploy: the operator fixes the flag, retries, and finds nothing
listening. The host now logs the refusal, prints both identities, and keeps
waiting.

### The game window never appears

The client opens its SDL window **after** the handshake succeeds, so a failing
handshake shows nothing at all on screen. Watch the terminal: the window follows
the `connected:` line and nothing before it.

If `connected:` did print and there is still no window, it opened on another
workspace. Under Hyprland:

```bash
hyprctl clients -j | jq -r '.[] | "\(.class)  \(.title)  ws=\(.workspace.name)"'
```

The window is titled `rollback-netcode :: <simulation> :: <profile>`. Click it
before playing: SDL only receives the keyboard when the window has focus.

## Asking for help usefully

Gather:

```bash
just test 2>&1 | tail -40
jq -c 'select(.record=="session_start" or .record=="session_end")' artifacts/logs/*.jsonl
curl -s http://127.0.0.1:9898/metrics | grep -v '^#'
git rev-parse HEAD
```

The two `session_start` records show whether the peers agreed on commit, seed
and configuration, which answers most questions before they are asked.
