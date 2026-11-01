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
- `cargo clippy --workspace --all-targets --all-features --offline --locked -- -D warnings`: passed.
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

## Realm-aware DOM host binding update — 2026-11-01

- Installed the project-owned minimal `Node` host class into each newly created V8 Context, not only the default Context. The shared `FunctionTemplate` produces a context-local constructor/prototype for each Realm.
- Stored the opaque Realm ID in V8 Context embedder data. Each native wrapper records its owning Realm, and the weak wrapper identity cache is now keyed by `(native Node identity, Realm ID)` rather than only by native Node address.
- `node_wrapper_from()` now validates the current isolate and Realm before exposing a native Node to host methods. `appendChild` rejects nodes that do not belong to the same Runtime and Realm. Child re-wrapping uses the parent's Realm-specific cache entry.
- Added tests proving that `Node` is installed in two independent Realms, wrapper identity is stable within each Realm, and JavaScript globals remain isolated. FreeBSD V8 FFI tests: 15/15; full workspace tests: 52/52. `cargo fmt --all -- --check`, Clippy with `-D warnings`, and `git diff --check` passed.
- Added a test-only native probe that wraps the same native Node identity in two simultaneously live Realms. It verifies repeated wrapping preserves `===` within each Realm, wrappers are distinct across Realms, and a wrapper from Realm A is rejected when inspected under Realm B. This directly exercises cache partitioning and the current-Realm guard without exposing a public cross-Realm transfer API. Origin/security-token policy, host callbacks crossing Rust/C++ boundaries, Agent/Worker mapping, and Realm teardown stress remain open.
- This does not migrate the shared_ptr DOM prototype to CppHeap and does not establish production DOM or Renderer isolation.

## DOM detach and per-Realm wrapper isolation update — 2026-11-01

- Added `Node.removeChild(child)`: it validates the receiver and child against the active Runtime/Realm, removes the strong native parent-to-child edge, clears the child's weak parent link, returns the removed child, and reports `NotFoundError` when the node is not a child.
- Added a GC lifecycle test proving a detached child is no longer retained by its former parent and its wrapper becomes collectible after the last native owner is released.
- Added a test-only native bridge probe that wraps the same native Node in two live Realms. It verifies stable `===` identity within each Realm, distinct wrappers across Realms, and rejection of a Realm-A wrapper while Realm B is active. The focused Realm/cache test passed 20/20 independent test-process runs.
- FreeBSD V8 FFI tests: 16/16; full workspace tests: 53/53 (Boa 7, UDS 9, Headless 4, V8 FFI 16, Network 17). `cargo fmt --all -- --check`, Clippy with `-D warnings`, and `git diff --check` passed.
- Scope remains a small DOM lifecycle PoC, not WHATWG DOM conformance. Navigation/Document teardown, cross-Origin policy and security tokens, Page World versus Agent Inspection World authorization, MutationObserver, Frame/Worker lifecycle, CppHeap migration, and sustained GC/heap pressure remain open.

## Realm-scoped Host Callback contract update — 2026-11-01

- Added an opt-in `V8Runtime::install_host_callback(realm, callback)` adapter API. It grants one synchronous `hostCall(string)` capability to one explicitly selected non-default Realm; the engine-neutral `JsRuntime` trait remains callback-agnostic, and a Runtime accepts only one active callback grant.
- Callback input/output use explicit byte lengths, preserve embedded NUL, and are bounded to 16 KiB in each direction. Rust callback `Err` values become JavaScript `Error` exceptions; Rust panics are caught at the FFI boundary and become a bounded JavaScript error. Reentrant dispatch into the same callback state is rejected before borrowing the callback closure.
- The capability is installed with an own-property definition, not an assignment that could invoke a page-defined setter. The V8 callback checks `GetIncumbentContext()` against the granted Realm, so transferring the function object to a different Realm does not transfer its capability. A native cross-Realm transfer probe verifies the call is rejected.
- Destroying the granted Realm clears the native callback pointer before the Rust-owned callback state is released. A replacement Realm can then receive a new callback grant.
- FreeBSD V8 FFI tests: 20/20; workspace tests: 57/57. All four host-callback tests (capability scope/UTF-8, error and panic conversion, reentrant dispatch rejection, and cross-Realm function-transfer rejection) passed 20/20 independent test-process runs each. `cargo fmt --all -- --check`, Clippy with `-D warnings`, and `git diff --check` passed.
- Reentrancy scope: the current test recursively enters the Rust FFI dispatcher and proves the guard; it does not yet validate nested V8 script execution from inside a host callback. Async/Promise callbacks, cancellation/deadlines, structured-clone arguments, multi-capability registries, and a full Origin/agent security-token model remain open. This is a feasibility contract, not a production host-permission system.


## Origin-grouped DOM wrapper access — 2026-11-01

### Objective

Separate JavaScript Realm identity from the host's same-origin grouping decision. A Realm is an execution environment; an origin group is an explicit host authorization boundary for native DOM wrapper access. Host callbacks remain scoped to one exact Realm and must not become transferable merely because two Realms share an origin group.

### Implementation

- Added opaque `JsOriginId`, scoped to its owning `JsRuntime`; the Rust adapter issues monotonically increasing IDs and rejects handles from another Runtime.
- Added `create_origin()` and `create_realm_for_origin()`. The existing `create_realm()` creates a fresh origin group, preserving isolation by default.
- Passed the origin-group ID across the narrow C ABI and stored it separately from the Realm ID in V8 Context embedder data.
- DOM wrapper cache identity remains `(native Node identity, Realm ID)`, so wrapper identity is still per-Realm.
- Each native wrapper records its host origin-group ID. Native wrapper entry checks both V8's current Context and incumbent Context against the wrapper's origin group.
- `appendChild` and `removeChild` accept nodes from another Realm only when the host origin group matches; different-origin wrappers are rejected at the native entry point.
- Strengthened the host-callback transfer test: the grant Realm and caller Realm deliberately share an origin group, but transferring `hostCall` still fails because the callback capability is tied to the exact granted Realm.

### Verification

- Focused same-origin/cross-origin DOM wrapper test: 20/20 independent test-process runs passed on FreeBSD 15.1-RELEASE-p6.
- Full FreeBSD workspace: 58 tests passed, 0 failed (Boa 7, UDS 9, Headless 4, V8 FFI 21, Network 17).
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`: passed.
- `git diff --check`: passed.

### Evidence boundary and limitations

- This verifies an embedder-enforced origin-group check for this native Node wrapper PoC. It does not prove general JavaScript object isolation: ordinary JS objects are not automatically protected by this wrapper check.
- `JsOriginId` is an internal host-issued grouping handle, not a URL origin parser or cryptographic security principal. Do not derive or grant it from untrusted page input.
- URL scheme/host/port computation, opaque origins, sandboxed frames, `document.domain`, agent clusters, structured clone, `postMessage`, cross-origin DOM access semantics, and process/site isolation remain unimplemented.
- The earlier V8 `Context::SetSecurityToken` experiments did not establish the required same-token Global Proxy behavior. This implementation does not claim that V8 security tokens alone enforce the browser's origin policy.
- This remains a Phase 0 feasibility PoC. It is not production DOM, a complete same-origin policy, or a Renderer Sandbox.

### Next work

1. Add a formal host-owned Origin model and URL-origin canonicalization tests before mapping real page URLs to origin groups.
2. Test hostile wrapper transfer, nested callbacks, event listeners, and stale wrappers after Realm destruction.
3. Define Document/Frame/Agent ownership and decide which origin-group handles may be shared across contexts/processes.
4. Continue the independent Renderer Sandbox work: process separation, Capsicum FD rights, Jail network denial, rctl, IPC authentication, and crash recovery.


## Host URL Origin canonicalization PoC — 2026-11-01

### Scope

Added an independent Rust experiment under `experiments/origin-policy/`. URL parsing and origin calculation belong to the trusted browser host, not page JavaScript or the V8 adapter. The crate uses the workspace-pinned `url = 2.5.8` implementation and keeps the resulting group handles separate from `JsOriginId`; a future Browser Runtime layer must explicitly map the host registry handle to the selected JS Runtime's opaque origin handle.

### Behavior

- Tuple origins are interned using the URL library's WHATWG-compatible origin calculation: scheme, canonical host, and effective port; path, query, fragment, and userinfo do not affect origin identity.
- Default ports, host case, IDNA names, IPv4 normalization, and IPv6 textual variants are covered by tests.
- `blob:` URLs inherit the parsed inner URL's origin.
- Opaque origins are unique per creation, including repeated parsing of the same `data:`, `about:blank`, or `file:` URL text. `file:` is treated conservatively as opaque because its origin behavior is implementation-defined.
- Inherited origins are explicit: `inherit(parent)` propagates a creator's group; parsing `about:blank` alone creates a fresh opaque origin and does not infer an opener/creator relationship.
- Sandboxed documents can receive a fresh opaque group. Registry handles cannot be validated by another registry.

### Verification

- Origin policy unit tests: 15/15 passed on FreeBSD 15.1-RELEASE-p6.
- Dependency was already present in `Cargo.lock` as `url 2.5.8`; workspace lock metadata updated offline, without fetching or changing the dependency version.
- The registry is now connected to the generic Browser Runtime coordinator PoC, but not yet to real Navigation/Document/Frame objects. It does not yet handle inherited-origin navigation, sandbox flags, `document.domain`, agent clusters, or process/site isolation.

### Security boundary

- The registry handle is an internal host grouping token, not a user-visible serialization and not a cryptographic secret.
- Never derive an authorization grant from untrusted page content. The trusted navigation/Document lifecycle must decide when a URL's computed origin is inherited, replaced, or forced opaque.
- This is a Phase 0 origin primitive, not a complete browser Same-Origin Policy implementation.


## Browser Runtime Origin-to-Realm coordinator PoC — 2026-11-01

### Objective

Connect the host URL Origin Registry to the JavaScript Runtime's opaque Origin/Realm API without moving URL parsing into the V8 adapter. The Browser Runtime owns the mapping and the lifecycle reference counts.

### Implementation

- Added `experiments/browser-runtime/` (`aibe-browser-runtime-origin-poc`).
- Added a small `OriginRealmRuntime` lifecycle contract with a blanket adapter for the project `JsRuntime` trait; tests use a deterministic mock so the orchestration logic can run in hosted CI without V8 binaries.
- Added `OriginRealmCoordinator`: canonical URL tuple origins map to one JS Runtime origin handle while one or more Realms remain active; every navigation/document receives a distinct Realm/Context.
- Added explicit inherited-origin creation from a live creator Realm, and fresh opaque-origin creation for sandboxed contexts.
- Destroying the last Realm releases both the Browser Runtime mapping and tuple-origin interning entry; opaque handles are not retained in the tuple map, preventing unbounded accumulation of visited origins. Failed Realm creation releases its new group; inherited creation from a destroyed creator is rejected.
- Added a FreeBSD-only V8 integration test for canonical URL → host Origin Group → V8 origin handle → Realm creation/destruction, confirming same-origin Realms still have separate JS global environments.
- Updated SBOM generation/validation to include seven workspace crates.

### Verification

- Hosted-compatible coordinator unit tests: 6/6 passed using the deterministic mock; FreeBSD-only V8 integration test: 1/1 passed, including stale Realm evaluation rejection.
- Full FreeBSD workspace: 80/80 tests passed (Boa 7, Browser Runtime 7, UDS 9, Headless 4, V8 FFI 21, Network 17, Origin Policy 15).
- `cargo fmt --all -- --check`: passed.
- SBOM generator/validator now expect 7 crate artifacts; shell syntax, Python AST, and 7/7 workspace coverage checks passed. Local cargo-cyclonedx remains unavailable, so CI must verify actual generation and format.

### Limitations

- This is a lifecycle/orchestration PoC, not the production Browser Runtime.
- It does not yet model Document, Frame, Worker, Agent Cluster, browsing context groups, opener relationships, sandbox flags, process/site isolation, or navigation commit/rollback.
- Origin mapping is retained only while at least one Realm in the group is active. Persistent storage partitioning and browsing-session lifetime are separate concerns.
- The V8 integration test proves URL-origin mapping and Context lifecycle; cross-origin native wrapper access is covered by the FFI crate's dedicated probe, not by a general DOM or Same-Origin Policy implementation.


## UDS control-protocol framing and client hardening — 2026-11-01

### Implementation
- Added shared bounded newline-frame reading for both broker-side request handling and client-side response handling.
- Both directions enforce a 64 KiB maximum frame size, including the newline delimiter; missing newline termination is rejected as a truncated frame.
- Client request serialization now uses a bounded writer, preventing an extra unbounded serialized copy of a caller-supplied JSON value.
- Client and server set five-second stream read/write timeouts.
- Client validates that the broker response is a JSON object and rejects oversized or truncated responses.
- Requests must be JSON objects. Request IDs, method names, and task IDs have explicit 256-byte limits; invalid request IDs are not echoed into error responses.
- Server response write errors are propagated instead of panicking if the client disconnects.
- FreeBSD test-server accept now checks both kernel-reported effective UID and GID through getpeereid; a mismatched GID is rejected even when UID matches.

### Verification
- UDS protocol tests: 17/17 passed on FreeBSD 15.1-RELEASE-p6.
- New negative tests cover exact frame-size boundary, one-byte-over limit, oversized outbound request before connect, oversized inbound broker response, truncated response, invalid top-level JSON shape, oversized request ID, and client disconnect during response write.
- UDS crate Clippy with -D warnings: passed.
- Full FreeBSD workspace: 88/88 tests passed (Boa 7, Browser Runtime 7, UDS 17, Headless 4, V8 FFI 21, Network 17, Origin Policy 15).
- cargo fmt --all -- --check, workspace Clippy with -D warnings, and git diff --check: passed.

### Limits
- This remains a protocol PoC; no production Broker service or renderer task execution has been enabled.
- Peer UID authentication is available through a FreeBSD kernel helper and used by the test server, but the full authorization/capability policy is not yet integrated.
- No request replay/idempotency store, per-peer quotas, concurrent scheduling, cancellation token, durable task journal, or production broker crash recovery exists.
- The five-second timeout is a local stream I/O deadline, not a full end-to-end task deadline.


### CI portability correction — 2026-11-01
- GitHub Actions Run 41281608108 exposed a Linux-only test-harness difference: the truncated-response mock server closed the Unix socket while the client request was still unread, so Linux reported ConnectionReset instead of the test's expected UnexpectedEof.
- The mock server now reads the complete newline-terminated request before writing its deliberately truncated response and closing. This isolates the intended response-framing condition and avoids platform-dependent reset behavior.
- Reverified locally on FreeBSD: UDS 17/17 and full workspace 88/88; fmt and Clippy pass. The corrected test is awaiting the next GitHub Actions run.

## Per-origin descriptor grant hardening — 2026-11-01

- Broker checks the connected renderer peer with FreeBSD `getpeereid()` and exact effective UID/GID matching before granting Origin A storage.
- Renderer closes inherited Origin A and Origin B directory descriptors, then receives only the authorized Origin A directory descriptor over `SCM_RIGHTS`.
- The receiver rejects ancillary messages containing multiple descriptors and invalid grant markers. Both negative tests verify that every installed descriptor is closed on rejection.
- The delegated storage FD is restricted to `CAP_LOOKUP | CAP_READ | CAP_FSTAT`; cross-origin traversal and write/create attempts are denied after `cap_enter()`.
- Broker closes its storage descriptor copies after transfer. Renderer acknowledges readiness over IPC; Broker waits for process exit.
- The oversized UDS request test now tolerates an expected early BrokenPipe/ConnectionReset when the server rejects an over-limit frame, then validates the structured `request_too_large` response. The focused test passed 20/20 independent runs.
- `scripts/run-security-pocs.sh`: all Capsicum, Broker/Renderer, UDS identity, storage delegation, malformed ancillary cleanup, renderer crash-recovery and Jail network-isolation checks passed.
- FreeBSD workspace: 88/88 tests passed; Rustfmt and Clippy (`-D warnings`) passed. Changed C PoCs passed `clang --analyze`.
- Latest commit: `e7fdf02d40fdf09bfdbfca347b66ce22d3b94894`, pushed to `main`; working tree clean.
- GitHub Actions Run [41282807723](https://github.com/lycupay-debug/autonomous-ai-browser-engine/actions/runs/41282807723) is still running; Rust and supply-chain final conclusions are pending.
- Remaining: process identity stronger than UID/GID, production Broker capability grants and revocation, per-peer quotas, rctl (host `kern.racct.enable=0`), multi-renderer scheduling, production storage semantics, network broker enforcement and hostile IPC stress.
- This is a feasibility PoC, not a production Renderer Sandbox. No persistent host configuration or production service was changed.

## Versioned per-renderer Origin storage grant envelope — 2026-11-01

### Design and implementation

- Replaced the one-byte storage marker with a fixed-size 48-byte host-local C ABI grant envelope: magic, protocol version, descriptor count, broker-tracked Renderer PID, read-only rights mask, Renderer instance ID, registry ID, grant sequence, and host-issued Origin Group ID.
- Changed this PoC's Broker/Renderer channel from `SOCK_STREAM` to `SOCK_SEQPACKET`, so the grant header and its `SCM_RIGHTS` ancillary descriptor are received as one bounded record rather than relying on stream read boundaries.
- Broker still obtains kernel-reported peer effective UID/GID with `getpeereid()` and requires exact equality with the local policy before sending the descriptor.
- Renderer validates protocol version, magic, exactly-one-descriptor declaration, expected child PID, read-only grant mask, grant ID, and expected Origin Group before accepting the descriptor. It then applies `CAP_LOOKUP | CAP_READ | CAP_FSTAT` and enters Capsicum capability mode.
- Negative tests send multiple descriptors, a validly shaped grant for the wrong Origin Group, and a grant addressed to a different Renderer PID. All rejected received descriptors are checked for closure.
- The PID value is bound to the broker-created `socketpair()` and `fork()` relationship, which the Broker tracks locally. It is not a kernel-reported peer PID and must not be treated as a standalone authentication factor. `getpeereid()` in this experiment authenticates effective UID/GID only.

### Verification

- FreeBSD 15.1: `clang -Wall -Wextra -Werror -O2` compile passed.
- Grant-envelope and per-Origin storage PoC: all six positive/negative checks passed, including wrong-Origin and wrong-PID rejection, continued renderer access after the Broker closes its local FD copy, and Broker `SIGKILL` termination of the renderer.
- The complete focused PoC passed 20/20 independent executions.
- `clang --analyze -Wall -Wextra -O2` passed; generated plist was removed.
- `git diff --check` passed.

### Limits and next gates

- This is still a single-renderer, local feasibility PoC. Grant IDs are test constants, not a durable or cryptographically random capability registry.
- No production grant issuance/revocation registry, expiry, replay protection, renderer quotas, multi-renderer scheduling, privilege-separated service accounts, or persistent storage backend exists.
- The receiver validates a broker-issued envelope on a broker-created private channel; this does not replace kernel peer credential checks or establish a production cross-process authorization protocol.
- Continue with explicit grant lifecycle/state transitions, duplicate/replay rejection, bounded peer quotas, and hostile malformed/truncated packet tests before considering any production Broker contract.

## FreeBSD V8 SDK prefix default correction — 2026-11-01

- A clean Cargo invocation without `AIBE_RUNTIME_PREFIX` exposed a mismatch: the FFI `build.rs` defaulted to `/usr/local`, while the project's documented extracted, non-installed runtime SDK and helper scripts use `/tmp/aibe-runtime-root/usr/local`.
- Updated the build-script fallback to the documented isolated prefix. No packages were installed and no host include/library paths were modified.
- Verified the full workspace with `AIBE_RUNTIME_PREFIX` explicitly unset and `LD_LIBRARY_PATH` pointing at the extracted SDK: 88/88 tests passed. Formatting and Clippy with `-D warnings` passed.
- This fixes the default build path for the existing Phase 0 extracted SDK layout; it does not make the external V8/SpiderMonkey SDK part of the repository or remove the documented runtime-library environment requirement.

## Broker grant lifecycle, replay rejection, and quota policy PoC — 2026-11-01

### Model

- Added the dependency-free workspace crate `aibe-broker-policy-poc` under `experiments/broker-policy/`; it models Broker policy only and performs no socket, process, or filesystem operations.
- Each grant is bound to a Broker-issued Renderer instance ID plus PID/UID/GID, a host-issued Origin Group ID, and the exact read-only storage rights set (`LOOKUP | READ | FSTAT`). PID reuse with a new Renderer instance ID is rejected.
- Grant IDs are monotonically sequenced and scoped to an in-memory registry, preventing accidental cross-registry ID aliasing. They are not cryptographic tokens or durable IDs.
- Transfer is one-shot: an `Issued` grant becomes `Delegated`; duplicate transfer is rejected as replay. Wrong Renderer, wrong Origin, and rights escalation are rejected without consuming the grant.
- Global and per-Renderer quotas bound the number of live grant records. Revoking an untransferred grant removes it immediately.
- Revoking a delegated grant marks all grants for that Renderer `RevocationPending`, blocks further transfer, and returns a `TerminateRenderer` action with the pending-grant count. The registry retains those records and counts against quotas until the supervisor confirms process exit via `release_renderer()`. This avoids silently freeing quota while a process may still hold delegated descriptors.

### Verification

- Broker policy crate: 13/13 tests passed on FreeBSD 15.1, including C ABI size/offset parity checks for the 48-byte grant envelope.
- Tests cover read-only grant binding, one-shot/replay behavior, same-PID/new-instance rejection, Origin mismatch, rights escalation, unsupported rights, per-Renderer quota, global quota, two-phase delegated-grant revocation, process-exit cleanup, registry-scoped grant IDs, and invalid quota/identity values.
- SBOM generation/validation inventory updated from seven to eight workspace crates. Local SBOM generator availability remains subject to the CI supply-chain job; do not claim local artifact generation unless run.

### Limits

- This is an in-memory policy model, not connected to the C `SCM_RIGHTS` PoC or a production Broker. The caller must bind the supplied identity to a kernel-authenticated socket and Broker-owned process handle, execute the returned termination action, and call `release_renderer()` only after process exit is confirmed.
- No durable grant journal, expiry, cryptographic token, cross-process synchronization, concurrent scheduler, or host-level resource quota exists. `rctl` remains blocked by `kern.racct.enable=0`.


## Incremental R&D record｜2026-11-01（Dedicated Worker ownership lifecycle）

- `NavigationController` now tracks Dedicated Worker IDs, owner Document IDs, script URLs, Origin Groups, Realm handles, and `Running` / `Terminating` states.
- Worker script initial URL must resolve to the owning Document's canonical Origin Group. Cross-origin scripts are rejected and the temporary candidate Realm is released. A Worker receives a distinct JS Realm while sharing its owner Document's Origin Group.
- Document navigation commit and Frame subtree detach terminate all Workers owned by the Document before destroying its Realm. Runtime teardown failures leave the Worker in `Terminating` so cleanup can be retried; a failed Worker teardown prevents Document commit/detach from completing.
- Dedicated Worker ownership tests cover same-origin Realm separation, cross-origin rejection without Realm leaks, termination retry, Document replacement, and recursive Frame detach.

**Scope limits:** this is an ownership/lifetime model only. It does not fetch or validate redirected Worker scripts, create `WorkerGlobalScope`, implement Worker event loops, `postMessage`/structured clone, CSP/referrer policy, module/classic script semantics, SharedWorker, or ServiceWorker.

## Incremental R&D record｜2026-11-01（Parent Document replacement and descendant Frame teardown）

### Finding and implementation

- Review of the Navigation / Document / Frame ownership model found a lifecycle gap: committing a replacement Document removed the outgoing Document and its Dedicated Workers, but did not detach descendant Frame subtrees owned by that outgoing Document.
- `commit_navigation()` now tears down descendant Frame subtrees in stable Frame ID order before terminating the outgoing Document's Workers and Realm.
- Teardown remains retryable: if a descendant Worker/Realm destruction fails, the parent Document remains current and the Navigation remains pending. Already detached descendants stay detached; remaining descendants can be cleaned on retry.
- This is a bounded lifecycle-model correction. It does not claim full HTML navigable, iframe element, unload/pagehide, or WHATWG navigation semantics.

### Verification

- Browser Runtime: 23/23 tests passed, including nested descendant Frame/Worker cleanup and injected descendant Worker teardown failure followed by successful retry.
- FreeBSD workspace: 117/117 tests passed with `AIBE_RUNTIME_PREFIX` unset and `LD_LIBRARY_PATH=/tmp/aibe-runtime-root/usr/local/lib`.
- `cargo fmt --all -- --check`: passed.
- Workspace Clippy with `-D warnings`: passed.
- Full `scripts/run-security-pocs.sh`: passed, including Capsicum FD rights, Broker/Renderer boundary, UDS peer credentials, per-Origin storage grants, malformed SCM_RIGHTS cleanup, Renderer crash recovery, and Jail IPv4 isolation.
- SBOM validator: all eight crate BOM fixtures passed, including the zero-dependency Broker policy crate; Python and shell syntax checks passed.
- `git diff --check`: passed.

### Limits

- Frame/Document/Worker ownership is still a deterministic host-side model. There is no actual iframe DOM integration, parser-driven browsing-context creation, page lifecycle event dispatch, real Network Loader, or browser task/event loop.
- Failure recovery is tested with an injected Runtime teardown error; this does not prove recovery from arbitrary native V8 process crashes.
- No production service, persistent FreeBSD configuration, sysctl, or host service state was changed.


## Incremental R&D record｜2026-11-01（Web Capability Matrix evidence reconciliation）

- Updated `docs/phase-0/web-capability-matrix.md` to show current Phase 0 evidence and implementation status beside each target user task. It explicitly distinguishes absent capabilities from isolated PoCs and end-to-end implementation.
- Current P0 blockers are visible: no HTML parser/response-body loader, no parser-backed DOM or stable AI node query/evidence API, no forms/input model, and no HTML Event Loop/SPA integration. Navigation/redirect, V8 DOM host objects, Origin isolation, and microtask scheduling remain separate PoCs until connected.
- Worker evidence is recorded as ownership/Realm lifecycle plus a caller-fed redirect-chain Origin validation contract; no Worker script fetch/execution or WorkerGlobalScope is claimed.
- Updated `README.md` and `dependency-audit.md` to correct stale SBOM/workspace counts. Current Cargo metadata: 211 total packages, eight workspace crates, 203 third-party packages; all 203 third-party packages have license metadata. The tracked `sbom/` directory still contains five historical files; CI regenerates and validates the complete eight-file set as artifacts.
- GitHub Actions Run `41288165444` for commit `e4421a17cbf4afd9c40cc3287961ff5c9be14335` completed successfully: Rust format/test/Clippy job passed; `cargo-audit` 0.22.2 passed; eight CycloneDX 1.5 SBOMs generated, validated and uploaded.
- Project license remains undecided. Cargo metadata and SBOM coverage do not constitute a legal audit and do not cover native FreeBSD packages or extracted runtime assets.

## Incremental R&D record｜2026-11-01（Bounded HTTPS response-body loading PoC）

### Implementation

- Extended the Network Policy PoC with `fetch_https_get()` and `FetchResult`; the existing header-only API remains available.
- Added bounded HTTP/1.1 response-body framing for `Content-Length`, `Transfer-Encoding: chunked`, and connection-close-delimited bodies.
- Hard response body cap: 8 MiB. Chunked responses additionally cap chunk count at 4,096, chunk-framing bytes at 1 MiB, trailer bytes at 64 KiB, trailer count at 100, and each line at 8 KiB.
- Rejects duplicate framing headers, simultaneous Content-Length and Transfer-Encoding, unsupported transfer/content encodings, truncated Content-Length bodies, malformed chunk terminators, invalid chunk-size bytes, forbidden framing fields in trailers, excessive body/chunk/framing/trailer sizes, and oversized caller limits.
- Sends `Accept-Encoding: identity`; compressed response decoding is deliberately not implemented. Redirect response bodies are discarded; every redirect still resolves, validates HTTPS, re-resolves DNS, revalidates the complete IP answer set, and connects to a pinned validated address.
- Added CLI `--body` mode that reports body length and a bounded hex prefix. Updated the opt-in live integration script to exercise body reads for a direct request and a redirected request.

### Verification

- Network Policy: 27/27 tests passed, including Content-Length, chunked, close-delimited, size limits, malformed framing, duplicate headers, trailer validation, chunk-count bounds, and unsupported encoding rejection.
- Full FreeBSD workspace: 133/133 tests passed.
- Workspace Clippy `-D warnings`, Rustfmt, `git diff --check`, full security PoC suite, shell/Python syntax checks, and all eight synthetic SBOM fixtures passed.
- Live opt-in integration `AIBE_RUN_NETWORK_INTEGRATION=1 ./scripts/run-network-poc.sh` passed:
  - `https://example.com/`: HTTP 200, chunked body decoded, 577 bytes.
  - `https://httpbin.org/redirect/1`: 302 redirect to `/get`, final HTTP 200, Content-Length body decoded, 240 bytes.
- These are live point-in-time endpoint checks, not a reproducible general Internet compatibility test.

### Limits

- HTTP/1.1 GET only. No HTTP/2/3, compression, cookies, cache, CORS/Fetch semantics, request methods/bodies, streaming API, cancellation propagation, download broker, or persistent connection pool.
- The Network Policy PoC is not connected to NavigationController, HTML Parser, Worker script execution, or a production Network Service. No browser document was parsed from the retrieved body.
- The 8 MiB body limit and chunk limits are Phase 0 safety bounds, not final product resource budgets.


## Incremental R&D record｜2026-11-01（Document Realm initialization and Navigation integration）

### Implementation

- Added `JsRuntime::initialize_document(realm)` as an explicit host lifecycle hook. The V8 adapter installs a minimal `document` object (`nodeType: 9`) with a native Node-backed `documentElement`.
- `NavigationController` initializes the host Document surface for root Frames, child Frames (inherited and sandboxed), navigation candidates and redirect replacement Realms. Dedicated Worker Realms intentionally do not receive a `document` global.
- Added `evaluate_document_value(document_id, source)`, which only resolves a currently owned Document and evaluates in its Realm. Pending candidates, removed Documents, and Worker Realms are not reachable through this entry point.
- Initialization failure destroys the unpublished candidate Realm. If immediate destruction also fails, the Realm handle is retained and surfaced as `DocumentInitializationCleanupFailed`; `retry_document_initialization_cleanup()` retries it instead of silently dropping the handle.
- Added V8 integration coverage for root/child Document initialization, script evaluation, navigation commit replacement, and invalidation of outgoing parent/child Documents. Mock-runtime tests cover root/child initialization, redirect replacement, Worker exclusion, initialization failure, and cleanup retry.

### Verification

- Browser Runtime: 34/34 tests passed, including the new V8 Navigation integration test.
- V8 FFI: 22/22 tests passed.
- Full FreeBSD workspace: 141/141 tests passed.
- Workspace Clippy `-D warnings`, Rustfmt, `git diff --check`, complete security PoC suite, Python/shell syntax checks and all eight SBOM validator fixtures passed.
- Verification used `AIBE_RUNTIME_PREFIX` unset and `LD_LIBRARY_PATH=/tmp/aibe-runtime-root/usr/local/lib` for the native V8 runtime.

### Limits

- The current `document` is a JavaScript placeholder object, not a standards-compliant `Document`; `Node` remains the narrow native DOM-like PoC, not a complete DOM or Web IDL implementation.
- No HTML parsing, HTML-to-DOM construction, Window/Location/History, lifecycle event dispatch, CSS, layout, form controls, MutationObserver, HTML Event Loop, or browser task scheduling is implemented.
- `evaluate_document_value` is a Phase 0 host-side entry point, not the final AI Control protocol or a production script-execution security model.
- No production service, persistent FreeBSD configuration, sysctl, or host service state was changed.

## Incremental R&D record｜2026-11-01（HTML5 parser, bounded DOM tree and Document ownership）

### Implementation

- Added workspace crate `aibe-html-parser-poc`, using `html5ever` 0.40.1 for tokenization and HTML5 tree construction, with a project-owned index-based `TreeSink`/node arena. `markup5ever_rcdom` is not used.
- Implemented Document, DocumentFragment, Doctype, Element, Text, Comment and ProcessingInstruction records; HTML, SVG and MathML namespaces; template contents; duplicate-attribute handling; quirks mode; basic tag/local-name/id/attribute/text queries.
- Added resource bounds: input <= 1 MiB, DOM nodes <= 65,536 (including Document root), recorded parse errors <= 256. Node-budget exhaustion returns a typed error; it does not expose a truncated tree as a successful parse.
- Input currently assumes UTF-8; `from_utf8()` decoding is lossy. No encoding sniffing, streaming parser, `document.write`, script execution, parser-blocking script lifecycle or WPT/html5lib conformance claim.
- Integrated parser ownership into `NavigationController::Document`: `parse_document_html()` attaches a tree to a live Document; `commit_navigation_with_html()` parses before destructive navigation commit, so parse failure leaves the old Document and pending navigation unchanged.
- Added `--body-stdout` to the bounded HTTPS fetch PoC and `aibe-html-parser-poc` stdin CLI. `scripts/run-html-pipeline-poc.sh` fetches in one process, writes the bounded body to a temporary file, then parses it in a separate process; the temporary file is removed on exit.
- Live pipeline result: `https://example.com/` returned HTTP 200, 577 response bytes; parser produced 16 DOM nodes, title `Example Domain`, `NoQuirks`, and zero parse errors.

### Verification

- HTML Parser: 9/9 tests passed, covering implied document structure, malformed paragraph repair, entities, raw-text handling, SVG/MathML namespaces, template contents, duplicate attributes, and input/node/error limits.
- Browser Runtime: 37/37 tests passed, including Document-owned tree attachment and parse-before-commit failure safety.
- Full FreeBSD workspace: 153/153 tests passed.
- Workspace Clippy `-D warnings`, Rustfmt, `git diff --check`, complete Security PoC suite, Python/shell syntax checks and all nine synthetic SBOM fixtures passed.
- Cargo metadata: 223 packages total, nine workspace crates, 214 third-party packages; all 214 third-party packages declare license metadata. This is metadata presence, not legal approval.

### Integration risk discovered

- An exploratory same-process test linking Network Policy and Browser Runtime/JS runtime libraries stopped with SIGBUS in FreeBSD libc `getaddrinfo` (`comp_dst`) before V8 initialization. The Network Policy CLI succeeds independently, and the process-separated fetch-to-parser script succeeds. The exact cause is not established; the direct Network Policy dependency was removed from Browser Runtime. Keep the process boundary and investigate the native-linking/resolver interaction before attempting same-process integration again.
- The temporary-file handoff is a test harness only, not the final authenticated UDS Network Service protocol. Parsed Rust HTML nodes are not yet the same objects as V8 native Node wrappers.

### Remaining limits

- No HTML Parser → V8 Node wrapper bridge, stable node IDs, DOM mutation/events, Window/Web IDL, forms, CSS, Layout, Paint, accessibility tree or AI evidence snapshot.
- No production Network Service/IPC integration, streaming/cancellation, cache/cookies/compression, HTTP/2/3 or non-UTF-8 encoding support.
- `html5ever` provides parser algorithms, but project compatibility still requires pinned html5lib/WPT tests, fuzzing, hostile-input testing and resource profiling.

## 2026-11-01 DOM snapshot export feasibility

- Added `HtmlDocument::to_snapshot_json()` in the HTML Parser crate. It emits a flat `schemaVersion: 1` representation with Document-local node IDs, parent/child edges, node type, namespace-aware element names, attributes, template-content references, quirks mode and parse diagnostics.
- The encoded payload is capped at 8 MiB. Flat node records avoid recursive serialization of deeply nested untrusted input. The source HTML remains capped at 1 MiB and the parser node budget remains 65,536.
- Node IDs are only stable within one parsed Document generation and must be paired with a host Document ID/revision. They are not authorization tokens and must be invalidated when the Document tree is replaced.
- FreeBSD parser tests: 10/10 passed, including UTF-8 CJK text, HTML entity decoding, attribute escaping, SVG namespace and doctype snapshot checks. Parser Clippy with `-D warnings` passed.
- Full FreeBSD workspace: 154/154 tests passed; workspace Rustfmt and Clippy with `-D warnings` passed. `scripts/run-security-pocs.sh` passed Capsicum FD rights, Broker/Renderer IPC, UDS peer identity, per-Origin storage grant, malformed multi-FD cleanup, renderer crash recovery and Jail IPv4 isolation.
- This is immutable snapshot serialization only. It does not provide live DOM mutation, V8 wrapper identity, Web IDL, CSS/layout, a production IPC channel, or browser-agent authorization. No production service or persistent host configuration was changed.

## 2026-11-01 Document-scoped snapshot envelope

- Browser Runtime now exposes `document_snapshot_json(DocumentId)`, wrapping the parser snapshot in `envelopeVersion: 1` with host-issued `documentId` and monotonic `revision`.
- A Document begins at revision 0 before a parsed tree exists; each successful `parse_document_html()` replacement increments the revision. A navigation commit with parsed HTML creates a new Document ID at revision 1. Failed parsing leaves the current tree and revision unchanged.
- Parser Node IDs are deterministic local indices and can be reused after a reparse. Consumers must therefore treat `(documentId, revision, nodeId)` as the complete node-handle scope; `nodeId` alone is never stable across tree replacement. A replaced Document is no longer addressable by the controller.
- Snapshot envelopes are capped at 8 MiB and reject requests for Documents without a parsed HTML tree.
- Browser Runtime tests: 38/38 passed, including revision increment, same-index reuse across revisions, navigation replacement, and stale Document rejection.
- Full FreeBSD workspace: 155/155 passed; Rustfmt, Workspace Clippy (`-D warnings`), `git diff --check`, and the full security PoC suite passed. The V8 test binary requires `LD_LIBRARY_PATH=/tmp/aibe-runtime-root/usr/local/lib` with `AIBE_RUNTIME_PREFIX` unset.
- This is host-side identity/revision binding for immutable snapshots only. It is not an authenticated IPC protocol, renderer-side handle validation, live DOM, or a security grant. The next task is to bind this envelope to the authenticated, bounded UDS channel and test malformed/stale/cross-Document payload rejection.

## 2026-11-01 Document-scoped Node handle validation

- Added host-issued `DocumentNodeHandle` carrying `(DocumentId, revision, NodeId)`; parser-local NodeId is not accepted as a complete external handle.
- `node_handle()` issues handles only for nodes present in the current parsed tree. `resolve_node_handle(expected_document, handle)` rejects cross-Document use, stale revisions after reparse, unknown Documents, missing trees and unknown Node IDs.
- Snapshot envelope encodes the u64 Document ID and revision as decimal strings, avoiding precision loss in JSON consumers whose native Number type is only exact through 53 bits. Node IDs remain bounded parser-local indices.
- Added regression coverage for a valid handle, cross-Document rejection, and stale-handle rejection after a reparse that deliberately reuses the same NodeId index.
- Browser Runtime: 39/39 passed; full workspace after this addition: 156/156 passed; Rustfmt, Clippy (`-D warnings`), diff check and Security PoC suite passed.
- This validates host-side handle scoping only. No remote Renderer has yet parsed or enforced the envelope, and no UDS transfer or live V8 DOM wrapper mapping is claimed.

## 2026-11-01 UDS DOM Snapshot binary frame codec

- Added a separate DOM Snapshot frame codec to the UDS control-protocol PoC. Existing newline-delimited control RPC remains capped at 64 KiB; DOM Snapshot uses a dedicated 20-byte big-endian header (`AIBS` magic, transport version, message type, nonzero request ID, payload length) and a maximum 8 MiB payload.
- Payload validation requires envelope version 1, decimal-string nonzero Document ID/revision, tree schema version 1, a bounded node count, unique parser-local node IDs, valid root Document, known parent/child/template references, reciprocal parent/child edges, and an acyclic parent graph.
- Decoder checks the declared payload length before allocating the payload buffer; malformed, truncated, unsupported-version/type, zero-length, oversized and structurally inconsistent frames are rejected.
- Added a UnixStream socket-pair round-trip and negative tests. UDS crate: 27/27 tests passed; workspace: 166/166 passed; Rustfmt, Clippy (`-D warnings`), diff check and full Security PoC suite passed.
- The codec is deliberately not wired into the existing authenticated server path. It does not authenticate peers, authorize Document/Origin/Renderer grants, or transfer SCM_RIGHTS descriptors. Callers must enforce FreeBSD getpeereid identity, Renderer instance and Origin grant before accepting the payload; callers must also set finite socket read/write timeouts.
- This is a bounded framing and schema-validation PoC, not cross-process Broker/Renderer integration or production IPC. No service or persistent host configuration was changed.

## 2026-11-01 authenticated cross-process UDS Snapshot round-trip

- Added a dedicated test sender binary and FreeBSD integration test. The parent creates a private 0600 UDS endpoint; an independent child process connects and writes a V1 DOM Snapshot frame; the parent accepts the socket, verifies kernel peer UID/GID with `getpeereid()` against its expected identity, then decodes and validates the frame.
- The integration test uses a finite accept deadline and socket read/write timeouts. It verifies request ID 99, Document ID 7, revision 3 and the payload graph received from the child process.
- This closes the gap between the earlier same-process UnixStream-pair codec test and a real process boundary. It is still a test-only endpoint; the production/test Broker request handler is not yet wired to the new Snapshot frame codec, and no Renderer/Origin grant is currently checked by this path.
- FreeBSD UDS crate: 28 tests passed (27 unit + 1 independent-process integration); full workspace: 167/167 passed; Rustfmt, Workspace Clippy (`-D warnings`), diff check and Security PoC suite passed.
- GitHub-hosted Linux CI compiles the helper but the FreeBSD-only getpeereid integration test is cfg-disabled there; this integration result was observed on the FreeBSD host.


## 2026-11-01 Broker-to-Renderer DOM Snapshot authorization gate

- Extended the FreeBSD UDS peer identity path with `LOCAL_PEERCRED` (`xucred.cr_pid`, UID, GID), cross-checking UID/GID against `getpeereid()`. `getpeereid()` alone does not return a PID. The Rust FFI struct validates the FreeBSD amd64 `xucred` size/offset/version before reading the peer PID.
- Added a dedicated Broker-side `snapshot_endpoint::send_authorized_snapshot()` gate. The Broker verifies that the connected peer is the expected Renderer process, checks the embedded Snapshot envelope, authorizes the exact Document revision, and only then writes the frame to the Renderer.
- Added `SnapshotGrantRegistry` in the Broker policy crate. Each one-shot grant is host-issued and bound to Renderer instance ID + PID/UID/GID principal, Origin Group, Document ID and exact revision. It enforces global/per-Renderer quotas and rejects replay, wrong Renderer instance, wrong Origin, wrong Document and stale revision. Rejected mismatches do not consume the grant.
- Added FreeBSD independent-process Broker-to-Renderer integration coverage: successful transfer; wrong kernel PID rejected before sending; same PID but wrong Renderer instance rejected after peer authentication; wrong Origin, wrong Document, stale revision and frame/envelope mismatch rejected without sending data.
- The endpoint sets finite five-second read/write timeouts. The grant is consumed before writing to avoid replay after partial delivery. Revoking an unconsumed grant blocks transfer; a grant consumed for a send attempt is reported as `AlreadyConsumed`. The registry cannot recall bytes that may already have reached Renderer memory and does not infer whether a failed write delivered a partial frame.
- This is a dedicated testable authorization gate, not yet the production Broker accept loop. Renderer process registration, Origin/Document grant issuance and teardown remain caller-owned; there is no PID-reuse supervisor lifecycle, request-ID replay cache, SCM_RIGHTS binding or live NavigationController ownership integration.
- FreeBSD verification: UDS 27/27 unit + 8/8 independent-process integration tests; the independent-process integration suite also passed 20/20 repeated runs; Broker policy 18/18; Browser Runtime 39/39; Boa 7/7; Headless 4/4; HTML Parser 10/10; V8 FFI 22/22; Network Policy 27/27; Origin Policy 17/17. Full workspace: **179/179 passed**. Rustfmt, Workspace Clippy (`-D warnings`), `git diff --check`, and `scripts/run-security-pocs.sh` passed.


## Incremental R&D record｜2026-11-01（Renderer Supervisor、request replay guard 与 V8 Snapshot ingestion）

### 目标

把此前 Browser Runtime → Broker → UDS → Renderer 的单次数据传输 PoC 推进到真实进程生命周期管理，并验证 Renderer 收到的 Snapshot 可以进入独立 V8 Runtime。同步复核阶段 0 既有供应链 CI 覆盖范围，避免新 workspace crate 未进入 SBOM。

### 实施内容

- 新增 `aibe-renderer-supervisor-poc` workspace crate。Supervisor 创建 owner-only UDS endpoint、启动 Renderer 子进程、读取 FreeBSD kernel peer PID/UID/GID，并将身份绑定到 Broker 签发的 RendererPrincipal；调用方不能自行提供 Principal。
- Supervisor 持有 Child 与已认证 UnixStream；accept 有限 deadline，连接有 5 秒读写 timeout。拒绝非目标 PID 的本地 peer，成功 accept 后移除 socket filesystem path。
- 增加 `poll_exits`、`terminate`、`shutdown` 生命周期路径。只有确认子进程退出后才释放 Storage Grant、Snapshot Grant 和请求历史；replacement Renderer 获得新的实例 ID，不能继承旧进程 grant。
- 增加每个 Renderer 的非零 request ID replay guard。默认全局上限 4096、每 Renderer 上限 256；配额可配置并拒绝零值/不合法组合。重复 ID 在新 grant 签发和第二帧写入之前拒绝。即使写入状态不确定，请求 ID 仍保留到 Renderer 退出，避免部分交付后的重放。
- 新增 `send_supervised_document_snapshot()`，将请求预留、Supervisor 持有的已认证连接、当前 Document Snapshot 和一次性 Broker grant 串成单一调用路径。
- Renderer 辅助进程现在在 FreeBSD 上创建独立 V8 Runtime / Origin / Document Realm，从 Snapshot 重建 generic native Node-wrapper graph，重建 parent/child 与 template-content 引用，验证节点数量、Document Element、树关系和 wrapper identity，然后执行 GC 并销毁 Realm。
- Snapshot 的 node type/name/value/attributes 目前作为 JavaScript-side metadata 保存；尚不是 typed native DOM storage、标准 Document/Element/Text 接口或 Web IDL。
- 复核发现此前 SBOM generator/validator 只列出九个 crate，遗漏 Snapshot Bridge 和 Renderer Supervisor。此前 PR #7 的 supply-chain job 虽然显示成功，但新生成的两个 BOM 未移动到上传目录，因此不代表完整 11-crate 覆盖。已修复 generator/validator，将预期集合扩展到全部 11 个 workspace crates。

### FreeBSD 15.1-RELEASE-p6 amd64 验证

- 完整 Workspace：**186/186 passed**。
- Snapshot Bridge：3/3 unit tests；Supervisor + Renderer 独立进程集成：2/2 passed。
- 集成测试覆盖真实 PID 绑定、请求 ID 重放拒绝、零 ID 拒绝、全局/per-Renderer 配额、进程退出 grant 清理、replacement instance、主动终止和 shutdown。
- Supervisor / Renderer 两个集成测试连续执行 20 轮：**40/40 executions passed**。
- Renderer 子进程实际运行 V8 Snapshot ingestion；V8 Runtime 创建、Document Realm 初始化、Node-wrapper graph 构造、身份检查、GC 与 Realm teardown 均在独立进程中执行并通过。
- Rustfmt：passed。
- Workspace Clippy（all targets/all features，`-D warnings`）：passed。
- `git diff --check`：passed。
- `scripts/run-security-pocs.sh`：passed，覆盖 Capsicum FD rights、Broker/Renderer IPC、UDS peer credentials、per-Origin storage capability、malformed SCM_RIGHTS FD cleanup、Renderer crash recovery、Jail IPv4 isolation。
- Pinned Test262 synchronous subset：V8 284/284 passed；SpiderMonkey 284/284 passed。该子集不代表完整 Test262 conformance。
- Live Network Policy：example.com HTTPS GET 200 / 577 bytes；httpbin redirect 302 → 200 / 240 bytes。
- Live fetch-to-parser pipeline：example.com 577 bytes，HTML Parser 输出 16 nodes、title `Example Domain`、0 parse errors。
- Cargo metadata：225 packages total、11 workspace crates、214 third-party packages；214 个第三方包均有 license metadata。该结果不是法律许可审查。

### GitHub / CI / 当前工作树

- 基线 PR #7：<https://github.com/lycupay-debug/autonomous-ai-browser-engine/pull/7>，分支 `work/renderer-supervisor-lifecycle`，commit `837ee6c9fa7111b44ea831307dfcee087617f984`。
- PR #7 原有 Rust 与 supply-chain checks 曾显示成功；复核确认其 SBOM generator/validator 只覆盖九个 crate，漏掉 Snapshot Bridge 与 Renderer Supervisor。该缺口已在当前工作树修复，但修复后的 CI 尚未运行。
- 当前分支工作树包含 Supervisor 异常路径收紧、request replay guard、V8 Snapshot ingestion、README/阶段文档及 11-crate SBOM inventory 修复；尚未提交或推送。
- 本轮仅在隔离 worktree 中编译、测试和运行临时 PoC；未部署服务、未启用 rc.d、未修改 sysctl/持久系统配置、未重启主机或生产服务。

### 结论与限制

- **已实施并在 FreeBSD 本机验证：** Renderer PID/UID/GID 与 Broker-issued instance identity 绑定；Supervisor 管理子进程、连接和 grant cleanup；请求重放与配额防护；真实独立 Renderer 进程完成 V8 Native Node-wrapper graph ingestion。
- **仍为 Phase 0 PoC：** 不是 production Broker daemon、多 Renderer 并发调度服务或完整 Renderer sandbox；没有对 Renderer 实施 Capsicum、Jail、rctl、独立低权限账户或完整 descriptor inheritance policy。
- **尚未实现：** typed native DOM node/attribute/text state、live DOM mutation、Web IDL、HTML Event Loop、SCM_RIGHTS 与 Snapshot grant 的统一授权、真实 Network Service → Parser → Document → Renderer 连续导航闭环、CSS/Layout/Paint、WPT 和真实网站交互测试。
- request replay guard 是当前 Supervisor 实例内存状态；不提供 Broker 重启后的持久幂等结果缓存。其 at-most-once 拒绝语义刻意不重试不确定的部分写入。
- 11-crate SBOM / cargo-audit 的更新后 CI 仍待执行；不能沿用旧 PR 的 supply-chain success 声称当前 lockfile 已完成审计。

### 下一步

1. 提交并推送当前修订，重新运行 Rust、cargo-audit 和 11-crate CycloneDX SBOM CI；核对上传 artifact 中确有全部 11 个 crate。
2. 增加 UDS peer mismatch、Renderer 在 Snapshot transfer 中退出、partial frame、并发 duplicate request、accept 异常连接和所有错误分支 cleanup 测试。
3. 将 generic Node metadata 迁移到 typed native node storage，补充属性、文本、Document/Element 语义和 live mutation。
4. 研究 Renderer 的真实 sandbox 配置：descriptor close-on-exec、Capsicum rights、Jail/resource limits、独立身份与 network broker；先在隔离实验中验证，不修改主机持久配置。
5. 完成 Snapshot / Renderer 生命周期与 Navigation Document teardown 的自动联动，再评估阶段 0 退出条件。


## Incremental R&D record｜2026-11-02（One Renderer per Jail lifecycle / identity / RCTL）

### 架构决策

- 项目负责人确认采用 **每个 Renderer 一个独立 FreeBSD Jail**。Supervisor 留在宿主机可信边界内，负责 Renderer 生命周期、Broker IPC、身份登记、权限授权和回收。
- 新增 ADR：`docs/adr/0002-renderer-isolation-one-jail-per-renderer.md`，状态 Accepted。FreeBSD 是当前原生目标；本决策不代表已经实现跨平台 Sandbox Provider。
- 生产配置禁止使用 `path=/`；每个 Renderer 必须有独立 rootfs。默认禁用 Jail 内 IPv4/IPv6 直连，网络访问经独立 Network Service/Broker 授权。
- `jail(8)` launcher PID 与 Jail 内 Renderer PID 不相同；身份登记必须用内核 `LOCAL_PEERCRED` 取得实际 peer PID/UID/GID，并进一步核对该 PID 的内核 Jail ID，不能把 launcher PID 当作 Renderer PID。

### 新增实现

- 新增 `experiments/security/renderer-jail-lifecycle-poc.c` 和 `scripts/run-renderer-jail-lifecycle-poc.sh`，并接入 `scripts/run-security-pocs.sh`。
- PoC 为每次运行创建独立临时 rootfs，仅放入静态测试 Renderer、独立 `/run` IPC 目录和临时目录；未把宿主根目录映射进 Jail。
- 宿主 Broker 在该 rootfs 内创建 UDS endpoint，将 socket owner 设置为 `nobody:nobody`、mode 0600；Jail 内 Renderer 以 UID/GID 65534 连接。监听 FD 设置 `FD_CLOEXEC`，子进程在连接前扫描 FD 3–1023，确认没有宿主监听 FD 或其他非预期描述符被继承。宿主通过 `LOCAL_PEERCRED` 读取 peer PID/UID/GID，再用 `KERN_PROC_PID` 读取该 PID 的内核 `ki_jid` / `ki_uid`，并与当前 Jail JID 对比。
- 创建仅作用于该 Jail 的临时 RCTL 规则 `vmemoryuse:deny=128M/jail`；256 MiB 映射返回 `ENOMEM`。使用 `rctl -hu jail:<name>` 读取该 Jail 的运行时资源快照。
- PoC 宿主控制器通过 `jail -r` 销毁 Jail；验证实际测试 Renderer PID 随 Jail 销毁退出。正常结束和错误清理均删除临时 rootfs、UDS、Jail 与 RCTL 规则。

### FreeBSD 15.1-RELEASE-p6 验证

- 单次生命周期测试通过：kernel peer PID/UID/GID 与实际进程一致；内核进程表报告该 PID 属于目标非零 Jail ID；UID/GID 为 65534；宿主专用 marker 在 Jail rootfs 内不可见。
- Jail-scoped `vmemoryuse` 负向测试通过：128 MiB 上限拒绝 256 MiB 匿名映射。
- Jail 销毁后实际测试 Renderer PID 已不存在；`jls` 无残留 Jail，`rctl` 无残留规则，临时 rootfs 无残留。
- UDS listener 的 `FD_CLOEXEC` 与子进程 FD 扫描通过；这是最小继承验证，不等于完整生产 FD allowlist。
- 最新实现连续运行 10 次通过；5 轮双实例并发（共 10 个独立 Renderer Jail）全部通过，未发生 Jail 名称、UDS、RCTL 规则或 rootfs 交叉污染。
- 完整 `scripts/run-security-pocs.sh`：通过，包括 Capsicum FD rights、Broker/Renderer IPC、UDS peer credentials、per-Origin storage、SCM_RIGHTS malformed-FD cleanup、Renderer crash recovery、Jail IPv4 isolation 和单 Renderer Jail lifecycle/identity/RCTL。
- 完整 Rust Workspace：**186/186 passed**；Rustfmt、Workspace Clippy（`-D warnings`）和 `git diff --check` 通过。V8 SDK 测试环境要求 `AIBE_RUNTIME_PREFIX` 保持 unset，`LD_LIBRARY_PATH=/tmp/aibe-runtime-root/usr/local/lib`。

### 结论与未完成边界

- **已验证：** FreeBSD Jail 可作为单 Renderer 的进程、文件系统和网络隔离单元；内核 peer 身份可以关联到精确 Jail；Jail 级 RCTL 生效；宿主监听 FD 不会经 exec 继承给 Renderer；销毁 Jail 可终止该测试进程。
- **当前仍是独立安全 PoC：** 尚未将 Jail launcher 接入 Rust `RendererSupervisor::spawn()` / `terminate()` / `poll_exits()`；尚未将真实 V8 Renderer runtime、动态库、字体与资源文件打包进专用 rootfs。
- 尚未验证 Jail + Capsicum 的组合顺序、完整 Renderer FD inheritance allowlist、Broker UDS 的生产挂载/路径策略、SCM_RIGHTS 统一授权、网络 Broker 的真实出站路径、Renderer 后代进程持续监督，以及 10/50/100 Renderer 的资源和启动开销。
- 本次 128 MiB 是负向测试阈值，不是生产内存预算；尚未为 Renderer 配置持久 RCTL 规则。GSM、PostgreSQL、root 用户和全局范围均未添加规则。
- 未创建持久系统账户、未修改 `/etc/jail.conf`、`/etc/rc.conf`、`/etc/rctl.conf`、`/boot/loader.conf`；未重启主机或任何服务；未部署生产 Renderer。

### 下一子阶段

1. 在 Rust Renderer Supervisor 中实现 FreeBSD Jail launcher adapter：独立 rootfs、唯一 Jail 名称、低权限 UID/GID、Broker UDS path、内核 PID/JID 验证、Jail teardown 与 grant cleanup。
2. 定义真实 Renderer runtime rootfs 组装策略，确保系统运行库只读、每个 Renderer 的 `/tmp` 与 `/run` 独立，且不向 Jail 暴露宿主敏感目录。
3. 将 Capsicum FD rights / close-on-exec allowlist 与 Jail IPC 组合，完成正向及负向测试。
4. 在实际 V8 Renderer 负载下采集资源峰值，再制定每 Jail 的 RCTL 限额；GSM 与 PostgreSQL 始终保持在限制范围之外。
