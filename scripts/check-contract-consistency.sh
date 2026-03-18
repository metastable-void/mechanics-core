#!/usr/bin/env bash
set -euo pipefail

echo "[1/6] Build baseline"
cargo check

echo "[2/6] Test baseline"
cargo test --all-targets

echo "[3/6] Clippy baseline"
cargo clippy --all-targets --all-features -- -D warnings

echo "[4/6] Rustdoc warnings as errors"
RUSTDOCFLAGS='-D warnings' cargo doc --no-deps

echo "[5/6] Missing docs gate"
RUSTFLAGS='-D missing-docs' cargo check

echo "[6/6] Contract drift scan (docs/decl/schema naming)"
if rg -n "run_try_enqueue|mod_source|allow_non_success_status|with_allow_non_success_status" \
  README.md docs/*.md ts-types/*.d.ts json-schema/*.json src; then
  echo "found stale contract names; update docs/schema/decl/code"
  exit 1
fi

if rg -n "src/http/|src/pool/|src/runtime/|src/executor.rs" README.md docs/*.md; then
  echo "found stale source paths in docs; update to src/internal/* paths"
  exit 1
fi

echo "contract-consistency checks passed"
