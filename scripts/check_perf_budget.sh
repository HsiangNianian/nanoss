#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <site_output_dir>" >&2
  exit 1
fi

SITE_DIR="$1"
MAX_HTML_BYTES="${MAX_HTML_BYTES:-200000}"
MAX_JS_BYTES="${MAX_JS_BYTES:-350000}"
MAX_CSS_BYTES="${MAX_CSS_BYTES:-250000}"
MAX_TOTAL_BYTES="${MAX_TOTAL_BYTES:-30000000}"

if [[ ! -d "$SITE_DIR" ]]; then
  echo "site output dir not found: $SITE_DIR" >&2
  exit 1
fi

status=0

check_ext_budget() {
  local ext="$1"
  local budget="$2"
  while IFS='|' read -r size file; do
    [[ -z "$file" ]] && continue
    if (( size > budget )); then
      echo "budget exceeded: $file is ${size}B (limit ${budget}B)"
      status=1
    fi
  done < <(rg --files -g "*.${ext}" "$SITE_DIR" | while IFS= read -r f; do
    printf '%s|%s\n' "$(wc -c < "$f")" "$f"
  done)
}

check_ext_budget "html" "$MAX_HTML_BYTES"
check_ext_budget "js" "$MAX_JS_BYTES"
check_ext_budget "css" "$MAX_CSS_BYTES"

total_bytes="$(du -sb "$SITE_DIR" | awk '{print $1}')"
if (( total_bytes > MAX_TOTAL_BYTES )); then
  echo "total output budget exceeded: ${total_bytes}B (limit ${MAX_TOTAL_BYTES}B)"
  status=1
fi

if (( status != 0 )); then
  exit "$status"
fi

echo "performance budgets passed for $SITE_DIR"
