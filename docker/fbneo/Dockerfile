# Reproducible build container for the official FBNeo libretro core.
#
# The core is pinned to a single upstream commit so that both peers can verify
# they are running byte-identical emulation code before a session starts.
#
# NOTE ON THE PIN (see docs/09-sfa3.md, "Sobre o commit pinado"):
# The lab spec pins finalburnneo/FBNeo@f1c3545fcdfca4dd5fcf9c1baaac6bba143785f8.
# That tree contains no `src/burner/libretro` directory -- upstream FBNeo does
# not host the libretro port. The libretro core is maintained in the
# libretro/FBNeo fork, so that is what we pin and build. Both SHAs are written
# into the provenance file so the discrepancy stays auditable.
FROM ubuntu:24.04

# Commit of libretro/FBNeo that is actually compiled.
ARG FBNEO_LIBRETRO_COMMIT=0332bb983c8f8a3e9b61cb79ade30f97a5032535
# Commit named by the lab spec, recorded for provenance only.
ARG FBNEO_SPEC_COMMIT=f1c3545fcdfca4dd5fcf9c1baaac6bba143785f8
ARG JOBS=4

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        git \
        make \
        perl \
        python3 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src

# Shallow-fetch exactly the pinned commit: no history, no other refs.
RUN git init -q \
    && git remote add origin https://github.com/libretro/FBNeo.git \
    && git fetch -q --depth 1 origin "${FBNEO_LIBRETRO_COMMIT}" \
    && git checkout -q FETCH_HEAD

RUN make -j"${JOBS}" -C src/burner/libretro

# Collect the artefact plus the provenance the handshake will compare.
RUN mkdir -p /out \
    && cp src/burner/libretro/fbneo_libretro.so /out/fbneo_libretro.so \
    && printf 'libretro_fork_commit=%s\nspec_commit=%s\n' \
        "$(git rev-parse HEAD)" "${FBNEO_SPEC_COMMIT}" > /out/fbneo-commit.txt \
    && sha256sum /out/fbneo_libretro.so | awk '{print $1}' > /out/fbneo_libretro.so.sha256

CMD ["/bin/sh", "-c", "cp -a /out/. /export/"]
