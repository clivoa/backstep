#!/usr/bin/env bash
# The experiment: 180-second bot-vs-bot sessions across every network profile.
#
# Repeatable by construction. Both peers are bots with a fixed seed, the
# impairment emulator is seeded too, and the only thing that varies between runs
# is the real network underneath -- which for a local bench is nothing.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT}"

DURATION="${DURATION:-180}"
SIM="${SIM:-arena}"
PROFILES="${PROFILES:-natural delay20 jitter30 loss2 combined}"
SEED="${SEED:-4242}"
PORT="${PORT:-7000}"
PEER="${PEER:-}"
LOG_DIR="${LOG_DIR:-artifacts/logs}"
REPORT_DIR="${REPORT_DIR:-artifacts/report}"
CORE="${CORE:-${ROOT}/cores/fbneo_libretro.so}"
ROM="${ROM:-}"

BOT="${ROOT}/target/release/rollback-bot"
REPORT="${ROOT}/target/release/rollback-report"

if [[ "${SIM}" == "sfa3" && -z "${ROM}" ]]; then
    echo "SIM=sfa3 needs ROM=/path/to/sfa3.zip" >&2
    exit 2
fi

echo "==> building release binaries"
cargo build --release -p rollback-bot -p rollback-report

if [[ -z "${ROLLBACK_SESSION_KEY:-}" ]]; then
    if [[ -f "${ROOT}/artifacts/session.key" ]]; then
        ROLLBACK_SESSION_KEY="$(cat "${ROOT}/artifacts/session.key")"
    else
        ROLLBACK_SESSION_KEY="$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')"
    fi
fi
export ROLLBACK_SESSION_KEY

mkdir -p "${LOG_DIR}" "${REPORT_DIR}"

sim_args=(--sim "${SIM}")
if [[ "${SIM}" == "sfa3" ]]; then
    sim_args+=(--core "${CORE}" --rom "${ROM}")
fi

failures=0
for profile in ${PROFILES}; do
    echo "==> ${profile}: ${DURATION}s, sim=${SIM}, seed=${SEED}"

    if [[ -n "${PEER}" ]]; then
        # Against a real remote peer: only P1 runs here.
        "${BOT}" "${sim_args[@]}" --player p1 --bind 0.0.0.0:0 --peer "${PEER}" \
            --profile "${profile}" --seed "${SEED}" --duration "${DURATION}" \
            --log-dir "${LOG_DIR}" --mode bench || failures=$((failures + 1))
        continue
    fi

    # Local bench: two processes on loopback, P2 hosting.
    "${BOT}" "${sim_args[@]}" --player p2 --bind "127.0.0.1:${PORT}" \
        --profile "${profile}" --seed "${SEED}" --duration "${DURATION}" \
        --log-dir "${LOG_DIR}" --metrics 127.0.0.1:9899 --mode bench &
    host=$!
    sleep 1
    "${BOT}" "${sim_args[@]}" --player p1 --bind 127.0.0.1:0 --peer "127.0.0.1:${PORT}" \
        --profile "${profile}" --seed "${SEED}" --duration "${DURATION}" \
        --log-dir "${LOG_DIR}" --metrics 127.0.0.1:9898 --mode bench &
    client=$!

    wait "${host}" || failures=$((failures + 1))
    wait "${client}" || failures=$((failures + 1))
    sleep 1
done

echo "==> building the report"
"${REPORT}" --logs "${LOG_DIR}" --out "${REPORT_DIR}"

if [[ ${failures} -ne 0 ]]; then
    echo "FAILED: ${failures} peer process(es) exited non-zero" >&2
    exit 1
fi
echo "OK"
