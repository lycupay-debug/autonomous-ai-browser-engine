# Outbound IP policy PoC

The Rust policy helper rejects private, loopback, link-local, carrier-grade NAT, documentation, multicast, reserved, IPv4-mapped-private, and selected special IPv6 ranges. It rejects an entire DNS answer set if any returned address is disallowed, and deduplicates accepted addresses.

This is only an address-classification layer. A production Network Service must resolve once, validate every A/AAAA answer, connect to the exact validated `SocketAddr` (not re-resolve the hostname in the HTTP client), preserve the original hostname for TLS SNI/certificate validation, and repeat resolution/policy checks for every redirect and subresource. Proxy settings, DNS rebinding, IPv6 transition mechanisms, service workers, WebSockets, WebRTC, and local network access require separate controls and tests.

Run:

```sh
cargo test -p aibe-network-policy-poc --locked
```
