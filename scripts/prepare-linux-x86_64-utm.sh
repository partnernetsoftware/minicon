#!/bin/bash
# Verify the pinned Ubuntu Server LTS x86_64 installer and publish a VM recipe receipt.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
OUT_DIR="${MINICON_LINUX_X86_64_UTM_DIR:-$REPO_ROOT/target-six/linux-x86_64-utm}"
MEDIA="$OUT_DIR/ubuntu-24.04.4-live-server-amd64.iso"
RECEIPT="$OUT_DIR/preparation-receipt.json"
EXPECTED_SHA256="e907d92eeec9df64163a7e454cbc8d7755e8ddc7ed42f99dbc80c40f1a138433"
SOURCE_URL="https://releases.ubuntu.com/24.04.4/ubuntu-24.04.4-live-server-amd64.iso"
SEED="$OUT_DIR/autoinstall-seed.iso"

umask 077

mkdir -p "$OUT_DIR"
if [ ! -f "$MEDIA" ]; then
  printf 'media missing: %s\nsource: %s\n' "$MEDIA" "$SOURCE_URL" >&2
  exit 3
fi

actual_sha256="$(shasum -a 256 "$MEDIA" | awk '{print $1}')"
[ "$actual_sha256" = "$EXPECTED_SHA256" ] || {
  printf 'media digest mismatch: expected %s, got %s\n' \
    "$EXPECTED_SHA256" "$actual_sha256" >&2
  exit 1
}

if [ -z "${MINICON_LINUX_X86_64_PASSWORD_HASH:-}" ]; then
  [ -t 0 ] || {
    echo "MINICON_LINUX_X86_64_PASSWORD_HASH is required outside an interactive terminal" >&2
    exit 2
  }
  printf 'Linux x86_64 guest password: ' >&2
  IFS= read -r -s password
  printf '\nConfirm Linux x86_64 guest password: ' >&2
  IFS= read -r -s password_confirm
  printf '\n' >&2
  [ -n "$password" ] && [ "$password" = "$password_confirm" ] || {
    unset password password_confirm
    echo "passwords are empty or do not match" >&2
    exit 2
  }
  MINICON_LINUX_X86_64_PASSWORD_HASH="$(printf '%s\n' "$password" | openssl passwd -6 -stdin)"
  unset password password_confirm
fi
case "$MINICON_LINUX_X86_64_PASSWORD_HASH" in '$6$'*) ;; *)
  echo "MINICON_LINUX_X86_64_PASSWORD_HASH must start with \$6\$" >&2
  exit 2
esac

seed_root="$(mktemp -d)"
trap 'rm -rf "$seed_root"' EXIT
sed "s|@PASSWORD_HASH@|$MINICON_LINUX_X86_64_PASSWORD_HASH|" \
  "$SCRIPT_DIR/linux-x86_64-autoinstall.yaml" >"$seed_root/user-data"
printf '%s\n' \
  'instance-id: minicon-linux-x86-64-v1' \
  'local-hostname: minicon-x86-court' >"$seed_root/meta-data"
rm -f "$SEED"
hdiutil makehybrid -quiet -iso -joliet -default-volume-name cidata \
  -o "$SEED" "$seed_root"

media_bytes="$(stat -f '%z' "$MEDIA")"
recipe_sha256="$(shasum -a 256 "$SCRIPT_DIR/linux-x86_64-autoinstall.yaml" | awk '{print $1}')"
python3 - "$RECEIPT" "$EXPECTED_SHA256" "$media_bytes" "$SOURCE_URL" "$recipe_sha256" <<'PY'
import json
import sys
from pathlib import Path

receipt = {
    "schema": 1,
    "court": "lnx-x86_64-desktop",
    "cell": "lnx-x86_64",
    "media": {
        "file": "ubuntu-24.04.4-live-server-amd64.iso",
        "sha256": sys.argv[2],
        "bytes": int(sys.argv[3]),
        "source": sys.argv[4],
    },
    "recipe_sha256": sys.argv[5],
    "seed_attachment": "qemu-cd-dvd",
    "seed_contains_credential_hash": True,
    "utm_recipe": {
        "backend": "qemu-tcg",
        "architecture": "x86_64",
        "machine": "q35",
        "cpu_count": 2,
        "memory_mib": 4096,
        "disk_virtual_gib": 32,
        "display_acceleration": False,
        "network": "shared",
        "guest_agent": "qemu-guest-agent",
        "idle": "stop",
    },
    "template_state": "media-verified",
    "non_goals": [
        "performance evidence under cross-ISA emulation",
        "build or compilation inside the guest",
        "credentials or product artifacts in the reusable template",
    ],
}
path = Path(sys.argv[1])
temporary = path.with_suffix(path.suffix + ".tmp")
temporary.write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
temporary.replace(path)
PY

printf 'verified_media=%s\nseed=%s\nreceipt=%s\n' "$MEDIA" "$SEED" "$RECEIPT"
