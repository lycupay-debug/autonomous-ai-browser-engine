#!/bin/sh
# Run the FreeBSD Capsicum, Broker/Renderer, origin storage, crash recovery, UDS and Jail PoCs.
set -eu
REPO_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUT_DIR=${AIBE_SECURITY_OUT:-/tmp/aibe-security-pocs}
mkdir -p "$OUT_DIR"

clang -Wall -Wextra -Werror -O2 \
  "$REPO_DIR/experiments/security/capsicum-minimal/src/main.c" \
  -o "$OUT_DIR/capsicum-minimal"
clang -Wall -Wextra -Werror -O2 \
  "$REPO_DIR/experiments/security/broker-renderer-capsicum-poc.c" \
  -o "$OUT_DIR/broker-renderer-capsicum"
clang -Wall -Wextra -Werror -O2 \
  "$REPO_DIR/experiments/security/uds-peer-credentials-poc.c" \
  -o "$OUT_DIR/uds-peer-credentials"
clang -Wall -Wextra -Werror -O2 \
  "$REPO_DIR/experiments/security/origin-storage-capsicum-poc.c" \
  -o "$OUT_DIR/origin-storage-capsicum"
clang -Wall -Wextra -Werror -O2 \
  "$REPO_DIR/experiments/security/renderer-supervisor-poc.c" \
  -o "$OUT_DIR/renderer-supervisor"

printf '%s\n' '=== Capsicum descriptor rights ==='
"$OUT_DIR/capsicum-minimal"
printf '%s\n' '=== Broker/Renderer process boundary ==='
"$OUT_DIR/broker-renderer-capsicum"
printf '%s\n' '=== UDS peer credentials ==='
"$OUT_DIR/uds-peer-credentials"
printf '%s\n' '=== Per-origin storage isolation ==='
"$OUT_DIR/origin-storage-capsicum"
printf '%s\n' '=== Renderer crash recovery ==='
"$OUT_DIR/renderer-supervisor"
printf '%s\n' '=== Jail network isolation ==='
"$REPO_DIR/scripts/run-jail-network-poc.sh"
