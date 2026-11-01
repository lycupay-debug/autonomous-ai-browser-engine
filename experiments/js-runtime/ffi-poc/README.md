# Rust-to-native JavaScript Runtime FFI PoC

**Selected runtime: V8 15.6.75.19.** SpiderMonkey 153.1.0_2 remains available only as a tested comparison/fallback. The owner decision is recorded in `docs/adr/0001-javascript-runtime-v8.md`.

On FreeBSD, set `AIBE_RUNTIME_PREFIX` to the extracted package prefix and `LD_LIBRARY_PATH` to its `lib` directory:

```sh
AIBE_RUNTIME_PREFIX=/tmp/aibe-runtime-root/usr/local \
LD_LIBRARY_PATH=/tmp/aibe-runtime-root/usr/local/lib \
AIBE_V8_EXECUTABLE=/tmp/aibe-runtime-root/usr/local/bin/d8 \
cargo test -p aibe-js-runtime-ffi-poc --locked
```

The V8 adapter keeps V8 types inside C++, exposes a narrow C ABI, and lets Rust own the runtime handle through RAII. The V8 platform is initialized once per process; each isolate/context belongs to its creating thread. Rust's wrapper is deliberately `!Send + !Sync`, and C ABI calls reject wrong-thread access. The returned string ABI includes an explicit byte length, preserving embedded NUL bytes.

Current tests cover isolate creation/destruction, simultaneous isolated globals, syntax/runtime errors, explicit Promise microtask checkpoints, a native `Node` wrapper with append/childCount, moving nodes between parents, rejecting ancestor cycles, self-referential JS objects, native parent/child ownership, and V8 GC weak finalization. SpiderMonkey's narrow C ABI test remains for fallback comparison.

This is not the final Browser Engine Adapter. It does not yet provide DOM Wrapper Cache identity, event listeners retaining JS closures, full realm/agent mapping, host callbacks, Worker support, Wasm, memory/CPU quotas, interrupt/deadline handling, production exception objects, or complete WHATWG task scheduling. V8 microtask checkpoints are not the HTML Event Loop. V8's internal sandbox is not the operating-system renderer sandbox.
