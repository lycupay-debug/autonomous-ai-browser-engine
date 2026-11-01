#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
response_body=$(mktemp -t aibe-html-body)
trap 'rm -f "$response_body"' EXIT HUP INT TERM

# Keep the network fetcher and browser/HTML runtime in separate processes.
# This is a temporary bounded handoff, not the final Broker/Renderer IPC.
cargo run --offline --locked -p aibe-network-policy-poc --   https://example.com/ /etc/ssl/cert.pem --body-stdout > "$response_body"
cargo run --offline --locked -p aibe-html-parser-poc --bin aibe-html-parser-poc   < "$response_body"
