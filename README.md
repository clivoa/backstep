# Rollback Netcode Lab: Madrid ⇄ Frankfurt, São Paulo, Tokyo

A Rust workspace that demonstrates rollback netcode at two levels:

1. A deterministic, fully instrumented **2D arena**. Every byte of state is
   auditable, the snapshot is 204 bytes, and the checksum covers all of it.
2. **The Last Blade 2** running on the official FBNeo core through the libretro
   API, driven by exactly the same rollback engine.

The local player drives P1 through SDL2. A headless EC2 instance drives P2
through a scripted FSM. The peers exchange only inputs, over UDP authenticated
with HMAC-SHA256, while Prometheus, Grafana, JSONL logs and an HTML report
record connectivity, predictions, rollbacks and desyncs.

Both simulations ran from one desk in Madrid against three AWS regions on three
continents, so distance is a variable the results actually cover rather than an
assumption.

![A rollback session, delay20 profile](docs/media/rollback-delay20.gif)

*The Last Blade 2 under rollback. The band across the top is the session's own
telemetry, burned in from its log: 142 rollbacks so far, 18.5% of a frame's work
being re-simulated, and `ROLLBACK -5` firing on the frame it happened. The game
underneath does not stutter.*

## Results

Both simulations ran between a desktop in Madrid and EC2 instances in Frankfurt,
São Paulo and Tokyo, over the public internet.

| | Arena | The Last Blade 2 |
|---|---|---|
| Snapshot size | 204 bytes | 415 155 bytes |
| CPU, 300 s session | ~2 s | ~116 s |
| Prediction accuracy | 92.7–95.5% | 91.5–92.7% |
| Desyncs | 0 | 0 |

**2 997 checksum comparisons agreed, across three continents and zero desyncs.**
Two different CPUs, two operating systems, two libcs. That is the evidence
behind the determinism rules in [05](docs/05-determinism.md), and two processes
of one binary on one machine could never have produced it.

What distance does, at a glance:

| From Madrid to | SRTT | Max depth | Stalls | FPS |
|---|---|---|---|---|
| Frankfurt | 50 ms | 3 | 0 | 60.01 |
| São Paulo | 272 ms | 8 (the limit) | 827 | 57.37 |
| Tokyo | 267 ms | 8 (the limit) | 1 283 | 56.01 |

The default 8-frame prediction window is 133 ms at 60 Hz — exactly one way at
267 ms. Beyond that the session stops speculating and waits, which is the design
working correctly and also the point at which it needs retuning. What that
retuning costs, and who pays for it, is measured in
[08 — Experiments](docs/08-experiments.md) along with all five synthetic
profiles.

## Documentation

| Document | Subject |
|---|---|
| [00 — Glossary](docs/00-glossary.md) | Start here. Every technical term from scratch |
| [01 — Theory](docs/01-theory.md) | What rollback is, why it exists, what it costs |
| [02 — Architecture](docs/02-architecture.md) | The crates, and why the boundary sits where it does |
| [03 — Protocol](docs/03-protocol.md) | Datagram format, authentication, handshake |
| [04 — Running locally](docs/04-running-locally.md) | Controls, commands, a session end to end |
| [05 — Determinism](docs/05-determinism.md) | The rules that prevent desync, and how they were checked |
| [06 — AWS](docs/06-aws.md) | The infrastructure, the threat model, the session key |
| [07 — Dashboard](docs/07-dashboard.md) | Prometheus, Grafana, what each panel means |
| [08 — Experiments](docs/08-experiments.md) | Five profiles, the method, the results |
| [09 — The Last Blade 2](docs/09-the-last-blade-2.md) | The FBNeo core, the boot script, the pinned commit |
| [10 — Costs](docs/10-costs.md) | What a session costs, and where money disappears |
| [11 — Cleanup](docs/11-cleanup.md) | How to destroy everything and confirm it went |
| [12 — Troubleshooting](docs/12-troubleshooting.md) | Symptoms, causes, what to look at first |
| [13 — Coverage](docs/13-coverage.md) | What was validated, what was not |
| [14 — Video](docs/14-video.md) | Recording sessions, and watching rollback happen |
| [15 — Elastic](docs/15-elastic.md) | Per-event analysis: what `summary.csv` cannot answer |
| [16 — The algorithm](docs/16-algorithm.md) | Data structures, invariants, code paths, complexity |

Diagrams: [system topology and crate graph](docs/02-architecture.md#the-system-end-to-end),
[a rollback on a timeline](docs/16-algorithm.md#one-correction-on-a-timeline),
[AWS network topology](docs/06-aws.md#what-gets-built).

## Against the specification

What the lab set out to do, and what actually happened. The two deviations are
listed with the rest rather than buried.

| Requirement | Status | Where |
|---|---|---|
| Rollback engine: prediction, limit, history, re-simulation | done | [16](docs/16-algorithm.md) |
| Defaults: 1 input delay, 8 prediction, 16 states, 60 Hz | done | `SessionConfig::default` |
| Stop on window full; end after 3 s of silence | done | [16](docs/16-algorithm.md#the-stall-condition) |
| Checksums every 60 confirmed frames; desync ends the session | done | [05](docs/05-determinism.md) |
| UDP/7000, versioned wire, 1 200-byte limit, HMAC-SHA256 | done | [03](docs/03-protocol.md) |
| Six message types, 8-input redundancy, sequence + ACK | done | [03](docs/03-protocol.md) |
| Handshake validates version, commit, config, seed, hashes | done | [03](docs/03-protocol.md) |
| Integer-only 2D arena + FSM bot | done | [02](docs/02-architecture.md) |
| SDL2 client, overlay, keyboard and gamepad | built, **never played by a person** | [13](docs/13-coverage.md) |
| FBNeo core in a reproducible container | done, **different commit** | [09](docs/09-the-last-blade-2.md) |
| Emulated game via `retro_serialize`, scripted boot, no ROM offsets | done | [09](docs/09-the-last-blade-2.md) |
| **Street Fighter Alpha 3** | **not run** — romset lacks `sfa3.key` | [09](docs/09-the-last-blade-2.md) |
| Prometheus, Grafana, JSONL, all listed metrics | done | [07](docs/07-dashboard.md) |
| Five profiles, 180 s, `summary.csv` + self-contained HTML | done | [08](docs/08-experiments.md) |
| Terraform VPC, SSM, S3, IMDSv2, 4 h terminate, no SSH | done | [06](docs/06-aws.md) |
| `just test / local-up / aws-up / play / bench / collect / aws-down` | done | [04](docs/04-running-locally.md) |
| Unit, property, 100 k replay, golden protocol, fake core, E2E | done | [13](docs/13-coverage.md) |
| Gates: fmt, clippy, tests, shellcheck, terraform, docs | done | `just test` |
| AWS smoke: handshake, session, collect, destroy | done, ×6 across three regions | [08](docs/08-experiments.md) |
| Didactic documentation of every technical term | done | [00](docs/00-glossary.md) |

**Two deviations, both forced and both documented.**

*SFA3 was replaced by The Last Blade 2.* The available romset is missing
`sfa3.key`, the 20-byte CPS-2 decryption key, and all eleven SFA3 variants in
FBNeo require one. The substitute exercises the same core, the same libretro
host and the same rollback engine, so nothing being demonstrated changed.

*The FBNeo commit differs.* The spec pinned
`finalburnneo/FBNeo@f1c3545f…`, which has no `makefile.libretro`. The build uses
`libretro/FBNeo@0332bb98…` and records **both** hashes in the artefact's
provenance, so the deviation is visible from the binary rather than only from
this table.

Beyond the spec: video recording with burned-in telemetry ([14](docs/14-video.md)),
per-event Elasticsearch analysis ([15](docs/15-elastic.md)), an algorithm
reference ([16](docs/16-algorithm.md)), and runs against three regions rather
than one.

The one acceptance criterion genuinely unmet is **a human on P1**. Every session
so far has been bot against bot, which leaves the question rollback exists to
answer — what does it feel like to play? — measured by nothing here.

## Quick start

### Prerequisites

| Tool | For | Check |
|---|---|---|
| Rust ≥ 1.82 | building everything | `cargo --version` |
| SDL2 ≥ 2.0.20 | the graphical client | `pkg-config --modversion sdl2` |
| Docker | FBNeo build, Prometheus, Grafana, Elastic | `docker --version` |
| ffmpeg | recording sessions (optional) | `ffmpeg -version` |
| `just` | the commands below | `just --version` |
| Terraform ≥ 1.6 | AWS infrastructure | `terraform version` |
| AWS CLI | `aws-up`, `collect`, `aws-down` | `aws sts get-caller-identity` |
| shellcheck | the script lint gate | `shellcheck --version` |

x86_64. The observability `docker-compose` uses host networking and is therefore
Linux-only; the reason is written in the file itself.

### A full test, no AWS and no ROM

```bash
just test        # fmt, clippy, tests (debug and release), shellcheck, terraform, docs
just e2e         # two real processes, a real socket, all five profiles
just bench       # 180 s per profile, writes summary.csv and report.html
```

`just bench` produces `artifacts/report/summary.csv`, one row per session across
about 37 columns, and `artifacts/report/report.html`, which is self-contained:
no CDN, no script, charts as inline SVG.

### Local observability

```bash
just local-up
# Grafana     http://127.0.0.1:3000
# Prometheus  http://127.0.0.1:9090
# Exporter    http://127.0.0.1:9898/metrics
```

### A session against AWS

```bash
cp terraform/example.tfvars terraform/terraform.tfvars
$EDITOR terraform/terraform.tfvars     # allowed_cidr = your address, as a /32
curl -s https://checkip.amazonaws.com  # to find it

just aws-up arena
just play arena
just collect      # ALWAYS before aws-down
just aws-down
```

### The Last Blade 2

You supply the ROM. It needs `neogeo.zip`, the Neo Geo BIOS, in
`artifacts/system/`: a Neo Geo game is only half the code that runs, and the
BIOS is hashed into the handshake alongside the ROM.

```bash
just build-core                                  # builds FBNeo in a container
just check-determinism /path/lastbld2.zip        # check the core before anything
just e2e 90 lastblade2 /path/lastbld2.zip
just aws-up lastblade2 /path/lastbld2.zip
just play lastblade2 /path/lastbld2.zip
```

`just check-determinism` is not fussiness. FBNeo as shipped seeds its RNG and
its emulated calendar clock from the host clock, so two peers that start in
different wall-clock seconds diverge before the first input. `just build-core`
patches that; the measurement and the fix are in
[05 — Determinism](docs/05-determinism.md).

## What this repository does not contain

No ROMs and no BIOS. `lastbld2.zip` and `neogeo.zip` are yours, and are never
committed, redistributed, or included in any artefact here.

No savestates or personal logs. All of `artifacts/` is gitignored.

No keys. The session key is ephemeral, generated per run, kept in SSM
SecureString and in a local file with mode 0600, and never enters Terraform
state or a command line.

## Street Fighter Alpha 3

The lab specification named SFA3, and the code still carries the boot script and
the `--sim sfa3` path. It was never run, because the available romset is
missing `sfa3.key`, the 20-byte CPS-2 decryption key. All eleven SFA3 variants
in FBNeo require one, so no other revision avoids it.

The Last Blade 2 took its place. It exercises the same core, the same libretro
host, the same `LibretroSimulation` and the same rollback engine, so the thing
being demonstrated is unchanged. The full diagnosis, down to the FBNeo source
that makes the key mandatory, is in
[09 — The Last Blade 2](docs/09-the-last-blade-2.md).

Nothing in the measured results depends on SFA3, and nothing claims it ran.

## Out of scope

STUN, relay, matchmaking, spectating, reconnection, state synchronisation,
Tekken 3, vision-based AI and memory-reading bots. Fightcade is a reference
point, not a dependency.

Multiple regions were originally out of scope and are now covered: Frankfurt,
São Paulo and Tokyo, with `ops/scripts/region-run.sh` to reproduce any of them.
