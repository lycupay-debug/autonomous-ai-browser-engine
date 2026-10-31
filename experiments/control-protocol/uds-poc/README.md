# Native AI Control Protocol / UDS PoC

A newline-delimited JSON envelope with `protocol_version`, `request_id`, `method`, and `params` is exercised over a Unix Domain Socket. The socket path is set to mode `0600`. Contract tests cover health, version rejection, request-ID preservation, task creation, task lookup, queued cancellation, running cancellation requests, and terminal task-state preservation.

This is not a production broker. It does not authenticate peer UID/GID, use Capsicum, implement request deadlines or cancellation tokens, execute browser work, enforce capabilities, or protect against malformed/oversized input. The test-only server logic is intentionally not exposed as a deployable service.

Run:

```sh
cargo test -p aibe-control-protocol-uds-poc --locked
```
