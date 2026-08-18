#!/usr/bin/env bash
# End-to-end: two real processes, a real socket, every network profile.
#
# This is the test the unit suite cannot be: two independent peers, each with
# its own rollback session, its own transport and its own log, agreeing frame by
# frame over UDP. A confirmed desync in any profile fails the run.
#
# Both peers run on loopback here, so the only impairment is the one the
# emulator injects -- which is the point: the profile is the independent
# variable and the network underneath is as close to nothing as possible.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT}"

DURATION="${DURATION:-20}"
SIM="${SIM:-arena}"
PROFILES="${PROFILES:-natural delay20 jitter30 loss2 combined}"
PORT="${PORT:-7100}"
LOG_DIR="${LOG_DIR:-artifacts/e2e/logs}"
REPORT_DIR="${REPORT_DIR:-artifacts/e2e/report}"
SEED="${SEED:-4242}"
CORE="${CORE:-${ROOT}/cores/fbneo_libretro.so}"
ROM="${ROM:-}"
BIOS="${BIOS:-${ROOT}/artifacts/system/neogeo.zip}"

# Emulated simulations need a core and a ROM, and each peer needs its *own*
# system directory: FBNeo writes NVRAM there, and two peers sharing one
# directory would race to clear and rewrite the same file. The BIOS is copied
# into both, identical, because it is hashed into the handshake.
SIM_ARGS=()
if [[ "${SIM}" != "arena" ]]; then
    if [[ -z "${ROM}" ]]; then
        echo "!!! SIM=${SIM} needs a ROM: ROM=/path/to/game.zip $0" >&2
        exit 2
    fi
    SIM_ARGS=(--core "${CORE}" --rom "${ROM}")
fi

BOT="${ROOT}/target/release/rollback-bot"
REPORT="${ROOT}/target/release/rollback-report"

echo "==> building release binaries"
cargo build --release -p rollback-bot -p rollback-report

# An ephemeral key, generated here and never written to disk. Both peers are
# started from this shell, so exporting it is enough.
if [[ -z "${ROLLBACK_SESSION_KEY:-}" ]]; then
    ROLLBACK_SESSION_KEY="$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')"
fi
export ROLLBACK_SESSION_KEY

rm -rf "${LOG_DIR}" "${REPORT_DIR}"
mkdir -p "${LOG_DIR}" "${REPORT_DIR}"

failures=0

for profile in ${PROFILES}; do
    echo "==> profile ${profile} (${DURATION}s, sim=${SIM})"
    host_log="$(mktemp)"
    client_log="$(mktemp)"

    p1_sys="artifacts/e2e/sys-p1"
    p2_sys="artifacts/e2e/sys-p2"
    if [[ "${SIM}" != "arena" ]]; then
        rm -rf "${p1_sys}" "${p2_sys}"
        mkdir -p "${p1_sys}" "${p2_sys}"
        if [[ -f "${BIOS}" ]]; then
            cp "${BIOS}" "${p1_sys}/"
            cp "${BIOS}" "${p2_sys}/"
        fi
    fi

    # P2 binds and waits; P1 dials it. Same arrangement as the real lab, with
    # the EC2 instance replaced by a second local process.
    "${BOT}" \
        --sim "${SIM}" --player p2 \
        "${SIM_ARGS[@]+"${SIM_ARGS[@]}"}" --system-dir "${p2_sys}" \
        --bind "127.0.0.1:${PORT}" \
        --profile "${profile}" --seed "${SEED}" \
        --duration "${DURATION}" \
        --log-dir "${LOG_DIR}" \
        --metrics 127.0.0.1:9899 \
        --mode e2e \
        >"${host_log}" 2>&1 &
    host_pid=$!

    # Give the host a moment to bind before the client starts dialling.
    sleep 1

    "${BOT}" \
        --sim "${SIM}" --player p1 \
        "${SIM_ARGS[@]+"${SIM_ARGS[@]}"}" --system-dir "${p1_sys}" \
        --bind "127.0.0.1:0" \
        --peer "127.0.0.1:${PORT}" \
        --profile "${profile}" --seed "${SEED}" \
        --duration "${DURATION}" \
        --log-dir "${LOG_DIR}" \
        --metrics 127.0.0.1:9898 \
        --mode e2e \
        >"${client_log}" 2>&1 &
    client_pid=$!

    host_status=0
    client_status=0
    wait "${host_pid}" || host_status=$?
    wait "${client_pid}" || client_status=$?

    if [[ ${host_status} -ne 0 || ${client_status} -ne 0 ]]; then
        echo "!!! profile ${profile} failed (p2=${host_status} p1=${client_status})"
        echo "--- p2 ---"; cat "${host_log}"
        echo "--- p1 ---"; cat "${client_log}"
        failures=$((failures + 1))
    else
        tail -1 "${client_log}"
    fi
    rm -f "${host_log}" "${client_log}"

    # Let the port clear before the next profile rebinds it.
    sleep 1
done

echo "==> building the report"
"${REPORT}" --logs "${LOG_DIR}" --out "${REPORT_DIR}" --strict

if [[ ${failures} -ne 0 ]]; then
    echo "FAILED: ${failures} profile(s)"
    exit 1
fi
echo "OK: every profile completed with no confirmed desync"
