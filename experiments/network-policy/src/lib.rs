//! Experimental outbound IP policy for browser navigation and Fetch.
//!
//! This crate validates resolved IP addresses. It does not perform DNS itself,
//! open sockets, implement TLS, or follow redirects. The caller must connect to
//! the exact validated SocketAddr and repeat validation after every redirect.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::OnceLock;

use ipnet::IpNet;

static BLOCKED_V4: OnceLock<Vec<IpNet>> = OnceLock::new();
static BLOCKED_V6: OnceLock<Vec<IpNet>> = OnceLock::new();

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

#[cfg(test)]
mod tests {
    use super::{is_allowed_public_ip, validate_resolution};
    use std::net::IpAddr;

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
}
