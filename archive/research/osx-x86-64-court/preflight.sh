#!/usr/bin/env bash
set -euo pipefail

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
source_file="${UTM_COURT_IMAGE_SOURCES:-$repo_root/../utm-court/courts/image-sources.json}"

fail=0
check() {
  local label=$1
  shift
  if "$@" >/dev/null 2>&1; then
    printf 'PASS: %s\n' "$label"
  else
    printf 'FAIL: %s\n' "$label"
    fail=1
  fi
}

check "UTM application is installed" test -d /Applications/UTM.app
check "QEMU x86_64 engine is bundled" test -x /Applications/UTM.app/Contents/Frameworks/qemu-x86_64-softmmu.framework/Versions/A/qemu-x86_64-softmmu
check "image-source registry exists" test -f "$source_file"
check "osx-x86_64 remains explicitly unqualified" \
  jq -e '.cells[] | select(.cell == "osx-x86_64" and .selection == "no-qualified-image")' "$source_file"

apple_media=${MINICON_OSX_X86_APPLE_MEDIA:-}
opencore_media=${MINICON_OSX_X86_OPENCORE_MEDIA:-}
prebuilt_image=${MINICON_OSX_X86_PREBUILT_IMAGE:-}
prebuilt_manifest=${MINICON_OSX_X86_PREBUILT_MANIFEST:-}

if [[ -n "$prebuilt_image" || -n "$prebuilt_manifest" ]]; then
  if [[ ! -f "$prebuilt_image" || ! -f "$prebuilt_manifest" ]]; then
    printf 'FAIL: preinstalled route requires both image and provenance manifest files\n'
    exit 2
  fi
  if ! jq -e '
    .schema == 1 and
    (.builder | type == "string" and length > 0) and
    (.apple_media.source | type == "string" and length > 0) and
    (.apple_media.sha256 | test("^[0-9a-fA-F]{64}$")) and
    (.image.sha256 | test("^[0-9a-fA-F]{64}$")) and
    (.machine.arch == "x86_64") and
    (.machine.qemu_args | type == "array") and
    (.default_credentials_disclosed | type == "boolean")
  ' "$prebuilt_manifest" >/dev/null; then
    printf 'FAIL: preinstalled provenance manifest is incomplete\n'
    exit 2
  fi
  actual_image_sha=$(shasum -a 256 "$prebuilt_image" | awk '{print $1}')
  declared_image_sha=$(jq -r '.image.sha256 | ascii_downcase' "$prebuilt_manifest")
  if [[ "$actual_image_sha" != "$declared_image_sha" ]]; then
    printf 'FAIL: preinstalled image hash does not match its manifest\n'
    exit 2
  fi
  printf 'PASS: reviewed preinstalled x86_64 image supplied (sha256=%s)\n' "$actual_image_sha"
  printf 'DECISION: C1 preinstalled branch passed; import scratch VM, rotate credentials, then run C3\n'
  exit 0
fi

if [[ -z "$apple_media" ]]; then
  printf 'NEEDED: set MINICON_OSX_X86_APPLE_MEDIA to one Apple-origin Intel installer path\n'
  fail=1
elif [[ ! -f "$apple_media" ]]; then
  printf 'FAIL: Apple installer path is not a regular file\n'
  fail=1
else
  printf 'PASS: Apple installer supplied (sha256=%s)\n' "$(shasum -a 256 "$apple_media" | awk '{print $1}')"
fi

if [[ -z "$opencore_media" ]]; then
  printf 'NEEDED: set MINICON_OSX_X86_OPENCORE_MEDIA to one pinned OpenCore image path\n'
  fail=1
elif [[ ! -f "$opencore_media" ]]; then
  printf 'FAIL: OpenCore path is not a regular file\n'
  fail=1
else
  printf 'PASS: OpenCore image supplied (sha256=%s)\n' "$(shasum -a 256 "$opencore_media" | awk '{print $1}')"
fi

if (( fail != 0 )); then
  printf 'DECISION: C1 not passed; VM creation is not authorized\n'
  exit 2
fi

printf 'DECISION: C1 inputs present; record provenance before attempting C2\n'
