//! Phase 0 Browser Runtime orchestration for URL origins and JS Realms.
//!
//! This layer owns the mapping from canonical host Origin Groups to opaque
//! JavaScript Runtime origin handles. The URL parser is not part of the V8
//! adapter, and page JavaScript cannot request or manufacture these grants.

pub mod navigation;

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
    fn initialize_document(&mut self, _realm: Self::Realm) -> Result<(), Self::Error> {
        Ok(())
    }
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

    fn initialize_document(&mut self, realm: Self::Realm) -> Result<(), Self::Error> {
        JsRuntime::initialize_document(self, realm)
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

    /// Installs the minimal host Document object only in a tracked Document Realm.
    pub fn initialize_document(
        &mut self,
        realm: R::Realm,
    ) -> Result<(), BrowserRuntimeOriginError<R::Error>> {
        if !self.realm_origins.contains_key(&realm) {
            return Err(BrowserRuntimeOriginError::UnknownRealm);
        }
        self.runtime
            .initialize_document(realm)
            .map_err(BrowserRuntimeOriginError::Runtime)
    }

    /// Checks a URL against a live host-issued Origin Group without creating
    /// a temporary JS Realm or extending that group's lifetime.
    pub fn url_matches_origin_group(
        &self,
        origin: OriginGroupId,
        url: &str,
    ) -> Result<bool, BrowserRuntimeOriginError<R::Error>> {
        self.registry
            .url_matches_origin(origin, url)
            .map_err(BrowserRuntimeOriginError::Origin)
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
    use std::collections::{HashMap, HashSet};
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
        fail_on_realm: Option<u64>,
        fail_destroy_realm_once: Option<u64>,
        fail_initialize_document_once: Option<u64>,
        initialized_documents: HashSet<MockRealm>,
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
            let candidate_realm = self.next_realm + 1;
            if self.fail_next_realm || self.fail_on_realm == Some(candidate_realm) {
                self.fail_next_realm = false;
                return Err(MockError);
            }
            self.next_realm = candidate_realm;
            let realm = MockRealm(self.next_realm);
            self.realms.insert(realm, origin);
            Ok(realm)
        }

        fn initialize_document(&mut self, realm: Self::Realm) -> Result<(), Self::Error> {
            if self.fail_initialize_document_once == Some(realm.0) {
                self.fail_initialize_document_once = None;
                return Err(MockError);
            }
            if !self.realms.contains_key(&realm) {
                return Err(MockError);
            }
            self.initialized_documents.insert(realm);
            Ok(())
        }

        fn destroy_realm(&mut self, realm: Self::Realm) -> Result<(), Self::Error> {
            if self.fail_destroy_realm_once == Some(realm.0) {
                self.fail_destroy_realm_once = None;
                return Err(MockError);
            }
            self.realms.remove(&realm).ok_or(MockError)?;
            self.initialized_documents.remove(&realm);
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
    fn document_realms_are_initialized_but_worker_realms_are_not() {
        use super::navigation::{NavigationController, NavigationMode};
        let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
        let frame = controller.create_root_frame("https://a.example/").unwrap();
        let document = controller.current_document_id(frame).unwrap();
        let document_realm = controller.document(document).unwrap().realm();
        assert!(
            controller
                .coordinator()
                .runtime()
                .initialized_documents
                .contains(&document_realm)
        );

        let worker = controller
            .create_dedicated_worker(document, "https://a.example/worker.js")
            .unwrap();
        let worker_realm = controller.worker(worker).unwrap().realm();
        assert!(
            !controller
                .coordinator()
                .runtime()
                .initialized_documents
                .contains(&worker_realm)
        );

        let navigation = controller
            .begin_navigation(frame, "https://b.example/next", NavigationMode::Url)
            .unwrap();
        assert_eq!(
            controller
                .coordinator()
                .runtime()
                .initialized_documents
                .len(),
            2,
            "the candidate Document is initialized before commit while the old Document remains live"
        );
        let committed = controller.commit_navigation(navigation).unwrap();
        let committed_realm = controller.document(committed).unwrap().realm();
        assert!(
            controller
                .coordinator()
                .runtime()
                .initialized_documents
                .contains(&committed_realm)
        );
        assert!(
            !controller
                .coordinator()
                .runtime()
                .initialized_documents
                .contains(&document_realm)
        );
        assert!(
            !controller
                .coordinator()
                .runtime()
                .initialized_documents
                .contains(&worker_realm)
        );
    }

    #[test]
    fn child_document_realms_initialize_for_inherited_and_sandboxed_frames() {
        use super::navigation::{NavigationController, NavigationMode};
        let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
        let root = controller.create_root_frame("https://a.example/").unwrap();
        let inherited = controller
            .create_child_frame(root, "about:blank", NavigationMode::Inherited)
            .unwrap();
        let sandboxed = controller
            .create_child_frame(root, "about:blank", NavigationMode::Sandboxed)
            .unwrap();

        for frame in [root, inherited, sandboxed] {
            let document = controller.current_document_id(frame).unwrap();
            let realm = controller.document(document).unwrap().realm();
            assert!(
                controller
                    .coordinator()
                    .runtime()
                    .initialized_documents
                    .contains(&realm)
            );
        }
        assert_eq!(
            controller
                .coordinator()
                .runtime()
                .initialized_documents
                .len(),
            3
        );
    }

    #[test]
    fn redirect_reinitializes_document_surface_for_the_final_candidate_realm() {
        use super::navigation::{NavigationController, NavigationMode};
        let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
        let frame = controller.create_root_frame("https://a.example/").unwrap();
        let navigation = controller
            .begin_navigation(frame, "https://b.example/first", NavigationMode::Url)
            .unwrap();
        assert_eq!(
            controller
                .coordinator()
                .runtime()
                .initialized_documents
                .len(),
            2
        );
        controller
            .redirect(navigation, "https://c.example/final")
            .unwrap();
        assert_eq!(
            controller
                .coordinator()
                .runtime()
                .initialized_documents
                .len(),
            2
        );
        let committed = controller.commit_navigation(navigation).unwrap();
        let realm = controller.document(committed).unwrap().realm();
        assert!(
            controller
                .coordinator()
                .runtime()
                .initialized_documents
                .contains(&realm)
        );
        assert_eq!(
            controller
                .coordinator()
                .runtime()
                .initialized_documents
                .len(),
            1
        );
    }

    #[test]
    fn root_document_initialization_failure_releases_the_new_realm_and_origin() {
        let runtime = MockRuntime {
            fail_initialize_document_once: Some(1),
            ..MockRuntime::default()
        };
        let mut controller = super::navigation::NavigationController::new(runtime).unwrap();
        assert!(controller.create_root_frame("https://a.example/").is_err());
        assert_eq!(controller.coordinator().active_origin_group_count(), 0);
        assert!(controller.coordinator().runtime().realms.is_empty());
        assert!(
            controller
                .coordinator()
                .runtime()
                .initialized_documents
                .is_empty()
        );
    }

    #[test]
    fn failed_document_initialization_cleanup_is_retained_and_retryable() {
        let runtime = MockRuntime {
            fail_initialize_document_once: Some(1),
            fail_destroy_realm_once: Some(1),
            ..MockRuntime::default()
        };
        let mut controller = super::navigation::NavigationController::new(runtime).unwrap();
        assert!(matches!(
            controller.create_root_frame("https://a.example/"),
            Err(super::navigation::NavigationError::DocumentInitializationCleanupFailed { .. })
        ));
        assert_eq!(controller.coordinator().active_origin_group_count(), 1);
        assert_eq!(controller.coordinator().runtime().realms.len(), 1);
        assert_eq!(
            controller.retry_document_initialization_cleanup().unwrap(),
            1
        );
        assert_eq!(controller.coordinator().active_origin_group_count(), 0);
        assert!(controller.coordinator().runtime().realms.is_empty());
    }

    #[test]
    fn document_initialization_failure_cleans_candidate_and_preserves_current_document() {
        use super::navigation::{NavigationController, NavigationMode};
        let runtime = MockRuntime {
            fail_initialize_document_once: Some(2),
            ..MockRuntime::default()
        };
        let mut controller = NavigationController::new(runtime).unwrap();
        let frame = controller.create_root_frame("https://a.example/").unwrap();
        let original = controller.current_document_id(frame).unwrap();
        assert!(
            controller
                .begin_navigation(frame, "https://b.example/next", NavigationMode::Url)
                .is_err()
        );
        assert_eq!(controller.current_document_id(frame), Some(original));
        assert_eq!(controller.pending_navigation_count(), 0);
        assert_eq!(controller.coordinator().active_origin_group_count(), 1);
        assert_eq!(controller.coordinator().runtime().realms.len(), 1);
        assert_eq!(
            controller
                .coordinator()
                .runtime()
                .initialized_documents
                .len(),
            1
        );
    }

    #[test]
    fn current_document_owns_the_parsed_html_tree() {
        use super::navigation::NavigationController;
        let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
        let frame = controller.create_root_frame("https://a.example/").unwrap();
        let document = controller.current_document_id(frame).unwrap();
        controller
            .parse_document_html(
                document,
                b"<!doctype html><title>Owned</title><main><p id='status'>ready</p></main>",
            )
            .unwrap();
        let html = controller
            .document(document)
            .unwrap()
            .html_document()
            .unwrap();
        let title = html.find_first_element_by_tag_name("title").unwrap();
        let status = html.find_element_by_id("status").unwrap();
        assert_eq!(html.text_content(title).as_deref(), Some("Owned"));
        assert_eq!(html.text_content(status).as_deref(), Some("ready"));
    }

    #[test]
    fn failed_html_parse_preserves_the_current_document_tree() {
        use super::navigation::{NavigationController, NavigationError};
        use aibe_html_parser_poc::{HtmlParseError, MAX_HTML_INPUT_BYTES};
        let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
        let frame = controller.create_root_frame("https://a.example/").unwrap();
        let document = controller.current_document_id(frame).unwrap();
        controller
            .parse_document_html(document, b"<p>stable</p>")
            .unwrap();
        let oversized = vec![b'x'; MAX_HTML_INPUT_BYTES + 1];
        assert!(matches!(
            controller.parse_document_html(document, &oversized),
            Err(NavigationError::HtmlParse(
                HtmlParseError::InputTooLarge { .. }
            ))
        ));
        let html = controller
            .document(document)
            .unwrap()
            .html_document()
            .unwrap();
        let paragraph = html.find_first_element_by_tag_name("p").unwrap();
        assert_eq!(html.text_content(paragraph).as_deref(), Some("stable"));
    }

    #[test]
    fn navigation_parse_failure_does_not_commit_or_destroy_the_current_document() {
        use super::navigation::{NavigationController, NavigationError, NavigationMode};
        use aibe_html_parser_poc::{HtmlParseError, MAX_HTML_INPUT_BYTES};
        let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
        let frame = controller.create_root_frame("https://a.example/").unwrap();
        let original = controller.current_document_id(frame).unwrap();
        let navigation = controller
            .begin_navigation(frame, "https://b.example/", NavigationMode::Url)
            .unwrap();
        let oversized = vec![b'x'; MAX_HTML_INPUT_BYTES + 1];
        assert!(matches!(
            controller.commit_navigation_with_html(navigation, &oversized),
            Err(NavigationError::HtmlParse(
                HtmlParseError::InputTooLarge { .. }
            ))
        ));
        assert_eq!(controller.current_document_id(frame), Some(original));
        assert_eq!(controller.pending_navigation_count(), 1);

        let committed = controller
            .commit_navigation_with_html(navigation, b"<!doctype html><title>new</title>")
            .unwrap();
        assert_ne!(committed, original);
        assert!(controller.document(original).is_none());
        let html = controller
            .document(committed)
            .unwrap()
            .html_document()
            .unwrap();
        let title = html.find_first_element_by_tag_name("title").unwrap();
        assert_eq!(html.text_content(title).as_deref(), Some("new"));
    }

    #[test]
    fn root_navigation_commit_replaces_document_only_at_commit() {
        use super::navigation::{NavigationController, NavigationMode};
        let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
        let frame = controller
            .create_root_frame("https://a.example/start")
            .unwrap();
        let original = controller.current_document_id(frame).unwrap();
        let original_origin = controller.document(original).unwrap().origin();
        let navigation = controller
            .begin_navigation(frame, "https://b.example/next", NavigationMode::Url)
            .unwrap();
        assert_eq!(controller.current_document_id(frame), Some(original));
        assert_eq!(
            controller.document(original).unwrap().origin(),
            original_origin
        );
        assert_eq!(controller.pending_navigation_count(), 1);
        let committed = controller.commit_navigation(navigation).unwrap();
        assert_ne!(committed, original);
        assert_eq!(controller.current_document_id(frame), Some(committed));
        assert_eq!(
            controller.document(committed).unwrap().url(),
            "https://b.example/next"
        );
        assert_ne!(
            controller.document(committed).unwrap().origin(),
            original_origin
        );
        assert!(controller.document(original).is_none());
        assert_eq!(controller.pending_navigation_count(), 0);
    }

    #[test]
    fn parent_document_commit_detaches_descendant_frames_and_workers() {
        use super::navigation::{NavigationController, NavigationMode};
        let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
        let root = controller.create_root_frame("https://a.example/").unwrap();
        let child = controller
            .create_child_frame(root, "about:blank", NavigationMode::Inherited)
            .unwrap();
        let grandchild = controller
            .create_child_frame(child, "https://b.example/", NavigationMode::Url)
            .unwrap();
        let old_document = controller.current_document_id(root).unwrap();
        let child_document = controller.current_document_id(child).unwrap();
        let grandchild_document = controller.current_document_id(grandchild).unwrap();
        let child_worker = controller
            .create_dedicated_worker(child_document, "https://a.example/child.js")
            .unwrap();
        let grandchild_worker = controller
            .create_dedicated_worker(grandchild_document, "https://b.example/grandchild.js")
            .unwrap();
        let navigation = controller
            .begin_navigation(root, "https://c.example/new", NavigationMode::Url)
            .unwrap();

        let new_document = controller.commit_navigation(navigation).unwrap();

        assert_ne!(new_document, old_document);
        assert!(controller.document(old_document).is_none());
        assert!(controller.document(child_document).is_none());
        assert!(controller.document(grandchild_document).is_none());
        assert!(controller.worker(child_worker).is_none());
        assert!(controller.worker(grandchild_worker).is_none());
        assert!(controller.frame_children(root).unwrap().is_empty());
        assert!(controller.frame_parent(child).is_none());
        assert!(controller.frame_parent(grandchild).is_none());
        assert_eq!(controller.active_frame_count(), 1);
        assert_eq!(controller.active_document_count(), 1);
        assert_eq!(controller.active_worker_count(), 0);
        assert_eq!(controller.coordinator().active_origin_group_count(), 1);
    }

    #[test]
    fn descendant_teardown_failure_keeps_parent_navigation_retryable() {
        use super::navigation::{NavigationController, NavigationMode, WorkerState};
        let runtime = MockRuntime {
            // Root, child, and grandchild Realms are allocated first; the
            // grandchild Worker Realm is therefore the fourth Runtime Realm.
            fail_destroy_realm_once: Some(4),
            ..MockRuntime::default()
        };
        let mut controller = NavigationController::new(runtime).unwrap();
        let root = controller.create_root_frame("https://a.example/").unwrap();
        let child = controller
            .create_child_frame(root, "about:blank", NavigationMode::Inherited)
            .unwrap();
        let grandchild = controller
            .create_child_frame(child, "https://b.example/", NavigationMode::Url)
            .unwrap();
        let old_document = controller.current_document_id(root).unwrap();
        let grandchild_document = controller.current_document_id(grandchild).unwrap();
        let worker = controller
            .create_dedicated_worker(grandchild_document, "https://b.example/worker.js")
            .unwrap();
        let navigation = controller
            .begin_navigation(root, "https://c.example/new", NavigationMode::Url)
            .unwrap();

        assert!(controller.commit_navigation(navigation).is_err());
        assert_eq!(controller.current_document_id(root), Some(old_document));
        assert!(controller.document(old_document).is_some());
        assert!(controller.frame_parent(grandchild).is_some());
        assert_eq!(
            controller.worker(worker).unwrap().state(),
            WorkerState::Terminating
        );
        assert_eq!(controller.pending_navigation_count(), 1);

        let committed = controller.commit_navigation(navigation).unwrap();
        assert_ne!(committed, old_document);
        assert!(controller.frame_parent(child).is_none());
        assert!(controller.frame_parent(grandchild).is_none());
        assert!(controller.worker(worker).is_none());
        assert_eq!(controller.active_frame_count(), 1);
        assert_eq!(controller.active_document_count(), 1);
        assert_eq!(controller.active_worker_count(), 0);
    }

    #[test]
    fn abort_preserves_current_document_and_releases_candidate_realm() {
        use super::navigation::{NavigationController, NavigationMode};
        let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
        let frame = controller.create_root_frame("https://a.example/").unwrap();
        let original = controller.current_document_id(frame).unwrap();
        let navigation = controller
            .begin_navigation(frame, "https://b.example/", NavigationMode::Url)
            .unwrap();
        assert_eq!(controller.coordinator().active_origin_group_count(), 2);
        controller.abort_navigation(navigation).unwrap();
        assert_eq!(controller.current_document_id(frame), Some(original));
        assert_eq!(controller.coordinator().active_origin_group_count(), 1);
        assert_eq!(controller.pending_navigation_count(), 0);
    }

    #[test]
    fn redirect_replaces_candidate_origin_and_enforces_limit() {
        use super::navigation::{NavigationController, NavigationError, NavigationMode};
        let mut controller =
            NavigationController::with_redirect_limit(MockRuntime::default(), 1).unwrap();
        let frame = controller.create_root_frame("https://a.example/").unwrap();
        let navigation = controller
            .begin_navigation(frame, "https://b.example/", NavigationMode::Url)
            .unwrap();
        controller
            .redirect(navigation, "https://c.example/final")
            .unwrap();
        assert!(matches!(
            controller.redirect(navigation, "https://d.example/"),
            Err(NavigationError::RedirectLimit)
        ));
        let document = controller.commit_navigation(navigation).unwrap();
        assert_eq!(
            controller.document(document).unwrap().url(),
            "https://c.example/final"
        );
    }

    #[test]
    fn starting_new_navigation_cancels_previous_candidate() {
        use super::navigation::{NavigationController, NavigationMode};
        let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
        let frame = controller.create_root_frame("https://a.example/").unwrap();
        let first = controller
            .begin_navigation(frame, "https://b.example/", NavigationMode::Url)
            .unwrap();
        let second = controller
            .begin_navigation(frame, "https://c.example/", NavigationMode::Url)
            .unwrap();
        assert_eq!(controller.pending_navigation_count(), 1);
        assert!(controller.abort_navigation(first).is_err());
        let document = controller.commit_navigation(second).unwrap();
        assert_eq!(
            controller.document(document).unwrap().url(),
            "https://c.example/"
        );
        assert_eq!(controller.coordinator().active_origin_group_count(), 1);
    }

    #[test]
    fn inherited_and_sandboxed_child_frames_have_distinct_origin_semantics() {
        use super::navigation::{NavigationController, NavigationMode};
        let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
        let root = controller.create_root_frame("https://a.example/").unwrap();
        let inherited = controller
            .create_child_frame(root, "about:blank", NavigationMode::Inherited)
            .unwrap();
        let sandboxed = controller
            .create_child_frame(root, "https://a.example/frame", NavigationMode::Sandboxed)
            .unwrap();
        let root_doc = controller
            .document(controller.current_document_id(root).unwrap())
            .unwrap();
        let inherited_doc = controller
            .document(controller.current_document_id(inherited).unwrap())
            .unwrap();
        let sandboxed_doc = controller
            .document(controller.current_document_id(sandboxed).unwrap())
            .unwrap();
        assert_eq!(root_doc.origin(), inherited_doc.origin());
        assert_ne!(root_doc.realm(), inherited_doc.realm());
        assert_ne!(root_doc.origin(), sandboxed_doc.origin());
        assert_eq!(controller.frame_parent(inherited), Some(root));
        assert_eq!(controller.frame_children(root).unwrap().len(), 2);
    }

    #[test]
    fn failed_candidate_realm_creation_keeps_current_document() {
        use super::navigation::{NavigationController, NavigationError, NavigationMode};
        let runtime = MockRuntime {
            fail_on_realm: Some(2),
            ..MockRuntime::default()
        };
        let mut controller = NavigationController::new(runtime).unwrap();
        let frame = controller.create_root_frame("https://a.example/").unwrap();
        let original = controller.current_document_id(frame).unwrap();
        assert!(matches!(
            controller.begin_navigation(frame, "https://b.example/", NavigationMode::Url),
            Err(NavigationError::Coordinator(_))
        ));
        assert_eq!(controller.current_document_id(frame), Some(original));
        assert_eq!(controller.pending_navigation_count(), 0);
        assert_eq!(controller.coordinator().active_origin_group_count(), 1);
    }

    #[test]
    fn detaching_frame_recursively_releases_documents_and_pending_navigation() {
        use super::navigation::{NavigationController, NavigationMode};
        let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
        let root = controller.create_root_frame("https://a.example/").unwrap();
        let child = controller
            .create_child_frame(root, "about:blank", NavigationMode::Inherited)
            .unwrap();
        let grandchild = controller
            .create_child_frame(child, "https://other.example/", NavigationMode::Sandboxed)
            .unwrap();
        let pending = controller
            .begin_navigation(child, "https://b.example/", NavigationMode::Url)
            .unwrap();
        assert_eq!(controller.detach_frame(root).unwrap(), 3);
        assert_eq!(controller.active_frame_count(), 0);
        assert_eq!(controller.active_document_count(), 0);
        assert_eq!(controller.pending_navigation_count(), 0);
        assert!(controller.abort_navigation(pending).is_err());
        assert!(controller.frame_children(root).is_none());
        assert!(controller.frame_parent(grandchild).is_none());
        assert_eq!(controller.coordinator().active_origin_group_count(), 0);
    }

    #[test]
    fn frame_detach_processes_siblings_in_stable_id_order_on_failure() {
        use super::navigation::{NavigationController, NavigationMode};
        let runtime = MockRuntime {
            // Root Realm is 1 and the first-created child Realm is 2.
            fail_destroy_realm_once: Some(2),
            ..MockRuntime::default()
        };
        let mut controller = NavigationController::new(runtime).unwrap();
        let root = controller.create_root_frame("https://a.example/").unwrap();
        let first_child = controller
            .create_child_frame(root, "https://b.example/", NavigationMode::Url)
            .unwrap();
        let second_child = controller
            .create_child_frame(root, "https://c.example/", NavigationMode::Url)
            .unwrap();

        assert!(controller.detach_frame(root).is_err());
        assert_eq!(controller.frame_parent(first_child), Some(root));
        assert_eq!(controller.frame_parent(second_child), Some(root));
        assert_eq!(controller.active_frame_count(), 3);

        assert_eq!(controller.detach_frame(root).unwrap(), 3);
        assert_eq!(controller.active_frame_count(), 0);
        assert_eq!(controller.active_document_count(), 0);
        assert_eq!(controller.coordinator().active_origin_group_count(), 0);
    }

    #[test]
    fn dedicated_worker_uses_a_separate_realm_in_the_document_origin_group() {
        use super::navigation::{NavigationController, WorkerState};
        let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
        let frame = controller.create_root_frame("https://a.example/").unwrap();
        let document = controller.current_document_id(frame).unwrap();
        let document_realm = controller.document(document).unwrap().realm();
        let worker = controller
            .create_dedicated_worker(document, "https://a.example/worker.js")
            .unwrap();
        let worker_record = controller.worker(worker).unwrap();
        assert_eq!(worker_record.state(), WorkerState::Running);
        assert_eq!(worker_record.owner_document(), document);
        assert_eq!(
            worker_record.origin(),
            controller.document(document).unwrap().origin()
        );
        assert_ne!(worker_record.realm(), document_realm);
        assert_eq!(controller.active_worker_count(), 1);
        assert_eq!(controller.workers_for_document(document), vec![worker]);
        controller.terminate_worker(worker).unwrap();
        assert_eq!(controller.active_worker_count(), 0);
        assert!(controller.terminate_worker(worker).is_err());
    }

    #[test]
    fn dedicated_worker_accepts_same_origin_redirect_chain_and_uses_final_url() {
        use super::navigation::NavigationController;
        let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
        let frame = controller.create_root_frame("https://a.example/").unwrap();
        let document = controller.current_document_id(frame).unwrap();
        let worker = controller
            .create_dedicated_worker_after_fetch(
                document,
                "https://a.example/worker.js",
                &["https://a.example/assets/worker-v2.js"],
                "https://a.example/assets/worker-v2.js",
            )
            .unwrap();
        let record = controller.worker(worker).unwrap();
        assert_eq!(record.script_url(), "https://a.example/assets/worker-v2.js");
        assert_eq!(
            record.origin(),
            controller.document(document).unwrap().origin()
        );
        assert_eq!(controller.active_worker_count(), 1);
        assert_eq!(controller.coordinator().active_origin_group_count(), 1);
    }

    #[test]
    fn dedicated_worker_rejects_cross_origin_intermediate_redirect_without_realm_leak() {
        use super::navigation::{NavigationController, NavigationError};
        let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
        let frame = controller.create_root_frame("https://a.example/").unwrap();
        let document = controller.current_document_id(frame).unwrap();
        assert!(matches!(
            controller.create_dedicated_worker_after_fetch(
                document,
                "https://a.example/worker.js",
                &[
                    "https://cdn.example/worker.js",
                    "https://a.example/final-worker.js",
                ],
                "https://a.example/final-worker.js",
            ),
            Err(NavigationError::CrossOriginWorkerScript)
        ));
        assert_eq!(controller.active_worker_count(), 0);
        assert_eq!(controller.coordinator().active_origin_group_count(), 1);
    }

    #[test]
    fn dedicated_worker_rejects_cross_origin_final_response_without_realm_leak() {
        use super::navigation::{NavigationController, NavigationError};
        let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
        let frame = controller.create_root_frame("https://a.example/").unwrap();
        let document = controller.current_document_id(frame).unwrap();
        assert!(matches!(
            controller.create_dedicated_worker_after_fetch(
                document,
                "https://a.example/worker.js",
                &["https://a.example/redirected.js"],
                "https://b.example/final.js",
            ),
            Err(NavigationError::CrossOriginWorkerScript)
        ));
        assert_eq!(controller.active_worker_count(), 0);
        assert_eq!(controller.coordinator().active_origin_group_count(), 1);
    }

    #[test]
    fn cross_origin_dedicated_worker_script_is_rejected_without_leaking_a_realm() {
        use super::navigation::{NavigationController, NavigationError};
        let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
        let frame = controller.create_root_frame("https://a.example/").unwrap();
        let document = controller.current_document_id(frame).unwrap();
        assert!(matches!(
            controller.create_dedicated_worker(document, "https://b.example/worker.js"),
            Err(NavigationError::CrossOriginWorkerScript)
        ));
        assert_eq!(controller.active_worker_count(), 0);
        assert_eq!(controller.coordinator().active_origin_group_count(), 1);
    }

    #[test]
    fn document_commit_terminates_workers_owned_by_the_replaced_document() {
        use super::navigation::{NavigationController, NavigationMode};
        let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
        let frame = controller.create_root_frame("https://a.example/").unwrap();
        let old_document = controller.current_document_id(frame).unwrap();
        let worker = controller
            .create_dedicated_worker(old_document, "https://a.example/worker.js")
            .unwrap();
        let navigation = controller
            .begin_navigation(frame, "https://b.example/", NavigationMode::Url)
            .unwrap();
        let new_document = controller.commit_navigation(navigation).unwrap();
        assert_ne!(new_document, old_document);
        assert!(controller.document(old_document).is_none());
        assert!(controller.worker(worker).is_none());
        assert_eq!(controller.active_worker_count(), 0);
        assert_eq!(controller.coordinator().active_origin_group_count(), 1);
    }

    #[test]
    fn worker_termination_failure_keeps_a_retryable_terminating_record() {
        use super::navigation::{NavigationController, WorkerState};
        let runtime = MockRuntime {
            fail_destroy_realm_once: Some(2),
            ..MockRuntime::default()
        };
        let mut controller = NavigationController::new(runtime).unwrap();
        let frame = controller.create_root_frame("https://a.example/").unwrap();
        let document = controller.current_document_id(frame).unwrap();
        let worker = controller
            .create_dedicated_worker(document, "https://a.example/worker.js")
            .unwrap();
        assert!(controller.terminate_worker(worker).is_err());
        assert_eq!(
            controller.worker(worker).unwrap().state(),
            WorkerState::Terminating
        );
        controller.terminate_worker(worker).unwrap();
        assert_eq!(controller.active_worker_count(), 0);
        assert_eq!(controller.coordinator().active_origin_group_count(), 1);
    }

    #[test]
    fn frame_detach_terminates_workers_for_root_and_descendant_documents() {
        use super::navigation::{NavigationController, NavigationMode};
        let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
        let root = controller.create_root_frame("https://a.example/").unwrap();
        let child = controller
            .create_child_frame(root, "about:blank", NavigationMode::Inherited)
            .unwrap();
        let root_document = controller.current_document_id(root).unwrap();
        let child_document = controller.current_document_id(child).unwrap();
        controller
            .create_dedicated_worker(root_document, "https://a.example/root-worker.js")
            .unwrap();
        controller
            .create_dedicated_worker(child_document, "https://a.example/child-worker.js")
            .unwrap();
        assert_eq!(controller.active_worker_count(), 2);
        assert_eq!(controller.detach_frame(root).unwrap(), 2);
        assert_eq!(controller.active_worker_count(), 0);
        assert_eq!(controller.coordinator().active_origin_group_count(), 0);
    }

    #[test]
    fn inherited_origin_mode_rejects_non_about_urls() {
        use super::navigation::{NavigationController, NavigationError, NavigationMode};
        let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
        let root = controller.create_root_frame("https://a.example/").unwrap();
        assert!(matches!(
            controller.create_child_frame(root, "https://evil.example/", NavigationMode::Inherited),
            Err(NavigationError::InheritedOriginRequiresAboutUrl)
        ));
        assert!(matches!(
            controller.begin_navigation(root, "https://evil.example/", NavigationMode::Inherited),
            Err(NavigationError::InheritedOriginRequiresAboutUrl)
        ));
        let child = controller
            .create_child_frame(root, "about:blank#fragment", NavigationMode::Inherited)
            .unwrap();
        assert_eq!(controller.frame_parent(child), Some(root));
        assert_eq!(controller.active_frame_count(), 2);
    }

    #[test]
    fn failed_old_realm_destruction_keeps_navigation_retryable() {
        use super::navigation::{NavigationController, NavigationError, NavigationMode};
        let runtime = MockRuntime {
            fail_destroy_realm_once: Some(1),
            ..MockRuntime::default()
        };
        let mut controller = NavigationController::new(runtime).unwrap();
        let frame = controller.create_root_frame("https://a.example/").unwrap();
        let original = controller.current_document_id(frame).unwrap();
        let navigation = controller
            .begin_navigation(frame, "https://b.example/", NavigationMode::Url)
            .unwrap();
        assert!(matches!(
            controller.commit_navigation(navigation),
            Err(NavigationError::Coordinator(_))
        ));
        assert_eq!(controller.current_document_id(frame), Some(original));
        assert_eq!(controller.active_document_count(), 1);
        assert_eq!(controller.pending_navigation_count(), 1);
        let committed = controller.commit_navigation(navigation).unwrap();
        assert_ne!(committed, original);
        assert_eq!(controller.pending_navigation_count(), 0);
        assert_eq!(controller.coordinator().active_origin_group_count(), 1);
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
    fn navigation_controller_installs_and_evaluates_document_surface_in_v8() {
        use super::navigation::{NavigationController, NavigationMode};
        let runtime = V8Runtime::new().expect("V8 runtime initializes");
        let mut controller = NavigationController::new(runtime).expect("controller initializes");
        let frame = controller
            .create_root_frame("https://example.com/")
            .expect("root Frame creates a Document");
        let original = controller.current_document_id(frame).unwrap();
        assert_eq!(
            controller
                .evaluate_document_value(original, "document.nodeType")
                .unwrap(),
            JsValue::Number(9.0)
        );
        assert_eq!(
            controller
                .evaluate_document_value(
                    original,
                    "document.documentElement.appendChild(new Node()); document.documentElement.childCount()",
                )
                .unwrap(),
            JsValue::Number(1.0)
        );

        let child = controller
            .create_child_frame(frame, "about:blank", NavigationMode::Inherited)
            .expect("inherited child Frame creates a Document");
        let child_document = controller.current_document_id(child).unwrap();
        assert_eq!(
            controller
                .evaluate_document_value(child_document, "document.nodeType")
                .unwrap(),
            JsValue::Number(9.0)
        );

        let navigation = controller
            .begin_navigation(frame, "https://other.example/", NavigationMode::Url)
            .unwrap();
        assert_eq!(
            controller.current_document_id(frame),
            Some(original),
            "the old Document remains current until navigation commit"
        );
        let replacement = controller.commit_navigation(navigation).unwrap();
        assert_eq!(
            controller
                .evaluate_document_value(replacement, "document.nodeType")
                .unwrap(),
            JsValue::Number(9.0)
        );
        assert!(matches!(
            controller.evaluate_document_value(original, "document.nodeType"),
            Err(super::navigation::NavigationError::UnknownDocument)
        ));
        assert!(matches!(
            controller.evaluate_document_value(child_document, "document.nodeType"),
            Err(super::navigation::NavigationError::UnknownDocument)
        ));
    }

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
