//! Broker-side authorization gate for sending immutable DOM Snapshot frames.
//!
//! The Broker authenticates the connected Renderer process using FreeBSD
//! LOCAL_PEERCRED/getpeereid, checks a Broker-issued one-shot grant bound to
//! Renderer instance + Origin Group + Document ID + revision, and only then
//! writes the snapshot. No identity or Origin claim from the payload is trusted.

use std::io;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use aibe_origin_policy_poc::OriginGroupId as HostOriginGroupId;

use aibe_broker_policy_poc::{
    RendererPrincipal, SnapshotGrantError, SnapshotGrantId, SnapshotGrantRegistry,
};

use crate::peer_process_credentials;
use crate::snapshot_frame::{
    DomSnapshotFrame, validate_dom_snapshot_payload, write_dom_snapshot_frame,
};

const SNAPSHOT_IO_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthorizedRenderer {
    pub renderer: RendererPrincipal,
    pub pid: u32,
    pub uid: u32,
    pub gid: u32,
    pub origin: HostOriginGroupId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapshotTransferReceipt {
    pub peer: AuthorizedRenderer,
    pub request_id: u64,
    pub document_id: u64,
    pub revision: u64,
    pub payload_bytes: usize,
}

fn deny(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

/// Authenticate the Renderer, authorize the exact immutable snapshot, then
/// send it. A grant is consumed before writing so a partial/failed write cannot
/// be retried with the same capability and accidentally duplicate delivery.
pub fn send_authorized_snapshot(
    stream: &mut UnixStream,
    registered_renderer: RendererPrincipal,
    registered_origin: HostOriginGroupId,
    grant: SnapshotGrantId,
    grants: &mut SnapshotGrantRegistry,
    frame: &DomSnapshotFrame,
) -> io::Result<SnapshotTransferReceipt> {
    stream.set_read_timeout(Some(SNAPSHOT_IO_TIMEOUT))?;
    stream.set_write_timeout(Some(SNAPSHOT_IO_TIMEOUT))?;

    // This endpoint is the Broker's connected socket to the Renderer. The
    // kernel peer is therefore the Renderer, not an identity supplied by it.
    let credentials = peer_process_credentials(stream)?;
    if credentials.pid != registered_renderer.pid()
        || credentials.uid != registered_renderer.uid()
        || credentials.gid != registered_renderer.gid()
    {
        return Err(deny(
            "kernel-authenticated peer does not match the registered Renderer process",
        ));
    }

    if frame.request_id == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "snapshot request ID must be nonzero",
        ));
    }
    let (payload_document, payload_revision) = validate_dom_snapshot_payload(&frame.payload)
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "snapshot payload failed schema validation",
            )
        })?;
    if payload_document != frame.document_id || payload_revision != frame.revision {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame identity fields do not match the embedded snapshot envelope",
        ));
    }

    grants
        .authorize(
            grant,
            registered_renderer,
            registered_origin,
            frame.document_id,
            frame.revision,
        )
        .map_err(|error| match error {
            SnapshotGrantError::WrongRenderer => {
                deny("snapshot grant belongs to another Renderer instance")
            }
            SnapshotGrantError::WrongOrigin => {
                deny("snapshot grant belongs to another Origin Group")
            }
            SnapshotGrantError::WrongDocument => deny("snapshot grant belongs to another Document"),
            SnapshotGrantError::StaleRevision => {
                deny("snapshot revision does not match the granted revision")
            }
            SnapshotGrantError::Replay => deny("snapshot grant has already been consumed"),
            SnapshotGrantError::UnknownGrant => deny("snapshot grant is unknown or revoked"),
            _ => deny("snapshot grant authorization failed"),
        })?;

    write_dom_snapshot_frame(stream, frame.request_id, &frame.payload)?;

    Ok(SnapshotTransferReceipt {
        peer: AuthorizedRenderer {
            renderer: registered_renderer,
            pid: credentials.pid,
            uid: credentials.uid,
            gid: credentials.gid,
            origin: registered_origin,
        },
        request_id: frame.request_id,
        document_id: frame.document_id,
        revision: frame.revision,
        payload_bytes: frame.payload.len(),
    })
}

/// Authorize one immutable Snapshot and transfer its bytes as a read-only
/// anonymous shared-memory descriptor using SCM_RIGHTS. This is the FreeBSD
/// Renderer IPC path; the existing byte-frame endpoint remains for codec tests.
#[cfg(target_os = "freebsd")]
pub fn send_authorized_snapshot_descriptor(
    stream: &UnixStream,
    registered_renderer: RendererPrincipal,
    registered_origin: HostOriginGroupId,
    grant: SnapshotGrantId,
    grants: &mut SnapshotGrantRegistry,
    frame: &DomSnapshotFrame,
) -> io::Result<SnapshotTransferReceipt> {
    use std::os::fd::AsRawFd;

    use crate::snapshot_descriptor::{
        SnapshotDescriptorEnvelope, create_snapshot_descriptor, send_snapshot_descriptor,
    };

    stream.set_read_timeout(Some(SNAPSHOT_IO_TIMEOUT))?;
    stream.set_write_timeout(Some(SNAPSHOT_IO_TIMEOUT))?;
    let credentials = peer_process_credentials(stream)?;
    if credentials.pid != registered_renderer.pid()
        || credentials.uid != registered_renderer.uid()
        || credentials.gid != registered_renderer.gid()
    {
        return Err(deny(
            "kernel-authenticated peer does not match the registered Renderer process",
        ));
    }
    if frame.request_id == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "snapshot request ID must be nonzero",
        ));
    }
    let (payload_document, payload_revision) = validate_dom_snapshot_payload(&frame.payload)
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "snapshot payload failed schema validation",
            )
        })?;
    if payload_document != frame.document_id || payload_revision != frame.revision {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame identity fields do not match the embedded snapshot envelope",
        ));
    }
    grants
        .authorize(
            grant,
            registered_renderer,
            registered_origin,
            frame.document_id,
            frame.revision,
        )
        .map_err(|error| match error {
            SnapshotGrantError::WrongRenderer => {
                deny("snapshot grant belongs to another Renderer instance")
            }
            SnapshotGrantError::WrongOrigin => {
                deny("snapshot grant belongs to another Origin Group")
            }
            SnapshotGrantError::WrongDocument => deny("snapshot grant belongs to another Document"),
            SnapshotGrantError::StaleRevision => {
                deny("snapshot revision does not match the granted revision")
            }
            SnapshotGrantError::Replay => deny("snapshot grant has already been consumed"),
            SnapshotGrantError::UnknownGrant => deny("snapshot grant is unknown or revoked"),
            _ => deny("snapshot grant authorization failed"),
        })?;

    let descriptor = create_snapshot_descriptor(&frame.payload)?;
    let (origin_registry_id, origin_group_id) = registered_origin.identity_parts();
    let envelope = SnapshotDescriptorEnvelope {
        request_id: frame.request_id,
        renderer_instance_id: registered_renderer.id().value(),
        renderer_pid: registered_renderer.pid(),
        renderer_uid: registered_renderer.uid(),
        renderer_gid: registered_renderer.gid(),
        origin_registry_id,
        origin_group_id,
        grant_registry_id: grant.registry_id(),
        grant_id: grant.value(),
        document_id: frame.document_id,
        revision: frame.revision,
        payload_bytes: frame.payload.len() as u64,
    };
    send_snapshot_descriptor(stream, envelope, descriptor.as_raw_fd())?;

    Ok(SnapshotTransferReceipt {
        peer: AuthorizedRenderer {
            renderer: registered_renderer,
            pid: credentials.pid,
            uid: credentials.uid,
            gid: credentials.gid,
            origin: registered_origin,
        },
        request_id: frame.request_id,
        document_id: frame.document_id,
        revision: frame.revision,
        payload_bytes: frame.payload.len(),
    })
}

#[cfg(all(test, target_os = "freebsd"))]
mod descriptor_tests {
    use super::send_authorized_snapshot_descriptor;
    use crate::snapshot_descriptor::receive_snapshot_descriptor;
    use crate::snapshot_frame::DomSnapshotFrame;
    use aibe_broker_policy_poc::{RendererId, RendererPrincipal, SnapshotGrantRegistry};
    use aibe_origin_policy_poc::OriginRegistry;
    use std::io;
    use std::os::unix::net::UnixStream;

    const PAYLOAD: &[u8] = br##"{"envelopeVersion":1,"documentId":"7","revision":"3","tree":{"schemaVersion":1,"nodeCount":1,"root":1,"nodes":[{"id":1,"parent":null,"children":[],"type":"document","nodeType":9,"name":"#document","value":null,"attributes":[]}]}}"##;

    fn principal() -> RendererPrincipal {
        RendererPrincipal::new(
            RendererId::new(0xBEEF).unwrap(),
            unsafe { libc::getpid() as u32 },
            unsafe { libc::geteuid() },
            unsafe { libc::getegid() },
        )
        .unwrap()
    }

    fn frame() -> DomSnapshotFrame {
        DomSnapshotFrame {
            request_id: 71,
            document_id: 7,
            revision: 3,
            payload: PAYLOAD.to_vec(),
        }
    }

    #[test]
    fn descriptor_transfer_consumes_the_exact_one_shot_grant_and_rejects_replay() {
        let renderer = principal();
        let mut origins = OriginRegistry::new().unwrap();
        let origin = origins.resolve_url("https://descriptor.example/").unwrap();
        let mut grants = SnapshotGrantRegistry::new(8, 2).unwrap();
        let grant = grants.issue(renderer, origin, 7, 3).unwrap();
        let (broker, mut renderer_socket) = UnixStream::pair().unwrap();

        let receipt = send_authorized_snapshot_descriptor(
            &broker,
            renderer,
            origin,
            grant,
            &mut grants,
            &frame(),
        )
        .unwrap();
        assert_eq!(receipt.peer.pid, renderer.pid());
        assert_eq!(receipt.document_id, 7);
        assert_eq!(receipt.revision, 3);
        let (envelope, payload) =
            receive_snapshot_descriptor(&mut renderer_socket, renderer).unwrap();
        assert_eq!(envelope.grant_registry_id, grant.registry_id());
        assert_eq!(envelope.grant_id, grant.value());
        assert_eq!(payload, PAYLOAD);

        assert_eq!(
            send_authorized_snapshot_descriptor(
                &broker,
                renderer,
                origin,
                grant,
                &mut grants,
                &frame(),
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::PermissionDenied,
            "a consumed SCM_RIGHTS grant cannot be replayed"
        );
    }

    #[test]
    fn wrong_origin_descriptor_grant_is_rejected_without_consuming_the_valid_grant() {
        let renderer = principal();
        let mut origins = OriginRegistry::new().unwrap();
        let origin_a = origins.resolve_url("https://origin-a.example/").unwrap();
        let origin_b = origins.resolve_url("https://origin-b.example/").unwrap();
        let mut grants = SnapshotGrantRegistry::new(8, 2).unwrap();
        let grant = grants.issue(renderer, origin_a, 7, 3).unwrap();
        let (broker, _renderer_socket) = UnixStream::pair().unwrap();
        assert_eq!(
            send_authorized_snapshot_descriptor(
                &broker,
                renderer,
                origin_b,
                grant,
                &mut grants,
                &frame(),
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(grants.authorize(grant, renderer, origin_a, 7, 3).is_ok());
    }

    #[test]
    fn frame_identity_mismatch_is_rejected_before_grant_consumption_or_fd_creation() {
        let renderer = principal();
        let mut origins = OriginRegistry::new().unwrap();
        let origin = origins.resolve_url("https://descriptor.example/").unwrap();
        let mut grants = SnapshotGrantRegistry::new(8, 2).unwrap();
        let grant = grants.issue(renderer, origin, 7, 3).unwrap();
        let (broker, _renderer_socket) = UnixStream::pair().unwrap();
        let mut invalid = frame();
        invalid.revision = 4;
        assert_eq!(
            send_authorized_snapshot_descriptor(
                &broker,
                renderer,
                origin,
                grant,
                &mut grants,
                &invalid,
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::InvalidData
        );
        assert!(grants.authorize(grant, renderer, origin, 7, 3).is_ok());
    }
}
