#!/bin/sh
# Generate pinned CycloneDX 1.5 SBOMs for each Cargo workspace crate.
set -eu
REPO_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
CARGO_CYCLONEDX=${CARGO_CYCLONEDX:-cargo-cyclonedx}
command -v "$CARGO_CYCLONEDX" >/dev/null 2>&1 || {
  echo "cargo-cyclonedx 0.5.9 is required; install it with cargo install cargo-cyclonedx --version 0.5.9 --locked" >&2
  exit 127
}
mkdir -p "$REPO_DIR/sbom"
for entry in \
  "experiments/js-runtime/boa-host:boa-host" \
  "experiments/js-runtime/ffi-poc:js-runtime-ffi" \
  "experiments/headless/raster-poc:headless-raster" \
  "experiments/network-policy:network-policy" \
  "experiments/control-protocol/uds-poc:control-protocol-uds"
do
  manifest=${entry%%:*}/Cargo.toml
  name=${entry##*:}
  "$CARGO_CYCLONEDX" cyclonedx \
    --manifest-path "$REPO_DIR/$manifest" \
    --format json --spec-version 1.5 --all \
    --override-filename "aibe-$name"
  mv "$REPO_DIR/${entry%%:*}/aibe-$name.json" "$REPO_DIR/sbom/aibe-$name.cdx.json"
done
