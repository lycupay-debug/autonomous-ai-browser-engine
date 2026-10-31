# Autonomous AI Browser Engine

A FreeBSD-native, AI-first browser engine research and development project.

## Project status

This repository is in **Phase 0: technical feasibility research**. It is not a usable browser and does not claim Web compatibility. All architecture choices remain provisional until supported by reproducible experiments.

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
- `experiments/js-runtime/benchmarks/`: common cross-runtime JavaScript microbenchmark.
- `experiments/security/`: FreeBSD Capsicum Broker/Renderer and Jail network-isolation negative-test PoCs.
- `experiments/headless/raster-poc/`: CPU glyph rasterization and PNG output PoC.
- `experiments/network-policy/`: outbound IP classification and DNS answer-set policy PoC.
- `experiments/control-protocol/uds-poc/`: versioned JSON-over-UDS contract and task-state tests.
- `docs/phase-0/`: Phase 0 scope, capability matrix, evidence, dependency audit and exit criteria.
- `.github/workflows/`: hosted CI checks; this is not a FreeBSD-native test result.

## Build and test

On a FreeBSD 15.1 amd64 host with Rust/Cargo installed:

```sh
cargo test --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
./scripts/run-test262-subset.sh
AIBE_RUNTIME_PREFIX=/tmp/aibe-runtime-root/usr/local ./scripts/build-native-js-runtimes.sh
AIBE_RUNTIME_PREFIX=/tmp/aibe-runtime-root/usr/local LD_LIBRARY_PATH=/tmp/aibe-runtime-root/usr/local/lib AIBE_V8_EXECUTABLE=/tmp/aibe-runtime-root/usr/local/bin/d8 cargo test -p aibe-js-runtime-ffi-poc --locked
AIBE_RUNTIME_PREFIX=/tmp/aibe-runtime-root/usr/local ./scripts/run-test262-shells.sh
clang -Wall -Wextra -Werror -O2 experiments/security/capsicum-minimal/src/main.c -o /tmp/aibe-capsicum-minimal
/tmp/aibe-capsicum-minimal
./scripts/run-security-pocs.sh
```

The Test262 runners execute a pinned, deliberately limited synchronous subset across Boa, V8 and SpiderMonkey; they are not full Test262 conformance. The V8/SpiderMonkey scripts require FreeBSD runtime packages extracted under `/tmp` as documented in `docs/phase-0/js-runtime-comparison.md`. The Capsicum test demonstrates only descriptor-rights and capability-mode restrictions. It is not a browser sandbox or proof of renderer isolation. The headless raster PoC requires an external font file and does not implement shaping or layout.

## Decisions and licensing

The final JavaScript runtime, renderer backend, project license, and public API are not selected yet. See `docs/phase-0/` before treating any candidate as an adopted decision.
