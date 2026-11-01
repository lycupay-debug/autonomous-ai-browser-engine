//! In-memory Broker grant-policy feasibility model.
//!
//! This crate models one-shot descriptor-grant authorization, per-renderer and
//! global quotas, and the revocation action required by direct FD delegation.
//! It does not open sockets, pass descriptors, authenticate kernel peers, or
//! terminate processes. The caller must bind RendererPrincipal to a
//! Broker-owned process/socketpair and execute TerminateRenderer outcomes.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_REGISTRY_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RendererId(u64);

impl RendererId {
    /// Creates a Broker-issued process-instance ID. Reusing a PID must create a
    /// new RendererId so grants from an earlier process cannot be replayed.
    pub const fn new(value: u64) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RendererPrincipal {
    id: RendererId,
    pid: u32,
    uid: u32,
    gid: u32,
}

impl RendererPrincipal {
    pub const fn new(id: RendererId, pid: u32, uid: u32, gid: u32) -> Option<Self> {
        if pid == 0 {
            None
        } else {
            Some(Self { id, pid, uid, gid })
        }
    }

    pub const fn id(self) -> RendererId {
        self.id
    }

    pub const fn pid(self) -> u32 {
        self.pid
    }

    pub const fn uid(self) -> u32 {
        self.uid
    }

    pub const fn gid(self) -> u32 {
        self.gid
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct OriginGroupId(u64);

impl OriginGroupId {
    pub const fn new(value: u64) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct GrantId {
    registry_id: u64,
    sequence: u64,
}

impl GrantId {
    pub const fn value(self) -> u64 {
        self.sequence
    }

    pub const fn registry_id(self) -> u64 {
        self.registry_id
    }
}

pub const GRANT_WIRE_MAGIC: u32 = 0x4149_4245;
pub const GRANT_WIRE_VERSION: u16 = 1;
pub const GRANT_WIRE_DESCRIPTOR_COUNT: u16 = 1;

/// Fixed host-local C ABI mirrored by `origin-storage-capsicum-poc.c`.
/// This is not a network serialization format; peers must share the same ABI.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GrantWireHeader {
    pub magic: u32,
    pub version: u16,
    pub descriptor_count: u16,
    pub renderer_pid: u32,
    pub rights: u32,
    pub renderer_instance_id: u64,
    pub registry_id: u64,
    pub grant_sequence: u64,
    pub origin_group_id: u64,
}

const _: [(); 48] = [(); std::mem::size_of::<GrantWireHeader>()];

/// Capability subset supported by this prototype. Storage grants are
/// intentionally read-only; write/create rights are not currently grantable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StorageRights(u8);

impl StorageRights {
    const LOOKUP: u8 = 1 << 0;
    const READ: u8 = 1 << 1;
    const FSTAT: u8 = 1 << 2;
    const WRITE: u8 = 1 << 3;
    const CREATE: u8 = 1 << 4;
    const KNOWN: u8 = Self::LOOKUP | Self::READ | Self::FSTAT | Self::WRITE | Self::CREATE;

    pub const READ_ONLY: Self = Self(Self::LOOKUP | Self::READ | Self::FSTAT);

    pub const fn from_bits(bits: u8) -> Option<Self> {
        if bits & !Self::KNOWN == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    pub const fn bits(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GrantState {
    Issued,
    Delegated,
    RevocationPending,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaScope {
    Global,
    PerRenderer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RevocationOutcome {
    /// The descriptor was not transferred; no Renderer termination is needed.
    RevokedBeforeTransfer,
    /// A descriptor was delegated. Closing the Broker's copy cannot revoke it,
    /// so the caller must terminate this Renderer to remove all process-held FDs.
    TerminateRenderer {
        renderer: RendererPrincipal,
        pending_grants: usize,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GrantPermit {
    id: GrantId,
    renderer: RendererPrincipal,
    origin: OriginGroupId,
    rights: StorageRights,
}

impl GrantPermit {
    pub const fn id(self) -> GrantId {
        self.id
    }

    pub const fn renderer(self) -> RendererPrincipal {
        self.renderer
    }

    pub const fn origin(self) -> OriginGroupId {
        self.origin
    }

    pub const fn rights(self) -> StorageRights {
        self.rights
    }

    pub const fn wire_header(self) -> GrantWireHeader {
        GrantWireHeader {
            magic: GRANT_WIRE_MAGIC,
            version: GRANT_WIRE_VERSION,
            descriptor_count: GRANT_WIRE_DESCRIPTOR_COUNT,
            renderer_pid: self.renderer.pid,
            rights: self.rights.bits() as u32,
            renderer_instance_id: self.renderer.id.value(),
            registry_id: self.id.registry_id,
            grant_sequence: self.id.sequence,
            origin_group_id: self.origin.value(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GrantError {
    InvalidQuota,
    UnsupportedRights,
    QuotaExceeded(QuotaScope),
    UnknownGrant,
    WrongRenderer,
    WrongOrigin,
    RightsMismatch,
    Replay,
    TerminationPending,
    TerminationAlreadyPending,
    IdExhausted,
}

impl fmt::Display for GrantError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for GrantError {}

#[derive(Clone, Copy, Debug)]
struct GrantRecord {
    renderer: RendererPrincipal,
    origin: OriginGroupId,
    rights: StorageRights,
    state: GrantState,
}

/// A deterministic, in-memory policy registry. IDs are monotonic within this
/// registry and are not cryptographic tokens or durable identifiers.
pub struct GrantRegistry {
    registry_id: u64,
    next_id: u64,
    max_total_grants: usize,
    max_per_renderer: usize,
    grants: HashMap<GrantId, GrantRecord>,
}

impl GrantRegistry {
    pub fn new(max_total_grants: usize, max_per_renderer: usize) -> Result<Self, GrantError> {
        if max_total_grants == 0 || max_per_renderer == 0 || max_per_renderer > max_total_grants {
            return Err(GrantError::InvalidQuota);
        }
        let registry_id = NEXT_REGISTRY_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| GrantError::IdExhausted)?;
        Ok(Self {
            registry_id,
            next_id: 1,
            max_total_grants,
            max_per_renderer,
            grants: HashMap::new(),
        })
    }

    pub fn issue(
        &mut self,
        renderer: RendererPrincipal,
        origin: OriginGroupId,
        rights: StorageRights,
    ) -> Result<GrantId, GrantError> {
        if rights != StorageRights::READ_ONLY {
            return Err(GrantError::UnsupportedRights);
        }
        if self.active_total_count() >= self.max_total_grants {
            return Err(GrantError::QuotaExceeded(QuotaScope::Global));
        }
        if self.active_count_for_renderer(renderer) >= self.max_per_renderer {
            return Err(GrantError::QuotaExceeded(QuotaScope::PerRenderer));
        }
        let id = GrantId {
            registry_id: self.registry_id,
            sequence: self.next_id,
        };
        self.next_id = self.next_id.checked_add(1).ok_or(GrantError::IdExhausted)?;
        self.grants.insert(
            id,
            GrantRecord {
                renderer,
                origin,
                rights,
                state: GrantState::Issued,
            },
        );
        Ok(id)
    }

    /// Consumes a grant exactly once after the Broker has authenticated the
    /// socket peer and confirmed this process instance is the intended target.
    pub fn authorize_transfer(
        &mut self,
        id: GrantId,
        renderer: RendererPrincipal,
        origin: OriginGroupId,
        requested_rights: StorageRights,
    ) -> Result<GrantPermit, GrantError> {
        if id.registry_id != self.registry_id {
            return Err(GrantError::UnknownGrant);
        }
        let record = self.grants.get_mut(&id).ok_or(GrantError::UnknownGrant)?;
        if record.renderer != renderer {
            return Err(GrantError::WrongRenderer);
        }
        if record.origin != origin {
            return Err(GrantError::WrongOrigin);
        }
        if record.rights != requested_rights {
            return Err(GrantError::RightsMismatch);
        }
        match record.state {
            GrantState::Issued => record.state = GrantState::Delegated,
            GrantState::Delegated => return Err(GrantError::Replay),
            GrantState::RevocationPending => return Err(GrantError::TerminationPending),
        }
        Ok(GrantPermit {
            id,
            renderer,
            origin,
            rights: requested_rights,
        })
    }

    /// Revoking an issued grant removes it immediately. Revoking any delegated
    /// grant marks every grant for that Renderer pending and requires process
    /// termination before the records and quota can be released.
    pub fn revoke(&mut self, id: GrantId) -> Result<RevocationOutcome, GrantError> {
        if id.registry_id != self.registry_id {
            return Err(GrantError::UnknownGrant);
        }
        let record = *self.grants.get(&id).ok_or(GrantError::UnknownGrant)?;
        match record.state {
            GrantState::Issued => {
                self.grants.remove(&id);
                Ok(RevocationOutcome::RevokedBeforeTransfer)
            }
            GrantState::Delegated => {
                let mut pending_grants = 0;
                for grant in self.grants.values_mut() {
                    if grant.renderer == record.renderer {
                        grant.state = GrantState::RevocationPending;
                        pending_grants += 1;
                    }
                }
                Ok(RevocationOutcome::TerminateRenderer {
                    renderer: record.renderer,
                    pending_grants,
                })
            }
            GrantState::RevocationPending => Err(GrantError::TerminationAlreadyPending),
        }
    }

    /// Called only after the supervisor has confirmed that the process exited.
    pub fn release_renderer(&mut self, renderer: RendererPrincipal) -> usize {
        let renderer_grants: Vec<GrantId> = self
            .grants
            .iter()
            .filter_map(|(grant_id, grant)| (grant.renderer == renderer).then_some(*grant_id))
            .collect();
        let released = renderer_grants.len();
        for grant_id in renderer_grants {
            self.grants.remove(&grant_id);
        }
        released
    }

    pub fn state_of(&self, id: GrantId) -> Option<GrantState> {
        (id.registry_id == self.registry_id)
            .then(|| self.grants.get(&id).map(|record| record.state))
            .flatten()
    }

    pub fn active_count_for_renderer(&self, renderer: RendererPrincipal) -> usize {
        self.grants
            .values()
            .filter(|grant| grant.renderer == renderer)
            .count()
    }

    pub fn active_total_count(&self) -> usize {
        self.grants.len()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SnapshotGrantId {
    registry_id: u64,
    sequence: u64,
}

impl SnapshotGrantId {
    pub const fn value(self) -> u64 {
        self.sequence
    }
    pub const fn registry_id(self) -> u64 {
        self.registry_id
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotGrantRevocation {
    RevokedBeforeUse,
    AlreadyConsumed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotGrantError {
    InvalidIdentity,
    InvalidQuota,
    QuotaExceeded(QuotaScope),
    UnknownGrant,
    WrongRenderer,
    WrongOrigin,
    WrongDocument,
    StaleRevision,
    Replay,
    IdExhausted,
}

impl fmt::Display for SnapshotGrantError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for SnapshotGrantError {}

#[derive(Clone, Copy, Debug)]
struct SnapshotGrantRecord {
    renderer: RendererPrincipal,
    origin: OriginGroupId,
    document_id: u64,
    revision: u64,
    consumed: bool,
}

/// Broker-owned, one-shot authorization for one immutable Document snapshot.
/// All identity fields are supplied by the Broker when the grant is issued;
/// the renderer's payload is only checked against this record, never trusted.
pub struct SnapshotGrantRegistry {
    registry_id: u64,
    next_id: u64,
    max_total_grants: usize,
    max_per_renderer: usize,
    grants: HashMap<SnapshotGrantId, SnapshotGrantRecord>,
}

impl SnapshotGrantRegistry {
    pub fn new(
        max_total_grants: usize,
        max_per_renderer: usize,
    ) -> Result<Self, SnapshotGrantError> {
        if max_total_grants == 0 || max_per_renderer == 0 || max_per_renderer > max_total_grants {
            return Err(SnapshotGrantError::InvalidQuota);
        }
        let registry_id = NEXT_REGISTRY_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| SnapshotGrantError::IdExhausted)?;
        Ok(Self {
            registry_id,
            next_id: 1,
            max_total_grants,
            max_per_renderer,
            grants: HashMap::new(),
        })
    }

    pub fn issue(
        &mut self,
        renderer: RendererPrincipal,
        origin: OriginGroupId,
        document_id: u64,
        revision: u64,
    ) -> Result<SnapshotGrantId, SnapshotGrantError> {
        if document_id == 0 || revision == 0 {
            return Err(SnapshotGrantError::InvalidIdentity);
        }
        if self.grants.len() >= self.max_total_grants {
            return Err(SnapshotGrantError::QuotaExceeded(QuotaScope::Global));
        }
        if self
            .grants
            .values()
            .filter(|record| record.renderer == renderer)
            .count()
            >= self.max_per_renderer
        {
            return Err(SnapshotGrantError::QuotaExceeded(QuotaScope::PerRenderer));
        }
        let id = SnapshotGrantId {
            registry_id: self.registry_id,
            sequence: self.next_id,
        };
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(SnapshotGrantError::IdExhausted)?;
        self.grants.insert(
            id,
            SnapshotGrantRecord {
                renderer,
                origin,
                document_id,
                revision,
                consumed: false,
            },
        );
        Ok(id)
    }

    pub fn authorize(
        &mut self,
        id: SnapshotGrantId,
        renderer: RendererPrincipal,
        origin: OriginGroupId,
        document_id: u64,
        revision: u64,
    ) -> Result<(), SnapshotGrantError> {
        if id.registry_id != self.registry_id {
            return Err(SnapshotGrantError::UnknownGrant);
        }
        let record = self
            .grants
            .get_mut(&id)
            .ok_or(SnapshotGrantError::UnknownGrant)?;
        if record.renderer != renderer {
            return Err(SnapshotGrantError::WrongRenderer);
        }
        if record.origin != origin {
            return Err(SnapshotGrantError::WrongOrigin);
        }
        if record.document_id != document_id {
            return Err(SnapshotGrantError::WrongDocument);
        }
        if record.revision != revision {
            return Err(SnapshotGrantError::StaleRevision);
        }
        if record.consumed {
            return Err(SnapshotGrantError::Replay);
        }
        record.consumed = true;
        Ok(())
    }

    pub fn revoke(
        &mut self,
        id: SnapshotGrantId,
    ) -> Result<SnapshotGrantRevocation, SnapshotGrantError> {
        if id.registry_id != self.registry_id {
            return Err(SnapshotGrantError::UnknownGrant);
        }
        let record = self
            .grants
            .remove(&id)
            .ok_or(SnapshotGrantError::UnknownGrant)?;
        Ok(if record.consumed {
            SnapshotGrantRevocation::AlreadyConsumed
        } else {
            SnapshotGrantRevocation::RevokedBeforeUse
        })
    }

    pub fn release_renderer(&mut self, renderer: RendererPrincipal) -> usize {
        let ids: Vec<_> = self
            .grants
            .iter()
            .filter_map(|(id, record)| (record.renderer == renderer).then_some(*id))
            .collect();
        let count = ids.len();
        for id in ids {
            self.grants.remove(&id);
        }
        count
    }

    pub fn active_count(&self) -> usize {
        self.grants.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GRANT_WIRE_DESCRIPTOR_COUNT, GRANT_WIRE_MAGIC, GRANT_WIRE_VERSION, GrantError,
        GrantRegistry, GrantState, GrantWireHeader, OriginGroupId, RendererId, RendererPrincipal,
        RevocationOutcome, StorageRights,
    };

    fn renderer(id: u64, pid: u32) -> RendererPrincipal {
        RendererPrincipal::new(RendererId::new(id).unwrap(), pid, 1001, 1001).unwrap()
    }

    fn origin(id: u64) -> OriginGroupId {
        OriginGroupId::new(id).unwrap()
    }

    fn registry() -> GrantRegistry {
        GrantRegistry::new(8, 2).unwrap()
    }

    #[test]
    fn grant_is_bound_to_renderer_origin_and_read_only_rights() {
        let mut registry = registry();
        let owner = renderer(1, 4101);
        let grant = registry
            .issue(owner, origin(10), StorageRights::READ_ONLY)
            .unwrap();
        let permit = registry
            .authorize_transfer(grant, owner, origin(10), StorageRights::READ_ONLY)
            .unwrap();
        assert_eq!(permit.renderer(), owner);
        assert_eq!(permit.origin(), origin(10));
        assert_eq!(permit.rights(), StorageRights::READ_ONLY);
        assert_eq!(registry.state_of(grant), Some(GrantState::Delegated));
    }

    #[test]
    fn grant_wire_header_matches_the_c_abi_layout() {
        use std::mem::{offset_of, size_of};

        assert_eq!(size_of::<GrantWireHeader>(), 48);
        assert_eq!(offset_of!(GrantWireHeader, renderer_instance_id), 16);
        assert_eq!(offset_of!(GrantWireHeader, registry_id), 24);
        assert_eq!(offset_of!(GrantWireHeader, grant_sequence), 32);
        assert_eq!(offset_of!(GrantWireHeader, origin_group_id), 40);

        let mut registry = registry();
        let owner = renderer(0x5001, 4101);
        let grant = registry
            .issue(owner, origin(10), StorageRights::READ_ONLY)
            .unwrap();
        let permit = registry
            .authorize_transfer(grant, owner, origin(10), StorageRights::READ_ONLY)
            .unwrap();
        let header = permit.wire_header();
        assert_eq!(header.magic, GRANT_WIRE_MAGIC);
        assert_eq!(header.version, GRANT_WIRE_VERSION);
        assert_eq!(header.descriptor_count, GRANT_WIRE_DESCRIPTOR_COUNT);
        assert_eq!(header.renderer_pid, owner.pid());
        assert_eq!(header.rights, StorageRights::READ_ONLY.bits() as u32);
        assert_eq!(header.renderer_instance_id, owner.id().value());
        assert_eq!(header.registry_id, grant.registry_id());
        assert_eq!(header.grant_sequence, grant.value());
        assert_eq!(header.origin_group_id, origin(10).value());
    }

    #[test]
    fn transfer_is_one_shot_and_replay_is_rejected() {
        let mut registry = registry();
        let owner = renderer(1, 4101);
        let grant = registry
            .issue(owner, origin(10), StorageRights::READ_ONLY)
            .unwrap();
        registry
            .authorize_transfer(grant, owner, origin(10), StorageRights::READ_ONLY)
            .unwrap();
        assert_eq!(
            registry.authorize_transfer(grant, owner, origin(10), StorageRights::READ_ONLY),
            Err(GrantError::Replay)
        );
    }

    #[test]
    fn wrong_renderer_with_reused_pid_is_rejected() {
        let mut registry = registry();
        let owner = renderer(1, 4101);
        let replacement = renderer(2, 4101);
        let grant = registry
            .issue(owner, origin(10), StorageRights::READ_ONLY)
            .unwrap();
        assert_eq!(
            registry.authorize_transfer(grant, replacement, origin(10), StorageRights::READ_ONLY),
            Err(GrantError::WrongRenderer)
        );
        assert_eq!(registry.state_of(grant), Some(GrantState::Issued));
    }

    #[test]
    fn wrong_kernel_uid_or_gid_is_rejected() {
        let mut registry = registry();
        let owner = renderer(1, 4101);
        let wrong_uid =
            RendererPrincipal::new(RendererId::new(1).unwrap(), 4101, 1002, 1001).unwrap();
        let wrong_gid =
            RendererPrincipal::new(RendererId::new(1).unwrap(), 4101, 1001, 1002).unwrap();
        let grant = registry
            .issue(owner, origin(10), StorageRights::READ_ONLY)
            .unwrap();
        assert_eq!(
            registry.authorize_transfer(grant, wrong_uid, origin(10), StorageRights::READ_ONLY),
            Err(GrantError::WrongRenderer)
        );
        assert_eq!(
            registry.authorize_transfer(grant, wrong_gid, origin(10), StorageRights::READ_ONLY),
            Err(GrantError::WrongRenderer)
        );
        assert_eq!(registry.state_of(grant), Some(GrantState::Issued));
    }

    #[test]
    fn wrong_origin_and_rights_escalation_are_rejected_without_consuming_grant() {
        let mut registry = registry();
        let owner = renderer(1, 4101);
        let grant = registry
            .issue(owner, origin(10), StorageRights::READ_ONLY)
            .unwrap();
        assert_eq!(
            registry.authorize_transfer(grant, owner, origin(11), StorageRights::READ_ONLY),
            Err(GrantError::WrongOrigin)
        );
        let write_rights =
            StorageRights::from_bits(StorageRights::READ_ONLY.bits() | (1 << 3)).unwrap();
        assert_eq!(
            registry.authorize_transfer(grant, owner, origin(10), write_rights),
            Err(GrantError::RightsMismatch)
        );
        assert_eq!(registry.state_of(grant), Some(GrantState::Issued));
    }

    #[test]
    fn unsupported_rights_cannot_be_issued() {
        let mut registry = registry();
        let owner = renderer(1, 4101);
        let write_rights =
            StorageRights::from_bits(StorageRights::READ_ONLY.bits() | (1 << 3)).unwrap();
        assert_eq!(
            registry.issue(owner, origin(10), write_rights),
            Err(GrantError::UnsupportedRights)
        );
        assert_eq!(StorageRights::from_bits(0x80), None);
    }

    #[test]
    fn per_renderer_quota_is_enforced_and_revocation_releases_capacity() {
        let mut registry = registry();
        let owner = renderer(1, 4101);
        let first = registry
            .issue(owner, origin(10), StorageRights::READ_ONLY)
            .unwrap();
        let second = registry
            .issue(owner, origin(11), StorageRights::READ_ONLY)
            .unwrap();
        assert_eq!(
            registry.issue(owner, origin(12), StorageRights::READ_ONLY),
            Err(GrantError::QuotaExceeded(super::QuotaScope::PerRenderer))
        );
        assert_eq!(
            registry.revoke(first),
            Ok(RevocationOutcome::RevokedBeforeTransfer)
        );
        assert!(
            registry
                .issue(owner, origin(12), StorageRights::READ_ONLY)
                .is_ok()
        );
        assert_eq!(registry.state_of(second), Some(GrantState::Issued));
    }

    #[test]
    fn global_quota_is_enforced_across_renderers() {
        let mut registry = GrantRegistry::new(2, 2).unwrap();
        registry
            .issue(renderer(1, 4101), origin(10), StorageRights::READ_ONLY)
            .unwrap();
        registry
            .issue(renderer(2, 4102), origin(11), StorageRights::READ_ONLY)
            .unwrap();
        assert_eq!(
            registry.issue(renderer(3, 4103), origin(12), StorageRights::READ_ONLY),
            Err(GrantError::QuotaExceeded(super::QuotaScope::Global))
        );
    }

    #[test]
    fn revoking_delegated_grant_requires_terminating_renderer_and_revokes_all_its_grants() {
        let mut registry = registry();
        let owner = renderer(1, 4101);
        let first = registry
            .issue(owner, origin(10), StorageRights::READ_ONLY)
            .unwrap();
        let second = registry
            .issue(owner, origin(11), StorageRights::READ_ONLY)
            .unwrap();
        registry
            .authorize_transfer(first, owner, origin(10), StorageRights::READ_ONLY)
            .unwrap();
        assert_eq!(
            registry.revoke(first),
            Ok(RevocationOutcome::TerminateRenderer {
                renderer: owner,
                pending_grants: 2
            })
        );
        assert_eq!(
            registry.state_of(first),
            Some(GrantState::RevocationPending)
        );
        assert_eq!(
            registry.state_of(second),
            Some(GrantState::RevocationPending)
        );
        assert_eq!(registry.active_count_for_renderer(owner), 2);
        assert_eq!(
            registry.authorize_transfer(second, owner, origin(11), StorageRights::READ_ONLY),
            Err(GrantError::TerminationPending)
        );
        assert_eq!(
            registry.revoke(second),
            Err(GrantError::TerminationAlreadyPending)
        );
        assert_eq!(registry.release_renderer(owner), 2);
        assert_eq!(registry.state_of(first), None);
        assert_eq!(registry.state_of(second), None);
        assert_eq!(registry.active_count_for_renderer(owner), 0);
    }

    #[test]
    fn renderer_exit_releases_all_grants() {
        let mut registry = registry();
        let owner = renderer(1, 4101);
        let first = registry
            .issue(owner, origin(10), StorageRights::READ_ONLY)
            .unwrap();
        let second = registry
            .issue(owner, origin(11), StorageRights::READ_ONLY)
            .unwrap();
        registry
            .authorize_transfer(first, owner, origin(10), StorageRights::READ_ONLY)
            .unwrap();
        assert_eq!(registry.release_renderer(owner), 2);
        assert_eq!(registry.state_of(first), None);
        assert_eq!(registry.state_of(second), None);
    }

    #[test]
    fn grant_ids_are_scoped_to_their_registry() {
        let mut first_registry = registry();
        let mut second_registry = registry();
        let owner = renderer(1, 4101);
        let first_grant = first_registry
            .issue(owner, origin(10), StorageRights::READ_ONLY)
            .unwrap();
        let second_grant = second_registry
            .issue(owner, origin(10), StorageRights::READ_ONLY)
            .unwrap();
        assert_eq!(first_grant.value(), second_grant.value());
        assert_ne!(first_grant.registry_id(), second_grant.registry_id());
        assert_eq!(
            second_registry.authorize_transfer(
                first_grant,
                owner,
                origin(10),
                StorageRights::READ_ONLY
            ),
            Err(GrantError::UnknownGrant)
        );
    }

    #[test]
    fn invalid_quotas_and_zero_identity_values_are_rejected() {
        assert_eq!(
            GrantRegistry::new(0, 1).err(),
            Some(GrantError::InvalidQuota)
        );
        assert_eq!(
            GrantRegistry::new(1, 2).err(),
            Some(GrantError::InvalidQuota)
        );
        assert_eq!(RendererId::new(0), None);
        assert_eq!(OriginGroupId::new(0), None);
        assert_eq!(
            RendererPrincipal::new(RendererId::new(1).unwrap(), 0, 1, 1),
            None
        );
    }

    #[test]
    fn snapshot_grant_binds_renderer_origin_document_and_revision_once() {
        use super::{SnapshotGrantError, SnapshotGrantRegistry};
        let owner = renderer(91, 8191);
        let origin_id = origin(77);
        let mut grants = SnapshotGrantRegistry::new(8, 2).unwrap();
        let grant = grants.issue(owner, origin_id, 7001, 4).unwrap();
        grants.authorize(grant, owner, origin_id, 7001, 4).unwrap();
        assert_eq!(
            grants.authorize(grant, owner, origin_id, 7001, 4),
            Err(SnapshotGrantError::Replay)
        );
        assert_eq!(
            grants.revoke(grant),
            Ok(super::SnapshotGrantRevocation::AlreadyConsumed)
        );
    }

    #[test]
    fn snapshot_grant_rejects_renderer_instance_origin_document_and_revision_mismatch() {
        use super::{SnapshotGrantError, SnapshotGrantRegistry};
        let owner = renderer(91, 8191);
        let replacement_instance = renderer(92, 8191);
        let origin_id = origin(77);
        let mut grants = SnapshotGrantRegistry::new(8, 2).unwrap();
        let grant = grants.issue(owner, origin_id, 7001, 4).unwrap();
        assert_eq!(
            grants.authorize(grant, replacement_instance, origin_id, 7001, 4),
            Err(SnapshotGrantError::WrongRenderer)
        );
        assert_eq!(
            grants.authorize(grant, owner, origin(78), 7001, 4),
            Err(SnapshotGrantError::WrongOrigin)
        );
        assert_eq!(
            grants.authorize(grant, owner, origin_id, 7002, 4),
            Err(SnapshotGrantError::WrongDocument)
        );
        assert_eq!(
            grants.authorize(grant, owner, origin_id, 7001, 5),
            Err(SnapshotGrantError::StaleRevision)
        );
        grants.authorize(grant, owner, origin_id, 7001, 4).unwrap();
    }

    #[test]
    fn snapshot_grant_enforces_quotas_and_renderer_release() {
        use super::{SnapshotGrantError, SnapshotGrantRegistry};
        let first = renderer(91, 8191);
        let second = renderer(92, 8192);
        let mut grants = SnapshotGrantRegistry::new(2, 1).unwrap();
        let first_grant = grants.issue(first, origin(77), 7001, 1).unwrap();
        assert_eq!(
            grants.issue(first, origin(77), 7002, 1),
            Err(SnapshotGrantError::QuotaExceeded(
                super::QuotaScope::PerRenderer
            ))
        );
        grants.issue(second, origin(78), 7003, 1).unwrap();
        assert_eq!(
            grants.issue(renderer(93, 8193), origin(79), 7004, 1),
            Err(SnapshotGrantError::QuotaExceeded(super::QuotaScope::Global))
        );
        assert_eq!(grants.release_renderer(first), 1);
        assert_eq!(grants.active_count(), 1);
        grants
            .authorize(first_grant, first, origin(77), 7001, 1)
            .unwrap_err();
    }

    #[test]
    fn snapshot_grant_rejects_zero_document_or_revision() {
        use super::{SnapshotGrantError, SnapshotGrantRegistry};
        let mut grants = SnapshotGrantRegistry::new(2, 1).unwrap();
        let owner = renderer(91, 8191);
        assert_eq!(
            grants.issue(owner, origin(77), 0, 1),
            Err(SnapshotGrantError::InvalidIdentity)
        );
        assert_eq!(
            grants.issue(owner, origin(77), 1, 0),
            Err(SnapshotGrantError::InvalidIdentity)
        );
    }

    #[test]
    fn snapshot_grant_can_be_revoked_and_renderer_cleanup_releases_remaining_grants() {
        use super::{SnapshotGrantError, SnapshotGrantRegistry};
        let owner = renderer(91, 8191);
        let origin_id = origin(77);
        let mut grants = SnapshotGrantRegistry::new(4, 4).unwrap();
        let revoked = grants.issue(owner, origin_id, 7001, 1).unwrap();
        let remaining = grants.issue(owner, origin_id, 7002, 1).unwrap();
        assert_eq!(
            grants.revoke(revoked),
            Ok(super::SnapshotGrantRevocation::RevokedBeforeUse)
        );
        assert_eq!(
            grants.authorize(revoked, owner, origin_id, 7001, 1),
            Err(SnapshotGrantError::UnknownGrant)
        );
        assert_eq!(grants.release_renderer(owner), 1);
        assert_eq!(
            grants.authorize(remaining, owner, origin_id, 7002, 1),
            Err(SnapshotGrantError::UnknownGrant)
        );
        assert_eq!(grants.active_count(), 0);
    }
}
