#!/bin/bash
# Build an isolated consumer so workspace feature unification cannot hide a
# dependency on core's default editor/codec features. Owner: PRD_02_28.
set -euo pipefail

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"
python_bin=""
for candidate in python3 python py; do
  if command -v "$candidate" >/dev/null 2>&1; then
    python_bin="$candidate"
    break
  fi
done
if [[ -z "$python_bin" ]]; then
  echo "check-reuse: Python is required" >&2
  exit 127
fi

probe=target/reuse-consumer
mkdir -p "$probe/src"
cat > "$probe/Cargo.toml" <<'TOML'
[package]
name = "minicon-reuse-consumer"
version = "0.0.0"
edition = "2024"
publish = false

[workspace]

[dependencies]
minicon-core = { path = "../../crates/minicon-core", default-features = false }
minicon-ui-core = { path = "../../crates/minicon-ui-core", default-features = false }
minicon-platform = { path = "../../crates/minicon-platform", default-features = false }
TOML
cat > "$probe/src/main.rs" <<'RUST'
fn main() {
    let nodes: [minicon_core::tree::TreeDepthNode<u32>; 1] = [
        minicon_ui_core::TreeDepthNode { id: 1, parent: None },
    ];
    let depths: Result<Vec<u32>, minicon_core::tree::TreeDepthError<u32>> =
        minicon_ui_core::compute_tree_depths(&nodes);
    assert_eq!(depths, Ok(vec![0]));
    let round: fn(f32) -> f32 = minicon_platform::numeric::round_f32;
    assert_eq!(round(-0.0).to_bits(), minicon_core::numeric::round_f32(-0.0).to_bits());
}
RUST
# Seed from the product lock to retain already-qualified registry versions;
# Cargo adjusts only this ignored consumer lock, never the product's lock.
cp Cargo.lock "$probe/Cargo.lock"
cargo metadata --format-version 1 --manifest-path "$probe/Cargo.toml" > "$probe/metadata.json"
"$python_bin" - "$probe/metadata.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    metadata = json.load(source)
packages = {package["id"]: package for package in metadata["packages"]}
nodes = {packages[node["id"]]["name"]: node for node in metadata["resolve"]["nodes"]}
core = nodes["minicon-core"]
ui = nodes["minicon-ui-core"]
assert not core["features"], f"core features leaked into minimal consumer: {core['features']}"
assert not core["deps"], "minimal core must have no dependency edges"
assert not ui["features"], f"UI features leaked into minimal consumer: {ui['features']}"
assert {edge["name"] for edge in ui["deps"]} == {"minicon_core"}, "minimal UI must depend only on core"
print("minimal reuse graph: core has no dependencies; UI depends only on core")
PY
cargo run --locked --manifest-path "$probe/Cargo.toml" \
  --config 'build.rustflags=["-D","dead_code","-D","unused_variables","-D","unused_must_use"]'
echo "minimal reuse consumer: PASS (native host only)"
