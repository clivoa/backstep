#!/usr/bin/env bash
# Record one video per network profile, with the session's own telemetry burned
# into it.
#
# Both peers are recorded, not just one. That is the point: under any profile
# with delay the two sides do wildly different amounts of work (one peer paid
# 1 006 rollbacks to the other's zero), and a single-peer video would show a
# smooth fight and hide the entire phenomenon.
#
#   ROM=/path/to/lastbld2.zip ./ops/scripts/record-scenarios.sh
#   PROFILES="natural combined" DURATION=90 ROM=... ./ops/scripts/record-scenarios.sh
#
# Output lands in artifacts/video/.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT}"

SIM="${SIM:-lastblade2}"
ROM="${ROM:-}"
CORE="${CORE:-${ROOT}/cores/fbneo_libretro.so}"
BIOS="${BIOS:-${ROOT}/artifacts/system/neogeo.zip}"
# Long enough to be past the ~33 s boot script and well into a round.
DURATION="${DURATION:-90}"
PROFILES="${PROFILES:-natural delay20 jitter30 loss2 combined}"
SEED="${SEED:-4242}"
PORT="${PORT:-7200}"
OUT="${OUT:-${ROOT}/artifacts/video}"
WORK="${OUT}/raw"

if [[ -z "${ROM}" ]]; then
    echo "ROM=/path/to/game.zip is required" >&2
    exit 2
fi
for tool in ffmpeg python3; do
    command -v "${tool}" >/dev/null || { echo "${tool} is not on PATH" >&2; exit 1; }
done

echo "==> building"
cargo build --release -p rollback-bot

rm -rf "${OUT}"
mkdir -p "${WORK}"

if [[ -z "${ROLLBACK_SESSION_KEY:-}" ]]; then
    ROLLBACK_SESSION_KEY="$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')"
fi
export ROLLBACK_SESSION_KEY

BOT="${ROOT}/target/release/rollback-bot"

for profile in ${PROFILES}; do
    echo "==> ${profile} (${DURATION}s)"
    logs="${WORK}/${profile}"
    p1_sys="${WORK}/${profile}-sys-p1"
    p2_sys="${WORK}/${profile}-sys-p2"
    rm -rf "${logs}" "${p1_sys}" "${p2_sys}"
    mkdir -p "${logs}" "${p1_sys}" "${p2_sys}"
    # Each peer needs its own system directory: FBNeo writes NVRAM there, and
    # two peers sharing one would race to clear and rewrite the same file.
    [[ -f "${BIOS}" ]] && cp "${BIOS}" "${p1_sys}/" && cp "${BIOS}" "${p2_sys}/"

    "${BOT}" --sim "${SIM}" --player p2 \
        --core "${CORE}" --rom "${ROM}" --system-dir "${p2_sys}" \
        --bind "127.0.0.1:${PORT}" --profile "${profile}" --seed "${SEED}" \
        --duration "${DURATION}" --mode record --log-dir "${logs}" \
        --metrics 127.0.0.1:9899 \
        --record "${WORK}/${profile}-p2.mp4" \
        >"${WORK}/${profile}-p2.txt" 2>&1 &
    p2=$!

    sleep 1

    "${BOT}" --sim "${SIM}" --player p1 \
        --core "${CORE}" --rom "${ROM}" --system-dir "${p1_sys}" \
        --bind 127.0.0.1:0 --peer "127.0.0.1:${PORT}" \
        --profile "${profile}" --seed "${SEED}" \
        --duration "${DURATION}" --mode record --log-dir "${logs}" \
        --metrics 127.0.0.1:9898 \
        --record "${WORK}/${profile}-p1.mp4" \
        >"${WORK}/${profile}-p1.txt" 2>&1 &
    p1=$!

    ok=1
    wait "${p2}" || ok=0
    wait "${p1}" || ok=0
    if [[ ${ok} -eq 0 ]]; then
        echo "!!! ${profile} failed" >&2
        tail -5 "${WORK}/${profile}-p1.txt" "${WORK}/${profile}-p2.txt" >&2
        continue
    fi

    for player in p1 p2; do
        log="$(find "${logs}" -name "*-${player}-*.jsonl" | head -1)"
        [[ -n "${log}" ]] || { echo "!!! no log for ${profile} ${player}" >&2; continue; }
        python3 "${ROOT}/ops/scripts/annotate-video.py" \
            "${WORK}/${profile}-${player}.mp4" "${log}" \
            "${OUT}/${SIM}-${profile}-${player}.mp4" \
            "${SIM}  ${profile}  ${player}"
    done

    # Side by side, so the asymmetry is visible rather than described.
    if [[ -f "${OUT}/${SIM}-${profile}-p1.mp4" && -f "${OUT}/${SIM}-${profile}-p2.mp4" ]]; then
        ffmpeg -hide_banner -loglevel error -y \
            -i "${OUT}/${SIM}-${profile}-p1.mp4" \
            -i "${OUT}/${SIM}-${profile}-p2.mp4" \
            -filter_complex "[0:v][1:v]hstack=inputs=2" \
            -c:v libx264 -preset veryfast -crf 22 -pix_fmt yuv420p \
            "${OUT}/${SIM}-${profile}-both.mp4"
        echo "    ${OUT}/${SIM}-${profile}-both.mp4  (P1 left, P2 right)"
    fi

    sleep 1
done

echo
echo "==> videos in ${OUT}"
ls -la "${OUT}"/*.mp4 2>/dev/null || echo "    (none produced)"
echo
echo "Raw recordings and logs kept in ${WORK} -- delete when done."
