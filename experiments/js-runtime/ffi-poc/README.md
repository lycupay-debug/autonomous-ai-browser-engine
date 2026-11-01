# Rust-to-native JavaScript Runtime FFI PoC

**Selected runtime: V8 15.6.75.19.** SpiderMonkey 153.1.0_2 remains available only as a tested comparison/fallback. The owner decision is recorded in `docs/adr/0001-javascript-runtime-v8.md`.

On FreeBSD, set `AIBE_RUNTIME_PREFIX` to the extracted package prefix and `LD_LIBRARY_PATH` to its `lib` directory:

```sh
AIBE_RUNTIME_PREFIX=/tmp/aibe-runtime-root/usr/local \
LD_LIBRARY_PATH=/tmp/aibe-runtime-root/usr/local/lib \
AIBE_V8_EXECUTABLE=/tmp/aibe-runtime-root/usr/local/bin/d8 \
cargo test -p aibe-js-runtime-ffi-poc --locked
```

The V8 adapter keeps V8 types inside C++, exposes a narrow C ABI, and lets Rust own the runtime handle through RAII. The V8 platform is initialized once per process; each isolate/context belongs to its creating thread. Rust's wrapper is deliberately `!Send + !Sync`, and C ABI calls reject wrong-thread access. A first project-owned `JsRuntime` trait now hides the concrete V8 type and exposes stringified evaluation, explicit microtask checkpoints, and GC for the Phase 0 PoC. It is not the final contract: typed values, explicit realm/context handles, callbacks, host objects and scheduler integration remain open. The returned string ABI includes an explicit byte length, preserving embedded NUL bytes.

Current tests cover isolate creation/destruction, simultaneous isolated globals, syntax/runtime errors, explicit Promise microtask checkpoints, a native `Node` wrapper with append/childCount/firstChild, moving nodes between parents, rejecting ancestor cycles, self-referential JS objects, native parent/child ownership, and V8 GC weak finalization. A weak per-runtime wrapper cache preserves repeated `firstChild()` identity (`===`); after an unreachable JS wrapper is collected, the native parent retains the node and a subsequent `firstChild()` call creates a new stable wrapper. SpiderMonkey's narrow C ABI test remains for fallback comparison.

The Phase 0 adapter now also exposes host-issued, Runtime-scoped `JsOriginId` handles. `create_origin()` issues an opaque origin-group handle; `create_realm_for_origin()` explicitly places a Realm in that group. A Realm created with the default `create_realm()` receives a fresh origin group. The native Node wrapper records both its Realm and origin group: wrapper identity remains partitioned per Realm, while native method access is allowed across Realms only when both the current and incumbent V8 Contexts match the host-issued origin group. A focused native probe transfers a Node wrapper into a same-origin Realm, successfully uses it in a DOM operation, and rejects the same operation from a cross-origin Realm. Host callbacks remain stricter and are bound to one exact Realm even when another Realm shares its origin group.

This is not a complete web-origin implementation. The IDs are host-side grouping handles, not URL parsing or cryptographic principals; scheme/host/port calculation, opaque origins, sandboxed iframes, document.domain, agent clusters, structured cloning, postMessage, and site/process isolation are not implemented. It also does not provide production DOM, full Web IDL, Worker support, Wasm, memory/CPU quotas, interrupt/deadline handling, production exception objects, or complete WHATWG task scheduling. V8 microtask checkpoints are not the HTML Event Loop. V8's internal sandbox is not the operating-system renderer sandbox.
