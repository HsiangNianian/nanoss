#!/usr/bin/env bash
set -euo pipefail

THRESHOLDS_FILE="${1:-bench/thresholds.toml}"

if [[ ! -f "$THRESHOLDS_FILE" ]]; then
  echo "threshold file not found: $THRESHOLDS_FILE" >&2
  exit 1
fi

if ! command -v hyperfine >/dev/null 2>&1; then
  echo "hyperfine not found; skipping benchmark timing (non-blocking)." >&2
else
  echo "Running benchmark gate with $THRESHOLDS_FILE"
  hyperfine --warmup 1 \
    'cargo run -p nanoss-cli -- build --content-dir examples/blog-basic/content --template-dir examples/blog-basic/templates --output-dir public' \
    --export-json /tmp/nanoss-bench.json >/dev/null
fi

echo "Running regression gate (images + i18n + remote data)"
cargo run -p nanoss-cli -- build \
  --content-dir examples/blog-basic/content \
  --template-dir examples/blog-basic/templates \
  --output-dir public >/tmp/nanoss-regression.log

if [[ ! -f "public/sitemap.xml" || ! -f "public/rss.xml" ]]; then
  echo "BENCH_GATE status=failed reason=missing_seo_outputs" >&2
  exit 1
fi

echo "BENCH_GATE status=ok bench_json=/tmp/nanoss-bench.json regression_log=/tmp/nanoss-regression.log"
