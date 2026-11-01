#!/bin/sh
# Reproducible policy tests; live network tests are opt-in.
set -eu
REPO_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$REPO_DIR"
cargo test --locked -p aibe-network-policy-poc -- --test-threads=1
if [ "${AIBE_RUN_NETWORK_INTEGRATION:-0}" = "1" ]; then
  cargo run --locked -p aibe-network-policy-poc -- https://example.com/ /etc/ssl/cert.pem
  cargo run --locked -p aibe-network-policy-poc -- https://httpbin.org/redirect/1 /etc/ssl/cert.pem
fi
