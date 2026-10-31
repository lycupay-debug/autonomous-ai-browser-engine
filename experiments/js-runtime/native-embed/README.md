# FreeBSD native JS Runtime embedding PoCs

The V8 and SpiderMonkey candidates were fetched as FreeBSD 15 amd64 packages and extracted under `/tmp/aibe-runtime-root`; no packages were installed into the host system.

## V8

The package build enables pointer compression and the sandbox. The embedder must compile with matching V8 configuration defines or V8 aborts during isolate initialization:

- `V8_COMPRESS_POINTERS`
- `V8_31BIT_SMIS_ON_64BIT_ARCH`
- `V8_ENABLE_SANDBOX`

The link set includes V8, libplatform, libbase, PartitionAlloc and package-provided support libraries. The tested minimal program creates an isolate/context and evaluates `1 + 2`.

## SpiderMonkey

The package exposes `libmozjs-153`. The minimal JSAPI program must call `JS_Init`, `JS_NewContext`, `JS::InitSelfHostedCode`, create a global object, enter its realm, initialize standard classes, and evaluate source text. The `JS::InitSelfHostedCode` call is required before creating the global object in this embedding path; omitting it caused a reproducible crash in the initial experiment.

The public headers require `XP_UNIX` and `XP_FREEBSD` compile definitions for this FreeBSD package build. The minimal program evaluates `1 + 2` and returns `3`.

## Reproduction

Set `AIBE_RUNTIME_PREFIX` to the extracted package prefix (default `/tmp/aibe-runtime-root/usr/local`) and run:

```sh
./scripts/build-native-js-runtimes.sh
```

These are C++ embedding feasibility probes only. They do not yet provide Rust FFI wrappers, DOM bindings, event-loop integration, sandboxing, or a final engine selection.

The common Test262 subset runner also passes 284 selected synchronous tests on both extracted shells. See `docs/phase-0/js-runtime-comparison.md` for the exact revision, exclusions, benchmark methodology, package metadata and limitations.
