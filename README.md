# Autonomous AI Browser Engine

A FreeBSD-native, AI-first browser engine research and development project.

## Project status

This repository is in **Phase 0: technical feasibility research**. It is not a usable browser and does not claim Web compatibility. The project owner has selected V8 15.6.75.19 as the primary JavaScript Runtime; Phase 0 still has open security and integration exit conditions.

The current V8 FFI PoC models host-issued origin groups separately from JavaScript Realms. Same-origin Realms can use a transferred native DOM wrapper; cross-origin access is rejected at the native wrapper boundary. This is a narrow in-process host-object policy experiment, not a complete web-origin implementation, site-isolation architecture, or Renderer Sandbox. See `docs/phase-0/README.md` for evidence and limitations.

## Principles

- FreeBSD is the first native target.
- Rust is the primary implementation language; C/C++ dependencies must stay behind narrow, reviewed boundaries.
- The engine owns its architecture, process model, control protocol, permission model, and browser-facing APIs.
- No Playwright runtime, Chromium, or Servo as the final execution core.
- Headless-first; headed mode is retained as a later backend.
- AI has broad browser-level control, while host access and dangerous operations remain policy-gated.
- Security threat modeling and negative tests start in Phase 0 and continue throughout development.
- No compatibility claims without pinned WPT/Test262 versions and published results.

## Repository layout

- `experiments/js-runtime/boa-host/`: Boa embedding, DOM/JS lifecycle, Test262 subset and script runner PoCs.
- `experiments/js-runtime/native-embed/`: FreeBSD V8 and SpiderMonkey C++ embedding PoCs.
- `experiments/js-runtime/ffi-poc/`: Rust-owned RAII wrappers over native V8 and SpiderMonkey C ABI.
- `experiments/origin-policy/`: host-owned URL-origin canonicalization and opaque origin-group registry PoC.
- `experiments/browser-runtime/`: orchestration PoC mapping canonical origin groups to JS Runtime origin handles and Realm lifetimes.
- `experiments/js-runtime/benchmarks/`: common cross-runtime JavaScript microbenchmark.
- `experiments/security/`: FreeBSD Capsicum, per-origin storage descriptor, crash-recovery, peer-credential and Jail network-isolation PoCs.
- `experiments/headless/raster-poc/`: OpenType shaping, CPU glyph rasterization and PNG/JPEG/GIF/WebP decode PoC.
- `experiments/network-policy/`: outbound IP policy, pinned TCP/TLS, SNI/certificate verification and per-redirect revalidation PoC.
- `docs/adr/`: accepted architecture decision records.
- `experiments/control-protocol/uds-poc/`: versioned JSON-over-UDS contract and task-state tests.
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
```

The Test262 runners execute a pinned, deliberately limited synchronous subset across Boa, V8 and SpiderMonkey; they are not full Test262 conformance. The V8/SpiderMonkey scripts require FreeBSD runtime packages extracted under `/tmp` as documented in `docs/phase-0/js-runtime-comparison.md`. Security experiments are narrow primitives, not a complete renderer sandbox. The network PoC validates DNS/IP/TLS and each HTTPS redirect, and can read a bounded response body (Content-Length, chunked, or connection-close framing; 8 MiB hard cap). It is not a complete Fetch implementation and does not yet provide browser cookies, compression, HTTP/2, or a body-to-HTML pipeline. The headless PoC exercises text shaping, glyph rasterization and common image decoding, not CSS layout or a full rendering engine. Supply-chain CI generates and validates eight per-crate CycloneDX 1.5 SBOMs as CI artifacts. The tracked `sbom/` directory still contains an older five-file snapshot and must not be treated as current; native system packages and assets are outside Cargo SBOM scope.

## Decisions and licensing

V8 15.6.75.19 is the accepted primary JavaScript Runtime; SpiderMonkey remains a fallback candidate. See `docs/adr/0001-javascript-runtime-v8.md`. The project-wide license remains undecided; public repository visibility is not a grant of reuse rights. The current Cargo lockfile resolves 211 packages: eight project-owned workspace crates and 203 third-party packages, all 203 of which declare license metadata. Metadata presence is not legal approval. Supply-chain CI runs cargo-audit and generates/validates the eight-crate SBOM set; the project-wide license remains undecided.
