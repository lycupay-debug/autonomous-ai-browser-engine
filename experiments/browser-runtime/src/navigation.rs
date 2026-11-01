//! Phase 0 navigation, Document, and Frame lifecycle model.
//!
//! This controller creates a candidate Realm before commit, keeps the current
//! Document alive until commit succeeds, and destroys abandoned candidate
//! Realms on abort/replacement. It is a lifecycle model, not a network loader,
//! HTML parser, browsing-context implementation, or WHATWG navigation stack.

use std::collections::{HashMap, HashSet};
use std::fmt;

use aibe_origin_policy_poc::OriginGroupId;

use crate::{BrowserRuntimeOriginError, OriginRealmCoordinator, OriginRealmRuntime};

const DEFAULT_MAX_REDIRECTS: usize = 20;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FrameId(u64);

impl FrameId {
    pub const fn value(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DocumentId(u64);

impl DocumentId {
    pub const fn value(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NavigationId(u64);

impl NavigationId {
    pub const fn value(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NavigationMode {
    /// Resolve a fresh tuple origin from the target URL.
    Url,
    /// Explicitly inherit the current creator Document's origin group.
    Inherited,
    /// Create a fresh opaque origin group, e.g. for a sandboxed frame.
    Sandboxed,
}

#[derive(Debug)]
pub enum NavigationError<E> {
    Coordinator(BrowserRuntimeOriginError<E>),
    UnknownFrame,
    UnknownDocument,
    UnknownNavigation,
    NoCurrentDocument,
    NavigationNotPending,
    RedirectNotAllowed,
    InheritedOriginRequiresAboutUrl,
    RedirectLimit,
    IdExhausted,
}

impl<E: fmt::Display> fmt::Display for NavigationError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Coordinator(error) => write!(formatter, "navigation coordinator error: {error}"),
            Self::UnknownFrame => formatter.write_str("unknown Frame"),
            Self::UnknownDocument => formatter.write_str("unknown Document"),
            Self::UnknownNavigation => formatter.write_str("unknown Navigation"),
            Self::NoCurrentDocument => formatter.write_str("Frame has no current Document"),
            Self::NavigationNotPending => {
                formatter.write_str("Navigation is not pending on its Frame")
            }
            Self::RedirectNotAllowed => {
                formatter.write_str("redirect is not supported for this navigation mode")
            }
            Self::InheritedOriginRequiresAboutUrl => formatter.write_str(
                "inherited-origin navigation is limited to about:blank/about:srcdoc URLs",
            ),
            Self::RedirectLimit => formatter.write_str("navigation redirect limit exceeded"),
            Self::IdExhausted => formatter.write_str("navigation identifier space exhausted"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for NavigationError<E> {}

impl<E> From<BrowserRuntimeOriginError<E>> for NavigationError<E> {
    fn from(error: BrowserRuntimeOriginError<E>) -> Self {
        Self::Coordinator(error)
    }
}

pub struct Document<R: OriginRealmRuntime> {
    id: DocumentId,
    frame: FrameId,
    url: String,
    origin: OriginGroupId,
    realm: R::Realm,
}

impl<R: OriginRealmRuntime> Document<R> {
    pub const fn id(&self) -> DocumentId {
        self.id
    }
    pub const fn frame(&self) -> FrameId {
        self.frame
    }
    pub fn url(&self) -> &str {
        &self.url
    }
    pub const fn origin(&self) -> OriginGroupId {
        self.origin
    }
    pub const fn realm(&self) -> R::Realm {
        self.realm
    }
}

struct Frame {
    parent: Option<FrameId>,
    children: HashSet<FrameId>,
    current_document: Option<DocumentId>,
    pending_navigation: Option<NavigationId>,
}

struct Navigation<R: OriginRealmRuntime> {
    frame: FrameId,
    target_url: String,
    mode: NavigationMode,
    redirect_count: usize,
    target_origin: OriginGroupId,
    target_realm: R::Realm,
}

impl<R: OriginRealmRuntime> Clone for Navigation<R> {
    fn clone(&self) -> Self {
        Self {
            frame: self.frame,
            target_url: self.target_url.clone(),
            mode: self.mode,
            redirect_count: self.redirect_count,
            target_origin: self.target_origin,
            target_realm: self.target_realm,
        }
    }
}

/// Owns Frame/Document/Navigation references and delegates Realm lifetime to
/// `OriginRealmCoordinator`. No network, parser, layout, or Worker is included.
pub struct NavigationController<R: OriginRealmRuntime> {
    coordinator: OriginRealmCoordinator<R>,
    frames: HashMap<FrameId, Frame>,
    documents: HashMap<DocumentId, Document<R>>,
    navigations: HashMap<NavigationId, Navigation<R>>,
    next_frame_id: u64,
    next_document_id: u64,
    next_navigation_id: u64,
    max_redirects: usize,
}

impl<R: OriginRealmRuntime> NavigationController<R> {
    pub fn new(runtime: R) -> Result<Self, aibe_origin_policy_poc::OriginPolicyError> {
        Self::with_redirect_limit(runtime, DEFAULT_MAX_REDIRECTS)
    }

    pub fn with_redirect_limit(
        runtime: R,
        max_redirects: usize,
    ) -> Result<Self, aibe_origin_policy_poc::OriginPolicyError> {
        Ok(Self {
            coordinator: OriginRealmCoordinator::new(runtime)?,
            frames: HashMap::new(),
            documents: HashMap::new(),
            navigations: HashMap::new(),
            next_frame_id: 1,
            next_document_id: 1,
            next_navigation_id: 1,
            max_redirects,
        })
    }

    pub fn create_root_frame(&mut self, url: &str) -> Result<FrameId, NavigationError<R::Error>> {
        let frame = self.allocate_frame_id()?;
        let document = self.allocate_document_id()?;
        let realm = self.coordinator.create_realm_for_url(url)?;
        let origin = match self.coordinator.origin_group_for_realm(realm) {
            Some(origin) => origin,
            None => {
                let _ = self.coordinator.destroy_realm(realm);
                return Err(NavigationError::Coordinator(
                    BrowserRuntimeOriginError::UnknownRealm,
                ));
            }
        };
        self.frames.insert(
            frame,
            Frame {
                parent: None,
                children: HashSet::new(),
                current_document: Some(document),
                pending_navigation: None,
            },
        );
        self.documents.insert(
            document,
            Document {
                id: document,
                frame,
                url: url.to_owned(),
                origin,
                realm,
            },
        );
        Ok(frame)
    }

    pub fn create_child_frame(
        &mut self,
        parent: FrameId,
        url: &str,
        mode: NavigationMode,
    ) -> Result<FrameId, NavigationError<R::Error>> {
        Self::validate_navigation_target(url, mode)?;
        let parent_realm = {
            let parent_frame = self
                .frames
                .get(&parent)
                .ok_or(NavigationError::UnknownFrame)?;
            let document = parent_frame
                .current_document
                .ok_or(NavigationError::NoCurrentDocument)?;
            self.documents
                .get(&document)
                .ok_or(NavigationError::UnknownDocument)?
                .realm
        };
        let frame = self.allocate_frame_id()?;
        let document = self.allocate_document_id()?;
        let realm = self.create_realm(url, mode, Some(parent_realm))?;
        let origin = match self.coordinator.origin_group_for_realm(realm) {
            Some(origin) => origin,
            None => {
                let _ = self.coordinator.destroy_realm(realm);
                return Err(NavigationError::Coordinator(
                    BrowserRuntimeOriginError::UnknownRealm,
                ));
            }
        };
        self.frames.insert(
            frame,
            Frame {
                parent: Some(parent),
                children: HashSet::new(),
                current_document: Some(document),
                pending_navigation: None,
            },
        );
        self.frames
            .get_mut(&parent)
            .expect("validated parent remains present")
            .children
            .insert(frame);
        self.documents.insert(
            document,
            Document {
                id: document,
                frame,
                url: url.to_owned(),
                origin,
                realm,
            },
        );
        Ok(frame)
    }

    /// Creates a candidate Realm immediately but leaves the current Document
    /// active until `commit_navigation` succeeds. Starting another navigation
    /// replaces and destroys the previous candidate only after the new one exists.
    pub fn begin_navigation(
        &mut self,
        frame: FrameId,
        url: &str,
        mode: NavigationMode,
    ) -> Result<NavigationId, NavigationError<R::Error>> {
        Self::validate_navigation_target(url, mode)?;
        let creator_realm = {
            let frame_ref = self
                .frames
                .get(&frame)
                .ok_or(NavigationError::UnknownFrame)?;
            match mode {
                NavigationMode::Inherited => {
                    let document = frame_ref
                        .current_document
                        .ok_or(NavigationError::NoCurrentDocument)?;
                    Some(
                        self.documents
                            .get(&document)
                            .ok_or(NavigationError::UnknownDocument)?
                            .realm,
                    )
                }
                NavigationMode::Url | NavigationMode::Sandboxed => None,
            }
        };
        let id = self.allocate_navigation_id()?;
        let target_realm = self.create_realm(url, mode, creator_realm)?;
        let target_origin = match self.coordinator.origin_group_for_realm(target_realm) {
            Some(origin) => origin,
            None => {
                let _ = self.coordinator.destroy_realm(target_realm);
                return Err(NavigationError::Coordinator(
                    BrowserRuntimeOriginError::UnknownRealm,
                ));
            }
        };
        let previous = self
            .frames
            .get(&frame)
            .and_then(|entry| entry.pending_navigation);
        if let Some(previous_id) = previous
            && let Err(error) = self.abort_navigation(previous_id)
        {
            let _ = self.coordinator.destroy_realm(target_realm);
            return Err(error);
        }
        self.navigations.insert(
            id,
            Navigation {
                frame,
                target_url: url.to_owned(),
                mode,
                redirect_count: 0,
                target_origin,
                target_realm,
            },
        );
        self.frames
            .get_mut(&frame)
            .expect("validated frame remains present")
            .pending_navigation = Some(id);
        Ok(id)
    }

    pub fn redirect(
        &mut self,
        navigation: NavigationId,
        url: &str,
    ) -> Result<(), NavigationError<R::Error>> {
        let current = self
            .navigations
            .get(&navigation)
            .cloned()
            .ok_or(NavigationError::UnknownNavigation)?;
        if current.mode != NavigationMode::Url {
            return Err(NavigationError::RedirectNotAllowed);
        }
        if current.redirect_count >= self.max_redirects {
            return Err(NavigationError::RedirectLimit);
        }
        let target_realm = self.coordinator.create_realm_for_url(url)?;
        let target_origin = match self.coordinator.origin_group_for_realm(target_realm) {
            Some(origin) => origin,
            None => {
                let _ = self.coordinator.destroy_realm(target_realm);
                return Err(NavigationError::Coordinator(
                    BrowserRuntimeOriginError::UnknownRealm,
                ));
            }
        };
        if let Err(error) = self.coordinator.destroy_realm(current.target_realm) {
            let _ = self.coordinator.destroy_realm(target_realm);
            return Err(NavigationError::Coordinator(error));
        }
        let updated = self
            .navigations
            .get_mut(&navigation)
            .expect("navigation remains present");
        updated.target_url = url.to_owned();
        updated.target_realm = target_realm;
        updated.target_origin = target_origin;
        updated.redirect_count += 1;
        Ok(())
    }

    pub fn commit_navigation(
        &mut self,
        navigation: NavigationId,
    ) -> Result<DocumentId, NavigationError<R::Error>> {
        let candidate = self
            .navigations
            .get(&navigation)
            .cloned()
            .ok_or(NavigationError::UnknownNavigation)?;
        let frame = self
            .frames
            .get(&candidate.frame)
            .ok_or(NavigationError::UnknownFrame)?;
        if frame.pending_navigation != Some(navigation) {
            return Err(NavigationError::NavigationNotPending);
        }
        let old_document_id = frame.current_document;
        let document_id = self.allocate_document_id()?;
        if let Some(old_document_id) = old_document_id {
            let old_document = self
                .documents
                .get(&old_document_id)
                .ok_or(NavigationError::UnknownDocument)?;
            self.coordinator.destroy_realm(old_document.realm)?;
            self.documents.remove(&old_document_id);
        }
        self.documents.insert(
            document_id,
            Document {
                id: document_id,
                frame: candidate.frame,
                url: candidate.target_url,
                origin: candidate.target_origin,
                realm: candidate.target_realm,
            },
        );
        let frame = self
            .frames
            .get_mut(&candidate.frame)
            .expect("frame remains present during commit");
        frame.current_document = Some(document_id);
        frame.pending_navigation = None;
        self.navigations.remove(&navigation);
        Ok(document_id)
    }

    pub fn abort_navigation(
        &mut self,
        navigation: NavigationId,
    ) -> Result<(), NavigationError<R::Error>> {
        let candidate = self
            .navigations
            .get(&navigation)
            .cloned()
            .ok_or(NavigationError::UnknownNavigation)?;
        self.coordinator.destroy_realm(candidate.target_realm)?;
        self.navigations.remove(&navigation);
        if let Some(frame) = self.frames.get_mut(&candidate.frame)
            && frame.pending_navigation == Some(navigation)
        {
            frame.pending_navigation = None;
        }
        Ok(())
    }

    /// Detaches a Frame subtree in post-order. If Runtime destruction fails,
    /// already-detached descendants stay detached and the remaining tree can be retried.
    pub fn detach_frame(&mut self, frame: FrameId) -> Result<usize, NavigationError<R::Error>> {
        let children = self
            .frames
            .get(&frame)
            .ok_or(NavigationError::UnknownFrame)?
            .children
            .iter()
            .copied()
            .collect::<Vec<_>>();
        let mut detached = 0;
        for child in children {
            detached += self.detach_frame(child)?;
        }
        let current = self
            .frames
            .get(&frame)
            .ok_or(NavigationError::UnknownFrame)?;
        if let Some(navigation) = current.pending_navigation {
            self.abort_navigation(navigation)?;
        }
        let current_document = self
            .frames
            .get(&frame)
            .and_then(|entry| entry.current_document);
        if let Some(document_id) = current_document {
            let document = self
                .documents
                .get(&document_id)
                .ok_or(NavigationError::UnknownDocument)?;
            self.coordinator.destroy_realm(document.realm)?;
            self.documents.remove(&document_id);
        }
        let removed = self
            .frames
            .remove(&frame)
            .ok_or(NavigationError::UnknownFrame)?;
        if let Some(parent) = removed.parent
            && let Some(parent_frame) = self.frames.get_mut(&parent)
        {
            parent_frame.children.remove(&frame);
        }
        Ok(detached + 1)
    }

    pub fn current_document_id(&self, frame: FrameId) -> Option<DocumentId> {
        self.frames
            .get(&frame)
            .and_then(|entry| entry.current_document)
    }

    pub fn document(&self, document: DocumentId) -> Option<&Document<R>> {
        self.documents.get(&document)
    }

    pub fn frame_parent(&self, frame: FrameId) -> Option<FrameId> {
        self.frames.get(&frame).and_then(|entry| entry.parent)
    }

    pub fn frame_children(&self, frame: FrameId) -> Option<Vec<FrameId>> {
        self.frames.get(&frame).map(|entry| {
            let mut children = entry.children.iter().copied().collect::<Vec<_>>();
            children.sort_by_key(|child| child.0);
            children
        })
    }

    pub fn active_frame_count(&self) -> usize {
        self.frames.len()
    }
    pub fn active_document_count(&self) -> usize {
        self.documents.len()
    }
    pub fn pending_navigation_count(&self) -> usize {
        self.navigations.len()
    }
    pub fn coordinator(&self) -> &OriginRealmCoordinator<R> {
        &self.coordinator
    }

    fn validate_navigation_target(
        url: &str,
        mode: NavigationMode,
    ) -> Result<(), NavigationError<R::Error>> {
        if mode == NavigationMode::Inherited && !is_inheriting_about_url(url) {
            return Err(NavigationError::InheritedOriginRequiresAboutUrl);
        }
        Ok(())
    }

    fn create_realm(
        &mut self,
        url: &str,
        mode: NavigationMode,
        creator_realm: Option<R::Realm>,
    ) -> Result<R::Realm, NavigationError<R::Error>> {
        match mode {
            NavigationMode::Url => self
                .coordinator
                .create_realm_for_url(url)
                .map_err(Into::into),
            NavigationMode::Inherited => self
                .coordinator
                .create_inherited_realm(creator_realm.ok_or(NavigationError::NoCurrentDocument)?)
                .map_err(Into::into),
            NavigationMode::Sandboxed => self
                .coordinator
                .create_sandboxed_realm()
                .map_err(Into::into),
        }
    }

    fn allocate_frame_id(&mut self) -> Result<FrameId, NavigationError<R::Error>> {
        let id = FrameId(self.next_frame_id);
        self.next_frame_id = self
            .next_frame_id
            .checked_add(1)
            .ok_or(NavigationError::IdExhausted)?;
        Ok(id)
    }

    fn allocate_document_id(&mut self) -> Result<DocumentId, NavigationError<R::Error>> {
        let id = DocumentId(self.next_document_id);
        self.next_document_id = self
            .next_document_id
            .checked_add(1)
            .ok_or(NavigationError::IdExhausted)?;
        Ok(id)
    }

    fn allocate_navigation_id(&mut self) -> Result<NavigationId, NavigationError<R::Error>> {
        let id = NavigationId(self.next_navigation_id);
        self.next_navigation_id = self
            .next_navigation_id
            .checked_add(1)
            .ok_or(NavigationError::IdExhausted)?;
        Ok(id)
    }
}

fn is_inheriting_about_url(url: &str) -> bool {
    ["about:blank", "about:srcdoc"].iter().any(|base| {
        url == *base
            || url
                .strip_prefix(base)
                .is_some_and(|suffix| suffix.starts_with('#') || suffix.starts_with('?'))
    })
}
