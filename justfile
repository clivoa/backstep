# Rollback Netcode lab.
#
#   just               list the recipes
#   just test          the full gate: fmt, clippy, tests, shellcheck, terraform
#   just local-up      Prometheus + Grafana on loopback
#   just aws-up sim=arena|sfa3 [rom=...]
#   just play sim=arena|sfa3 [rom=...]
#   just bench
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
test: fmt-check lint unit shell-lint tf-check
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

tf-check:
    @echo "==> terraform fmt + validate"
    terraform -chdir=terraform fmt -check -diff
    terraform -chdir=terraform init -backend=false -input=false >/dev/null
    terraform -chdir=terraform validate

# Two real processes over a real socket, every profile, no confirmed desync.
e2e duration="20":
    DURATION={{duration}} ops/scripts/e2e-local.sh

# --- build -----------------------------------------------------------------

build:
    cargo build --release

# Compile the pinned FBNeo libretro core in a reproducible container.
build-core:
    ops/scripts/build-fbneo.sh

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
    if [[ "{{sim}}" == "sfa3" ]]; then
        if [[ -z "{{rom}}" ]]; then
            echo "sim=sfa3 needs rom=/path/to/sfa3.zip" >&2
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
