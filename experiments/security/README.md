# FreeBSD security feasibility experiments

These tests use temporary processes, socketpairs, directories, and Jail roots. They do not modify `/etc/jail.conf`, `/etc/rc.conf`, `/boot/loader.conf`, persistent system accounts, or persistent service configuration.

## Capsicum descriptor rights

`capsicum-minimal/src/main.c` prepares an unlinked temporary file, forks a child, reduces the inherited descriptor to `CAP_READ | CAP_FSTAT`, and enters capability mode. Reading remains allowed; writing returns `ENOTCAPABLE`; path-based `/etc/passwd` open returns `ECAPMODE`.

## Broker/Renderer process boundary

`broker-renderer-capsicum-poc.c` forks a renderer over a Unix socketpair. The renderer closes all other inherited descriptors, verifies that no descriptor remains above its IPC FD, restricts stdin to `CAP_READ` and stdout/stderr to `CAP_WRITE`, limits the IPC channel to `CAP_READ | CAP_WRITE | CAP_FSTAT`, enters capability mode, and responds to a broker request. Path-based open is denied. An IPv4 socket can still be created in capability mode, but removing `CAP_CONNECT` from its rights makes outbound `connect()` fail with `ENOTCAPABLE`. Do not claim that `cap_enter()` alone blocks socket creation.

## Per-origin storage descriptor isolation

`origin-storage-capsicum-poc.c` creates two temporary origin storage directories and a broker/renderer `SOCK_SEQPACKET` socketpair. Before granting storage, the broker checks the connected renderer socket with `getpeereid()` and requires exact effective UID/GID equality with its local test policy. The broker sends one `SCM_RIGHTS` descriptor with a fixed-size 48-byte host-local C ABI grant envelope containing a magic value, protocol version, descriptor count, broker-tracked renderer PID, read-only rights mask, Renderer instance ID, registry ID, grant sequence, and host-issued Origin Group ID. The Rust broker-policy crate mirrors the C field order and verifies the size and field offsets. The renderer validates the envelope against its own PID and expected Origin/grant before accepting the descriptor. The broker closes its descriptor copies after transfer. The renderer applies `CAP_LOOKUP | CAP_READ | CAP_FSTAT`, enters capability mode, reads Origin A, and verifies Origin B traversal and write/create attempts are denied. After the broker closes its own copy, the renderer successfully reads Origin A again through its delegated descriptor. The broker then terminates the renderer with `SIGKILL` and verifies process exit; this demonstrates that closing the broker copy does not revoke the renderer's FD, while process termination removes the process-held capability. Negative tests cover multiple descriptors, a mismatched Origin Group, renderer PID, Renderer instance ID, and grant registry ID; rejected installed descriptors are verified closed. The PID field is bound by the broker-created socketpair/fork relationship; it is not a kernel-reported peer PID or a standalone authentication mechanism. This is a local protocol feasibility proof, not the complete web Origin model, cookie/storage semantics, revocation registry, multi-renderer policy, or production storage authorization.

## Renderer crash recovery

`renderer-supervisor-poc.c` kills a child renderer with `SIGKILL`, verifies the supervisor observes the signal, starts a replacement process, and completes a health IPC exchange. This is a basic process-supervision proof, not production recovery with state restoration, backoff, or compromised-process forensics.

## Jail network isolation

`jail-network-probe.c` connects to a host-local IPv4 TCP listener from the host and from a temporary Jail configured with `ip4=disable` and `ip6=disable`. Host baseline succeeds; the jail cannot create an IPv4 socket (`EPROTONOSUPPORT`, errno 43).

## UDS peer UID/GID authentication

`uds-peer-credentials-poc.c` drops a child to the existing `nobody` account, connects to a temporary UDS listener, and verifies `getpeereid()` returns the child's UID/GID (65534/65534). The Rust UDS PoC also exposes a FreeBSD-only `peer_credentials()` and `authorize_peer_uid()` function, checks the accepted peer UID in its FreeBSD test server, and has a kernel-credential integration test.

The Rust protocol test server additionally bounds a request to 64 KiB, applies read/write timeouts, rejects an existing filesystem entry rather than unlinking it, and tests oversized input rejection. The authenticated accept helper is wired into the FreeBSD test server, but the protocol/state-machine implementation remains test-only and is not a production Broker.

## Resource limits (rctl)

The host currently reports `kern.racct.enable=0`. Consequently, rctl enforcement cannot be meaningfully tested without changing a kernel runtime setting. No sysctl or persistent system configuration was changed. This remains an explicit Phase 0 limitation.

## Run all security PoCs

```sh
./scripts/run-security-pocs.sh
```

These experiments do not establish a complete browser Renderer Sandbox. Cross-Origin process policy, actual page-origin assignment, network broker enforcement, `rctl`, hostile IPC payload handling under load, privilege transitions, and production crash recovery remain separate gates.

## Broker grant lifecycle and quota policy model

`experiments/broker-policy/` is a dependency-free Rust policy prototype. It binds one-shot read-only storage grants to a Broker-issued Renderer instance ID, PID/UID/GID tuple, and host-issued Origin Group; rejects grant replay, cross-renderer/PID-reuse attempts, cross-Origin requests, and rights escalation; and enforces global and per-renderer active-grant quotas. Grant IDs are scoped to one in-memory registry. A delegated-grant revocation enters `RevocationPending`, blocks further transfers, and requires the caller to terminate the Renderer; quota and grant records are released only after the supervisor confirms process exit. This model does not authenticate kernel peer credentials, pass file descriptors, terminate processes, persist audit state, or constitute a production Broker.
