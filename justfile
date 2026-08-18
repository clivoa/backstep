# Rollback Netcode lab.
#
#   just               list the recipes
#   just test          the full gate: fmt, clippy, tests, shellcheck, terraform
#   just local-up      Prometheus + Grafana on loopback
#   just aws-up <sim> [rom]     sim = arena | sfa3 | lastblade2
#   just play   <sim> [rom]
#   just bench
#
# Recipe arguments are POSITIONAL. `just play sim=sfa3` does not set `sim` --
# just takes everything after the recipe name as a positional value, so that
# passes the literal string "sim=sfa3". Use `just play sfa3 /path/rom.zip`.
# `just --list` shows each recipe's parameters and defaults.
#   just collect       ALWAYS before aws-down
#   just aws-down
#
# Every recipe runs from the repository root.

set shell := ["bash", "-euo", "pipefail", "-c"]

root := justfile_directory()
core := root / "cores/fbneo_libretro.so"
logs := root / "artifacts/logs"
report_dir := root / "artifacts/report"

# The session key never appears on a command line: an argument is visible in
# `ps` to every user on the machine. It comes from the environment or a file.
key_file := root / "artifacts/session.key"

default:
    @just --list --unsorted

# --- gates -----------------------------------------------------------------

# Everything CI checks, in the order that fails fastest.
test: fmt-check lint unit shell-lint tf-check docs-check
    @echo "==> all gates passed"

fmt:
    cargo fmt --all
    terraform -chdir=terraform fmt

fmt-check:
    @echo "==> cargo fmt"
    cargo fmt --all -- --check

lint:
    @echo "==> clippy"
    cargo clippy --workspace --all-targets -- -D warnings

# The unit, integration, property and golden suites, in debug and release.
#
# Release matters: the 100 000-frame replay must produce the same checksum at
# both optimisation levels, and that is a real class of desync bug.
unit:
    @echo "==> tests (debug)"
    cargo test --workspace
    @echo "==> determinism replay (release)"
    cargo test --release -p rollback-arena --test replay_100k

shell-lint:
    @echo "==> shellcheck"
    @if command -v shellcheck >/dev/null; then \
        shellcheck ops/scripts/*.sh; \
    else \
        echo "    shellcheck not installed; skipping"; \
    fi

# The docs are a deliverable, so they get a gate too: links resolve, anchors
# resolve, and nothing is orphaned from the index.
docs-check:
    @echo "==> docs"
    @python3 ops/scripts/check-docs.py

tf-check:
    @echo "==> terraform fmt + validate"
    terraform -chdir=terraform fmt -check -diff
    terraform -chdir=terraform init -backend=false -input=false >/dev/null
    terraform -chdir=terraform validate

# Two real processes over a real socket, every profile, no confirmed desync.
#
# The emulated games need a longer duration than the arena: their boot script
# spends the first ~33 seconds walking the machine through its menus.
e2e duration="20" sim="arena" rom="":
    DURATION={{duration}} SIM={{sim}} ROM={{rom}} ops/scripts/e2e-local.sh

# --- build -----------------------------------------------------------------

build:
    cargo build --release

# Compile the pinned FBNeo libretro core in a reproducible container.
build-core:
    ops/scripts/build-fbneo.sh

# Ask the core what it thinks of a ROM. FBNeo reports an unusable romset only on
# the emulated screen, so this is how the failure gets a shape.
inspect-core rom:
    cargo run --release -p rollback-libretro --example inspect-core -- \
        "{{core}}" "{{rom}}" artifacts/system

# Does the core produce the same state twice, in two processes, seconds apart?
# If not, every session will desync and it will not be the rollback's fault.
# See docker/fbneo/determinism.md.
check-determinism rom game="lastblade2":
    ops/scripts/check-determinism.sh "{{rom}}" "{{game}}"

# Does a rollback change anything the game can see? Saves a state, runs on,
# restores, replays the same inputs, and compares. Answers the question the
# desync counter cannot: whether a desync verdict is trustworthy at all.
check-rollback-safety rom game="lastblade2" from="500" to="3200" step="150":
    cargo run --release -p rollback-libretro --example check-rollback-safety -- \
        "{{core}}" "{{rom}}" artifacts/system "{{game}}" "{{from}}" "{{to}}" "{{step}}"

# Run the machine and dump frames, to calibrate a boot macro by looking at it
# rather than guessing. Writes PPMs; montage them into a contact sheet.
probe-boot rom game="lastblade2" frames="1800" every="30" out="artifacts/probe":
    rm -rf "{{out}}" && mkdir -p "{{out}}"
    PROBE_SCRIPT="{{game}}" cargo run --release -p rollback-libretro --example probe-boot -- \
        "{{core}}" "{{rom}}" artifacts/system "{{out}}" "{{frames}}" "{{every}}"
    @command -v magick >/dev/null && \
        magick montage "{{out}}"/*.ppm -tile 6x -geometry +2+2 -background '#222' \
            -fill white -pointsize 14 -label '%f' "{{out}}/contact-sheet.png" && \
        echo "    contact sheet: {{out}}/contact-sheet.png" || \
        echo "    (install ImageMagick for a contact sheet; raw PPMs are in {{out}})"

# --- local observability ---------------------------------------------------

# Prometheus on :9090 and Grafana on :3000, both bound to loopback.
local-up:
    docker compose -f ops/docker-compose.yml up -d
    @echo
    @echo "    Grafana     http://127.0.0.1:3000"
    @echo "    Prometheus  http://127.0.0.1:9090"
    @echo "    Exporter    http://127.0.0.1:9898/metrics (once a session is running)"

local-down:
    docker compose -f ops/docker-compose.yml down

local-logs:
    docker compose -f ops/docker-compose.yml logs -f --tail=100

# --- Elastic ---------------------------------------------------------------

# Elasticsearch on :9200 and Kibana on :5601, both bound to loopback.
elastic-up:
    docker compose -f ops/elastic/docker-compose.yml up -d
    @echo
    @echo "    Kibana         http://127.0.0.1:5601  (takes ~40 s to be ready)"
    @echo "    Elasticsearch  http://127.0.0.1:9200"
    @echo
    @echo "    Then: just elastic-load"

elastic-down:
    docker compose -f ops/elastic/docker-compose.yml down

# Index the session logs. `all=1` also indexes every datagram record, which is
# tens of thousands of documents per session.
elastic-load logs="artifacts/logs" all="":
    @python3 ops/scripts/elastic-load.py --logs "{{logs}}" {{ if all == "" { "" } else { "--all" } }}

# Drop the indices and load again.
elastic-reload logs="artifacts/logs":
    @python3 ops/scripts/elastic-load.py --logs "{{logs}}" --reset

# The analyses a single-row summary cannot do: depth distribution, latency
# tail, whether rollbacks cluster, where the frame budget goes.
elastic-analyze session="":
    @python3 ops/scripts/elastic-analyze.py {{ if session == "" { "" } else { "--session " + session } }}

# --- video -----------------------------------------------------------------

# Record one annotated video per network profile, both peers, side by side.
# Needs ffmpeg. The arena has no framebuffer, so this is emulated games only.
record-scenarios rom duration="90" profiles="natural delay20 jitter30 loss2 combined":
    ROM="{{rom}}" DURATION="{{duration}}" PROFILES="{{profiles}}" \
        ops/scripts/record-scenarios.sh

# Burn a session's telemetry into an existing recording.
annotate video log out label="":
    @python3 ops/scripts/annotate-video.py "{{video}}" "{{log}}" "{{out}}" "{{label}}"

# --- AWS -------------------------------------------------------------------

# Apply the Terraform, push a fresh session key to SSM, upload the binaries and
# start the remote peer. Needs terraform/terraform.tfvars.
aws-up sim="arena" rom="" profile="natural" duration="180":
    SIM={{sim}} ROM={{rom}} PROFILE={{profile}} DURATION={{duration}} ops/scripts/aws-up.sh

# What the report needs from the remote side. Run this BEFORE aws-down.
collect:
    ops/scripts/collect.sh

# Destroy everything, including the ROM and the remote logs. Refuses to run if
# `collect` has not been run; override with FORCE=1.
aws-down:
    ops/scripts/aws-down.sh

# The plan, without applying it. Useful for reviewing a change to the infra.
aws-plan:
    terraform -chdir=terraform init -input=false
    terraform -chdir=terraform plan -input=false

# --- playing ---------------------------------------------------------------

# Human on P1 against the remote peer. `peer` defaults to the Terraform output.
play sim="arena" rom="" profile="natural" peer="":
    #!/usr/bin/env bash
    set -euo pipefail
    peer="{{peer}}"
    if [[ -z "$peer" ]]; then
        peer="$(terraform -chdir=terraform output -raw peer_address)"
    fi
    args=(--sim {{sim}} --peer "$peer" --profile {{profile}} --log-dir "{{logs}}")
    if [[ "{{sim}}" != "arena" ]]; then
        if [[ -z "{{rom}}" ]]; then
            echo "sim={{sim}} needs rom=/path/to/game.zip" >&2
            exit 2
        fi
        args+=(--core "{{core}}" --rom "{{rom}}")
    fi
    export ROLLBACK_SESSION_KEY_FILE="${ROLLBACK_SESSION_KEY_FILE:-{{key_file}}}"
    cargo run --release -p rollback-client -- "${args[@]}"

# Bot against bot, 180 s on every profile, repeatable from the seed.
bench duration="180" sim="arena" rom="":
    DURATION={{duration}} SIM={{sim}} ROM={{rom}} ops/scripts/bench.sh

# Rebuild summary.csv and report.html from whatever logs are already on disk.
report:
    cargo run --release -p rollback-report -- --logs "{{logs}}" --out "{{report_dir}}"

# --- housekeeping ----------------------------------------------------------

# Delete local session logs and reports. Does not touch AWS.
clean-logs:
    rm -rf "{{logs}}" "{{report_dir}}" artifacts/e2e

clean: clean-logs
    cargo clean
