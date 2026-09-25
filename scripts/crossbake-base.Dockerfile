# EXPERIMENTAL. Not wired into any workflow's build/release path.
#
# The slow, stable half of the crossbake image: everything that takes real
# minutes to build and changes rarely (apt packages, the Rust toolchain,
# cargo-xwin/cargo-zigbuild, zig, osxcross). Built and pushed as a real,
# pullable tag (crossbake-base:$BASE_VERSION), not as BuildKit registry
# cache -- a `docker pull` of a real manifest is a plain, reliable
# operation, unlike `--cache-from type=registry` on a Dockerfile with a
# fast-changing tail (scripts/crossbake.Dockerfile), which was observed
# (2026-09-25) to load a runtime image missing a binary that its own build
# log said it just installed successfully, once some layers came from cache
# import and others were freshly built in the same invocation -- a known
# class of moby/buildkit mixed-cache-hit inconsistency, not a Dockerfile
# bug. Splitting the volatile tail into its own thin image, FROM'd by a
# normal pull, removes that whole failure class for the tail's own iteration
# loop; this file itself should change rarely enough that its own
# cache-from/cache-to usage during THIS image's build is not the hot path.
#
# Bump BASE_VERSION (see .github/workflows/linux-crossbake-experiment.yml)
# whenever this file's content changes; the workflow only rebuilds this
# image when the target tag doesn't already exist in GHCR.
#
# Answers: can one Linux host produce all six MiniCon target cells --
# win-x86_64, win-aarch64 (cargo-xwin), lnx-x86_64, lnx-aarch64
# (cargo-zigbuild/native), osx-x86_64, osx-aarch64 (osxcross, validated by
# .github/workflows/osxcross-experiment.yml to produce a real Mach-O) --
# plus minicon.com (cosmocc, added by the derived crossbake.Dockerfile), so
# that build workload can move off GitHub-hosted macOS/Windows runners onto
# a dedicated Linux box?
#
# Tool versions are pinned to match .github/workflows/minicon-com.yml's
# `env:` block so this image and the CI-cached toolchain never drift apart.
FROM ubuntu:24.04

ARG CARGO_ZIGBUILD_VERSION=0.23.2
ARG CARGO_XWIN_VERSION=0.23.1
ARG RUST_VERSION=1.97.0
ARG MACOS_SDK_VERSION=15.5
ARG ZIG_VERSION=0.16.0

ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/opt/cargo \
    PATH=/opt/cargo/bin:/opt/osxcross/target/bin:$PATH

# osxcross's own build deps (tpoechtrager/osxcross README) plus curl/git for
# rustup and the toolchain clones. No apt package here is Windows/macOS
# specific -- xwin and zigbuild fetch their own SDKs into CARGO_HOME at
# first use, same as they do on the macOS runner in minicon-com.yml.
RUN apt-get update -qq && apt-get install -y -qq \
    curl git clang llvm cmake patch libssl-dev liblzma-dev zlib1g-dev \
    libxml2-dev bzip2 xz-utils python3 ca-certificates unzip perl \
    && rm -rf /var/lib/apt/lists/*

# winresource (cargo-xwin's Windows resource embedding, minicon's build.rs)
# shells out to a bare `llvm-rc` on PATH; Ubuntu's llvm package only installs
# the version-suffixed binary (e.g. /usr/lib/llvm-18/bin/llvm-rc), not
# `llvm-rc-<ver>` on PATH -- `command -v llvm-rc-*` doesn't glob a PATH
# lookup, it only glob-matches files in the current directory, so it always
# failed. Find the real binary under /usr/lib instead.
RUN llvm_rc="$(find /usr/lib -maxdepth 3 -type f -name llvm-rc | head -1)" \
    && test -n "$llvm_rc" \
    && ln -sf "$llvm_rc" /usr/local/bin/llvm-rc

RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal \
    --default-toolchain "$RUST_VERSION" -c clippy,rustfmt,rust-src

RUN cargo install cargo-xwin --locked --version "$CARGO_XWIN_VERSION" \
    && cargo install cargo-zigbuild --locked --version "$CARGO_ZIGBUILD_VERSION"

# cargo-zigbuild is only the cargo subcommand; it shells out to a real `zig`
# binary on PATH at build time. loader/install-zig.sh pins zig by a
# macOS-only tarball + sha256, so it can't be reused as-is for this Linux
# container; fetch the matching x86_64-linux tarball instead and verify it
# against ziglang.org's own published shasum for this exact ZIG_VERSION,
# same verify-before-trust standard.
RUN python3 -c "\
import hashlib, json, os, tarfile, urllib.request; \
ver = '$ZIG_VERSION'; \
idx = json.load(urllib.request.urlopen('https://ziglang.org/download/index.json')); \
info = idx[ver]['x86_64-linux']; \
urllib.request.urlretrieve(info['tarball'], '/tmp/zig.tar.xz'); \
got = hashlib.sha256(open('/tmp/zig.tar.xz', 'rb').read()).hexdigest(); \
assert got == info['shasum'], f'zig sha256 mismatch: got {got} want {info[\"shasum\"]}'; \
tarfile.open('/tmp/zig.tar.xz').extractall('/opt'); \
os.rename('/opt/zig-x86_64-linux-' + ver, '/opt/zig'); \
os.remove('/tmp/zig.tar.xz')" \
    && /opt/zig/zig version | grep -qx "$ZIG_VERSION"

# osxcross, built once against the SDK version osxcross-experiment.yml
# already proved produces a linkable Mach-O for this codebase (phracker's
# older SDKs are missing symbols agenterm-platform references).
RUN git clone --depth 1 https://github.com/tpoechtrager/osxcross.git /opt/osxcross
RUN curl -fsSL -o /opt/osxcross/tarballs/MacOSX${MACOS_SDK_VERSION}.sdk.tar.xz \
    "https://github.com/alexey-lysiuk/macos-sdk/releases/download/${MACOS_SDK_VERSION}/MacOSX${MACOS_SDK_VERSION}.tar.xz"
RUN cd /opt/osxcross && UNATTENDED=1 ./build.sh

# One absolute PATH for the final image, spelling out every directory this
# base provides, so a derived image's own FROM doesn't have to reconstruct
# the chain.
ENV PATH="/opt/zig:/opt/cargo/bin:/opt/osxcross/target/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
