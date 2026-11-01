# Native AI Control Protocol / UDS PoC

A newline-delimited JSON envelope with `protocol_version`, `request_id`, `method`, and `params` is exercised over a Unix Domain Socket. The socket path is set to mode `0600`. On FreeBSD, the test server accepts peers through kernel-authenticated `getpeereid()` effective UID and GID authorization; non-FreeBSD builds return `Unsupported` rather than trusting request-supplied identity.

The protocol boundary now enforces:
- 64 KiB maximum request and response frames, including the newline delimiter.
- Bounded request serialization on the client, bounded incremental reads on both sides, and mandatory newline termination.
- Five-second stream read/write timeouts on client and server.
- JSON object top-level validation; bounded non-empty string/number request IDs; bounded method and task identifiers.
- Invalid JSON, malformed request shapes, oversized frames, truncated frames, and oversized broker responses are rejected.
- Socket write failure after client disconnect is returned as an I/O error rather than panicking the handler.
- The bind helper refuses to unlink or replace an existing filesystem entry.

This remains a test-only protocol/state-machine PoC, not a production broker. It does not yet use Capsicum in the Rust protocol process, execute browser work, enforce the complete capability policy, implement durable task state, replay/idempotency handling, cancellation tokens, or concurrent request scheduling. The FreeBSD test server checks both effective UID and effective GID from kernel peer credentials; this is not yet connected to a production authorization/capability policy.

Run:

```sh
cargo test -p aibe-control-protocol-uds-poc --locked
```
