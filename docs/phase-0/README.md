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
