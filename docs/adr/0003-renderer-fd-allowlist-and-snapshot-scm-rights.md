# ADR 0003: Renderer FD Allowlist and Broker-Authorized Snapshot SCM_RIGHTS

- Status: Accepted
- Decision date: 2026-11-02
- Scope: Phase 0 Renderer isolation and Snapshot transport
- Platform: FreeBSD native target

## Context

The Renderer already starts in a dedicated Jail, initializes the V8 runtime and host objects, then enters Capsicum capability mode. Previously, the process retained its inherited descriptor table without a complete per-descriptor rights policy, and Browser Snapshot payloads were sent as bytes over the authenticated UDS stream. A separate C storage PoC demonstrated SCM_RIGHTS descriptor passing, but the Snapshot grant registry was not connected to descriptor transfer.

## Decision

### Renderer descriptor allowlist

Immediately before `cap_enter()`, the Renderer normalizes its authenticated Broker UDS to descriptor 3, closes every descriptor above 3 with `closefrom(4)`, and applies exact kernel-verified Capsicum rights:

| FD | Purpose | Allowed rights |
|---|---|---|
| 0 | Standard input | `CAP_READ` |
| 1 | Standard output | `CAP_WRITE` |
| 2 | Standard error | `CAP_WRITE` |
| 3 | Authenticated Broker UDS | `CAP_READ | CAP_WRITE` |
| 4+ | No ambient descriptor authority | Closed |

No `CAP_IOCTL`, `CAP_FCNTL`, `CAP_CONNECT`, `CAP_ACCEPT`, directory lookup, or path authority is granted to the Broker socket. The policy is applied after runtime initialization and before capability mode; new descriptors received later must carry an explicit Broker grant and receive their own rights limit.

### Snapshot descriptor transfer

- The Broker authenticates the connected peer using kernel `LOCAL_PEERCRED` PID/UID/GID and matches it to the Supervisor-owned `RendererPrincipal`.
- The Broker validates the immutable Snapshot payload and consumes a one-shot `SnapshotGrantRegistry` grant bound to Renderer instance, Origin Group, Document ID and exact revision.
- The Broker writes the bounded payload into a FreeBSD `SHM_ANON` object, truncates it to the exact payload length, rewinds it, sets mode 0400, and restricts the descriptor to `CAP_READ | CAP_FSTAT`.
- The Broker sends a fixed 104-byte little-endian grant envelope plus exactly one descriptor with `SCM_RIGHTS` over the authenticated UDS stream. The envelope binds request ID, Renderer instance/PID/UID/GID, Origin registry/group, grant registry/sequence, Document ID/revision and payload length.
- The Renderer receives the ancillary descriptor with `MSG_CMSG_CLOEXEC`, requires exactly one SCM_RIGHTS descriptor and no truncation/unknown ancillary data, checks the envelope against its process identity, verifies a regular read-only object and exact size, applies and verifies read-only Capsicum rights, proves writes are denied, reads the payload, then validates Document ID/revision and DOM Snapshot schema.
- The old byte-frame path remains as a codec/compatibility test path; supervised FreeBSD Renderer Snapshot delivery uses the descriptor grant path.

## Consequences

### Benefits

- The Renderer starts with a small, explicit descriptor authority surface.
- Large Snapshot payloads are transferred through a read-only descriptor rather than copied into the UDS control stream.
- Descriptor transfer is tied to kernel peer authentication and the existing one-shot Origin/Document/Renderer grant registry.
- The descriptor is anonymous; the Renderer does not need pathname access after `cap_enter()`.
- Malformed or overbroad descriptor messages are rejected and every received descriptor is closed on rejection.

### Constraints

- Capsicum rights are monotonic. A descriptor cannot gain rights after restriction.
- Closing the Broker's local descriptor does not revoke the Renderer copy. Renderer termination remains the revocation boundary for an already delegated descriptor.
- This is a FreeBSD-specific transport implementation; it does not imply equivalent Linux/Windows/macOS support.
- The descriptor grant authorizes immutable Snapshot bytes only. It is not a general filesystem or storage grant.

## Validation status

Verified on FreeBSD 15.1-RELEASE-p6:

- Renderer FD table is normalized to FD 3; `closefrom(4)` removes all other inherited descriptors; the kernel-reported rights are read back and compared exactly.
- Two real V8 Snapshot Renderers concurrently run in separate Jails, enter Capsicum, receive their Snapshot through SCM_RIGHTS, validate the grant envelope and read-only anonymous descriptor, and complete V8 DOM Snapshot ingestion.
- Positive and negative tests cover exact one-FD transfer, wrong Renderer identity, multiple descriptors, invalid envelope, wrong Origin, mismatched frame identity, one-shot replay rejection, descriptor cleanup, read-only rights and payload/document/revision binding.
- Full Rust workspace tests, Rustfmt, Clippy with `-D warnings`, `git diff --check`, and the full security PoC script pass.

## Not completed by this decision

- Production Network Service/Broker egress and real website navigation.
- 10/50/100 Renderer concurrency and sustained resource peak measurements.
- Production RCTL memory/swap budgets.
- Full typed DOM/Web IDL/live mutation and CSS/Layout/Paint pipeline.
