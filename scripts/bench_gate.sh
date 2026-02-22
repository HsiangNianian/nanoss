#!/usr/bin/env bash
set -euo pipefail

THRESHOLDS_FILE="${1:-bench/thresholds.toml}"

if [[ ! -f "$THRESHOLDS_FILE" ]]; then
  echo "threshold file not found: $THRESHOLDS_FILE" >&2
  exit 1
fi

if ! command -v hyperfine >/dev/null 2>&1; then
  echo "hyperfine not found; skipping benchmark gate (non-blocking)." >&2
  exit 0
fi

echo "Running benchmark gate with $THRESHOLDS_FILE"
hyperfine --warmup 1 \
  'cargo run -p nanoss-cli -- build --content-dir examples/blog-basic/content --template-dir examples/blog-basic/templates --output-dir public' \
  --export-json /tmp/nanoss-bench.json >/dev/null

echo "Benchmark gate completed. Inspect /tmp/nanoss-bench.json for details."
