#!/usr/bin/env python3
"""Validate the expected per-crate CycloneDX SBOM set."""
import json
import os
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
    "aibe-html-parser.cdx.json",
    "aibe-snapshot-bridge.cdx.json",
    "aibe-renderer-supervisor.cdx.json",
}
root = Path(os.environ.get("AIBE_SBOM_DIR", Path(__file__).resolve().parents[1] / "sbom"))
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
    metadata_component = document.get("metadata", {}).get("component")
    if not isinstance(metadata_component, dict) or not all(
        metadata_component.get(field) for field in ("type", "name", "version", "bom-ref")
    ):
        raise SystemExit(f"{name}: missing or incomplete metadata root component")

    components = document.get("components", [])
    dependencies = document.get("dependencies", [])
    if not isinstance(components, list) or not isinstance(dependencies, list):
        raise SystemExit(f"{name}: components/dependencies must be arrays")

    # A dependency-free workspace member legitimately has only its metadata
    # root component. Do not fabricate a dependency merely to satisfy the
    # validator; require a dependency graph whenever components are present.
    if components and not dependencies:
        raise SystemExit(f"{name}: components exist but dependency graph is missing")
    if not components and dependencies:
        root_ref = metadata_component["bom-ref"]
        if len(dependencies) != 1 or dependencies[0].get("ref") != root_ref or dependencies[0].get("dependsOn"):
            raise SystemExit(f"{name}: dependency-free BOM has an inconsistent root graph")

    known_refs = {metadata_component["bom-ref"]}
    for component in components:
        if not isinstance(component, dict) or not component.get("bom-ref"):
            raise SystemExit(f"{name}: component missing bom-ref")
        known_refs.add(component["bom-ref"])
    for dependency in dependencies:
        if not isinstance(dependency, dict) or dependency.get("ref") not in known_refs:
            raise SystemExit(f"{name}: dependency graph contains an unknown component reference")
        if not isinstance(dependency.get("dependsOn", []), list):
            raise SystemExit(f"{name}: dependency dependsOn must be an array")
        if any(reference not in known_refs for reference in dependency.get("dependsOn", [])):
            raise SystemExit(f"{name}: dependency edge references an unknown component")

    print(f"PASS {name}: {len(components)} components")
