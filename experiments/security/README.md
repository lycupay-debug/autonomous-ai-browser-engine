# FreeBSD security feasibility experiments

These tests use temporary processes, socketpairs, directories, and Jail roots. They do not modify `/etc/jail.conf`, `/etc/rc.conf`, `/boot/loader.conf`, persistent system accounts, or persistent service configuration.

## Capsicum descriptor rights

`capsicum-minimal/src/main.c` prepares an unlinked temporary file, forks a child, reduces the inherited descriptor to `CAP_READ | CAP_FSTAT`, and enters capability mode. Reading remains allowed; writing returns `ENOTCAPABLE`; path-based `/etc/passwd` open returns `ECAPMODE`.

## Broker/Renderer process boundary

`broker-renderer-capsicum-poc.c` forks a renderer over a Unix socketpair. The renderer closes all other inherited descriptors, verifies that no descriptor remains above its IPC FD, restricts stdin to `CAP_READ` and stdout/stderr to `CAP_WRITE`, limits the IPC channel to `CAP_READ | CAP_WRITE | CAP_FSTAT`, enters capability mode, and responds to a broker request. Path-based open is denied. An IPv4 socket can still be created in capability mode, but removing `CAP_CONNECT` from its rights makes outbound `connect()` fail with `ENOTCAPABLE`. Do not claim that `cap_enter()` alone blocks socket creation.

## Per-origin storage descriptor isolation

`origin-storage-capsicum-poc.c` creates two temporary origin storage directories and a broker/renderer Unix socketpair. The child closes inherited Origin A and Origin B directory FDs before receiving a single Origin A directory FD through `SCM_RIGHTS`; the broker closes its copies after transfer. The renderer restricts stdin/stdout/stderr and IPC rights, applies `CAP_LOOKUP | CAP_READ | CAP_FSTAT` to the delegated storage FD, then enters capability mode. It can read Origin A, cannot traverse to Origin B, and cannot create a file because write/create rights were not delegated. A negative test sends two descriptors in one ancillary message; the receiver rejects the message and verifies both installed descriptors were closed. The renderer confirms readiness over IPC and the broker waits for its exit. This tests descriptor delegation and an OS capability boundary, not the complete web Origin model, cookie/storage semantics, or production storage authorization.

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
