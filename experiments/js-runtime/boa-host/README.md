# Boa host embedding PoC

Pinned candidate: `boa_engine = 0.22.0`.

Scope: verify that a Rust host can construct a Boa context, evaluate ECMAScript, surface errors, and create independent contexts on the target FreeBSD host.

Not in scope: DOM, Web IDL, HTML event loop, browser APIs, network access, renderer sandboxing, full Test262 conformance, or a final runtime selection.

Run from repository root:

```sh
cargo test -p aibe-boa-host-poc --locked
```

Record compiler version, target triple, dependency lockfile, build output, test output, elapsed time, binary size, and memory observations in the Phase 0 Notion page.
