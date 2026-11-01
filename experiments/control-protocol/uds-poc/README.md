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
