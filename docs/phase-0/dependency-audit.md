# Phase 0 dependency and licensing audit — initial snapshot

Snapshot environment: FreeBSD 15.1-RELEASE-p6 amd64; Cargo.lock committed with the experiment workspace.

## Direct dependencies

| Crate | Version | Declared upstream license |
|---|---:|---|
| `boa_engine` | 0.22.0 | Unlicense OR MIT |
| `boa_gc` | 0.22.0 | Unlicense OR MIT |
| `swash` | 0.2.10 | Apache-2.0 OR MIT |
| `png` | 0.18.1 | MIT OR Apache-2.0 |
| `ipnet` | 2.12.2 | MIT OR Apache-2.0 |
| `serde_json` | 1.0.154 | MIT OR Apache-2.0 |
| `cc` (build dependency) | 1.10.0 | MIT OR Apache-2.0 |
| `rustls` | 0.23.46 | Apache-2.0 OR ISC OR MIT |
| `rustls-pki-types` | 1.15.1 | Apache-2.0 OR ISC OR MIT |
| `url` | 2.5.8 | MIT OR Apache-2.0 |
| `image` | 0.25.10 | MIT OR Apache-2.0 |

## Cargo metadata scan

- Initial snapshot: `cargo metadata --locked --format-version 1` returned 171 packages; 166 third-party packages declared license metadata; five project-owned PoC crates remained unlicensed.
- Previous scan after adding Rustls, URL parsing, Rustybuzz and image decoding: 212 packages in total; 5 workspace PoC crates and 207 third-party packages. After the Swash migration, the current lockfile contains 208 packages.
- All 207 third-party packages declare license metadata in Cargo package metadata. This is metadata presence, not legal approval or source/asset audit.
- The five project-owned PoC crates do not declare a license; this is intentional pending an explicit project-owner license decision.
- No project-wide LICENSE file has been created. Public repository visibility is not a grant of reuse rights.
- This scan reads package metadata only. It is not a full legal audit and does not yet inspect bundled assets, build scripts, optional feature combinations, source headers, or every transitive notice.

## Current decision

Do not choose or publish a project license until the owner explicitly approves it. Before shipping, run a dedicated license/notice/SBOM audit against the exact release dependency graph and retain required notices for fonts and other assets.


## 2026-11-01 expanded direct dependency set

- `rustls` 0.23.46 with the `ring`, `std`, and `tls12` features; default AWS-LC provider features are disabled.
- `rustls-pki-types` 1.15.1 for parsing the system PEM CA bundle.
- `url` 2.5.8 for URL parsing and relative redirect resolution.
- `swash` 0.2.10 for Unicode-script shaping, OpenType parsing, outline scaling, and glyph rasterization; its resolved font parser is `skrifa` 0.44.0.
- `image` 0.25.10 with only PNG, JPEG, GIF, and WebP features enabled.
- All versions are pinned exactly in workspace manifests and Cargo.lock. Current Cargo metadata reports 208 packages: 203 third-party packages with license metadata and five project-owned PoC crates without a selected project license.
- `cargo-audit` and `cargo-deny` are not installed in the FreeBSD host environment. GitHub Actions installed `cargo-audit` 0.22.2 and scanned the lockfile; Run 10 found five security advisories affecting the pinned rustls and rustls-webpki versions. The project has since upgraded those dependencies and removed the two unmaintained font crates; a fresh CI audit is required before claiming a clean scan.
- Five CycloneDX 1.5 SBOMs have now been generated and committed under `sbom/`, one per workspace crate. They describe Rust/Cargo dependencies only and do not include native V8/SpiderMonkey packages, the temporary Noto font, or the FreeBSD base/system package closure.


## SBOM generation

- Generator: `cargo-cyclonedx` 0.5.9 (`--locked` installation).
- Format: CycloneDX JSON 1.5.
- Artifacts: `sbom/aibe-boa-host.cdx.json`, `sbom/aibe-control-protocol-uds.cdx.json`, `sbom/aibe-headless-raster.cdx.json`, `sbom/aibe-js-runtime-ffi.cdx.json`, and `sbom/aibe-network-policy.cdx.json`.
- Component counts before font-stack remediation: 146, 5, 45, 3, and 53 respectively. The refreshed 2026-11-01 SBOM set contains 146, 5, 37, 3, and 53 components respectively.
- The SBOMs cover each Cargo crate's resolved dependency graph, not native packages, system libraries, fonts, or the complete host OS package closure.

## 2026-11-01 RustSec remediation and font stack maintenance

- GitHub Actions Run 10 successfully executed cargo-audit 0.22.2 against 212 Cargo dependencies. It reported four advisories for rustls 0.23.43 (RUSTSEC-2026-0346, RUSTSEC-2026-0285, RUSTSEC-2026-0348, RUSTSEC-2026-0349) and one for rustls-webpki 0.103.15 (RUSTSEC-2026-0382). The recommended patched versions were rustls >=0.23.46 and rustls-webpki >=0.103.16.
- Updated the direct exact pin to rustls 0.23.46 and Cargo.lock to rustls-webpki 0.103.16. Network-policy tests passed 17/17; HTTPS GET to example.com returned 200; HTTPS redirect httpbin.org/redirect/1 returned 302 then 200 with the final peer IP recorded.
- Run 10 also warned that rustybuzz 0.20.1 (RUSTSEC-2026-0206) and ttf-parser 0.25.1 (RUSTSEC-2026-0192) are unmaintained. Replaced the rustybuzz + fontdue shaping/raster pair with swash 0.2.10, backed by skrifa 0.44.0 / read-fonts 0.41.0. rustybuzz, fontdue, and ttf-parser are now absent from Cargo.lock.
- Headless tests passed 4/4, including mixed Latin/CJK script shaping and rasterization, malformed font rejection, invalid-size rejection, and PNG/JPEG/GIF/WebP decode roundtrips.
- Full FreeBSD workspace tests passed 46/46; cargo fmt --all -- --check and Clippy with -D warnings passed after formatting. All tests used the extracted V8/SpiderMonkey prefix under /tmp/aibe-runtime-root/usr/local; no system packages were installed.
- The unmaintained font-stack warnings have been addressed by dependency replacement. The upgraded Rustls advisories are pending a new GitHub Actions cargo-audit run; do not mark the dependency scan clean until that run completes.
- Post-remediation Cargo metadata scan: 208 total packages, 203 third-party packages, all 203 third-party packages declare license metadata, and five project-owned crates remain without a selected project license.
- The five committed SBOM files still describe the previous dependency graph; regenerate and validate them from this lockfile before treating repository SBOMs as current.
