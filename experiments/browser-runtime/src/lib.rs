//! Phase 0 Browser Runtime orchestration for URL origins and JS Realms.
//!
//! This layer owns the mapping from canonical host Origin Groups to opaque
//! JavaScript Runtime origin handles. The URL parser is not part of the V8
//! adapter, and page JavaScript cannot request or manufacture these grants.

use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;

use aibe_js_runtime_ffi_poc::{JsOriginId, JsRealmId, JsRuntime, JsRuntimeError, JsValue};
use aibe_origin_policy_poc::{OriginGroupId, OriginPolicyError, OriginRegistry};

/// Minimal lifecycle contract required by the Browser Runtime coordinator.
///
/// The blanket implementation connects the selected project-owned JS Runtime
/// trait. Tests can use a deterministic mock without depending on V8 binaries.
pub trait OriginRealmRuntime {
    type Origin: Copy + Eq + Hash;
    type Realm: Copy + Eq + Hash;
    type Error: std::error::Error;

    fn create_origin(&mut self) -> Result<Self::Origin, Self::Error>;
    fn create_realm_for_origin(&mut self, origin: Self::Origin)
    -> Result<Self::Realm, Self::Error>;
    fn destroy_realm(&mut self, realm: Self::Realm) -> Result<(), Self::Error>;
}

impl<T: JsRuntime> OriginRealmRuntime for T {
    type Origin = JsOriginId;
    type Realm = JsRealmId;
    type Error = JsRuntimeError;

    fn create_origin(&mut self) -> Result<Self::Origin, Self::Error> {
        JsRuntime::create_origin(self)
    }

    fn create_realm_for_origin(
        &mut self,
        origin: Self::Origin,
    ) -> Result<Self::Realm, Self::Error> {
        JsRuntime::create_realm_for_origin(self, origin)
    }

    fn destroy_realm(&mut self, realm: Self::Realm) -> Result<(), Self::Error> {
        JsRuntime::destroy_realm(self, realm)
    }
}

#[derive(Debug)]
pub enum BrowserRuntimeOriginError<E> {
    Origin(OriginPolicyError),
    Runtime(E),
    UnknownRealm,
}

impl<E: fmt::Display> fmt::Display for BrowserRuntimeOriginError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Origin(error) => write!(formatter, "origin policy error: {error}"),
            Self::Runtime(error) => write!(formatter, "JavaScript Runtime error: {error}"),
            Self::UnknownRealm => {
                formatter.write_str("Realm is not active in this Browser Runtime")
            }
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for BrowserRuntimeOriginError<E> {}

impl<E> From<OriginPolicyError> for BrowserRuntimeOriginError<E> {
    fn from(error: OriginPolicyError) -> Self {
        Self::Origin(error)
    }
}

struct ActiveOrigin<O> {
    runtime_origin: O,
    active_realms: usize,
}

/// Coordinates canonical URL origins, V8 origin handles, and Realm lifetimes.
///
/// A URL tuple origin is mapped to one JS Runtime origin handle while at least
/// one Realm in that group remains active. Each Realm still receives its own
/// isolated JavaScript Context. Inherited and sandboxed origins are explicit.
pub struct OriginRealmCoordinator<R: OriginRealmRuntime> {
    registry: OriginRegistry,
    runtime: R,
    active_origins: HashMap<OriginGroupId, ActiveOrigin<R::Origin>>,
    realm_origins: HashMap<R::Realm, OriginGroupId>,
}

impl<R: JsRuntime> OriginRealmCoordinator<R> {
    /// Evaluates only in a Realm currently owned by this coordinator.
    /// The raw Runtime is not exposed mutably, preventing callers from creating
    /// untracked Realms that bypass the origin-group lifecycle mapping.
    pub fn evaluate_value_in_realm(
        &mut self,
        realm: JsRealmId,
        source: &str,
    ) -> Result<JsValue, BrowserRuntimeOriginError<JsRuntimeError>> {
        if !self.realm_origins.contains_key(&realm) {
            return Err(BrowserRuntimeOriginError::UnknownRealm);
        }
        self.runtime
            .evaluate_value_in_realm(realm, source)
            .map_err(BrowserRuntimeOriginError::Runtime)
    }
}

impl<R: OriginRealmRuntime> OriginRealmCoordinator<R> {
    pub fn new(runtime: R) -> Result<Self, OriginPolicyError> {
        Ok(Self {
            registry: OriginRegistry::new()?,
            runtime,
            active_origins: HashMap::new(),
            realm_origins: HashMap::new(),
        })
    }

    /// Creates a fresh JavaScript Realm for the URL's canonical origin.
    pub fn create_realm_for_url(
        &mut self,
        url: &str,
    ) -> Result<R::Realm, BrowserRuntimeOriginError<R::Error>> {
        let origin = self.registry.resolve_url(url)?;
        self.create_realm_for_origin_group(origin)
    }

    /// Creates a Realm that explicitly inherits a live creator Realm's origin.
    /// This is the intended path for about:blank/srcdoc-style inherited origins.
    pub fn create_inherited_realm(
        &mut self,
        creator: R::Realm,
    ) -> Result<R::Realm, BrowserRuntimeOriginError<R::Error>> {
        let creator_origin = *self
            .realm_origins
            .get(&creator)
            .ok_or(BrowserRuntimeOriginError::UnknownRealm)?;
        let inherited = self.registry.inherit(creator_origin)?;
        self.create_realm_for_origin_group(inherited)
    }

    /// Creates a Realm with a fresh opaque origin, e.g. for a sandboxed frame.
    pub fn create_sandboxed_realm(
        &mut self,
    ) -> Result<R::Realm, BrowserRuntimeOriginError<R::Error>> {
        let opaque = self.registry.create_opaque_origin()?;
        self.create_realm_for_origin_group(opaque)
    }

    /// Destroys a Realm and releases its origin mapping when the last Realm exits.
    pub fn destroy_realm(
        &mut self,
        realm: R::Realm,
    ) -> Result<(), BrowserRuntimeOriginError<R::Error>> {
        let origin = *self
            .realm_origins
            .get(&realm)
            .ok_or(BrowserRuntimeOriginError::UnknownRealm)?;
        self.runtime
            .destroy_realm(realm)
            .map_err(BrowserRuntimeOriginError::Runtime)?;
        self.realm_origins.remove(&realm);
        if let Some(active) = self.active_origins.get_mut(&origin) {
            active.active_realms -= 1;
            if active.active_realms == 0 {
                self.active_origins.remove(&origin);
                self.registry.release(origin)?;
            }
        }
        Ok(())
    }

    pub fn origin_group_for_realm(&self, realm: R::Realm) -> Option<OriginGroupId> {
        self.realm_origins.get(&realm).copied()
    }

    pub fn active_realm_count_for_origin(&self, origin: OriginGroupId) -> usize {
        self.active_origins
            .get(&origin)
            .map(|active| active.active_realms)
            .unwrap_or(0)
    }

    pub fn active_origin_group_count(&self) -> usize {
        self.active_origins.len()
    }

    pub fn runtime(&self) -> &R {
        &self.runtime
    }

    fn create_realm_for_origin_group(
        &mut self,
        origin: OriginGroupId,
    ) -> Result<R::Realm, BrowserRuntimeOriginError<R::Error>> {
        if let Some(active) = self.active_origins.get(&origin) {
            let runtime_origin = active.runtime_origin;
            let realm = self
                .runtime
                .create_realm_for_origin(runtime_origin)
                .map_err(BrowserRuntimeOriginError::Runtime)?;
            self.active_origins
                .get_mut(&origin)
                .expect("active origin remains present during Realm creation")
                .active_realms += 1;
            self.realm_origins.insert(realm, origin);
            return Ok(realm);
        }

        let runtime_origin = match self.runtime.create_origin() {
            Ok(origin) => origin,
            Err(error) => {
                self.registry.release(origin)?;
                return Err(BrowserRuntimeOriginError::Runtime(error));
            }
        };
        let realm = match self.runtime.create_realm_for_origin(runtime_origin) {
            Ok(realm) => realm,
            Err(error) => {
                self.registry.release(origin)?;
                return Err(BrowserRuntimeOriginError::Runtime(error));
            }
        };
        self.active_origins.insert(
            origin,
            ActiveOrigin {
                runtime_origin,
                active_realms: 1,
            },
        );
        self.realm_origins.insert(realm, origin);
        Ok(realm)
    }
}

#[cfg(test)]
mod tests {
    use super::{BrowserRuntimeOriginError, OriginRealmCoordinator, OriginRealmRuntime};
    use std::collections::HashMap;
    use std::fmt;

    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    struct MockOrigin(u64);

    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    struct MockRealm(u64);

    #[derive(Debug)]
    struct MockError;

    impl fmt::Display for MockError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("mock runtime failure")
        }
    }

    impl std::error::Error for MockError {}

    #[derive(Default)]
    struct MockRuntime {
        next_origin: u64,
        next_realm: u64,
        realms: HashMap<MockRealm, MockOrigin>,
        fail_next_realm: bool,
    }

    impl OriginRealmRuntime for MockRuntime {
        type Origin = MockOrigin;
        type Realm = MockRealm;
        type Error = MockError;

        fn create_origin(&mut self) -> Result<Self::Origin, Self::Error> {
            self.next_origin += 1;
            Ok(MockOrigin(self.next_origin))
        }

        fn create_realm_for_origin(
            &mut self,
            origin: Self::Origin,
        ) -> Result<Self::Realm, Self::Error> {
            if self.fail_next_realm {
                self.fail_next_realm = false;
                return Err(MockError);
            }
            self.next_realm += 1;
            let realm = MockRealm(self.next_realm);
            self.realms.insert(realm, origin);
            Ok(realm)
        }

        fn destroy_realm(&mut self, realm: Self::Realm) -> Result<(), Self::Error> {
            self.realms.remove(&realm).ok_or(MockError)?;
            Ok(())
        }
    }

    #[test]
    fn canonical_urls_share_origin_group_but_receive_distinct_realms() {
        let runtime = MockRuntime::default();
        let mut coordinator = OriginRealmCoordinator::new(runtime).unwrap();
        let first = coordinator
            .create_realm_for_url("https://example.com/a")
            .unwrap();
        let second = coordinator
            .create_realm_for_url("https://example.com/b")
            .unwrap();
        let cross_origin = coordinator
            .create_realm_for_url("https://other.example/")
            .unwrap();

        assert_ne!(first, second);
        assert_eq!(
            coordinator.origin_group_for_realm(first),
            coordinator.origin_group_for_realm(second)
        );
        assert_ne!(
            coordinator.origin_group_for_realm(first),
            coordinator.origin_group_for_realm(cross_origin)
        );
        assert_eq!(coordinator.active_origin_group_count(), 2);
        assert_eq!(
            coordinator
                .active_realm_count_for_origin(coordinator.origin_group_for_realm(first).unwrap()),
            2
        );
    }

    #[test]
    fn inherited_realm_reuses_creator_origin_group() {
        let runtime = MockRuntime::default();
        let mut coordinator = OriginRealmCoordinator::new(runtime).unwrap();
        let creator = coordinator
            .create_realm_for_url("https://example.com/")
            .unwrap();
        let child = coordinator.create_inherited_realm(creator).unwrap();
        assert_ne!(creator, child);
        assert_eq!(
            coordinator.origin_group_for_realm(creator),
            coordinator.origin_group_for_realm(child)
        );
        assert_eq!(coordinator.active_origin_group_count(), 1);
    }

    #[test]
    fn sandboxed_realm_gets_a_fresh_opaque_origin_group() {
        let runtime = MockRuntime::default();
        let mut coordinator = OriginRealmCoordinator::new(runtime).unwrap();
        let normal = coordinator
            .create_realm_for_url("https://example.com/")
            .unwrap();
        let sandboxed = coordinator.create_sandboxed_realm().unwrap();
        assert_ne!(
            coordinator.origin_group_for_realm(normal),
            coordinator.origin_group_for_realm(sandboxed)
        );
    }

    #[test]
    fn origin_mapping_is_released_after_last_realm_is_destroyed() {
        let runtime = MockRuntime::default();
        let mut coordinator = OriginRealmCoordinator::new(runtime).unwrap();
        let first = coordinator
            .create_realm_for_url("https://example.com/")
            .unwrap();
        let second = coordinator
            .create_realm_for_url("https://example.com/next")
            .unwrap();
        let group = coordinator.origin_group_for_realm(first).unwrap();
        coordinator.destroy_realm(first).unwrap();
        assert_eq!(coordinator.active_realm_count_for_origin(group), 1);
        coordinator.destroy_realm(second).unwrap();
        assert_eq!(coordinator.active_realm_count_for_origin(group), 0);
        assert_eq!(coordinator.active_origin_group_count(), 0);
        assert!(matches!(
            coordinator.destroy_realm(second),
            Err(BrowserRuntimeOriginError::UnknownRealm)
        ));
        assert_eq!(coordinator.runtime().next_origin, 1);
        let replacement = coordinator
            .create_realm_for_url("https://example.com/new-document")
            .unwrap();
        assert_eq!(coordinator.runtime().next_origin, 2);
        assert_eq!(coordinator.active_origin_group_count(), 1);
        assert_ne!(coordinator.origin_group_for_realm(replacement), Some(group));
    }

    #[test]
    fn failed_realm_creation_does_not_leave_a_stale_origin_mapping() {
        let runtime = MockRuntime {
            fail_next_realm: true,
            ..MockRuntime::default()
        };
        let mut coordinator = OriginRealmCoordinator::new(runtime).unwrap();
        assert!(matches!(
            coordinator.create_realm_for_url("https://example.com/"),
            Err(BrowserRuntimeOriginError::Runtime(_))
        ));
        assert_eq!(coordinator.active_origin_group_count(), 0);
        let retry = coordinator
            .create_realm_for_url("https://example.com/")
            .unwrap();
        assert_eq!(coordinator.active_origin_group_count(), 1);
        assert!(coordinator.origin_group_for_realm(retry).is_some());
    }

    #[test]
    fn inherited_realm_requires_a_live_creator() {
        let runtime = MockRuntime::default();
        let mut coordinator = OriginRealmCoordinator::new(runtime).unwrap();
        let creator = coordinator
            .create_realm_for_url("https://example.com/")
            .unwrap();
        coordinator.destroy_realm(creator).unwrap();
        assert!(matches!(
            coordinator.create_inherited_realm(creator),
            Err(BrowserRuntimeOriginError::UnknownRealm)
        ));
    }
}

#[cfg(all(test, target_os = "freebsd"))]
mod v8_integration_tests {
    use super::OriginRealmCoordinator;
    use aibe_js_runtime_ffi_poc::{JsValue, V8Runtime};

    #[test]
    fn canonical_url_origins_are_mapped_to_v8_origin_groups_and_isolated_realms() {
        let runtime = V8Runtime::new().expect("V8 runtime initializes");
        let mut coordinator =
            OriginRealmCoordinator::new(runtime).expect("origin coordinator initializes");

        let first = coordinator
            .create_realm_for_url("https://example.com/one")
            .expect("first same-origin Realm is created");
        let second = coordinator
            .create_realm_for_url("https://example.com/two")
            .expect("second same-origin Realm is created");
        let cross_origin = coordinator
            .create_realm_for_url("https://other.example/")
            .expect("cross-origin Realm is created");
        let inherited = coordinator
            .create_inherited_realm(first)
            .expect("inherited-origin Realm is created");
        let sandboxed = coordinator
            .create_sandboxed_realm()
            .expect("sandboxed opaque-origin Realm is created");

        let first_group = coordinator.origin_group_for_realm(first).unwrap();
        assert_eq!(
            coordinator.origin_group_for_realm(second),
            Some(first_group)
        );
        assert_eq!(
            coordinator.origin_group_for_realm(inherited),
            Some(first_group)
        );
        assert_ne!(
            coordinator.origin_group_for_realm(cross_origin),
            Some(first_group)
        );
        assert_ne!(
            coordinator.origin_group_for_realm(sandboxed),
            Some(first_group)
        );

        assert_eq!(
            coordinator
                .evaluate_value_in_realm(first, "globalThis.marker = 42; marker")
                .unwrap(),
            JsValue::Number(42.0)
        );
        assert_eq!(
            coordinator
                .evaluate_value_in_realm(second, "typeof marker")
                .unwrap(),
            JsValue::String("undefined".to_string()),
            "same-origin Realms share the host origin group but not their JS global environments"
        );
        assert_eq!(
            coordinator
                .evaluate_value_in_realm(cross_origin, "typeof marker")
                .unwrap(),
            JsValue::String("undefined".to_string())
        );

        coordinator
            .destroy_realm(first)
            .expect("first Realm destroyed");
        coordinator
            .destroy_realm(second)
            .expect("second Realm destroyed");
        coordinator
            .destroy_realm(cross_origin)
            .expect("cross-origin Realm destroyed");
        coordinator
            .destroy_realm(inherited)
            .expect("inherited Realm destroyed");
        coordinator
            .destroy_realm(sandboxed)
            .expect("sandboxed Realm destroyed");
        assert_eq!(coordinator.active_origin_group_count(), 0);
        assert!(matches!(
            coordinator.evaluate_value_in_realm(first, "1 + 1"),
            Err(super::BrowserRuntimeOriginError::UnknownRealm)
        ));
    }
}
