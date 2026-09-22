#!/bin/bash
# Compatibility name. The lane is local-accelerated (host/Lima/UTM execute-only).
exec "$(cd "$(dirname "$0")" && pwd)/local-accelerated.sh" "$@"
