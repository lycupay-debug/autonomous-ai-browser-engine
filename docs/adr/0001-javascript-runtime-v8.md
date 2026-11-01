# ADR-0001: Select V8 as the primary JavaScript Runtime

- **Status:** Accepted by project owner
- **Decision date:** 2026-11-01
- **Scope:** Browser Engine Adapter and Phase 1 runtime skeleton
- **Selected candidate:** V8 15.6.75.19 (FreeBSD package `lang/v8`)
- **Fallback candidate:** SpiderMonkey 153.1.0_2, retained as a tested alternative
- **Not selected:** Boa 0.22.0 as the primary browser runtime

## Context

The engine requires an embeddable ECMAScript implementation that can run natively on FreeBSD, integrate behind a stable Rust-owned boundary, support browser-host integration, and sustain future DOM, Worker, WebAssembly, and real-site workloads. The decision cannot be made from language preference or a single microbenchmark.

The project owner explicitly selected V8 on 2026-11-01 after reviewing the Phase 0 evidence. This ADR records that decision; it does not claim that V8 is a complete browser engine or that all production integration risks have been resolved.

## Evidence available at decision time

- V8 15.6.75.19 was fetched from the FreeBSD 15 `latest` package repository and extracted under `/tmp`; it was not installed into the host's system package set.
- The FreeBSD C++ embedder initializes ICU/startup data, creates an isolate and context, evaluates `1 + 2`, and returns `3`.
- The Rust FFI PoC creates/destroys V8 through RAII and evaluates JavaScript through a narrow C ABI.
- V8 and SpiderMonkey each passed the same pinned Test262 subset: 284 passed, 0 failed, 10 skipped. This is not full Test262 conformance.
- In three small JIT-enabled microbenchmarks, V8's medians were 3 ms, 5 ms, and 5 ms; SpiderMonkey's were 3 ms, 9 ms, and 9 ms; Boa's were 173 ms, 322 ms, and 387 ms. These are directional microbenchmarks, not browser performance measurements.
- The tested V8 package's main shared library was approximately 42 MiB; package and memory figures are recorded in `docs/phase-0/js-runtime-comparison.md`.
- Follow-up FreeBSD tests now cover multiple isolate lifetimes, per-isolate global-state separation, syntax/runtime errors, embedded NUL in returned strings, explicit Promise microtask checkpoints, and a minimal native Node wrapper with child reparenting, ancestor-cycle rejection, and GC finalization.

## Decision

1. V8 is the primary JavaScript Runtime for the next implementation stage.
2. SpiderMonkey remains a comparison/fallback PoC. It is not part of the default runtime path unless a later ADR supersedes this decision.
3. Boa remains useful as a Rust-native research reference but is not the primary browser runtime.
4. The browser architecture remains Rust-first. V8 C++ APIs are confined behind a narrow, versioned C ABI; V8 types must not leak into Rust domain interfaces or public Browser Control Protocol types.
5. V8 process-wide platform initialization is shared and initialized once. Each runtime instance owns an isolate/context on its creating thread. The current PoC enforces thread affinity at the Rust type boundary and rejects wrong-thread C ABI calls.
6. V8 microtasks are explicitly pumped by the host. This is not the WHATWG HTML event loop; the browser scheduler must implement task sources, task queues, microtask checkpoints, rendering opportunities, and lifecycle integration separately.
7. The tested V8 version and FreeBSD package ABI/build configuration are pinned. The embedder must preserve the tested pointer-compression and sandbox build defines (`V8_COMPRESS_POINTERS`, `V8_31BIT_SMIS_ON_64BIT_ARCH`, `V8_ENABLE_SANDBOX`) and audit them on every package upgrade.

## Security constraints

- V8's internal sandbox is not the operating-system renderer sandbox. It does not replace Capsicum, Jail, FD minimization, network brokering, origin isolation, or privilege separation.
- Do not disable V8 sandboxing or Chromium-independent V8 security features to make embedding easier.
- Do not expose Node.js-style host APIs, unrestricted filesystem access, raw sockets, process execution, or privileged Browser Broker functions to page JavaScript.
- Page World and Agent Inspection World must remain separate. Agent-only capabilities must not be reachable from page script objects.
- JIT executable-memory behavior and V8 sandbox assumptions require a separate FreeBSD security review before untrusted internet content is enabled.

## Consequences and required follow-up

- The project accepts a C++ dependency and FFI maintenance boundary in exchange for the observed performance and ecosystem maturity.
- The exact V8 package version, shared libraries, startup data, build defines, and package checksums must be recorded in the release manifest.
- A minimal project-owned `JsRuntime` Rust trait now hides the concrete V8 type and exposes stringified evaluation, microtask checkpoints, and GC. It is only the Phase 0 boundary proof; extend it with typed values, explicit realm/context handles, callbacks, host objects, structured errors, and scheduler integration before treating it as the stable production contract.
- Expand DOM Wrapper Cache and native object lifetime tests: wrapper identity, listeners retaining JS closures, detach/navigation teardown, Frame/Worker shutdown, isolate disposal, GC pressure, and repeated context creation/destruction.
- Define the browser's agent/isolate/context mapping before production process topology is frozen. The current one-isolate/one-context PoC is not the final realm model.
- Add memory/CPU quotas, interrupt/deadline handling, stack limits, termination behavior, and crash recovery tests.
- Keep SpiderMonkey's comparison scripts available so a V8 upgrade regression or FreeBSD package breakage can be evaluated against a known alternative.

## Reversal conditions

Reconsider this decision only if reproducible FreeBSD evidence shows one or more of the following:

- V8 cannot be built, upgraded, or maintained on the supported FreeBSD release without unacceptable patch burden.
- Required security controls cannot be enforced around the V8 isolate/JIT model.
- The Rust adapter cannot provide safe and maintainable DOM/GC/callback integration.
- Realistic browser workloads, memory ceilings, or standards coverage materially favor SpiderMonkey or another candidate.

Any reversal requires a new ADR with comparative tests. Do not silently switch runtimes.
