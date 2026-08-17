#!/usr/bin/env bash
# Build the pinned FBNeo libretro core inside the reproducible container and
# export it (plus its provenance files) to cores/.
#
# See docker/fbneo/Dockerfile for why the pin points at libretro/FBNeo rather
# than the commit named in the lab spec.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FBNEO_LIBRETRO_COMMIT="${FBNEO_LIBRETRO_COMMIT:-0332bb983c8f8a3e9b61cb79ade30f97a5032535}"
FBNEO_SPEC_COMMIT="${FBNEO_SPEC_COMMIT:-f1c3545fcdfca4dd5fcf9c1baaac6bba143785f8}"
JOBS="${JOBS:-$(nproc)}"
IMAGE="rollback-netcode/fbneo:${FBNEO_LIBRETRO_COMMIT:0:12}"
OUT="${ROOT}/cores"

mkdir -p "${OUT}"

echo "==> building ${IMAGE} (libretro/FBNeo ${FBNEO_LIBRETRO_COMMIT}, -j${JOBS})"
docker build \
    --build-arg "FBNEO_LIBRETRO_COMMIT=${FBNEO_LIBRETRO_COMMIT}" \
    --build-arg "FBNEO_SPEC_COMMIT=${FBNEO_SPEC_COMMIT}" \
    --build-arg "JOBS=${JOBS}" \
    -t "${IMAGE}" \
    "${ROOT}/docker/fbneo"

echo "==> exporting core to ${OUT}"
docker run --rm -v "${OUT}:/export" "${IMAGE}"

echo "==> core sha256: $(cat "${OUT}/fbneo_libretro.so.sha256")"
cat "${OUT}/fbneo-commit.txt"
ls -la "${OUT}"
