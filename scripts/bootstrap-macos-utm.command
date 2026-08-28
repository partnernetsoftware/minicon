#!/bin/bash
# One-click bootstrap for a clean macOS UTM runtime court.

set -euo pipefail

AUTO_MOUNT="/Volumes/My Shared Files"
MOUNT_POINT="$HOME/Library/Caches/minicon-utm-share"

if [ -d "$AUTO_MOUNT/bootstrap" ]; then
  MOUNT_POINT="$AUTO_MOUNT"
elif [ -d "$AUTO_MOUNT/macos-utm-bridge/bootstrap" ]; then
  MOUNT_POINT="$AUTO_MOUNT/macos-utm-bridge"
else
  mkdir -p "$MOUNT_POINT"
  mount_virtiofs share "$MOUNT_POINT"
fi
"$MOUNT_POINT/bootstrap/setup-macos-utm-runner-v2.sh"

printf '\nMiniCon UTM runtime agent installed. You may close this window.\n'
