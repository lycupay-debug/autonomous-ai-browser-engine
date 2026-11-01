# Phase 0 dependency and licensing audit — initial snapshot

Snapshot environment: FreeBSD 15.1-RELEASE-p6 amd64; Cargo.lock committed with the experiment workspace.

## Direct dependencies

| Crate | Version | Declared upstream license |
|---|---:|---|
| `boa_engine` | 0.22.0 | Unlicense OR MIT |
| `boa_gc` | 0.22.0 | Unlicense OR MIT |
| `fontdue` | 0.9.5 | MIT OR Apache-2.0 OR Zlib |
| `png` | 0.18.1 | MIT OR Apache-2.0 |
| `ipnet` | 2.12.2 | MIT OR Apache-2.0 |
| `serde_json` | 1.0.154 | MIT OR Apache-2.0 |
| `cc` (build dependency) | 1.10.0 | MIT OR Apache-2.0 |
| `rustls` | 0.23.43 | Apache-2.0 OR ISC OR MIT |
| `rustls-pki-types` | 1.15.1 | Apache-2.0 OR ISC OR MIT |
| `url` | 2.5.8 | MIT OR Apache-2.0 |
| `rustybuzz` | 0.20.1 | MIT |
| `image` | 0.25.10 | MIT OR Apache-2.0 |

## Cargo metadata scan

- Initial snapshot: `cargo metadata --locked --format-version 1` returned 171 packages; 166 third-party packages declared license metadata; five project-owned PoC crates remained unlicensed.
- Current scan after adding Rustls, URL parsing, Rustybuzz and image decoding: 212 packages in total; 5 workspace PoC crates and 207 third-party packages.
- All 207 third-party packages declare license metadata in Cargo package metadata. This is metadata presence, not legal approval or source/asset audit.
- The five project-owned PoC crates do not declare a license; this is intentional pending an explicit project-owner license decision.
- No project-wide LICENSE file has been created. Public repository visibility is not a grant of reuse rights.
- This scan reads package metadata only. It is not a full legal audit and does not yet inspect bundled assets, build scripts, optional feature combinations, source headers, or every transitive notice.

## Current decision

Do not choose or publish a project license until the owner explicitly approves it. Before shipping, run a dedicated license/notice/SBOM audit against the exact release dependency graph and retain required notices for fonts and other assets.


## 2026-11-01 expanded direct dependency set

- `rustls` 0.23.43 with the `ring`, `std`, and `tls12` features; default AWS-LC provider features are disabled.
- `rustls-pki-types` 1.15.1 for parsing the system PEM CA bundle.
- `url` 2.5.8 for URL parsing and relative redirect resolution.
- `rustybuzz` 0.20.1 for OpenType text shaping.
- `image` 0.25.10 with only PNG, JPEG, GIF, and WebP features enabled.
- All versions are pinned exactly in workspace manifests and Cargo.lock. Cargo metadata reports 212 packages, including 207 third-party packages with license metadata and five unlicensed project-owned PoC crates.
- `cargo-audit` and `cargo-deny` are not installed in the FreeBSD host environment. GitHub Actions installed `cargo-audit` 0.22.2 successfully, but the first invocation failed before scanning because `cargo audit` does not accept Cargo's `--locked` flag. The workflow invocation has been corrected; no advisory result is claimed until the corrected run completes.
- Five CycloneDX 1.5 SBOMs have now been generated and committed under `sbom/`, one per workspace crate. They describe Rust/Cargo dependencies only and do not include native V8/SpiderMonkey packages, the temporary Noto font, or the FreeBSD base/system package closure.


## SBOM generation

- Generator: `cargo-cyclonedx` 0.5.9 (`--locked` installation).
- Format: CycloneDX JSON 1.5.
- Artifacts: `sbom/aibe-boa-host.cdx.json`, `sbom/aibe-control-protocol-uds.cdx.json`, `sbom/aibe-headless-raster.cdx.json`, `sbom/aibe-js-runtime-ffi.cdx.json`, and `sbom/aibe-network-policy.cdx.json`.
- Component counts: 146, 5, 45, 3, and 53 respectively.
- The SBOMs cover each Cargo crate's resolved dependency graph, not native packages, system libraries, fonts, or the complete host OS package closure.
