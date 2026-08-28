#!/bin/bash
# Resolve the image-first acquisition decision before provisioning a UTM court.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REGISTRY="${MINICON_UTM_IMAGE_SOURCES:-$SCRIPT_DIR/utm-image-sources.json}"

usage() {
  echo "usage: scripts/utm-image-source.sh list|select [CELL]" >&2
  exit 2
}

[ -f "$REGISTRY" ] || { echo "image source registry not found" >&2; exit 2; }
command="${1:-}"
case "$command" in
  list) [ "$#" -eq 1 ] || usage ;;
  select) [ "$#" -eq 2 ] || usage ;;
  *) usage ;;
esac

python3 - "$REGISTRY" "$command" "${2:-}" <<'PY'
import json
import sys

path, command, requested = sys.argv[1:]
with open(path, encoding="utf-8") as stream:
    registry = json.load(stream)

allowed = {"prebuilt-selected", "recipe-selected", "no-qualified-image"}
cells = registry.get("cells")
if registry.get("schema") != 1 or not isinstance(cells, list):
    raise SystemExit("invalid image source registry")

for cell in cells:
    if cell.get("selection") not in allowed:
        raise SystemExit(f"invalid selection for {cell.get('cell')}")

if command == "list":
    print(json.dumps({
        "schema": registry["schema"],
        "observed": registry.get("observed"),
        "cells": [{
            "cell": cell["cell"],
            "selection": cell["selection"],
            "source": cell.get("source"),
        } for cell in cells],
    }, ensure_ascii=False, sort_keys=True))
    raise SystemExit(0)

matches = [cell for cell in cells if cell.get("cell") == requested]
if len(matches) != 1:
    print(json.dumps({
        "cell": requested,
        "selection": "no-qualified-image",
        "reason": "cell is absent from the image source registry",
    }, ensure_ascii=False, sort_keys=True))
    raise SystemExit(3)
print(json.dumps(matches[0], ensure_ascii=False, sort_keys=True))
PY
