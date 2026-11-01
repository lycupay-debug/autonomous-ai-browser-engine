# Phase 0 — Technical feasibility and risk reduction

## Objective

Resolve the architecture risks that could invalidate the proposed FreeBSD-native AI-first browser engine before committing to a full browser implementation.

## Workstreams

1. FreeBSD toolchain and reproducible builds.
2. JavaScript runtime candidates and embedding.
3. DOM ↔ JS lifetime model.
4. Broker / renderer security prototype.
5. Headless text, image and CPU raster pipeline.
6. Network / Fetch and outbound policy.
7. Native AI control protocol V1.
8. Initial Web capability matrix.
9. Dependency, licensing, CI and SBOM governance.

## Evidence rule

For each experiment, record the exact source revision, OS release, compiler/tool versions, command, exit status, stdout/stderr, test count, and known limitations. A build is not a compatibility result; a unit test is not a security proof; an architecture proposal is not an implemented feature.

## Exit gate

See the parent architecture blueprint's Phase 0 exit criteria. Do not proceed to full runtime implementation while critical DOM/JS lifetime or renderer isolation risks remain unexplained.

## 2026-11-01 execution evidence update

- FreeBSD native Boa 0.22.0 host embedding: 7 tests passed.
- Minimal native DOM-like wrapper: JS class construction and parent/child graph test passed.
- Lifecycle: unrooted JS self-reference plus cross-linked native node graph was collected; wrapper and native state finalizers were observed.
- Scheduling: Promise callbacks remained queued until the host called `Context::run_jobs()`; this is an engine job pump, not a WHATWG HTML event loop.
- Capsicum: path-based `/etc/passwd` open denied after `cap_enter()`, errno 94.
- Remaining: selected Test262 suite, actual renderer process isolation, FD rights, jail/network boundaries, and broader lifecycle stress tests.

## 2026-11-01 expanded feasibility evidence

- Test262 pinned revision `91c70759363c46557b694792bf34a2551a7af5ba`: 284 selected synchronous tests passed, 0 failed, 10 skipped because the current runner does not support external includes, modules, async tests, raw tests, or strict-only tests. This is a deliberately narrow subset, not an overall conformance percentage. Re-run with `scripts/run-test262-subset.sh`.
- Headless CPU raster: `fontdue 0.9.5` parsed Noto Sans SC variable TTF; 24 Latin/Simplified Chinese characters, 0 missing glyphs, 3,248 nonzero coverage pixels, PNG 487x80 / 7.1 KiB. The font is temporary and not committed. Shaping, bidi, fallback and layout remain unimplemented.
- Network baseline: `drill example.com` returned public IPv4 answers; `fetch https://example.com/` retrieved 577 bytes; `openssl s_client` negotiated TLS 1.3 and verified the certificate. These are basic connectivity checks, not a browser Network Service.
- Network policy PoC: 8 tests pass for public IP acceptance, private/loopback/link-local/CGNAT/documentation/reserved/multicast/special IPv6 denial, IPv4-mapped private IPv6 denial, mixed DNS answer rejection, empty answer rejection and deduplication. Real socket pinning, TLS SNI with pinned IP, redirects and DNS rebinding are still unverified.
- UDS control protocol PoC: 5 tests pass for protocol version, request ID, health, 0600 socket permissions and task state transitions. Peer UID/GID authentication, capability authorization, request bounds, real task execution and cancellation tokens remain unverified.
- Capsicum PoC now verifies an inherited descriptor remains readable, write is denied with `ENOTCAPABLE`, and path open is denied with `ECAPMODE`.
- Jail PoC: a temporary jail root showed that host `/etc/passwd` and a host-only `/tmp` marker were not visible from the jail. The jail ran with `ip4=disable` and `ip6=disable`; a separate network-socket negative test is still required. No system jail configuration was changed; the temporary jail root was removed after the test.


## JS Runtime comparison update — 2026-11-01

- V8 15.6.75.19 and SpiderMonkey 153.1.0_2 were fetched from the FreeBSD 15 latest package repository, extracted into `/tmp`, and not installed globally.
- Both C++ embedding PoCs compile and execute `1 + 2` successfully. V8 requires matching pointer-compression/sandbox build defines; SpiderMonkey requires self-hosted-code initialization before global-object creation.
- The same 284 selected Test262 scripts pass on Boa, V8 and SpiderMonkey; 10 are skipped by the limited harness.
- A common JS microbenchmark shows V8/SpiderMonkey substantially faster than Boa in these three narrow workloads. This is preliminary, JIT-enabled versus non-JIT, and not a browser benchmark. See `docs/phase-0/js-runtime-comparison.md`.
- No JS Runtime has been selected. The next decision requires Rust FFI, DOM/GC integration, security and longer workload evidence.


## Rust FFI candidate integration update — 2026-11-01

- Added a FreeBSD-only `aibe-js-runtime-ffi-poc` crate with C ABI wrappers for V8 and SpiderMonkey.
- Rust creates each native runtime, evaluates `1 + 2`, verifies `3`, and destroys each runtime through RAII. The single FreeBSD integration test passed.
- The C++ FFI wrappers remain intentionally minimal; DOM GC roots, host callbacks, Promise scheduling, multi-context behavior and Worker isolation are not yet tested.
- GitHub-hosted Linux CI skips native FFI compilation; FreeBSD server verification is required for this crate.

## Broker/Renderer and Jail network isolation update — 2026-11-01

- A forked renderer process now receives only a Unix socketpair IPC descriptor; all other descriptors above stderr are closed.
- Renderer IPC is limited to `CAP_READ | CAP_WRITE | CAP_FSTAT`; `cap_enter()` then denies path-based `open()` with `ECAPMODE`.
- Important correction: `socket(AF_INET, ...)` itself remains allowed in Capsicum capability mode on this host. Removing `CAP_CONNECT` from the socket's rights causes outbound `connect()` to fail with `ENOTCAPABLE`; the test passes.
- A host-local TCP listener baseline succeeds. The same probe inside a temporary Jail with `ip4=disable` and `ip6=disable` cannot create an IPv4 socket (`EPROTONOSUPPORT`, errno 43). The repeatable test is `scripts/run-jail-network-poc.sh`.
- These tests establish basic process/IPC, FD-rights, and jail network primitives. Peer UID/GID authentication, rctl, Origin isolation, renderer crash recovery, and hostile IPC payload handling remain unverified.

## UDS peer identity update — 2026-11-01

- A separate UDS peer-credential test forks a child, drops it to the existing `nobody` account, and connects it to a temporary local socket.
- The broker's `getpeereid()` result matches the child's UID/GID (65534/65534). This verifies the FreeBSD kernel peer-identity primitive; it is not yet wired into the Rust protocol broker's authorization policy.
- Reproducible aggregate security checks are available through `scripts/run-security-pocs.sh`.


## Runtime decision — 2026-11-01

- The project owner selected **V8 15.6.75.19** as the primary JavaScript Runtime. SpiderMonkey 153.1.0_2 remains a tested fallback; Boa 0.22.0 is not the primary path.
- Decision rationale, constraints, reversal conditions and integration obligations are recorded in `docs/adr/0001-javascript-runtime-v8.md`.
- V8 process-wide platform initialization now runs once. Multiple isolate/context instances have independent global state and lifetimes. The Rust wrapper is thread-affine (`!Send + !Sync`) and the C ABI rejects wrong-thread operations.
- V8 microtasks use an explicit host checkpoint; this is not the WHATWG HTML Event Loop.
- A minimal project-owned `JsRuntime` trait now hides the concrete V8 runtime and exposes stringified evaluation, microtask checkpoints, and GC; trait-object dispatch is tested. This is not the final production contract because typed values, context/realm handles, callbacks, host objects, and scheduler integration are still missing.
- A V8 native `Node` wrapper PoC now validates append/childCount/firstChild, moving a child between parents, rejecting ancestor cycles, JS self-reference, native parent/child ownership, and GC weak finalization. A weak per-runtime wrapper identity cache preserves `===` identity and allows a native child to be re-wrapped after its previous JS wrapper is collected. Cross-Realm caching, event-listener closures, navigation teardown and Worker lifetimes remain open.

## Network pinned TLS and redirect evidence — 2026-11-01

- `aibe-network-policy-poc` now resolves a hostname once, rejects the entire answer set if any address is disallowed, and connects to the exact validated `SocketAddr`.
- TLS uses the original URL hostname for SNI and certificate-name verification with the host CA bundle.
- Each HTTPS redirect triggers a fresh resolve/validate/pinned-connect/TLS cycle. HTTP downgrade and duplicate/missing Location headers are rejected.
- Live tests: `https://example.com/` returned HTTP 200; `https://httpbin.org/redirect/1` returned 302 then 200 at `https://httpbin.org/get`. The actual connected peer is recorded by the probe.
- Unit tests cover private target denial, redirect parsing, duplicate Location rejection and downgrade rejection. A third-party proxy blocked the attempted external private-redirect test before the redirect response was returned, so that specific end-to-end case is not claimed as passed.
- This remains a bounded HTTPS header probe, not a complete Fetch implementation or body-streaming Network Service.

## Headless shaping and image decode evidence — 2026-11-01

- Added Rustybuzz 0.20.1 OpenType shaping before fontdue glyph rasterization.
- Added image 0.25.10 decoding tests for PNG, JPEG, GIF and WebP.
- FreeBSD PoC shaped and rasterized the 24-codepoint mixed Latin/Simplified Chinese sample with 0 missing glyphs, 3,248 nonzero coverage pixels, producing a 485×88 PNG which was decoded successfully.
- Bidi paragraph layout, script itemization, line breaking, font fallback, CSS layout and full paint remain unimplemented.

## Renderer isolation extensions — 2026-11-01

- Per-origin storage capability test: broker passed only Origin A's directory FD; renderer read Origin A but could not traverse into Origin B or create files without delegated write/create rights.
- Crash recovery test: supervisor observed renderer SIGKILL, spawned a replacement process and completed a health IPC exchange. This is a narrow lifecycle proof, not stateful production recovery.
- Rust UDS PoC now exposes FreeBSD `getpeereid()` peer credentials, UID authorization, and `accept_authorized_peer()`; the FreeBSD test server uses the authenticated accept path and rejects a mismatched UID. It applies a 64 KiB request cap and 5-second read/write timeout, refuses to unlink pre-existing filesystem entries, and tests oversized input rejection.
- `kern.racct.enable=0` on the host. rctl enforcement is not currently testable without changing a kernel runtime setting; no sysctl or persistent system configuration was changed.
- Full Renderer Sandbox remains unproven: cross-Origin process assignment, rctl, hostile IPC under load, network broker, and production crash recovery remain open.


## Network response parser hardening — 2026-11-01

- Replaced unbounded `BufRead::read_line` with a bounded incremental line reader: 8 KiB maximum per status/header line and 64 KiB total response headers.
- Enforced a 100-header ceiling, CRLF termination, valid HTTP status range, rejection of unsupported informational responses, and rejection of control characters in header values.
- Connect timeout is 8 seconds; TCP read/write timeout is 10 seconds.
- Added four negative tests: oversized header line, truncated headers, control characters, and informational status. Network policy test count is now 17/17.
- This reduces unbounded header allocation and indefinite socket wait in the PoC. DNS resolver deadlines and a production-grade HTTP parser remain unimplemented.

## Current verification snapshot — 2026-11-01

- FreeBSD `cargo test --workspace --locked -- --test-threads=1`: 45 passed, 0 failed (Boa 7, UDS 9, Headless 3, V8 FFI 9, Network 17).
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`: passed.
- Native V8 and SpiderMonkey embed builds: both executed `1 + 2` and returned `3`.
- Test262 fixed subset: V8 284 passed / 0 failed / 10 skipped; SpiderMonkey 284 passed / 0 failed / 10 skipped; Boa uses the same pinned project harness subset as recorded above. These remain limited harness results.
- Security PoCs: Capsicum FD rights, Broker/Renderer IPC restriction, UDS peer UID/GID, per-Origin storage FD restriction, Renderer SIGKILL/restart health exchange, and Jail IPv4 network denial all passed.
- Live HTTPS: `example.com` HTTP 200; `httpbin.org/redirect/1` followed 302 to `/get` and returned HTTP 200. The actual connected peer addresses were printed by the test.
- Headless shaping: 24 input codepoints, 24 shaped glyphs, 0 missing glyphs, 3,248 nonzero coverage pixels, 485×88 PNG successfully decoded.
- Five CycloneDX 1.5 SBOM files generated for the five Cargo workspace crates. Their component counts are 146, 5, 45, 3 and 53 respectively. SBOMs describe the Cargo graph only; native V8/SpiderMonkey system packages, the temporary font asset and the FreeBSD OS package closure are not included.
- Cargo metadata: 212 packages total; 207 third-party packages all have declared license metadata. This is not legal approval. Five workspace crates remain `UNLICENSED` pending owner decision.
- `cargo-audit` installation did not complete in the host tool session; no vulnerability scan is claimed. Add/verify a CI advisory scan before Phase 0 exit.


## Authenticated Broker accept path — 2026-11-01

- Added `accept_authorized_peer(listener, expected_uid)` to the Rust UDS PoC. It accepts a socket, obtains kernel peer credentials via FreeBSD `getpeereid()`, and returns the stream only when the UID matches.
- The FreeBSD UDS test server now uses this authenticated accept helper instead of performing credential checks inline.
- Added a negative test that connects a local client while the server expects a different UID; the accept path returns `PermissionDenied`.
- UDS test count is now 9/9 on FreeBSD. The protocol/state-machine server remains test-only; it is not yet the production Browser Broker.


## Project-owned JavaScript Runtime contract — 2026-11-01

- Added a Rust `JsRuntime` trait with no `Send`/`Sync` requirement, preserving V8 isolate thread affinity.
- V8 implements `evaluate_to_string`, `microtask_checkpoint`, and `collect_garbage` without exposing V8 C++ or V8 Rust types.
- A trait-object dispatch test confirms the selected runtime is usable through the project-owned interface.
- This is a Phase 0 contract proof, not a production API freeze. Typed JS values, explicit context/realm handles, native callbacks, host objects, structured errors, and browser scheduler integration remain required.
- V8 FFI test count at this historical snapshot was 9/9; total FreeBSD workspace tests were 45/45.

## DOM wrapper identity cache update — 2026-11-01

- Added a weak per-runtime wrapper identity map keyed by native Node address. Weak V8 Global handles ensure the cache does not keep JavaScript wrapper objects alive; the weak finalizer removes entries when wrappers are collected.
- Added `Node.firstChild()`. It returns the existing JS wrapper when one is alive, preserving `===` identity across repeated reads; when the old wrapper has been collected but the native parent still owns the child node, it creates a replacement wrapper for the same native node.
- Added tests for stable wrapper identity, `null` on an empty parent, GC of an unreachable child wrapper, and re-wrapping the still-attached native child.
- FreeBSD V8 FFI tests: 10/10 passed. Full workspace tests: 47/47 passed (Boa 7, UDS 9, Headless 4, V8 FFI 10, Network 17). Formatting and Clippy with `-D warnings` passed.
- Scope remains intentionally narrow: one Runtime/Context per cache. Cross-Realm/Agent wrapper semantics, listener-held JS closures, MutationObserver, navigation/Frame/Worker teardown, GC stress and resource quotas remain open.

## DOM EventTarget lifecycle update — 2026-11-01

- Added a minimal native Node listener store backed by V8 persistent callback handles. The native Node owns listener registrations, so a child node retained by its native parent can retain a callback after the child's JavaScript wrapper is collected.
- Added `addEventListener(type, callback)`, `removeEventListener(type, callback)`, and `dispatchEvent(type)` for a deliberately limited event model. Event types are bounded to 256 UTF-8 bytes and each native Node is limited to 1,024 listeners.
- Dispatch snapshots listener IDs before invoking JavaScript. Re-entrant removal suppresses a listener that has not yet run; listeners added during dispatch wait until the next dispatch. Duplicate registrations are ignored, and callback exceptions propagate to the caller.
- Added deferred weak-finalizer cleanup: the V8 weak callback now only resets its own wrapper handle, removes the cache entry, and enqueues the native wrapper on an intrusive per-Runtime list. The wrapper and its native node are destroyed after the GC callback phase, avoiding destruction of other persistent V8 callback handles inside the weak callback.
- FreeBSD V8 FFI tests: 12/12 passed. Full workspace tests: 49/49 passed (Boa 7, UDS 9, Headless 4, V8 FFI 12, Network 17). Formatting and Clippy with `-D warnings` passed. GitHub Actions Run 41266088154 passed both Rust and supply-chain jobs, including RustSec audit and CycloneDX SBOM generation/validation.
- Important limitations: this is not a WHATWG DOM EventTarget implementation. It has no Event object, capture/bubble phases, propagation path, default actions, once/passive/signal options, or HTML event-loop integration. In this shared_ptr implementation, callback handles are strong V8 roots; a listener closure that captures its own Node wrapper can create a cross-heap retention cycle. A separate CppHeap feasibility probe below demonstrates one candidate unified-tracing approach, but it is not integrated into this DOM implementation.
- Navigation, Frame/Worker teardown, cross-Realm wrapper semantics, MutationObserver, GC/heap stress, and resource quotas remain unimplemented.

## V8 CppHeap unified tracing feasibility — 2026-11-01

- Added a separate-isolate CppHeap feasibility probe; it does not replace the existing shared_ptr DOM PoC.
- The probe attaches `v8::CppHeap` to a fresh V8 isolate and models native nodes as `v8::Object::Wrappable` objects allocated by `cppgc::MakeGarbageCollected`.
- The JS wrapper/native edge uses `v8::Object::Wrap/Unwrap`; the native parent-to-child edge uses `cppgc::Member`; the native listener-to-JS callback edge uses `v8::TracedReference` visited from `AibeCppGcNode::Trace`.
- The test creates a native parent/child relationship and a child listener closure that captures the child JS wrapper, then removes the external JS root. After `Isolate::LowMemoryNotification()` and `CppHeap::FinishSweepingForTesting()`, the probe observes at least two native Node finalizers. This is direct evidence that the V8/C++ unified heap can collect this cross-heap cycle in the isolated prototype.
- Determinism check: the focused CppHeap unreachable-cycle test passed 20/20 independent test-process runs. V8 FFI tests are now 13/13; the full FreeBSD workspace is 50/50 (Boa 7, UDS 9, Headless 4, V8 FFI 13, Network 17). Formatting and Clippy with `-D warnings` passed.
- Two expanded experiments triggered `LowMemoryNotification()` while parent/child wrappers still had external JS roots. Both crashed with SIGSEGV on FreeBSD: one attempted listener dispatch afterward; the second only attempted to read the rooted wrappers after collection. Both variants were reverted. Only the unreachable cross-heap cycle collection is currently verified; the root-retained collection path itself is unresolved, and callback dispatch after such a collection is also unverified.
- This does not yet migrate the existing DOM implementation to CppHeap. The shared_ptr/Global listener PoC still has the documented cross-heap retention-cycle limitation. CppHeap adoption requires a separate design review for wrapper identity cache ownership, Node/Document/Realm lifetime, finalization, listener removal, cross-Realm references, thread affinity, heap limits, and performance under GC stress.
- The CppHeap probe is a feasibility result, not production DOM evidence and not a completed Renderer/Browser heap integration.

## Typed primitive JavaScript values in the project Runtime contract — 2026-11-01

- Extended the project-owned `JsRuntime` trait with `evaluate_value(source) -> Result<JsValue, JsRuntimeError>`; retained `evaluate_to_string` as a separate compatibility/debug convenience.
- Added an engine-neutral `JsValue` enum for `Undefined`, `Null`, `Boolean`, `Number`, `String`, and `Unsupported`.
- Added a tagged C ABI result path. V8-only values and handles do not cross into Rust; strings use explicit byte lengths and preserve embedded NUL bytes; IEEE-754 values preserve `-0` and `NaN`.
- Objects, functions, Symbols, and BigInts deliberately return `Unsupported` rather than being implicitly coerced. Opaque object handles, identity/lifetime management, BigInt representation, and structured exception payloads remain future work.
- Tests cover all primitive variants, empty strings, embedded NUL, negative zero, NaN, unsupported object/function/Symbol/BigInt values, and syntax/runtime errors. V8 FFI tests: 15/15; full FreeBSD workspace: 52/52 (Boa 7, UDS 9, Headless 4, V8 FFI 15, Network 17). Formatting and Clippy with `-D warnings` passed.
- Added opaque `JsRealmId` handles bound to a monotonically allocated Rust Runtime identity, independent V8 Context creation/destruction, and typed evaluation within a selected Realm. Handles from another Runtime are rejected before FFI dispatch; tests verify isolation between two Realms and the default Context, cross-Runtime rejection, explicit destruction, stale-handle rejection, and continued operation of a surviving Realm. The focused Realm lifecycle test passed 20/20 independent test-process runs.
- Realm PoC currently provides isolated ECMAScript globals only; project DOM host bindings are not yet installed into secondary Realms. Cross-Realm wrapper identity, security tokens/origin policy, host callbacks, native host objects, structured exception types, Agent/Worker integration, and HTML Event Loop scheduling remain open.
- This is still a Phase 0 boundary proof, not a frozen production API.

## Structured JavaScript evaluation errors — 2026-11-01

- Extended `JsRuntimeError` with the adapter status code, V8 message text, and optional source line/start-column metadata. Adapter-side failures retain a generic message and omit source coordinates.
- The narrow C ABI now copies V8 `TryCatch::Message()` text with an explicit byte length and returns line/start-column separately. Missing columns use an explicit sentinel and become `None` in Rust; V8 objects and exception handles do not cross the FFI boundary.
- Tests verify syntax errors include a message and source location, and runtime exceptions preserve the thrown Error message and the second source line. V8 FFI tests remain 15/15; the full workspace remains 52/52. Formatting and Clippy with `-D warnings` pass.
- The error model does not yet expose stack traces, structured thrown values, nested causes, or a cross-engine error taxonomy. V8 message/location data may be absent for some abrupt completions; the adapter status remains the stable fallback signal.
