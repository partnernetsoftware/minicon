# EXPERIMENTAL. Not wired into any workflow's build/release path.
#
# Answers: can one Linux host produce all six MiniCon target cells --
# win-x86_64, win-aarch64 (cargo-xwin), lnx-x86_64, lnx-aarch64
# (cargo-zigbuild/native), osx-x86_64, osx-aarch64 (osxcross, validated by
# .github/workflows/osxcross-experiment.yml to produce a real Mach-O) --
# plus minicon.com (cosmocc), so that build workload can move off
# GitHub-hosted macOS/Windows runners onto a dedicated Linux box?
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
# binary on PATH at build time, which was never installed here (the gap that
# made a probe run fail with "Failed to find zig / cannot find binary path").
# loader/install-zig.sh pins zig by a macOS-only tarball + sha256, so it can't
# be reused as-is for this Linux container; fetch the matching x86_64-linux
# tarball instead and verify it against ziglang.org's own published shasum
# for this exact ZIG_VERSION, same verify-before-trust standard.
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
ENV PATH=/opt/zig:$PATH

# osxcross, built once against the SDK version osxcross-experiment.yml
# already proved produces a linkable Mach-O for this codebase (phracker's
# older SDKs are missing symbols agenterm-platform references).
RUN git clone --depth 1 https://github.com/tpoechtrager/osxcross.git /opt/osxcross
RUN curl -fsSL -o /opt/osxcross/tarballs/MacOSX${MACOS_SDK_VERSION}.sdk.tar.xz \
    "https://github.com/alexey-lysiuk/macos-sdk/releases/download/${MACOS_SDK_VERSION}/MacOSX${MACOS_SDK_VERSION}.tar.xz"
RUN cd /opt/osxcross && UNATTENDED=1 ./build.sh

# cosmocc for minicon.com, via the repo's own signature/hash-verified
# installer so this image can't silently drift from what CI trusts.
COPY loader/install-cosmocc.sh /tmp/install-cosmocc.sh
ARG COSMOCC_VERSION=4.0.2
ARG COSMOCC_SHA256=85b8c37a406d862e656ad4ec14be9f6ce474c1b436b9615e91a55208aced3f44
ARG COSMOCC_BIN_SHA256=eef9db8fabfc0c08f1930cbba87f60f69a1c49f28e4de006a1b0c6863e943e4b
ENV COSMOCC_DIR=/opt/cosmocc
RUN COSMOCC_VERSION="$COSMOCC_VERSION" COSMOCC_SHA256="$COSMOCC_SHA256" \
    COSMOCC_BIN_SHA256="$COSMOCC_BIN_SHA256" bash /tmp/install-cosmocc.sh

# A single absolute PATH for the final image, not another chained
# `$PATH`-relative ENV -- a probe run got every one of the six cells built
# (zig, xwin, osxcross all resolved fine) but then hit "cosmocc: command not
# found" at the very last check despite `[cosmocc] cosmocc (GCC) 14.1.0`
# printing successfully during this same RUN step above, i.e. the binary
# exists but the runtime container's PATH didn't carry /opt/cosmocc/bin.
# Spelling out the whole PATH once, instead of trusting N accumulated
# `ENV PATH=X:$PATH` layers, removes that class of bug entirely.
ENV PATH="/opt/cosmocc/bin:/opt/zig:/opt/cargo/bin:/opt/osxcross/target/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
