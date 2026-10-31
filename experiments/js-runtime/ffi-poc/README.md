# Rust-to-native JS Runtime FFI PoC

This crate tests whether the Rust-owned browser runtime can own and call native V8 and SpiderMonkey instances through narrow C ABI wrappers.

On FreeBSD, set `AIBE_RUNTIME_PREFIX` to the extracted package prefix and `LD_LIBRARY_PATH` to its `lib` directory:

```sh
AIBE_RUNTIME_PREFIX=/tmp/aibe-runtime-root/usr/local \
LD_LIBRARY_PATH=/tmp/aibe-runtime-root/usr/local/lib \
AIBE_V8_EXECUTABLE=/tmp/aibe-runtime-root/usr/local/bin/d8 \
cargo test -p aibe-js-runtime-ffi-poc --locked
```

The wrappers expose only create/evaluate/destroy and do not leak V8 or SpiderMonkey types into Rust. The Rust side owns each runtime with RAII and invokes the C ABI. The current PoC is FreeBSD-only; non-FreeBSD builds skip native compilation so hosted CI can still validate the rest of the workspace.

This does not yet implement DOM bindings, host callbacks, Promise job queues, GC roots for DOM wrappers, Worker isolation, JIT policy, or concurrent multi-context operation. The current C ABI is a feasibility boundary, not the final Browser Engine Adapter contract.
