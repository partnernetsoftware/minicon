#!/bin/bash
# Prepare verified Ubuntu ARM64 disks and a credential-bearing NoCloud seed.

set -euo pipefail
umask 077

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
IMAGE="${MINICON_LINUX_DESKTOP_IMAGE:-$HOME/Downloads/noble-server-cloudimg-arm64.img}"
OUT="${MINICON_LINUX_DESKTOP_OUT:-$REPO_ROOT/target-six/linux-desktop-utm}"
EXPECTED_SHA256="4a281a921b8d7db952895ab619736f10efe9f63e111fa5b5779ed18f023818aa"
RAW_IMAGE="$OUT/ubuntu-noble-arm64.raw"
QEMU_IMAGE="$OUT/ubuntu-noble-arm64-qemu.qcow2"
RAW_SIZE="32G"

[ -f "$IMAGE" ] || { echo "missing Ubuntu cloud image: $IMAGE" >&2; exit 2; }
command -v qemu-img >/dev/null 2>&1 || {
  echo "qemu-img is required to derive the UTM execution disks" >&2
  exit 2
}
if [ -z "${MINICON_LINUX_PASSWORD_HASH:-}" ]; then
  [ -t 0 ] || {
    echo "MINICON_LINUX_PASSWORD_HASH is required outside an interactive terminal" >&2
    exit 2
  }
  printf 'Linux guest password: ' >&2
  IFS= read -r -s password
  printf '\nConfirm Linux guest password: ' >&2
  IFS= read -r -s password_confirm
  printf '\n' >&2
  [ -n "$password" ] && [ "$password" = "$password_confirm" ] || {
    unset password password_confirm
    echo "passwords are empty or do not match" >&2
    exit 2
  }
  MINICON_LINUX_PASSWORD_HASH="$(printf '%s\n' "$password" | openssl passwd -6 -stdin)"
  unset password password_confirm
fi
case "$MINICON_LINUX_PASSWORD_HASH" in
  '$6$'*) ;;
  *) echo "MINICON_LINUX_PASSWORD_HASH must start with \$6\$" >&2; exit 2 ;;
esac

actual_sha256="$(shasum -a 256 "$IMAGE" | awk '{print $1}')"
[ "$actual_sha256" = "$EXPECTED_SHA256" ] || {
  echo "Ubuntu cloud image digest mismatch" >&2
  exit 1
}

seed_root="$(mktemp -d)"
trap 'rm -rf "$seed_root"' EXIT
mkdir -p "$OUT"

sed "s|@PASSWORD_HASH@|$MINICON_LINUX_PASSWORD_HASH|" \
  "$SCRIPT_DIR/linux-desktop-cloud-init.yaml" >"$seed_root/user-data"
printf '%s\n' \
  'instance-id: minicon-linux-desktop-arm64-v1' \
  'local-hostname: minicon-linux-desktop' >"$seed_root/meta-data"

rm -f "$OUT/seed.iso"
hdiutil makehybrid -quiet -iso -joliet -default-volume-name cidata \
  -o "$OUT/seed.iso" "$seed_root"
cp "$IMAGE" "$OUT/ubuntu-noble-arm64.img"
cp "$IMAGE" "$QEMU_IMAGE"
qemu-img resize -f qcow2 "$QEMU_IMAGE" "$RAW_SIZE" >/dev/null
qemu-img convert -f qcow2 -O raw "$IMAGE" "$RAW_IMAGE"
qemu-img resize -f raw "$RAW_IMAGE" "$RAW_SIZE" >/dev/null

recipe_sha256="$(shasum -a 256 "$SCRIPT_DIR/linux-desktop-cloud-init.yaml" | awk '{print $1}')"
printf '{\n  "schema_version": 2,\n  "image_sha256": "%s",\n  "recipe_sha256": "%s",\n  "primary_utm_backend": "qemu-hvf",\n  "primary_disk_format": "qcow2",\n  "compatibility_disk_format": "raw",\n  "derived_disk_virtual_size_bytes": 34359738368,\n  "seed_attachment": "qemu-cd-dvd",\n  "seed_contains_credential_hash": true\n}\n' \
  "$actual_sha256" "$recipe_sha256" >"$OUT/preparation-receipt.json"
printf 'prepared=%s\n' "$OUT"
