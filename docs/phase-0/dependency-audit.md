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

## Cargo metadata scan

- `cargo metadata --locked --format-version 1` returned 167 packages in the resolved dependency graph.
- 163 third-party packages declared license metadata.
- The four project-owned PoC crates do not declare a license; this is intentional pending an explicit project-owner license decision.
- No project-wide LICENSE file has been created. Public repository visibility is not a grant of reuse rights.
- This scan reads package metadata only. It is not a full legal audit and does not yet inspect bundled assets, build scripts, optional feature combinations, source headers, or every transitive notice.

## Current decision

Do not choose or publish a project license until the owner explicitly approves it. Before shipping, run a dedicated license/notice/SBOM audit against the exact release dependency graph and retain required notices for fonts and other assets.
