# Autonomous AI Browser Engine

A FreeBSD-native, AI-first browser engine research and development project.

## Project status

This repository is in **Phase 0: technical feasibility research**. It is not a usable browser and does not claim Web compatibility. The project owner has selected V8 15.6.75.19 as the primary JavaScript Runtime; Phase 0 still has open security and integration exit conditions.

The current V8 FFI PoC models host-issued origin groups separately from JavaScript Realms. Browser Navigation owns parsed HTML trees and exports versioned, bounded Document snapshots with revision-scoped node identity. The Broker validates kernel-authenticated Renderer PID/UID/GID and one-shot Origin/Document/revision grants before sending snapshots over UDS. A host-owned Renderer Supervisor now starts and retains child processes and authenticated streams, binds Renderer principals to the actual process, cleans grants after confirmed exit, and bounds per-process request replay history. The independent Renderer process reconstructs a generic V8 native Node-wrapper graph from the snapshot and verifies node count, tree edges and wrapper identity. This is still a Phase 0 PoC: wrapper type/name/value/attributes remain snapshot metadata, not standards-compliant native DOM state; there is no live DOM mutation, Web IDL, full Renderer sandbox, CSS/layout/paint or production Broker daemon. See `docs/phase-0/README.md` for evidence and limitations.

## Principles

- FreeBSD is the first native target.
- Rust is the primary implementation language; C/C++ dependencies must stay behind narrow, reviewed boundaries.
- The engine owns its architecture, process model, control protocol, permission model, and browser-facing APIs.
- No Playwright runtime, Chromium, or Servo as the final execution core.
- Headless-first; headed mode is retained as a later backend.
- AI has broad browser-level control, while host access and dangerous operations remain policy-gated.
- Security threat modeling and negative tests start in Phase 0 and continue throughout development. The accepted Renderer isolation direction is one dedicated FreeBSD Jail per Renderer; the Supervisor-to-Jail lifecycle and V8+Capsicum integration now have FreeBSD 15.1 integration-test evidence. See `docs/adr/0002-renderer-isolation-one-jail-per-renderer.md` and `docs/phase-0/README.md`.
- No compatibility claims without pinned WPT/Test262 versions and published results.

## Repository layout

- `experiments/js-runtime/boa-host/`: Boa embedding, DOM/JS lifecycle, Test262 subset and script runner PoCs.
- `experiments/js-runtime/native-embed/`: FreeBSD V8 and SpiderMonkey C++ embedding PoCs.
- `experiments/js-runtime/ffi-poc/`: Rust-owned RAII wrappers over native V8 and SpiderMonkey C ABI.
- `experiments/origin-policy/`: host-owned URL-origin canonicalization and opaque origin-group registry PoC.
- `experiments/browser-runtime/`: orchestration PoC mapping canonical origin groups to JS Runtime origin handles, Document/Frame/Worker lifetimes, minimal Document host initialization, Document-scoped script evaluation and ownership of parsed HTML trees.
- `experiments/html-parser/`: html5ever-backed HTML5 parsing PoC with a project-owned bounded DOM node arena.
- `experiments/js-runtime/benchmarks/`: common cross-runtime JavaScript microbenchmark.
- `experiments/security/`: FreeBSD Capsicum, per-origin storage descriptor, crash-recovery, peer-credential, Jail network-isolation and one-Renderer/one-Jail lifecycle PoCs.
- `experiments/renderer-supervisor/`: host-side Renderer lifecycle Supervisor with a FreeBSD Jail adapter, kernel PID/JID binding, per-Jail RCTL and rootfs/devfs cleanup.
- `scripts/build-renderer-rootfs.sh`: assemble a root-owned per-Renderer rootfs from the built Renderer and its resolved FreeBSD shared-library dependencies.
- `experiments/headless/raster-poc/`: OpenType shaping, CPU glyph rasterization and PNG/JPEG/GIF/WebP decode PoC.
- `experiments/network-policy/`: outbound IP policy, pinned TCP/TLS, SNI/certificate verification and per-redirect revalidation PoC.
- `docs/adr/`: accepted architecture decision records.
- `experiments/control-protocol/uds-poc/`: versioned JSON-over-UDS control contract, bounded binary Snapshot framing and kernel peer authentication.
- `experiments/renderer-supervisor/`: host-owned Renderer process lifecycle, authenticated UDS connection and request replay/quota PoC.
- `experiments/snapshot-bridge/`: Browser Runtime Document Snapshot → Broker grant → supervised Renderer → V8 Native Node-wrapper ingestion PoC.
- `docs/phase-0/`: Phase 0 scope, capability matrix, evidence, dependency audit and exit criteria.
- `.github/workflows/`: hosted CI checks; this is not a FreeBSD-native test result.

## Build and test

On a FreeBSD 15.1 amd64 host with Rust/Cargo installed:

```sh
AIBE_RUNTIME_PREFIX=/tmp/aibe-runtime-root/usr/local LD_LIBRARY_PATH=/tmp/aibe-runtime-root/usr/local/lib AIBE_V8_EXECUTABLE=/tmp/aibe-runtime-root/usr/local/bin/d8 cargo test --workspace --locked -- --test-threads=1
cargo fmt --all -- --check
AIBE_RUNTIME_PREFIX=/tmp/aibe-runtime-root/usr/local LD_LIBRARY_PATH=/tmp/aibe-runtime-root/usr/local/lib AIBE_V8_EXECUTABLE=/tmp/aibe-runtime-root/usr/local/bin/d8 cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
./scripts/run-test262-subset.sh
AIBE_RUNTIME_PREFIX=/tmp/aibe-runtime-root/usr/local ./scripts/build-native-js-runtimes.sh
AIBE_RUNTIME_PREFIX=/tmp/aibe-runtime-root/usr/local LD_LIBRARY_PATH=/tmp/aibe-runtime-root/usr/local/lib AIBE_V8_EXECUTABLE=/tmp/aibe-runtime-root/usr/local/bin/d8 cargo test -p aibe-js-runtime-ffi-poc --locked
AIBE_RUNTIME_PREFIX=/tmp/aibe-runtime-root/usr/local ./scripts/run-test262-shells.sh
clang -Wall -Wextra -Werror -O2 experiments/security/capsicum-minimal/src/main.c -o /tmp/aibe-capsicum-minimal
/tmp/aibe-capsicum-minimal
./scripts/run-security-pocs.sh
./scripts/run-network-poc.sh
./scripts/run-html-pipeline-poc.sh
```

The Test262 runners execute a pinned, deliberately limited synchronous subset across Boa, V8 and SpiderMonkey; they are not full Test262 conformance. The V8/SpiderMonkey scripts require FreeBSD runtime packages extracted under `/tmp` as documented in `docs/phase-0/js-runtime-comparison.md`. Security experiments are narrow primitives, not a complete renderer sandbox. The network PoC validates DNS/IP/TLS and each HTTPS redirect, and can read a bounded response body (Content-Length, chunked, or connection-close framing; 8 MiB hard cap). The HTML parser PoC uses html5ever 0.40.1 and a project-owned 1 MiB/65,536-node bounded tree; `scripts/run-html-pipeline-poc.sh` hands a live fetched body between separate processes. This is not final Network Service IPC or a complete Fetch implementation; cookies, compression, HTTP/2 and HTML encoding sniffing are not implemented. The Snapshot-to-V8 path is a generic native Node-wrapper graph ingestion PoC, not a standards-compliant DOM. The headless PoC exercises text shaping, glyph rasterization and common image decoding, not CSS layout or a full rendering engine. Supply-chain CI generates and validates eleven per-crate CycloneDX 1.5 SBOMs as CI artifacts. The tracked `sbom/` directory still contains an older five-file snapshot and must not be treated as current; native system packages and assets are outside Cargo SBOM scope.

## Decisions and licensing

V8 15.6.75.19 is the accepted primary JavaScript Runtime; SpiderMonkey remains a fallback candidate. See `docs/adr/0001-javascript-runtime-v8.md`. The project-wide license remains undecided; public repository visibility is not a grant of reuse rights. The current Cargo lockfile resolves 225 packages: eleven project-owned workspace crates and 214 third-party packages, all 214 of which declare license metadata. Metadata presence is not legal approval. Supply-chain CI runs cargo-audit and generates/validates the eleven-crate SBOM set; the project-wide license remains undecided.
