# EXPERIMENTAL. Not wired into any workflow's build/release path.
#
# The fast, volatile half of the crossbake image: just cosmocc (for
# minicon.com) layered on top of the slow, stable crossbake-base.Dockerfile
# image via a plain `FROM`. A plain `docker pull` of an already-pushed base
# tag is a reliable, ordinary operation -- unlike relying on BuildKit's
# `--cache-from type=registry` to reconstruct a multi-layer, rarely-changing
# base every time this file's own fast-changing tail is edited, which was
# observed (2026-09-25) to occasionally `--load` a local image missing a
# binary its own build log said it had just installed, once some Dockerfile
# instructions hit cache and others didn't. See crossbake-base.Dockerfile
# for the base image's own rationale and BASE_VERSION bump convention.
ARG BASE_IMAGE
FROM ${BASE_IMAGE}

# cosmocc for minicon.com, via the repo's own signature/hash-verified
# installer so this image can't silently drift from what CI trusts.
COPY loader/install-cosmocc.sh /tmp/install-cosmocc.sh
ARG COSMOCC_VERSION=4.0.2
ARG COSMOCC_SHA256=85b8c37a406d862e656ad4ec14be9f6ce474c1b436b9615e91a55208aced3f44
ARG COSMOCC_BIN_SHA256=eef9db8fabfc0c08f1930cbba87f60f69a1c49f28e4de006a1b0c6863e943e4b
ENV COSMOCC_DIR=/opt/cosmocc
RUN COSMOCC_VERSION="$COSMOCC_VERSION" COSMOCC_SHA256="$COSMOCC_SHA256" \
    COSMOCC_BIN_SHA256="$COSMOCC_BIN_SHA256" bash /tmp/install-cosmocc.sh \
    && /opt/cosmocc/bin/cosmocc --version

ENV PATH="/opt/cosmocc/bin:${PATH}"
