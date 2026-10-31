# JavaScript Runtime Candidate Comparison — FreeBSD Phase 0

Snapshot date: 2026-11-01. Platform: FreeBSD 15.1-RELEASE-p6 amd64, Clang 19.1.7, rustc/Cargo 1.98.1.

## Candidate availability and embedding

| Candidate | Version | FreeBSD package | Minimal embedding | Current status |
|---|---:|---|---|---|
| Boa | 0.22.0 | Rust crate from crates.io | Rust `Context` / `Source` | Native Rust embedding and lifecycle PoCs pass |
| V8 | 15.6.75.19 | `lang/v8` from FreeBSD `latest` | C++ `Isolate` / `Context` / `Script` | C++ embed compiles and evaluates `1 + 2` |
| SpiderMonkey | 153.1.0_2 | `lang/spidermonkey153` from FreeBSD `latest` | C++ JSAPI `JSContext` / Realm / `JS::Evaluate` | C++ embed compiles and evaluates `1 + 2` |

V8 and SpiderMonkey packages were fetched using a temporary repository configuration under `/tmp/aibe-pkg-repos`, then extracted to `/tmp/aibe-runtime-root`. They were **not installed** into the host, and no system package repository configuration was changed. The configured quarterly repository did not expose these candidate packages; the temporary `latest` repository did.

Package metadata:

| Candidate | Package archive | Package flatsize | Main shared library | Shell binary | Declared package licenses |
|---|---:|---:|---:|---:|---|
| V8 15.6.75.19 | 22 MiB | 109,067,058 bytes | `libv8.so` ~42 MiB | `d8` ~940 KiB | Apache-2.0, BSD-3-Clause, MIT, Unicode-3.0 |
| SpiderMonkey 153.1.0_2 | 34 MiB | 125,193,516 bytes | `libmozjs-153.so` ~35 MiB | `js153` ~41 MiB | MPL-2.0 |

Boa's Rust crates declare `Unlicense OR MIT`. Package size is not a complete memory or process-size comparison; these are package metadata and file sizes only.

### Embedding integration observations

**Rust FFI integration**
- A FreeBSD-only Rust crate now wraps V8 and SpiderMonkey through a narrow C ABI (`create`, `eval`, `destroy`).
- One FreeBSD integration test creates each runtime from Rust, evaluates `1 + 2`, verifies the string result `3`, and drops each runtime through Rust RAII. The test passes.
- The FFI crate uses the `cc` build dependency and is excluded from native compilation on non-FreeBSD CI runners. The native FFI test must therefore remain part of the server-side FreeBSD verification gate.
- This proves a basic Rust ownership/FFI path only. It does not prove DOM object rooting, callback lifetime safety, Promise integration, Worker support, multi-context concurrency or production-grade error propagation.

**V8**
- The minimal C++ embed creates a V8 platform, initializes ICU and external startup data, creates an isolate and context, compiles and executes `1 + 2`, and returns `3`.
- The embedder must use matching build configuration defines: `V8_COMPRESS_POINTERS`, `V8_31BIT_SMIS_ON_64BIT_ARCH`, and `V8_ENABLE_SANDBOX`. Omitting the pointer-compression define caused V8 to abort during isolate initialization.
- Linking requires V8, libplatform, libbase, PartitionAlloc and package support libraries. The package's embedder build configuration is therefore a maintenance contract; it must be pinned and checked whenever the FreeBSD package is upgraded.

**SpiderMonkey**
- The minimal C++ JSAPI embed creates a context, initializes self-hosted code, creates a global object, enters its realm, initializes standard classes, evaluates `1 + 2`, and returns `3`.
- `JS::InitSelfHostedCode(cx)` is required before global-object creation in this embedding path. Omitting it caused a reproducible crash in `JS_NewGlobalObject`.
- The FreeBSD public-header build required `XP_UNIX` and `XP_FREEBSD` defines. Clang 19 emitted three `-Winvalid offsetof` warnings from upstream headers; compilation and execution still succeeded.

**Boa**
- Direct Rust embedding avoids a C++ FFI boundary and was the simplest candidate to integrate with the current Rust workspace.
- Seven native tests pass, including independent Contexts, Promise Job pumping, native wrapper registration and GC of cross-linked JS/native cycles.
- Boa's current `Context::run_jobs()` is an engine job pump, not a WHATWG HTML Event Loop.

## Selected Test262 subset

Pinned Test262 revision: `91c70759363c46557b694792bf34a2551a7af5ba`.

Selected synchronous tests from addition, bitwise-or, bitwise-and, multiplication, relational operators and `let` statements:

| Candidate | Passed | Failed | Skipped |
|---|---:|---:|---:|
| Boa 0.22.0 | 284 | 0 | 10 |
| V8 15.6.75.19 | 284 | 0 | 10 |
| SpiderMonkey 153.1.0_2 | 284 | 0 | 10 |

The 10 skipped tests require unsupported external harness includes, modules, async/raw execution or `onlyStrict` handling. The project runner uses the upstream `sta.js` and `assert.js` files and handles the selected negative tests, but it is not the official complete Test262 harness. This table is not an overall ECMAScript conformance percentage and says nothing about DOM, HTML, CSS, Fetch or browser APIs.

Reproduce with:

```sh
./scripts/run-test262-subset.sh
AIBE_RUNTIME_PREFIX=/tmp/aibe-runtime-root/usr/local ./scripts/run-test262-shells.sh
```

## Common JavaScript microbenchmark

Same JavaScript source was run on each engine. Each case had 5 warm-up calls and 10 measured calls; `Date.now()` provided millisecond timing. Three independent process runs were performed. V8 and SpiderMonkey used their default JIT-enabled package builds; Boa uses its current interpreter implementation.

| Workload | Boa median | V8 median | SpiderMonkey median |
|---|---:|---:|---:|
| Integer loop, 500,000 iterations | 173 ms | 3 ms | 3 ms |
| Allocate 50,000 plain objects | 322 ms | 5 ms | 9 ms |
| Concatenate 25,000 numeric strings | 387 ms | 5 ms | 9 ms |

All three candidates returned identical checksums. In a separate timed run of the same script, maximum RSS was approximately 32 MiB for Boa, 73 MiB for V8, and 85 MiB for SpiderMonkey.

**Interpretation limits**
- This is a small microbenchmark, not a browser benchmark. It excludes DOM bindings, layout, network, Web APIs, page workloads and cross-engine FFI overhead.
- V8 and SpiderMonkey run JIT-enabled optimized packages; Boa does not have an equivalent JIT in this candidate build. The result is useful for identifying a performance risk, not for declaring final engine superiority.
- Millisecond timing has limited resolution; the integer results near 3 ms are especially coarse.
- The object allocation case is not a controlled GC benchmark. Heap retention and collection behavior need dedicated stress tests.

## Provisional assessment — not a final decision

1. V8 is currently the raw-performance leader in this narrow benchmark and has a working FreeBSD C++ embedding PoC. Its compile-time configuration and native dependency surface require careful pinning and maintenance.
2. SpiderMonkey also has a working FreeBSD C++ embedding PoC and passes the same Test262 subset. Its JSAPI rooting and realm model are mature, but it adds a C++ FFI boundary and the tested package has a larger shell/runtime footprint.
3. Boa offers the cleanest Rust integration and lowest measured memory footprint in this test, but its performance gap is large and its browser/API maturity remains unproven.

**No runtime has been selected.** The next comparison must add Rust FFI adapters, identical GC/object-retention stress tests, Worker/Promise integration, full dependency and license review, security/JIT hardening, memory ceilings, and broader Test262/WPT coverage. The final choice must optimize total browser capability, maintainability and FreeBSD security—not just microbenchmark speed.
