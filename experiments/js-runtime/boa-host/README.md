# Boa host embedding PoC

Pinned candidate: `boa_engine = 0.22.0`.

Scope: verify that a Rust host can construct a Boa context, evaluate ECMAScript, surface errors, and create independent contexts on the target FreeBSD host.

Not in scope: DOM, Web IDL, HTML event loop, browser APIs, network access, renderer sandboxing, full Test262 conformance, or a final runtime selection.

Run from repository root:

```sh
cargo test -p aibe-boa-host-poc --locked
```

Record compiler version, target triple, dependency lockfile, build output, test output, elapsed time, binary size, and memory observations in the Phase 0 Notion page.

## Additional lifecycle and scheduling experiments

The PoC now also registers a minimal `DomNode` native class backed by Boa GC-managed Rust state. Tests cover parent/child node references, JS self-references, a cross-linked native node cycle, and explicit finalization after the objects become unreachable. A Promise test verifies that the host must explicitly drain Boa's job queue with `Context::run_jobs()` in this embedding setup.

These tests demonstrate the mechanics of this particular Boa integration only. They do not establish a standards-compliant DOM, a WHATWG event loop, browser navigation lifecycle semantics, or safe production-grade wrapper caching.
