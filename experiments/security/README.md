# FreeBSD security feasibility experiments

## Minimal Capsicum descriptor-rights test

`capsicum-minimal/src/main.c` prepares an unlinked temporary file, forks a child, reduces the inherited descriptor to `CAP_READ` and `CAP_FSTAT`, and enters Capsicum capability mode.

- Reading from the inherited descriptor remains allowed.
- Writing through the descriptor is denied with `ENOTCAPABLE`.
- Opening `/etc/passwd` by pathname is denied with `ECAPMODE`.

## Broker/Renderer process-boundary prototype

`broker-renderer-capsicum-poc.c` forks a restricted renderer process over a Unix socketpair. The renderer closes all inherited descriptors except the IPC channel, limits the channel to read/write/stat rights, enters Capsicum capability mode, and answers a broker request. It then verifies that path-based file open is denied with `ECAPMODE` and that an IPv4 socket cannot connect when `CAP_CONNECT` is omitted from that socket's rights. Socket creation itself remains permitted in capability mode on this host.

Important observation: `socket(AF_INET, ...)` itself is allowed in capability mode on this host. The outbound connection is denied by descriptor-rights restriction. The test therefore does not claim that `cap_enter()` alone blocks socket creation.

## Jail network negative test

`jail-network-probe.c` tests the same host-local IPv4 TCP listener from the host and from a temporary jail configured with `ip4=disable` and `ip6=disable`. The host connection succeeds; inside the jail, IPv4 socket creation is denied with `EPROTONOSUPPORT` (errno 43).

Run the repeatable test:

```sh
./scripts/run-jail-network-poc.sh
```

All experiments use temporary files, processes, and Jail roots. They do not modify `/etc/jail.conf`, `/etc/rc.conf`, `/boot/loader.conf`, or persistent system configuration.

These tests are narrow primitives, not a complete browser renderer sandbox. They do not test IPC peer UID/GID authentication, rctl, compromised payloads, cross-Origin/Storage isolation, renderer crash recovery, or a full Broker/Renderer service architecture.

## UDS peer UID/GID authentication primitive

`uds-peer-credentials-poc.c` creates a temporary UDS listener with mode 0666, forks a child, drops the child to the `nobody` account, and has it connect. The broker calls FreeBSD `getpeereid()` on the accepted connection and verifies the peer UID/GID match the dropped child. The test is run as root and does not create or modify any system account.

Run all security PoCs:

```sh
./scripts/run-security-pocs.sh
```
