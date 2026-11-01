#!/bin/sh
# Build and run a temporary one-Renderer/one-Jail lifecycle and RCTL proof.
set -eu
if [ "$(uname -s)" != "FreeBSD" ]; then
  echo "SKIP: Renderer Jail lifecycle PoC requires FreeBSD" >&2
  exit 77
fi
if [ "$(id -u)" -ne 0 ]; then
  echo "ERROR: run this PoC as root" >&2
  exit 77
fi
REPO_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUT_DIR=${AIBE_JAIL_POC_OUT:-/tmp/aibe-renderer-jail-poc}
mkdir -p "$OUT_DIR"
clang -Wall -Wextra -Werror -O2 -static \
  "$REPO_DIR/experiments/security/renderer-jail-lifecycle-poc.c" \
  -o "$OUT_DIR/renderer-jail-lifecycle-poc"
"$OUT_DIR/renderer-jail-lifecycle-poc"
