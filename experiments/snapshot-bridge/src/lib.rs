//! Trusted Browser Runtime → Broker Snapshot grant bridge.
//!
//! This adapter deliberately accepts a FrameId, not caller-supplied Document,
//! Origin or revision values. The current committed Document snapshot and its
//! Origin registry namespace are captured by Browser Runtime as one immutable
//! value, then used to issue the Broker grant and construct the outbound frame.

use std::io;
use std::os::unix::net::UnixStream;

use aibe_broker_policy_poc::{RendererPrincipal, SnapshotGrantRegistry};
use aibe_browser_runtime_origin_poc::OriginRealmRuntime;
use aibe_browser_runtime_origin_poc::navigation::{FrameId, NavigationController};
use aibe_control_protocol_uds_poc::snapshot_endpoint::{
    SnapshotTransferReceipt, send_authorized_snapshot,
};
use aibe_control_protocol_uds_poc::snapshot_frame::DomSnapshotFrame;

/// Sends the current committed Document snapshot using only host-owned
/// Document/Origin/revision metadata. No page-provided identity is accepted.
pub fn send_current_document_snapshot<R: OriginRealmRuntime>(
    controller: &NavigationController<R>,
    frame: FrameId,
    stream: &mut UnixStream,
    renderer: RendererPrincipal,
    request_id: u64,
    grants: &mut SnapshotGrantRegistry,
) -> io::Result<SnapshotTransferReceipt> {
    if request_id == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "snapshot request ID must be nonzero",
        ));
    }

    let snapshot = controller
        .current_document_snapshot(frame)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    let origin = snapshot.origin();
    let grant = grants
        .issue(
            renderer,
            origin,
            snapshot.document_id(),
            snapshot.revision(),
        )
        .map_err(|error| io::Error::new(io::ErrorKind::PermissionDenied, error.to_string()))?;
    let outbound = DomSnapshotFrame {
        request_id,
        document_id: snapshot.document_id(),
        revision: snapshot.revision(),
        payload: snapshot.payload().to_vec(),
    };

    match send_authorized_snapshot(stream, renderer, origin, grant, grants, &outbound) {
        Ok(receipt) => Ok(receipt),
        Err(error) => {
            // Failed authentication/validation leaves the grant unused; a
            // partial write consumes it. In either case remove its registry
            // record so a failed transfer cannot exhaust the Renderer quota.
            let _ = grants.revoke(grant);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::send_current_document_snapshot;
    use aibe_broker_policy_poc::{RendererId, RendererPrincipal, SnapshotGrantRegistry};
    use aibe_browser_runtime_origin_poc::OriginRealmRuntime;
    use aibe_browser_runtime_origin_poc::navigation::{FrameId, NavigationController};
    #[cfg(target_os = "freebsd")]
    use aibe_control_protocol_uds_poc::snapshot_frame::read_dom_snapshot_frame;
    use std::collections::HashMap;
    use std::fmt;
    use std::io;
    use std::os::unix::net::UnixStream;

    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    struct MockOrigin(u64);
    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    struct MockRealm(u64);
    #[derive(Debug)]
    struct MockError;
    impl fmt::Display for MockError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("mock runtime error")
        }
    }
    impl std::error::Error for MockError {}
    #[derive(Default)]
    struct MockRuntime {
        next_origin: u64,
        next_realm: u64,
        realms: HashMap<MockRealm, MockOrigin>,
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
            self.next_realm += 1;
            let realm = MockRealm(self.next_realm);
            self.realms.insert(realm, origin);
            Ok(realm)
        }
        fn destroy_realm(&mut self, realm: Self::Realm) -> Result<(), Self::Error> {
            self.realms.remove(&realm).map(|_| ()).ok_or(MockError)
        }
    }

    unsafe extern "C" {
        fn getpid() -> i32;
        fn geteuid() -> u32;
        fn getegid() -> u32;
    }

    fn renderer() -> RendererPrincipal {
        RendererPrincipal::new(
            RendererId::new(0xB001).unwrap(),
            unsafe { getpid() as u32 },
            unsafe { geteuid() },
            unsafe { getegid() },
        )
        .unwrap()
    }

    fn parsed_controller() -> (NavigationController<MockRuntime>, FrameId) {
        let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
        let frame = controller
            .create_root_frame("https://bridge.example/")
            .unwrap();
        let document = controller.current_document_id(frame).unwrap();
        controller
            .parse_document_html(document, b"<!doctype html><p>broker bridge</p>")
            .unwrap();
        (controller, frame)
    }

    #[cfg(target_os = "freebsd")]
    #[test]
    fn current_document_metadata_drives_one_shot_broker_grant_and_snapshot_transfer() {
        let (controller, frame) = parsed_controller();
        let snapshot = controller.current_document_snapshot(frame).unwrap();
        let (mut broker, mut renderer_socket) = UnixStream::pair().unwrap();
        let mut grants = SnapshotGrantRegistry::new(8, 2).unwrap();
        let receipt = send_current_document_snapshot(
            &controller,
            frame,
            &mut broker,
            renderer(),
            71,
            &mut grants,
        )
        .unwrap();
        let received = read_dom_snapshot_frame(&mut renderer_socket).unwrap();
        assert_eq!(receipt.document_id, snapshot.document_id());
        assert_eq!(receipt.revision, snapshot.revision());
        assert_eq!(received.document_id, snapshot.document_id());
        assert_eq!(received.revision, snapshot.revision());
        assert_eq!(received.payload, snapshot.payload());
        assert_eq!(grants.active_count(), 1);
    }

    #[test]
    fn request_id_zero_is_rejected_before_issuing_any_grant() {
        let (controller, frame) = parsed_controller();
        let (mut broker, _renderer_socket) = UnixStream::pair().unwrap();
        let mut grants = SnapshotGrantRegistry::new(8, 2).unwrap();
        let result = send_current_document_snapshot(
            &controller,
            frame,
            &mut broker,
            renderer(),
            0,
            &mut grants,
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidInput);
        assert_eq!(grants.active_count(), 0);
    }

    #[test]
    fn document_without_html_tree_cannot_issue_snapshot_grant() {
        let controller = NavigationController::new(MockRuntime::default()).unwrap();
        let mut controller = controller;
        let frame = controller
            .create_root_frame("https://bridge.example/")
            .unwrap();
        let (mut broker, _renderer_socket) = UnixStream::pair().unwrap();
        let mut grants = SnapshotGrantRegistry::new(8, 2).unwrap();
        let result = send_current_document_snapshot(
            &controller,
            frame,
            &mut broker,
            renderer(),
            72,
            &mut grants,
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert_eq!(grants.active_count(), 0);
    }
}
