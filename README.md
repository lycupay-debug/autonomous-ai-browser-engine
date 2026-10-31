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

- `experiments/js-runtime/boa-host/`: isolated Boa embedding feasibility PoC.
- `experiments/security/capsicum-minimal/`: minimal FreeBSD Capsicum negative-test PoC.
- `docs/phase-0/`: Phase 0 scope, evidence and exit criteria.
- `.github/workflows/`: hosted CI checks; this is not a FreeBSD-native test result.

## Build and test

On a FreeBSD 15.1 amd64 host with Rust/Cargo installed:

```sh
cargo test --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
clang -Wall -Wextra -Werror -O2 experiments/security/capsicum-minimal/src/main.c -o /tmp/aibe-capsicum-minimal
/tmp/aibe-capsicum-minimal
```

The Capsicum test demonstrates only a narrow capability-mode filesystem restriction. It is not a browser sandbox or proof of renderer isolation.

## Decisions and licensing

The final JavaScript runtime, renderer backend, project license, and public API are not selected yet. See `docs/phase-0/` before treating any candidate as an adopted decision.
