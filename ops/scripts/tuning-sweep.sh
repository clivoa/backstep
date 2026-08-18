#!/usr/bin/env bash
# Sweep rollback tuning against a fixed link, to answer the question the AWS
# runs raised but could not settle.
#
# Madrid to São Paulo and Madrid to Tokyo both measured ~267 ms round trip.
# At 60 Hz that is about 16 frames, so with the default 8-frame prediction
# limit the window fills before any input can answer, and the session stalls:
# 827 and 1 283 stalls respectively, with effective FPS down to 57 and 56.
# The AWS runs proved that happens. They did not show how to fix it, because
# each configuration would have cost another instance-hour on another
# continent.
#
# The `transcontinental` profile reproduces that link on loopback, so the
# tuning question costs nothing but CPU. Loss and reordering are deliberately
# absent: the real links showed 0.00-0.03% loss, so delay is the variable that
# matters and the sweep isolates it.
#
#   ./ops/scripts/tuning-sweep.sh              # arena, 4 configurations
#   DURATION=180 ./ops/scripts/tuning-sweep.sh # longer, tighter numbers
#
# Arena rather than the emulated game, on purpose. Its state is 204 bytes, so
# re-simulation is nearly free and what the numbers show is the *algorithm*
# saturating rather than the CPU running out. The Last Blade 2 at 415 KB
# suffers both at once, which is realistic but confounds the two causes.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

DURATION="${DURATION:-90}"
PROFILE="${PROFILE:-transcontinental}"
SIM="${SIM:-arena}"
SEED="${SEED:-4242}"
PORT="${PORT:-7200}"
LOG_DIR="${LOG_DIR:-${ROOT}/artifacts/tuning/logs}"

# name : input_delay : prediction_limit : state_history
#
# state_history always exceeds prediction_limit, which the config validator
# enforces: a rollback can reach back `prediction_limit` frames, so the
# snapshot at that frame must still be in the ring.
CONFIGS="${CONFIGS:-
baseline:1:8:16
wide-window:1:20:28
input-delay-8:8:8:16
both:6:16:24
}"

main() {
    cd "${ROOT}"
    cargo build --release -p rollback-bot

    local key
    key="${ROLLBACK_SESSION_KEY:-$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')}"
    export ROLLBACK_SESSION_KEY="${key}"

    rm -rf "${LOG_DIR}"
    mkdir -p "${LOG_DIR}"

    echo "==> ${SIM} on ${PROFILE}, ${DURATION}s per configuration"

    local entry name delay limit history
    for entry in ${CONFIGS}; do
        IFS=: read -r name delay limit history <<<"${entry}"

        echo
        echo "==> ${name}: input_delay=${delay} prediction_limit=${limit} state_history=${history}"

        # Both peers must agree on all three: they are hashed into the
        # handshake's compatibility check, so a mismatch is a refused session
        # rather than a subtly wrong experiment.
        "${ROOT}/target/release/rollback-bot" \
            --sim "${SIM}" --player p2 --bind "127.0.0.1:${PORT}" \
            --profile "${PROFILE}" --seed "${SEED}" --duration "${DURATION}" \
            --input-delay "${delay}" --prediction-limit "${limit}" \
            --state-history "${history}" \
            --log-dir "${LOG_DIR}" --mode "tune-${name}" \
            --metrics 127.0.0.1:9899 >"/tmp/tune-${name}-p2.log" 2>&1 &
        local p2=$!

        sleep 1

        "${ROOT}/target/release/rollback-bot" \
            --sim "${SIM}" --player p1 --bind 127.0.0.1:0 \
            --peer "127.0.0.1:${PORT}" \
            --profile "${PROFILE}" --seed "${SEED}" --duration "${DURATION}" \
            --input-delay "${delay}" --prediction-limit "${limit}" \
            --state-history "${history}" \
            --log-dir "${LOG_DIR}" --mode "tune-${name}" \
            --metrics 127.0.0.1:9898 >"/tmp/tune-${name}-p1.log" 2>&1 &
        local p1=$!

        local s2=0 s1=0
        wait "${p2}" || s2=$?
        wait "${p1}" || s1=$?
        if [[ ${s2} -ne 0 || ${s1} -ne 0 ]]; then
            echo "!!! ${name} failed (p2=${s2} p1=${s1})" >&2
            tail -5 "/tmp/tune-${name}-p1.log" >&2
            exit 1
        fi
        tail -1 "/tmp/tune-${name}-p1.log"
    done

    echo
    "${ROOT}/ops/scripts/tuning-table.py" "${LOG_DIR}"
}

main "$@"
