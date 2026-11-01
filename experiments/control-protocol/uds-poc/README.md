# UDS control protocol feasibility prototype

The existing control RPC remains newline-delimited JSON with 64 KiB request/response limits. Immutable DOM Snapshot payloads use a separate binary framing codec in `snapshot_frame` so the small control RPC limit is not silently expanded for every method.

## DOM Snapshot frame V1

The transport header is exactly 20 bytes, all integers big-endian:

| Offset | Bytes | Field |
|---:|---:|---|
| 0 | 4 | Magic `AIBS` |
| 4 | 2 | Transport protocol version (`1`) |
| 6 | 2 | Message type (`1` = DOM Snapshot) |
| 8 | 8 | Nonzero request ID |
| 16 | 4 | Payload length |

The payload is the Browser Runtime JSON envelope (`envelopeVersion: 1`) containing decimal-string `documentId`, decimal-string `revision`, and the parser tree snapshot (`schemaVersion: 1`). The payload is limited to 8 MiB. The decoder validates the envelope, node count, unique node IDs, parent/child consistency, template-content references and acyclic parent links before returning a frame.

The codec rejects bad magic, unsupported versions/types, zero IDs, truncated frames, malformed JSON, unknown node references, graph inconsistencies and payloads over the hard cap. It checks the declared payload length before allocating the payload buffer.

## Security boundary

The frame codec is serialization/framing only. It does **not** authenticate a socket peer, authorize a Document or Origin, validate a Renderer grant, or transfer file descriptors. Callers must first authenticate the UDS peer using the FreeBSD `getpeereid()` path and then enforce Renderer-instance / Origin / Document authorization. The current codec is not connected to the test server's newline JSON handler and is not a production Broker protocol.

Callers must also set finite read/write timeouts on the UnixStream. The generic Read/Write codec cannot impose socket deadlines itself.

## Validation

`cargo test -p aibe-control-protocol-uds-poc --locked` covers a UnixStream round-trip, exact u64 identity preservation, malformed headers, unsupported versions/types, truncated payloads, invalid envelopes, invalid node graphs and size limits.



## Broker-side Snapshot authorization gate (FreeBSD PoC)

`snapshot_endpoint::send_authorized_snapshot()` is a dedicated Broker-to-Renderer authorization gate, separate from the existing newline-delimited JSON task RPC:

1. The Broker accepts or holds a connected Renderer socket. FreeBSD `LOCAL_PEERCRED` (`xucred.cr_pid`, effective UID/GID) identifies the connected peer; UID/GID are cross-checked with `getpeereid()`.
2. Kernel PID/UID/GID must exactly match the Broker-registered `RendererPrincipal`. The Renderer instance ID is host-issued and is not accepted from the message payload.
3. The Broker validates that the frame's explicit Document ID/revision match the embedded JSON envelope.
4. A Broker-issued one-shot `SnapshotGrantRegistry` grant must match Renderer instance, Origin Group, Document ID and exact revision.
5. Only after all checks pass is the bounded Snapshot frame written to the Renderer socket. The grant is consumed before writing so a partial write cannot be retried with the same grant.

The snapshot grant registry has global and per-Renderer quotas, rejects replay, wrong Renderer instance, wrong Origin, wrong Document and stale revision, and supports grant revocation before use / Renderer cleanup. Revoking a grant already consumed for a send attempt is reported as `AlreadyConsumed`; the registry cannot recall bytes that may already have reached Renderer memory, and it does not infer whether a failed write delivered a partial frame. These grants authorize one immutable snapshot transfer; they are distinct from storage FD grants and do not transfer file descriptors.

This remains a Phase 0 PoC, not the production Broker server loop. Process registration and Document/Origin grant issuance are supplied by trusted host-side callers; there is no full Renderer supervisor, PID-reuse lifecycle management, request-ID replay cache, SCM_RIGHTS association, or live NavigationController ownership integration yet. The caller must keep Renderer PID/instance registrations current and release grants when a Renderer exits.
