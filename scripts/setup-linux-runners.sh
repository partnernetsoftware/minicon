#!/bin/bash
# Provision the two local Debian courts consumed by six-cell-qualify.sh.
# Run explicitly on an Apple Silicon Mac after installing:
#   brew install lima lima-additional-guestagents

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ARM_INSTANCE="${MINICON_LNX_AARCH64_LIMA:-minicon-lnx-aarch64}"
X86_INSTANCE="${MINICON_LNX_X86_64_KERNEL_LIMA:-minicon-lnx-x86_64}"

if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
  echo "setup-linux-runners.sh requires an Apple Silicon macOS host" >&2
  exit 2
fi
for tool in limactl python3; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "required tool missing: $tool" >&2
    exit 2
  }
done
arch -x86_64 /usr/bin/true >/dev/null 2>&1 || {
  echo "Rosetta 2 is required; install it with softwareupdate before provisioning" >&2
  exit 2
}

instance_exists() {
  limactl list --json "$1" >/dev/null 2>&1
}

instance_running() {
  limactl list --json "$1" 2>/dev/null |
    python3 -c 'import json,sys; value=json.load(sys.stdin); raise SystemExit(0 if value.get("status") == "Running" else 1)' \
      >/dev/null 2>&1
}

instance_matches() {
  instance="$1"
  expected_arch="$2"
  expected_vm_type="$3"
  limactl list --json "$instance" 2>/dev/null |
    EXPECTED_ARCH="$expected_arch" EXPECTED_VM_TYPE="$expected_vm_type" python3 -c '
import json, os, sys
value = json.load(sys.stdin)
matches = (
    value.get("arch") == os.environ["EXPECTED_ARCH"]
    and value.get("vmType") == os.environ["EXPECTED_VM_TYPE"]
)
raise SystemExit(0 if matches else 1)
' >/dev/null 2>&1
}

require_instance_shape() {
  instance="$1"
  expected_arch="$2"
  expected_vm_type="$3"
  if ! instance_matches "$instance" "$expected_arch" "$expected_vm_type"; then
    echo "existing Lima instance $instance does not match arch=$expected_arch vmType=$expected_vm_type" >&2
    echo "remove or rename that instance explicitly, then rerun this provisioner" >&2
    exit 2
  fi
}

rosetta_enabled() {
  limactl list --json "$1" 2>/dev/null |
    python3 -c 'import json,sys; value=json.load(sys.stdin); rosetta=value.get("config", {}).get("vmOpts", {}).get("vz", {}).get("rosetta", {}); raise SystemExit(0 if rosetta.get("enabled") and rosetta.get("binfmt") else 1)' \
      >/dev/null 2>&1
}

start_instance() {
  instance="$1"
  if ! instance_running "$instance"; then
    limactl start "$instance"
  fi
}

if ! instance_exists "$ARM_INSTANCE"; then
  limactl create --name="$ARM_INSTANCE" --arch=aarch64 --vm-type=vz \
    --rosetta --cpus=6 --memory=8 --disk=32 --containerd=none \
    --mount-only="$REPO_ROOT:w" --tty=false template:debian
else
  require_instance_shape "$ARM_INSTANCE" aarch64 vz
fi
if ! rosetta_enabled "$ARM_INSTANCE"; then
  if instance_running "$ARM_INSTANCE"; then
    limactl stop "$ARM_INSTANCE"
  fi
  limactl edit --rosetta --tty=false "$ARM_INSTANCE"
fi
start_instance "$ARM_INSTANCE"

if ! instance_exists "$X86_INSTANCE"; then
  limactl create --name="$X86_INSTANCE" --arch=x86_64 --vm-type=qemu \
    --cpus=4 --memory=8 --disk=32 --containerd=none \
    --mount-only="$REPO_ROOT:w" --tty=false template:debian
else
  require_instance_shape "$X86_INSTANCE" x86_64 qemu
fi
start_instance "$X86_INSTANCE"

COMMON_PACKAGES="libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 at-spi2-core dbus-x11 xvfb xauth fonts-dejavu-core libx11-6 libxcursor1 libxi6 libxrandr2 libxinerama1 libegl1 libgl1"

limactl shell "$X86_INSTANCE" -- bash -lc \
  "sudo -E apt-get update -qq && sudo -E DEBIAN_FRONTEND=noninteractive apt-get install -y $COMMON_PACKAGES"

limactl shell "$ARM_INSTANCE" -- bash -lc \
  "sudo -E apt-get update -qq && sudo -E DEBIAN_FRONTEND=noninteractive apt-get install -y $COMMON_PACKAGES && sudo dpkg --add-architecture amd64 && sudo -E apt-get update -qq && sudo -E DEBIAN_FRONTEND=noninteractive apt-get install -y libc6:amd64 libgcc-s1:amd64 libxkbcommon0:amd64 libxkbcommon-x11-0:amd64 libwayland-client0:amd64 libatspi2.0-0t64:amd64 libx11-6:amd64 libxcursor1:amd64 libxi6:amd64 libxrandr2:amd64 libxinerama1:amd64 libegl1:amd64 libgl1:amd64"

echo "Linux runtime courts are ready: $ARM_INSTANCE and $X86_INSTANCE"
