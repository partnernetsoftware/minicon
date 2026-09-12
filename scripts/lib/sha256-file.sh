#!/bin/bash
# One SHA-256 spelling for the release scripts. `sha256sum` ships with Linux and
# coreutils, `shasum` with macOS; a digest helper that only exists on one of them
# turns a release step into a platform-specific one. Sourced, not executed.

if command -v sha256sum >/dev/null 2>&1; then
  sha256_file() { sha256sum "$1" | awk '{print $1}'; }
elif command -v shasum >/dev/null 2>&1; then
  sha256_file() { shasum -a 256 "$1" | awk '{print $1}'; }
else
  echo "sha256-file.sh: no sha256sum or shasum on PATH" >&2
  return 1 2>/dev/null || exit 1
fi
