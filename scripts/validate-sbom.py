#!/usr/bin/env python3
"""Validate the expected per-crate CycloneDX SBOM set."""
import json
from pathlib import Path

EXPECTED = {
    "aibe-boa-host.cdx.json",
    "aibe-control-protocol-uds.cdx.json",
    "aibe-headless-raster.cdx.json",
    "aibe-js-runtime-ffi.cdx.json",
    "aibe-network-policy.cdx.json",
    "aibe-origin-policy.cdx.json",
    "aibe-browser-runtime-origin.cdx.json",
    "aibe-broker-policy.cdx.json",
}
root = Path(__file__).resolve().parents[1] / "sbom"
found = {path.name for path in root.glob("*.cdx.json")}
if found != EXPECTED:
    raise SystemExit(f"SBOM file set mismatch: expected={sorted(EXPECTED)} found={sorted(found)}")

for name in sorted(EXPECTED):
    path = root / name
    document = json.loads(path.read_text(encoding="utf-8"))
    if document.get("bomFormat") != "CycloneDX" or document.get("specVersion") != "1.5":
        raise SystemExit(f"{name}: expected CycloneDX 1.5")
    if not document.get("serialNumber", "").startswith("urn:uuid:"):
        raise SystemExit(f"{name}: missing serialNumber")
    if not document.get("metadata", {}).get("timestamp"):
        raise SystemExit(f"{name}: missing metadata timestamp")
    if not document.get("components") or not document.get("dependencies"):
        raise SystemExit(f"{name}: missing component/dependency graph")
    print(f"PASS {name}: {len(document['components'])} components")
