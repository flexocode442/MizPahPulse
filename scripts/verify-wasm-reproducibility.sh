#!/usr/bin/env bash
#
# Verify that the PulseContract WASM build is reproducible.
#
# The contract is built twice into *separate* clean target directories using
# the same source tree, toolchain, and dependency lockfile. If the resulting
# `pulse_contract.wasm` artifacts have identical sha256 hashes, the build is
# reproducible and deployed artifacts can be trusted to match this source.
#
# Usage:
#   bash scripts/verify-wasm-reproducibility.sh
#
# CI runs this on `main` (see .github/workflows/ci.yml, job
# `contract-reproducibility`). Requires the `wasm32-unknown-unknown` target:
#   rustup target add wasm32-unknown-unknown
#
# Exit codes: 0 = reproducible, 1 = hashes differ or build failed.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT/contracts"

TARGET="wasm32-unknown-unknown"
ARTIFACT="pulse_contract.wasm"

echo "── MizPahPulse WASM reproducibility check ──────────────────────────"
echo "Toolchain : $(rustc --version) ($(cargo --version))"
echo "Target    : ${TARGET} (release)"
echo "Lockfile  : $(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo 'n/a') (Cargo.lock)"
echo ""

# Isolated target dirs so neither build can reuse the other's artifacts.
BUILD_A="$(mktemp -d)"
BUILD_B="$(mktemp -d)"
trap 'rm -rf "$BUILD_A" "$BUILD_B"' EXIT

build_once() {
  local target_dir="$1"
  CARGO_TARGET_DIR="$target_dir" cargo build --target "$TARGET" --release --locked >/dev/null
  sha256sum "$target_dir/$TARGET/release/$ARTIFACT" | awk '{print $1}'
}

echo "[1/2] Building WASM (clean target dir A)..."
HASH_A="$(build_once "$BUILD_A")"
echo "      sha256: $HASH_A"

echo "[2/2] Building WASM (clean target dir B)..."
HASH_B="$(build_once "$BUILD_B")"
echo "      sha256: $HASH_B"

echo ""
if [ "$HASH_A" = "$HASH_B" ]; then
  echo "✅ Reproducible: both builds produced identical WASM ($HASH_A)"
  exit 0
else
  echo "❌ NOT reproducible: WASM hashes differ"
  echo "   build A: $HASH_A"
  echo "   build B: $HASH_B"
  echo ""
  echo "Check for nondeterminism: build.rs scripts, timestamps, toolchain"
  echo "drift, or uncommitted Cargo.lock changes."
  exit 1
fi