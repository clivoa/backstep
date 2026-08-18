#!/usr/bin/env bash
# Run the full scenario against one AWS region: bring up, play both
# simulations, collect, tear down.
#
#   ./ops/scripts/region-run.sh eu-central-1 frankfurt /path/lastbld2.zip
#   ./ops/scripts/region-run.sh sa-east-1    saopaulo  /path/lastbld2.zip
#   ./ops/scripts/region-run.sh ap-northeast-1 tokyo   /path/lastbld2.zip
#
# Distance is the independent variable. Frankfurt is 50 ms from Madrid and sits
# comfortably inside the 8-frame prediction window; São Paulo and Tokyo do not,
# which is the point of running them. A prediction limit is a bet about how far
# away your opponent is, and this is how you find out where the bet stops
# paying.
#
# Every log is labelled with the region, so runs never become ambiguous on disk
# and never overwrite each other.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT}"

REGION="${1:?usage: region-run.sh <aws-region> <label> [rom]}"
LABEL="${2:?usage: region-run.sh <aws-region> <label> [rom]}"
ROM="${3:-}"

GAME_DURATION="${GAME_DURATION:-300}"
ARENA_DURATION="${ARENA_DURATION:-150}"
SEED="${SEED:-4242}"
PREDICTION_LIMIT="${PREDICTION_LIMIT:-8}"
INPUT_DELAY="${INPUT_DELAY:-1}"
STATE_HISTORY="${STATE_HISTORY:-16}"
# Record by default. The instance is going to be destroyed and everything on it
# with it, so anything not pulled down first is gone for good.
RECORD="${RECORD:-1}"
# Which simulations to run. Narrow it when the question is about tuning rather
# than about the link, so a comparison costs one session instead of two.
SIMS="${SIMS:-lastblade2 arena}"
TF_DIR="${ROOT}/terraform"

if [[ ! -f "${TF_DIR}/terraform.tfvars" ]]; then
    echo "terraform/terraform.tfvars is missing. Copy example.tfvars." >&2
    exit 2
fi

main() {
    echo "############################################################"
    echo "##  ${LABEL}  (${REGION})"
    echo "############################################################"

    # Point Terraform at this region. Everything else in tfvars stays put.
    # Left-aligned on purpose: a quoted heredoc body is passed through verbatim,
    # so indenting it would indent the Python too, and the terminator has to sit
    # in column zero to be recognised at all.
    python3 - "${TF_DIR}/terraform.tfvars" "${REGION}" <<'PY'
import re, sys
path, region = sys.argv[1], sys.argv[2]
text = open(path).read()
if re.search(r'^\s*region\s*=', text, re.M):
    text = re.sub(r'^\s*region\s*=.*$', f'region = "{region}"', text, flags=re.M)
else:
    text += f'\nregion = "{region}"\n'
open(path, 'w').write(text)
PY

    # HCL aligns `=` within a block and a plain string substitution does not.
    # `just test` runs `terraform fmt -check`, so leaving it unformatted turns
    # this script into a failing test gate two steps later.
    terraform -chdir="${TF_DIR}" fmt >/dev/null

    run_one() {
        local sim="$1" duration="$2" extra_up=() extra_local=() record_arg=""
        # Only emulated simulations have a framebuffer to record.
        if [[ "${RECORD}" -eq 1 && "${sim}" != "arena" ]]; then
            record_arg="${ROOT}/artifacts/video/raw/${LABEL}-local.mp4"
        fi
        if [[ "${sim}" != "arena" ]]; then
            [[ -n "${ROM}" ]] || { echo "!!! ${sim} needs a ROM" >&2; return 1; }
            extra_up=(ROM="${ROM}")
            extra_local=(--core "${ROOT}/cores/fbneo_libretro.so" --rom "${ROM}"
                         --system-dir "${ROOT}/artifacts/system")
        fi

        echo
        echo "==> ${LABEL}: ${sim}, ${duration}s"

        # The remote peer runs a few seconds longer, so it is still listening while
        # the local side finishes its handshake and boot.
        env SIM="${sim}" MODE="${LABEL}" PROFILE=natural SEED="${SEED}" RECORD="${RECORD}" \
            PREDICTION_LIMIT="${PREDICTION_LIMIT}" INPUT_DELAY="${INPUT_DELAY}" \
            STATE_HISTORY="${STATE_HISTORY}" \
            DURATION="$((duration + 30))" "${extra_up[@]}" \
            "${ROOT}/ops/scripts/aws-up.sh" > "/tmp/aws-up-${LABEL}-${sim}.log" 2>&1 || {
            echo "!!! aws-up failed; tail:" >&2
            tail -20 "/tmp/aws-up-${LABEL}-${sim}.log" >&2
            return 1
        }

        local peer
        peer="$(terraform -chdir="${TF_DIR}" output -raw peer_address)"
        echo "    peer ${peer}"

        rm -rf "${ROOT}/artifacts/system/fbneo"
        ROLLBACK_SESSION_KEY="$(cat "${ROOT}/artifacts/session.key")" \
        "${ROOT}/target/release/rollback-bot" \
            --sim "${sim}" --player p1 --peer "${peer}" --bind 0.0.0.0:0 \
            --profile natural --seed "${SEED}" --duration "${duration}" \
            --input-delay "${INPUT_DELAY}" --prediction-limit "${PREDICTION_LIMIT}" \
            --state-history "${STATE_HISTORY}" \
            --mode "${LABEL}" --log-dir "${ROOT}/artifacts/logs" \
            ${record_arg:+--record "${record_arg}"} \
            "${extra_local[@]}" 2>&1 | tail -3
    }

    cargo build --release -p rollback-bot

    ok=1
    sims_run=""
    for sim in ${SIMS}; do
        case "${sim}" in
            lastblade2|sfa3)
                [[ -n "${ROM}" ]] || { echo "!!! ${sim} needs a ROM" >&2; exit 2; }
                run_one "${sim}" "${GAME_DURATION}" || ok=0
                ;;
            arena) run_one arena "${ARENA_DURATION}" || ok=0 ;;
            *) echo "!!! unknown simulation '${sim}'" >&2; exit 2 ;;
        esac
        sims_run="${sims_run} ${sim}"
    done

    echo
    echo "==> collecting ${LABEL}"
    "${ROOT}/ops/scripts/collect.sh" 2>&1 | grep -E "collected|jsonl|session\(s\)" || true

    # Verify before destroying. `aws-down` only checks that *some* log exists; this
    # checks that *this run* came back whole. Everything still on the instance dies
    # with it, and re-running costs another session.
    echo
    echo "==> verifying ${LABEL} came back whole"
    missing=0
    for sim in ${sims_run}; do
        for player in p1 p2; do
            if [[ -z "$(find "${ROOT}/artifacts/logs" -name "*-${sim}-*-${player}-${LABEL}.jsonl" -print -quit)" ]]; then
                echo "!!! missing ${sim} ${player} log for ${LABEL}" >&2
                missing=1
            fi
        done
    done
    if [[ "${RECORD}" -eq 1 && -n "${ROM}" ]]; then
        for v in "${ROOT}/artifacts/video/raw/${LABEL}-local.mp4" \
                 "${ROOT}/artifacts/video/remote/peer.mp4"; do
            if [[ ! -s "${v}" ]]; then
                echo "!!! missing recording ${v}" >&2
                missing=1
            fi
        done
        # The remote recording always lands on the same name, so keep this run's
        # copy before the next region overwrites it.
        if [[ -s "${ROOT}/artifacts/video/remote/peer.mp4" ]]; then
            mv "${ROOT}/artifacts/video/remote/peer.mp4" \
               "${ROOT}/artifacts/video/raw/${LABEL}-remote.mp4"
        fi
    fi
    find "${ROOT}/artifacts/logs" -name "*-${LABEL}.jsonl" -printf '    %p  %s bytes\n' 2>/dev/null
    find "${ROOT}/artifacts/video/raw" -name "${LABEL}-*.mp4" -printf '    %p  %s bytes\n' 2>/dev/null
    if [[ ${missing} -ne 0 ]]; then
        echo "!!! NOT tearing down: something did not come back. The instance is up," >&2
        echo "!!! costing money, and still holds it. Investigate, then aws-down." >&2
        exit 1
    fi

    echo
    echo "==> tearing down ${LABEL}"
    "${ROOT}/ops/scripts/aws-down.sh" 2>&1 | grep -E "Destroy complete|torn down" || true

    [[ ${ok} -eq 1 ]] || { echo "!!! ${LABEL} had failures" >&2; exit 1; }
    echo "==> ${LABEL} done"
}

# Called last, on purpose. Bash reads a top-level script incrementally, so
# editing this file while it runs makes the shell resume at a byte offset that
# no longer lines up -- which is exactly how a Tokyo run lost its teardown and
# left an instance up. Everything above is inside a function, which bash parses
# whole before executing any of it.
main "$@"
