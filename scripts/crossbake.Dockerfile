# EXPERIMENTAL. Not wired into any workflow's build/release path.
#
# Formerly added cosmocc on top of crossbake-base.Dockerfile as its own
# fast-changing tail; cosmocc moved into the base image itself (2026-09-25)
# since it changes as rarely as the rest of the base and every tail-image
# build was re-downloading/re-verifying it (~3.5min) for nothing. Kept as a
# trivial passthrough, not deleted outright, so a future genuinely-volatile
# tail layer (a cosmocc version bump ahead of the next BASE_VERSION bump, a
# probe-only tool) has a place to land without re-deriving this split.
ARG BASE_IMAGE
FROM ${BASE_IMAGE}
