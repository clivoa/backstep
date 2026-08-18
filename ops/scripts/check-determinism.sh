#!/usr/bin/env bash
# Does this core produce the same machine state twice, in two separate
# processes, at two different wall-clock times?
#
# If the answer is no, nothing else in this lab means anything: rollback is
# re-simulation, and re-simulation only converges if the simulation is a
# function of its inputs alone. An emulator that reads the host clock is not.
#
# This is not hypothetical. FBNeo seeds its RNG from time(NULL) and serves the
# host's calendar to the emulated machine's real-time clock chip; the Neo Geo
# BIOS reads that during boot. Two peers booting one second apart diverged
# before the first input. See docker/fbneo/determinism.md.
#
#   ./ops/scripts/check-determinism.sh /path/to/lastbld2.zip lastblade2
#
# The deliberate `sleep` between runs is the whole test. Two runs inside the
# same wall-clock second get the same time(NULL) and match even on a broken
# core, which is exactly how the bug hid.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ROM="${1:?usage: check-determinism.sh <rom.zip> [game] }"
GAME="${2:-lastblade2}"
CORE="${CORE:-${ROOT}/cores/fbneo_libretro.so}"
FRAMES="${FRAMES:-900}"
GAP="${GAP:-2}"

WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

if [ ! -f "${CORE}" ]; then
    echo "!!! no core at ${CORE}. Run 'just build-core' first." >&2
    exit 1
fi

echo "==> core   ${CORE}"
echo "==> rom    ${ROM}"
echo "==> game   ${GAME}"
echo "==> frames ${FRAMES}, ${GAP}s between runs"

cargo build --release -q -p rollback-libretro --example probe-boot

run_once() {
    local label="$1"
    local sys="${WORK}/sys-${label}"
    mkdir -p "${sys}"
    # Each run gets its own system directory: identical inputs, no shared
    # NVRAM. A difference here must come from the core, not from a file one
    # run left behind for the other.
    if [ -f "${ROOT}/artifacts/system/neogeo.zip" ]; then
        cp "${ROOT}/artifacts/system/neogeo.zip" "${sys}/"
    fi
    PROBE_SCRIPT="${GAME}" "${ROOT}/target/release/examples/probe-boot" \
        "${CORE}" "${ROM}" "${sys}" "${WORK}/frames-${label}" "${FRAMES}" 1000000 \
        2>&1 | grep '^checksum' > "${WORK}/${label}.txt"
}

run_once a
sleep "${GAP}"
run_once b

echo
paste "${WORK}/a.txt" "${WORK}/b.txt" | sed 's/^/    /'
echo

if diff -q "${WORK}/a.txt" "${WORK}/b.txt" > /dev/null; then
    echo "==> DETERMINISTIC: two processes, ${GAP}s apart, identical state."
    exit 0
fi

cat >&2 <<'EOF'
!!! NOT DETERMINISTIC.

    Two runs of the same core with the same ROM and the same inputs produced
    different machine state. Rollback cannot work on this: every re-simulation
    would land somewhere new, and every session would end in a desync that is
    not the rollback's fault.

    Check that cores/fbneo_libretro.so was built by 'just build-core' from
    docker/fbneo/Dockerfile, which patches kNetGame=1. Confirm with:

        grep patches cores/fbneo-commit.txt

    Background: docker/fbneo/determinism.md
EOF
exit 1
