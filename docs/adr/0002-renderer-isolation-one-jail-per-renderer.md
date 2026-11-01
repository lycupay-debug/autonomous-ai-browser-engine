# ADR 0002: One FreeBSD Jail per Renderer

- Status: Accepted
- Decision date: 2026-11-02
- Scope: Phase 0 Renderer isolation architecture
- Platform: FreeBSD native target

## Context

The Renderer executes untrusted website-controlled HTML, CSS, JavaScript and related content. The existing host-owned Renderer Supervisor binds a Renderer principal to kernel-authenticated PID/UID/GID credentials and owns its UDS connection and process lifecycle, but it does not yet place the Renderer in a Jail. A temporary Jail/RCTL proof has shown that Jail-scoped process and virtual-memory limits can be enforced.

The project architecture identifies FreeBSD as the first native target. This decision does not introduce a cross-platform sandbox abstraction or claim Linux, Windows or macOS support.

## Decision

Each Renderer process instance receives its own dedicated FreeBSD Jail. The host-side Browser/Renderer Supervisor remains outside the Jail and is the trusted lifecycle owner. The Renderer is launched with a non-root identity inside its Jail.

The initial Jail boundary must:

1. Use a dedicated root filesystem for that Renderer instance; never use `path=/` as a production configuration.
2. Disable direct IPv4 and IPv6 networking by default. Network access must be mediated by a separately authorized Network Service/Broker path.
3. Expose only the specific Broker IPC endpoint required by that Renderer. The socket's filesystem ownership/mode and kernel-authenticated peer PID/UID/GID must be checked. The host listener and all unrelated descriptors must be close-on-exec; the Renderer receives only its intended IPC endpoint.
4. Verify the connecting process belongs to the exact expected Jail before registering its RendererPrincipal. A PID match alone is insufficient for a Jail-launched process because `jail(8)` is the launcher parent.
5. Apply RCTL only to the individual Renderer Jail after representative workload measurements. No `user:root`, `user:postgres`, global or GSM-scoped limits are permitted.
6. Destroy the Jail as the unit of termination and cleanup, including any surviving descendant processes; then release the Renderer-bound grants and request history.
7. Keep Capsicum descriptor rights, FD inheritance allowlisting, and SCM_RIGHTS authorization as additional controls. Jail does not replace them.

## Consequences

### Benefits

- Per-Renderer filesystem, network and process namespace boundary.
- Per-Renderer resource accounting and independently enforceable RCTL budgets.
- Jail destruction provides a clear containment and cleanup unit for a Renderer process tree.
- Kernel-reported Jail membership strengthens process identity verification beyond PID/UID/GID alone.

### Costs and constraints

- Every Renderer needs a prepared, isolated root filesystem and a controlled runtime/library layout.
- Broker UDS access requires a deliberate socket path and ownership arrangement inside the Jail root.
- The Supervisor must track both the `jail(8)` launcher and the actual Renderer PID/JID; they are not the same PID.
- Jail creation, rootfs preparation, mount lifecycle, IPC and teardown add latency and operational complexity.
- Jails share the host kernel; they are not virtual machines and do not eliminate kernel attack-surface risk.
- Per-Jail RCTL limits must be based on measured Renderer workloads; the current PoC limits are only negative-test thresholds.

## Validation status

- Verified on FreeBSD 15.1-RELEASE-p6: one low-privilege process connects through a UDS located inside a dedicated temporary rootfs; `LOCAL_PEERCRED` reports PID/UID/GID; `KERN_PROC_PID` reports the same PID and the expected nonzero Jail ID; the listener is `FD_CLOEXEC` and the child verifies that no unexpected descriptors were inherited; a host-only marker is not visible; Jail-scoped `vmemoryuse` and `maxproc` rules deny a 256 MiB mapping at a 128 MiB limit and a second fork at a two-process limit; destroying the Jail terminates the Renderer process.
- Repeated and concurrent execution evidence is recorded in `docs/phase-0/README.md`.
- This is an isolated C lifecycle PoC, not yet integration into the Rust `RendererSupervisor`, not the production V8 Renderer runtime rootfs, and not the complete Renderer Sandbox.

## Alternatives considered

- One shared Jail for all Renderers: rejected because it weakens per-Renderer resource accounting, lifecycle cleanup and compromise containment.
- Host process plus Capsicum only: retained as a defense-in-depth layer, but not selected as the sole isolation boundary.
- One VM per Renderer: stronger kernel boundary but disproportionate startup/resource cost for the current Phase 0 target.
- `path=/` Jail: rejected for production because it does not provide a dedicated filesystem root.


## Implementation update｜2026-11-02

- The Rust `RendererSupervisor::spawn_in_jail()` adapter now creates a persistent Jail, mounts a dedicated minimal devfs ruleset, applies optional Jail-scoped RCTL deny limits **before** starting the Renderer with `jexec`, and authenticates the UDS peer against kernel PID/UID/GID/JID/PPID and process start time.
- The Supervisor owns Jail termination, RCTL rule removal, devfs unmount/ruleset deletion, grant cleanup and optional per-instance rootfs deletion. The unisolated host launcher has been renamed `spawn_unisolated_for_test` and is not the production launch path.
- `scripts/build-renderer-rootfs.sh` assembles a root-owned rootfs with the Renderer binary, resolved system/V8 shared libraries, private `/run`, per-instance sticky `/tmp`, and a minimal `/dev` mount supplied by the Supervisor. Runtime library files are shared only as read-only hardlinks where safe; immutable base-system files are copied with flags cleared on the copies, never on host source files.
- `snapshot-bridge-renderer` initializes V8 before entering Capsicum, then parses the Snapshot frame and evaluates the page-controlled tree only after `cap_enter()` has succeeded.
- FreeBSD 15.1-RELEASE-p6 integration tests launched two real V8 Renderers concurrently in two distinct Jails; both processed a DOM Snapshot under Capsicum and exited through Supervisor cleanup. Jail IPv4/IPv6 disablement, restricted devfs, descriptor inheritance, Jail-scoped `maxproc`, rootfs validation, process-tree termination and grant cleanup were also verified.
- No production RCTL memory/swap budget is set. A short two-Renderer sample showed approximately `memoryuse=35M`, `RSS=35,740 KB`, `vmemoryuse=1444G`, `swapuse≈1483–1500G`, `openfiles=40`, `maxproc=1`, `nthr=2`. This is a short sample, not a peak. V8's very large virtual address reservation makes `vmemoryuse`/`swapuse` unsuitable as direct physical-memory budgets; representative stress measurements are still required.

## Remaining work

- Complete the Renderer FD rights allowlist and SCM_RIGHTS capability authorization; the current Capsicum integration enters capability mode but does not yet define the final per-descriptor rights policy.
- Integrate the production Broker request protocol and mediated network service with the jailed Renderer.
- Measure 10/50/100 concurrent V8 Renderers, startup/teardown latency, failure recovery, physical memory peaks and virtual-address accounting before setting production RCTL limits.
- The current V8 Renderer remains a Snapshot bridge PoC, not the complete browser Renderer implementation. No production deployment or persistent host resource rules have been made.
