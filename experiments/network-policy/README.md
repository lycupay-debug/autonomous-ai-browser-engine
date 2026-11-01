# Outbound IP policy and pinned HTTPS PoC

The Rust policy helper rejects private, loopback, link-local, carrier-grade NAT, documentation, multicast, reserved, IPv4-mapped-private, and selected special IPv6 ranges. It rejects an entire DNS answer set if any returned address is disallowed and deduplicates accepted addresses.

The HTTPS probe now:

1. Resolves a hostname once and validates the complete A/AAAA answer set.
2. Connects to an exact validated `SocketAddr` with `TcpStream::connect_timeout`; it does not pass the hostname to a second resolver in the connector.
3. Preserves the original URL hostname for TLS SNI and certificate-name verification using the FreeBSD system CA bundle.
4. Applies connect/read/write timeouts, reads each HTTP line through a bounded buffer (8 KiB per line, 64 KiB total, 100 headers), rejects malformed CRLF, control characters and unsupported informational responses, then follows only a small set of HTTPS redirects.
5. Repeats DNS resolution, address validation, pinned TCP connection, and TLS verification for every redirect hop. HTTP downgrade redirects and duplicate/missing Location headers are rejected.

The live FreeBSD probes reached `https://example.com/` (HTTP 200) and followed `https://httpbin.org/redirect/1` to `https://httpbin.org/get` (302 then 200). The returned peer addresses are printed so the actual pinned endpoint is observable. Unit tests also verify loopback/private targets are denied before a TLS connection is attempted.

This is not a complete Fetch/Network Service. It currently performs GET and reads response headers only; body streaming, HTTP/2, proxy policy, cache, cookies, service workers, WebSockets, WebRTC, client certificates, DNS rebinding under an adversarial resolver, and private-network profiles remain future work. The bounded line parser and socket timeouts reduce resource-exhaustion exposure in this PoC but are not a substitute for a production HTTP parser or resolver deadline. The system CA path is supplied by the caller; production CA lifecycle and trust policy need an explicit design.

Run offline/local policy tests:

```sh
./scripts/run-network-poc.sh
```

Run opt-in external network tests:

```sh
AIBE_RUN_NETWORK_INTEGRATION=1 ./scripts/run-network-poc.sh
```
