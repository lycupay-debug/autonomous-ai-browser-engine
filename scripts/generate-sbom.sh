#!/bin/sh
# Generate one CycloneDX 1.5 SBOM per member of the Cargo workspace.
set -eu
REPO_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
CARGO_CYCLONEDX=${CARGO_CYCLONEDX:-cargo-cyclonedx}
command -v "$CARGO_CYCLONEDX" >/dev/null 2>&1 || {
  echo "cargo-cyclonedx 0.5.9 is required; install it with cargo install cargo-cyclonedx --version 0.5.9 --locked" >&2
  exit 127
}
mkdir -p "$REPO_DIR/sbom"
"$CARGO_CYCLONEDX" cyclonedx \
  --manifest-path "$REPO_DIR/Cargo.toml" \
  --format json --spec-version 1.5 --all

for entry in \
  "experiments/js-runtime/boa-host/aibe-boa-host-poc.cdx.json:aibe-boa-host.cdx.json" \
  "experiments/js-runtime/ffi-poc/aibe-js-runtime-ffi-poc.cdx.json:aibe-js-runtime-ffi.cdx.json" \
  "experiments/headless/raster-poc/aibe-headless-raster-poc.cdx.json:aibe-headless-raster.cdx.json" \
  "experiments/network-policy/aibe-network-policy-poc.cdx.json:aibe-network-policy.cdx.json" \
  "experiments/control-protocol/uds-poc/aibe-control-protocol-uds-poc.cdx.json:aibe-control-protocol-uds.cdx.json" \
  "experiments/origin-policy/aibe-origin-policy-poc.cdx.json:aibe-origin-policy.cdx.json" \
  "experiments/browser-runtime/aibe-browser-runtime-origin-poc.cdx.json:aibe-browser-runtime-origin.cdx.json" \
  "experiments/broker-policy/aibe-broker-policy-poc.cdx.json:aibe-broker-policy.cdx.json" \
  "experiments/html-parser/aibe-html-parser-poc.cdx.json:aibe-html-parser.cdx.json"
do
  source=${entry%%:*}
  destination=${entry##*:}
  mv "$REPO_DIR/$source" "$REPO_DIR/sbom/$destination"
done
