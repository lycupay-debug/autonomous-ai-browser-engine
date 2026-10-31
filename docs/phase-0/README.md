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
