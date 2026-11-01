//! Broker-side authorization gate for sending immutable DOM Snapshot frames.
//!
//! The Broker authenticates the connected Renderer process using FreeBSD
//! LOCAL_PEERCRED/getpeereid, checks a Broker-issued one-shot grant bound to
//! Renderer instance + Origin Group + Document ID + revision, and only then
//! writes the snapshot. No identity or Origin claim from the payload is trusted.

use std::io;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use aibe_broker_policy_poc::{
    OriginGroupId, RendererPrincipal, SnapshotGrantError, SnapshotGrantId, SnapshotGrantRegistry,
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
    pub origin: OriginGroupId,
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
    registered_origin: OriginGroupId,
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
