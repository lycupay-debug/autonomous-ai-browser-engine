//! Experimental outbound policy and pinned HTTPS header probe.
//!
//! DNS answers are validated as a complete set, then TCP connects to the exact
//! validated SocketAddr rather than resolving the hostname again. TLS verifies
//! the original hostname (SNI/certificate name). Every redirect starts a new
//! resolve/validate/pinned-connect cycle. This is still a narrow HTTPS PoC, not
//! a complete Fetch implementation or a production network broker.

use std::io::{BufRead, BufReader, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use ipnet::IpNet;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use url::Url;

static BLOCKED_V4: OnceLock<Vec<IpNet>> = OnceLock::new();
static BLOCKED_V6: OnceLock<Vec<IpNet>> = OnceLock::new();
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_REDIRECTS: usize = 5;
const MAX_HEADER_BYTES: usize = 64 * 1024;

fn networks(cache: &'static OnceLock<Vec<IpNet>>, cidrs: &[&str]) -> &'static [IpNet] {
    cache
        .get_or_init(|| {
            cidrs
                .iter()
                .map(|cidr| cidr.parse().expect("static CIDR is valid"))
                .collect()
        })
        .as_slice()
}

fn blocked_v4() -> &'static [IpNet] {
    networks(
        &BLOCKED_V4,
        &[
            "0.0.0.0/8",
            "10.0.0.0/8",
            "100.64.0.0/10",
            "127.0.0.0/8",
            "169.254.0.0/16",
            "172.16.0.0/12",
            "192.0.0.0/24",
            "192.0.2.0/24",
            "192.88.99.0/24",
            "192.168.0.0/16",
            "198.18.0.0/15",
            "198.51.100.0/24",
            "203.0.113.0/24",
            "224.0.0.0/4",
            "240.0.0.0/4",
        ],
    )
}

fn blocked_v6() -> &'static [IpNet] {
    networks(
        &BLOCKED_V6,
        &[
            "::/128",
            "::1/128",
            "::ffff:0:0/96",
            "64:ff9b::/96",
            "64:ff9b:1::/48",
            "100::/64",
            "2001::/23",
            "2001:db8::/32",
            "2002::/16",
            "fc00::/7",
            "fe80::/10",
            "ff00::/8",
        ],
    )
}

/// Return true only for public unicast addresses permitted by this PoC policy.
pub fn is_allowed_public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_allowed_v4(address),
        IpAddr::V6(address) => is_allowed_v6(address),
    }
}

fn is_allowed_v4(address: Ipv4Addr) -> bool {
    !address.is_unspecified()
        && !address.is_loopback()
        && !address.is_private()
        && !address.is_link_local()
        && !address.is_multicast()
        && !address.is_broadcast()
        && !blocked_v4()
            .iter()
            .any(|network| network.contains(&IpAddr::V4(address)))
}

fn is_allowed_v6(address: Ipv6Addr) -> bool {
    if let Some(mapped) = address.to_ipv4_mapped() {
        return is_allowed_v4(mapped);
    }
    let global_unicast = "2000::/3"
        .parse::<IpNet>()
        .expect("static global-unicast prefix is valid")
        .contains(&IpAddr::V6(address));
    global_unicast
        && !blocked_v6()
            .iter()
            .any(|network| network.contains(&IpAddr::V6(address)))
}

/// Validate a complete DNS answer set. Mixed public/private answers are rejected.
pub fn validate_resolution(addresses: &[IpAddr]) -> Result<Vec<IpAddr>, IpAddr> {
    if addresses.is_empty() {
        return Err(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    }
    let mut validated = Vec::with_capacity(addresses.len());
    for address in addresses {
        if !is_allowed_public_ip(*address) {
            return Err(*address);
        }
        if !validated.contains(address) {
            validated.push(*address);
        }
    }
    Ok(validated)
}

#[derive(Debug)]
pub enum FetchError {
    InvalidUrl(String),
    UnsupportedScheme(String),
    CredentialsNotAllowed,
    Dns(std::io::Error),
    AddressDenied(IpAddr),
    NoAddresses,
    Connect(std::io::Error),
    Tls(String),
    Http(std::io::Error),
    InvalidResponse(String),
    RedirectLimit,
    RedirectDowngrade,
    InvalidRedirect(String),
    CaBundle(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for FetchError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchHeadersResult {
    pub final_url: Url,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub redirect_chain: Vec<(u16, Url)>,
    pub connected_peer: SocketAddr,
}

struct FetchHopResult {
    status: u16,
    headers: Vec<(String, String)>,
    connected_peer: SocketAddr,
}

fn load_root_store(ca_bundle: &Path) -> Result<RootCertStore, FetchError> {
    let certificates = CertificateDer::pem_file_iter(ca_bundle)
        .map_err(|error| FetchError::CaBundle(error.to_string()))?;
    let mut roots = RootCertStore::empty();
    let mut loaded = 0usize;
    for certificate in certificates {
        let certificate = certificate.map_err(|error| FetchError::CaBundle(error.to_string()))?;
        roots
            .add(certificate)
            .map_err(|error| FetchError::CaBundle(error.to_string()))?;
        loaded += 1;
    }
    if loaded == 0 {
        return Err(FetchError::CaBundle("no PEM certificates found".into()));
    }
    Ok(roots)
}

fn resolve_and_validate(host: &str, port: u16) -> Result<Vec<SocketAddr>, FetchError> {
    let resolved = (host, port)
        .to_socket_addrs()
        .map_err(FetchError::Dns)?
        .collect::<Vec<_>>();
    if resolved.is_empty() {
        return Err(FetchError::NoAddresses);
    }
    let ips = resolved.iter().map(SocketAddr::ip).collect::<Vec<_>>();
    validate_resolution(&ips).map_err(FetchError::AddressDenied)?;
    let mut unique = Vec::new();
    for address in resolved {
        if !unique.contains(&address) {
            unique.push(address);
        }
    }
    Ok(unique)
}

fn connect_validated(addresses: &[SocketAddr]) -> Result<(TcpStream, SocketAddr), FetchError> {
    let mut last_error = None;
    for address in addresses {
        match TcpStream::connect_timeout(address, CONNECT_TIMEOUT) {
            Ok(stream) => {
                stream
                    .set_read_timeout(Some(IO_TIMEOUT))
                    .map_err(FetchError::Connect)?;
                stream
                    .set_write_timeout(Some(IO_TIMEOUT))
                    .map_err(FetchError::Connect)?;
                return Ok((stream, *address));
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(FetchError::Connect(last_error.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no validated address")
    })))
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    max_line_bytes: usize,
) -> std::io::Result<Option<String>> {
    let mut bytes = Vec::with_capacity(128);
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if bytes.is_empty() {
                return Ok(None);
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "HTTP line ended without CRLF",
            ));
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if bytes.len().saturating_add(take) > max_line_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "HTTP line exceeds configured limit",
            ));
        }
        bytes.extend_from_slice(&available[..take]);
        let complete = bytes.last() == Some(&b'\n');
        reader.consume(take);
        if complete {
            break;
        }
    }
    if !bytes.ends_with(b"\r\n") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "HTTP line must end with CRLF",
        ));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn read_response_headers<R: BufRead>(
    reader: &mut R,
) -> Result<(u16, Vec<(String, String)>), FetchError> {
    const MAX_STATUS_LINE_BYTES: usize = 8192;
    const MAX_HEADER_LINE_BYTES: usize = 8192;

    let status_line = read_bounded_line(reader, MAX_STATUS_LINE_BYTES)
        .map_err(FetchError::Http)?
        .ok_or_else(|| FetchError::InvalidResponse("missing HTTP status line".into()))?;
    let mut status_parts = status_line.split_whitespace();
    let protocol = status_parts.next().unwrap_or_default();
    let status = status_parts
        .next()
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| FetchError::InvalidResponse("missing HTTP status code".into()))?;
    if protocol != "HTTP/1.0" && protocol != "HTTP/1.1" {
        return Err(FetchError::InvalidResponse(
            "unsupported HTTP version".into(),
        ));
    }
    if !(200..=599).contains(&status) {
        return Err(FetchError::InvalidResponse(
            "informational and invalid status codes are unsupported".into(),
        ));
    }

    let mut headers = Vec::new();
    let mut total_bytes = status_line.len();
    loop {
        let line = read_bounded_line(reader, MAX_HEADER_LINE_BYTES)
            .map_err(FetchError::Http)?
            .ok_or_else(|| FetchError::InvalidResponse("truncated HTTP headers".into()))?;
        total_bytes = total_bytes.saturating_add(line.len());
        if total_bytes > MAX_HEADER_BYTES {
            return Err(FetchError::InvalidResponse(
                "response headers exceed byte limit".into(),
            ));
        }
        if line == "\r\n" {
            break;
        }
        if headers.len() >= 100 {
            return Err(FetchError::InvalidResponse(
                "response contains too many headers".into(),
            ));
        }
        let (name, value) = line
            .strip_suffix("\r\n")
            .and_then(|line| line.split_once(':'))
            .ok_or_else(|| FetchError::InvalidResponse("malformed HTTP header".into()))?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
        {
            return Err(FetchError::InvalidResponse(
                "invalid HTTP header name".into(),
            ));
        }
        if value
            .bytes()
            .any(|byte| byte == b'\0' || (byte < 0x20 && byte != b'\t') || byte == 0x7f)
        {
            return Err(FetchError::InvalidResponse(
                "HTTP header value contains a control character".into(),
            ));
        }
        headers.push((name.to_ascii_lowercase(), value.trim().to_string()));
    }
    Ok((status, headers))
}

fn redirect_location(headers: &[(String, String)]) -> Result<&str, FetchError> {
    let mut locations = headers
        .iter()
        .filter(|(name, _)| name == "location")
        .map(|(_, value)| value.as_str());
    let location = locations
        .next()
        .ok_or_else(|| FetchError::InvalidRedirect("redirect response has no Location".into()))?;
    if locations.next().is_some() {
        return Err(FetchError::InvalidRedirect(
            "redirect response contains multiple Location headers".into(),
        ));
    }
    Ok(location)
}

fn fetch_once(url: &Url, roots: &RootCertStore) -> Result<FetchHopResult, FetchError> {
    if url.scheme() != "https" {
        return Err(FetchError::UnsupportedScheme(url.scheme().into()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(FetchError::CredentialsNotAllowed);
    }
    let host = url
        .host_str()
        .ok_or_else(|| FetchError::InvalidUrl("URL has no host".into()))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| FetchError::InvalidUrl("URL has no known port".into()))?;

    // Resolve once, reject the entire answer set if any address is non-public,
    // then connect to one of those exact SocketAddr values (no second DNS lookup).
    let addresses = resolve_and_validate(host, port)?;
    let (socket, connected_peer) = connect_validated(&addresses)?;

    let server_name = ServerName::try_from(host.to_string())
        .map_err(|error| FetchError::Tls(error.to_string()))?;
    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|error| FetchError::Tls(error.to_string()))?
            .with_root_certificates(roots.clone())
            .with_no_client_auth();
    let connection = ClientConnection::new(Arc::new(config), server_name)
        .map_err(|error| FetchError::Tls(error.to_string()))?;
    let mut tls = StreamOwned::new(connection, socket);

    let host_header = match url.port() {
        Some(explicit_port) => format!("{host}:{explicit_port}"),
        None => host.to_string(),
    };
    let mut target = if url.path().is_empty() {
        "/".to_string()
    } else {
        url.path().to_string()
    };
    if let Some(query) = url.query() {
        target.push('?');
        target.push_str(query);
    }
    write!(tls, "GET {target} HTTP/1.1\r\nHost: {host_header}\r\nUser-Agent: AIBE-Phase0/0.1\r\nAccept: */*\r\nConnection: close\r\n\r\n")
        .map_err(FetchError::Http)?;
    tls.flush().map_err(FetchError::Http)?;

    let mut reader = BufReader::new(tls);
    let (status, headers) = read_response_headers(&mut reader)?;
    Ok(FetchHopResult {
        status,
        headers,
        connected_peer,
    })
}

/// Perform a bounded HTTPS GET header probe. Every redirect is resolved and
/// revalidated independently; non-HTTPS redirects are rejected.
pub fn fetch_https_headers(
    initial_url: &str,
    ca_bundle: &Path,
    max_redirects: usize,
) -> Result<FetchHeadersResult, FetchError> {
    let mut current =
        Url::parse(initial_url).map_err(|error| FetchError::InvalidUrl(error.to_string()))?;
    if current.scheme() != "https" {
        return Err(FetchError::UnsupportedScheme(current.scheme().into()));
    }
    let roots = load_root_store(ca_bundle)?;
    let mut redirect_chain = Vec::new();

    for hop in 0..=max_redirects.min(MAX_REDIRECTS) {
        let response = fetch_once(&current, &roots)?;
        if !is_redirect(response.status) {
            return Ok(FetchHeadersResult {
                final_url: current,
                status: response.status,
                headers: response.headers,
                redirect_chain,
                connected_peer: response.connected_peer,
            });
        }

        let location = redirect_location(&response.headers)?;
        if hop == max_redirects.min(MAX_REDIRECTS) {
            return Err(FetchError::RedirectLimit);
        }
        let next = current
            .join(location)
            .map_err(|error| FetchError::InvalidRedirect(error.to_string()))?;
        if next.scheme() != "https" {
            return Err(FetchError::RedirectDowngrade);
        }
        redirect_chain.push((response.status, next.clone()));
        current = next;
    }
    Err(FetchError::RedirectLimit)
}

#[cfg(test)]
mod tests {
    use super::{is_allowed_public_ip, is_redirect, read_response_headers, validate_resolution};
    use std::io::Cursor;
    use std::net::IpAddr;
    use url::Url;

    fn ip(value: &str) -> IpAddr {
        value.parse().expect("test IP is valid")
    }

    #[test]
    fn allows_public_unicast_addresses() {
        assert!(is_allowed_public_ip(ip("93.184.216.34")));
        assert!(is_allowed_public_ip(ip("2606:4700:4700::1111")));
    }

    #[test]
    fn blocks_ipv4_private_loopback_link_local_and_cgnat() {
        for value in [
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "127.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
        ] {
            assert!(!is_allowed_public_ip(ip(value)), "{value} must be denied");
        }
    }

    #[test]
    fn blocks_ipv4_documentation_multicast_and_reserved_ranges() {
        for value in [
            "192.0.2.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
        ] {
            assert!(!is_allowed_public_ip(ip(value)), "{value} must be denied");
        }
    }

    #[test]
    fn blocks_ipv6_loopback_unique_local_link_local_and_special_ranges() {
        for value in [
            "::",
            "::1",
            "fc00::1",
            "fe80::1",
            "ff02::1",
            "2001:db8::1",
            "2002::1",
            "64:ff9b::a00:1",
        ] {
            assert!(!is_allowed_public_ip(ip(value)), "{value} must be denied");
        }
    }

    #[test]
    fn rejects_ipv4_mapped_private_ipv6_addresses() {
        assert!(!is_allowed_public_ip(ip("::ffff:127.0.0.1")));
        assert!(!is_allowed_public_ip(ip("::ffff:10.0.0.1")));
    }

    #[test]
    fn rejects_mixed_public_and_private_dns_answers() {
        let answers = [ip("93.184.216.34"), ip("127.0.0.1")];
        assert_eq!(validate_resolution(&answers), Err(ip("127.0.0.1")));
    }

    #[test]
    fn deduplicates_public_dns_answers() {
        let answers = [ip("93.184.216.34"), ip("93.184.216.34")];
        assert_eq!(validate_resolution(&answers), Ok(vec![ip("93.184.216.34")]));
    }

    #[test]
    fn rejects_empty_dns_answers() {
        assert!(validate_resolution(&[]).is_err());
    }

    #[test]
    fn recognizes_only_supported_redirect_status_codes() {
        for status in [301, 302, 303, 307, 308] {
            assert!(is_redirect(status));
        }
        for status in [200, 201, 300, 304, 400, 500] {
            assert!(!is_redirect(status));
        }
    }

    #[test]
    fn parses_response_headers_with_case_insensitive_names() {
        let response = b"HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Type: text/html\r\n\r\n";
        let (status, headers) =
            read_response_headers(&mut Cursor::new(response)).expect("headers parse");
        assert_eq!(status, 302);
        assert_eq!(headers[0], ("location".into(), "/next".into()));
    }

    #[test]
    fn rejects_oversized_http_header_line_before_unbounded_allocation() {
        let response = format!("HTTP/1.1 200 OK\r\nX-Large: {}\r\n\r\n", "a".repeat(9000));
        assert!(matches!(
            read_response_headers(&mut Cursor::new(response)),
            Err(super::FetchError::Http(error)) if error.kind() == std::io::ErrorKind::InvalidData
        ));
    }

    #[test]
    fn rejects_http_headers_without_crlf_terminator() {
        let response = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n";
        assert!(matches!(
            read_response_headers(&mut Cursor::new(response)),
            Err(super::FetchError::InvalidResponse(_))
        ));
    }

    #[test]
    fn rejects_control_characters_in_http_header_values() {
        let response = b"HTTP/1.1 200 OK\r\nX-Test: safe\x01unsafe\r\n\r\n";
        assert!(matches!(
            read_response_headers(&mut Cursor::new(response)),
            Err(super::FetchError::InvalidResponse(_))
        ));
    }

    #[test]
    fn rejects_informational_status_without_final_response() {
        let response = b"HTTP/1.1 100 Continue\r\n\r\n";
        assert!(matches!(
            read_response_headers(&mut Cursor::new(response)),
            Err(super::FetchError::InvalidResponse(_))
        ));
    }

    #[test]
    fn rejects_redirects_with_duplicate_location_headers() {
        let headers = vec![
            ("location".into(), "https://example.com/a".into()),
            ("location".into(), "https://example.net/b".into()),
        ];
        assert!(matches!(
            super::redirect_location(&headers),
            Err(super::FetchError::InvalidRedirect(_))
        ));
    }

    #[test]
    fn rejects_private_ip_before_attempting_tls_connection() {
        let url = Url::parse("https://127.0.0.1/").expect("URL parses");
        let roots = rustls::RootCertStore::empty();
        assert!(matches!(
            super::fetch_once(&url, &roots),
            Err(super::FetchError::AddressDenied(IpAddr::V4(address))) if address.is_loopback()
        ));
    }

    #[test]
    fn resolves_relative_redirect_against_current_origin() {
        let current = Url::parse("https://example.com/a/b").expect("URL parses");
        let next = current
            .join("../login")
            .expect("relative redirect resolves");
        assert_eq!(next.as_str(), "https://example.com/login");
    }
}
