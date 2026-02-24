#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <artifact_dir_or_archive> [target_dir]" >&2
  exit 1
fi

SOURCE="$1"
TARGET="${2:-docs-public}"
TMP_DIR=""

cleanup() {
  if [[ -n "$TMP_DIR" && -d "$TMP_DIR" ]]; then
    rm -rf "$TMP_DIR"
  fi
}
trap cleanup EXIT

if [[ -f "$SOURCE" ]]; then
  TMP_DIR="$(mktemp -d)"
  case "$SOURCE" in
    *.tar.gz|*.tgz)
      tar -xzf "$SOURCE" -C "$TMP_DIR"
      SOURCE="$TMP_DIR"
      ;;
    *.zip)
      unzip -q "$SOURCE" -d "$TMP_DIR"
      SOURCE="$TMP_DIR"
      ;;
    *)
      echo "unsupported archive format: $SOURCE" >&2
      exit 1
      ;;
  esac
fi

if [[ ! -d "$SOURCE" ]]; then
  echo "source not found: $SOURCE" >&2
  exit 1
fi

rm -rf "$TARGET"
mkdir -p "$TARGET"
cp -R "$SOURCE"/. "$TARGET"/
echo "rollback completed: restored $TARGET from $1"
