//! Phase 0 origin-policy experiment.
//!
//! URL parsing and origin canonicalization live in the trusted browser host,
//! not in page JavaScript or the V8 adapter. The returned group handle is
//! registry-scoped and is intended to be mapped to a runtime's opaque origin
//! handle by the Browser Runtime.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use url::{Origin as UrlOrigin, Url};

static NEXT_REGISTRY_ID: AtomicU64 = AtomicU64::new(1);

/// Opaque identifier for one canonical origin group in one registry.
/// Fields are private so callers cannot manufacture or alter a grant.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct OriginGroupId {
    registry_id: u64,
    group_id: u64,
}

/// Errors produced while parsing URLs or validating origin-group handles.
#[derive(Debug)]
pub enum OriginPolicyError {
    InvalidUrl(url::ParseError),
    ForeignRegistry,
    IdentifierSpaceExhausted,
}

impl fmt::Display for OriginPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl(error) => write!(formatter, "invalid absolute URL: {error}"),
            Self::ForeignRegistry => {
                formatter.write_str("origin group belongs to another registry")
            }
            Self::IdentifierSpaceExhausted => {
                formatter.write_str("origin identifier space exhausted")
            }
        }
    }
}

impl std::error::Error for OriginPolicyError {}

/// Host-owned canonical origin registry.
///
/// Tuple origins are interned by WHATWG URL origin semantics (scheme, host,
/// effective port). Opaque origins remain unique. Inherited origins must be
/// propagated explicitly by the caller for about:blank/srcdoc-like contexts.
pub struct OriginRegistry {
    registry_id: u64,
    next_group_id: u64,
    groups: HashMap<UrlOrigin, u64>,
}

impl OriginRegistry {
    pub fn new() -> Result<Self, OriginPolicyError> {
        let registry_id = NEXT_REGISTRY_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                (current != 0 && current != u64::MAX).then_some(current + 1)
            })
            .map_err(|_| OriginPolicyError::IdentifierSpaceExhausted)?;
        Ok(Self {
            registry_id,
            next_group_id: 1,
            groups: HashMap::new(),
        })
    }

    /// Parses an absolute URL with the pinned WHATWG-compatible `url` crate
    /// and interns its computed origin. Repeated tuple origins share a handle;
    /// separately parsed opaque origins receive distinct handles.
    pub fn resolve_url(&mut self, input: &str) -> Result<OriginGroupId, OriginPolicyError> {
        let url = Url::parse(input).map_err(OriginPolicyError::InvalidUrl)?;
        self.intern_origin(url.origin())
    }

    /// Creates a fresh opaque origin, for example for a sandboxed document.
    pub fn create_opaque_origin(&mut self) -> Result<OriginGroupId, OriginPolicyError> {
        self.intern_origin(UrlOrigin::new_opaque())
    }

    /// Explicitly propagates a creator's origin to an inherited-origin context.
    /// This is not inferred from `about:blank` or `about:srcdoc` URL text.
    pub fn inherit(&self, parent: OriginGroupId) -> Result<OriginGroupId, OriginPolicyError> {
        self.validate(parent)?;
        Ok(parent)
    }

    /// Checks whether a handle was issued by this registry and remains valid.
    pub fn validate(&self, origin: OriginGroupId) -> Result<(), OriginPolicyError> {
        if origin.registry_id != self.registry_id
            || origin.group_id == 0
            || origin.group_id >= self.next_group_id
        {
            return Err(OriginPolicyError::ForeignRegistry);
        }
        Ok(())
    }

    fn intern_origin(&mut self, origin: UrlOrigin) -> Result<OriginGroupId, OriginPolicyError> {
        if let Some(group_id) = self.groups.get(&origin).copied() {
            return Ok(OriginGroupId {
                registry_id: self.registry_id,
                group_id,
            });
        }
        if self.next_group_id == 0 || self.next_group_id == u64::MAX {
            return Err(OriginPolicyError::IdentifierSpaceExhausted);
        }
        let group_id = self.next_group_id;
        self.next_group_id += 1;
        self.groups.insert(origin, group_id);
        Ok(OriginGroupId {
            registry_id: self.registry_id,
            group_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{OriginPolicyError, OriginRegistry};

    #[test]
    fn tuple_origin_ignores_path_query_and_fragment() {
        let mut registry = OriginRegistry::new().unwrap();
        let first = registry
            .resolve_url("https://example.com/a?x=1#top")
            .unwrap();
        let second = registry
            .resolve_url("https://example.com/b?x=2#bottom")
            .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn default_ports_and_host_case_are_canonicalized() {
        let mut registry = OriginRegistry::new().unwrap();
        let implicit = registry.resolve_url("https://EXAMPLE.com/").unwrap();
        let explicit = registry
            .resolve_url("https://example.com:443/path")
            .unwrap();
        assert_eq!(implicit, explicit);

        let http_implicit = registry.resolve_url("http://example.com/").unwrap();
        let http_explicit = registry.resolve_url("http://example.com:80/").unwrap();
        assert_eq!(http_implicit, http_explicit);
        assert_ne!(implicit, http_implicit);
    }

    #[test]
    fn scheme_and_non_default_port_are_part_of_tuple_origin() {
        let mut registry = OriginRegistry::new().unwrap();
        let https = registry.resolve_url("https://example.com/").unwrap();
        let http = registry.resolve_url("http://example.com/").unwrap();
        let alternate_port = registry.resolve_url("https://example.com:8443/").unwrap();
        assert_ne!(https, http);
        assert_ne!(https, alternate_port);
    }

    #[test]
    fn idna_hosts_are_canonicalized_before_origin_comparison() {
        let mut registry = OriginRegistry::new().unwrap();
        let unicode = registry.resolve_url("https://bücher.example/").unwrap();
        let ascii = registry
            .resolve_url("https://xn--bcher-kva.example/")
            .unwrap();
        assert_eq!(unicode, ascii);
    }

    #[test]
    fn credentials_do_not_participate_in_tuple_origin() {
        let mut registry = OriginRegistry::new().unwrap();
        let plain = registry.resolve_url("https://example.com/path").unwrap();
        let authenticated = registry
            .resolve_url("https://alice:secret@example.com/other")
            .unwrap();
        assert_eq!(plain, authenticated);
    }

    #[test]
    fn legacy_ipv4_forms_are_canonicalized_by_the_url_parser() {
        let mut registry = OriginRegistry::new().unwrap();
        let canonical = registry.resolve_url("http://127.0.0.1/").unwrap();
        let shortened = registry.resolve_url("http://127.1/").unwrap();
        assert_eq!(canonical, shortened);
    }

    #[test]
    fn ipv6_origins_use_canonical_host_and_effective_port() {
        let mut registry = OriginRegistry::new().unwrap();
        let implicit = registry.resolve_url("https://[2001:db8::1]/a").unwrap();
        let explicit = registry
            .resolve_url("https://[2001:0db8:0:0:0:0:0:1]:443/b")
            .unwrap();
        assert_eq!(implicit, explicit);
    }

    #[test]
    fn blob_urls_inherit_the_embedded_url_origin() {
        let mut registry = OriginRegistry::new().unwrap();
        let page = registry.resolve_url("https://example.com/").unwrap();
        let blob = registry
            .resolve_url("blob:https://example.com/550e8400-e29b-41d4-a716-446655440000")
            .unwrap();
        assert_eq!(page, blob);
    }

    #[test]
    fn opaque_url_origins_are_unique_even_when_url_text_matches() {
        let mut registry = OriginRegistry::new().unwrap();
        for url in ["data:text/html,hello", "about:blank", "file:///tmp/a"] {
            let first = registry.resolve_url(url).unwrap();
            let second = registry.resolve_url(url).unwrap();
            assert_ne!(
                first, second,
                "opaque origin for {url} must be unique per creation"
            );
        }
    }

    #[test]
    fn inherited_origin_is_explicit_and_differs_from_parsing_about_blank() {
        let mut registry = OriginRegistry::new().unwrap();
        let creator = registry.resolve_url("https://example.com/parent").unwrap();
        let inherited = registry.inherit(creator).unwrap();
        let parsed_about_blank = registry.resolve_url("about:blank").unwrap();
        assert_eq!(creator, inherited);
        assert_ne!(creator, parsed_about_blank);
    }

    #[test]
    fn sandboxed_opaque_origin_does_not_share_with_tuple_origin_or_another_opaque_origin() {
        let mut registry = OriginRegistry::new().unwrap();
        let tuple = registry.resolve_url("https://example.com/").unwrap();
        let opaque_a = registry.create_opaque_origin().unwrap();
        let opaque_b = registry.create_opaque_origin().unwrap();
        assert_ne!(tuple, opaque_a);
        assert_ne!(opaque_a, opaque_b);
    }

    #[test]
    fn origin_handles_are_registry_scoped() {
        let mut first = OriginRegistry::new().unwrap();
        let second = OriginRegistry::new().unwrap();
        let origin = first.resolve_url("https://example.com/").unwrap();
        assert!(matches!(
            second.validate(origin),
            Err(OriginPolicyError::ForeignRegistry)
        ));
        assert!(first.validate(origin).is_ok());
    }

    #[test]
    fn malformed_and_relative_urls_are_rejected() {
        let mut registry = OriginRegistry::new().unwrap();
        assert!(matches!(
            registry.resolve_url("/relative/path"),
            Err(OriginPolicyError::InvalidUrl(_))
        ));
        assert!(matches!(
            registry.resolve_url("https://exa mple.com/"),
            Err(OriginPolicyError::InvalidUrl(_))
        ));
    }
}
