#![cfg(target_os = "freebsd")]

use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use aibe_broker_policy_poc::{GrantRegistry, OriginGroupId, SnapshotGrantRegistry, StorageRights};
use aibe_browser_runtime_origin_poc::OriginRealmRuntime;
use aibe_browser_runtime_origin_poc::navigation::{FrameId, NavigationController};
use aibe_renderer_supervisor_poc::RendererSupervisor;
use aibe_snapshot_bridge_poc::send_current_document_snapshot;
use std::collections::HashMap;
use std::fmt;

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

#[test]
fn supervisor_binds_renderer_pid_and_releases_grants_after_exit_and_restart() {
    let mut supervisor = RendererSupervisor::new(std::env::temp_dir());
    let renderer_binary = env!("CARGO_BIN_EXE_snapshot-bridge-renderer");
    let principal = supervisor
        .spawn(Command::new(renderer_binary), Duration::from_secs(5))
        .unwrap();
    assert_eq!(supervisor.principal(principal.id()), Some(principal));

    let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
    let frame: FrameId = controller
        .create_root_frame("https://bridge.example/")
        .unwrap();
    let document = controller.current_document_id(frame).unwrap();
    controller
        .parse_document_html(
            document,
            b"<!doctype html><html><body><p>supervised snapshot bridge</p></body></html>",
        )
        .unwrap();
    let snapshot = controller.current_document_snapshot(frame).unwrap();

    let mut storage_grants = GrantRegistry::new(8, 2).unwrap();
    let mut grants = SnapshotGrantRegistry::new(8, 2).unwrap();
    storage_grants
        .issue(
            principal,
            OriginGroupId::new(0x7711).unwrap(),
            StorageRights::READ_ONLY,
        )
        .unwrap();
    let stream = supervisor.stream_mut(principal).unwrap();
    let receipt =
        send_current_document_snapshot(&controller, frame, stream, principal, 71, &mut grants)
            .unwrap();
    assert_eq!(receipt.peer.pid, principal.pid());
    assert_eq!(receipt.document_id, snapshot.document_id());
    assert_eq!(receipt.revision, snapshot.revision());
    assert_eq!(grants.active_count(), 1);

    let deadline = Instant::now() + Duration::from_secs(5);
    let first_exit = loop {
        let exits = supervisor
            .poll_exits(&mut storage_grants, &mut grants)
            .unwrap();
        if let Some(exit) = exits.into_iter().next() {
            break exit;
        }
        assert!(
            Instant::now() < deadline,
            "supervisor did not observe Renderer exit"
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert!(first_exit.status.success());
    assert_eq!(first_exit.principal, principal);
    assert_eq!(first_exit.released_storage_grants, 1);
    assert_eq!(first_exit.released_snapshot_grants, 1);
    assert_eq!(storage_grants.active_total_count(), 0);
    assert_eq!(grants.active_count(), 0);
    assert!(supervisor.is_empty());

    // Restart creates a fresh Broker-issued Renderer instance even if the OS
    // later reuses the old PID. Grants are bound to the complete principal.
    let replacement = supervisor
        .spawn(Command::new(renderer_binary), Duration::from_secs(5))
        .unwrap();
    assert_ne!(replacement.id(), principal.id());
    assert_eq!(
        replacement.pid(),
        supervisor.principal(replacement.id()).unwrap().pid()
    );
    storage_grants
        .issue(
            replacement,
            OriginGroupId::new(0x7712).unwrap(),
            StorageRights::READ_ONLY,
        )
        .unwrap();
    let replacement_grant = grants
        .issue(
            replacement,
            snapshot.origin(),
            snapshot.document_id(),
            snapshot.revision(),
        )
        .unwrap();
    let replacement_exit = supervisor
        .terminate(replacement.id(), &mut storage_grants, &mut grants)
        .unwrap();
    assert_eq!(replacement_exit.principal, replacement);
    assert_eq!(replacement_exit.released_storage_grants, 1);
    assert_eq!(replacement_exit.released_snapshot_grants, 1);
    assert_eq!(storage_grants.active_total_count(), 0);
    assert_eq!(grants.active_count(), 0);
    assert_eq!(
        grants.authorize(
            replacement_grant,
            replacement,
            snapshot.origin(),
            snapshot.document_id(),
            snapshot.revision(),
        ),
        Err(aibe_broker_policy_poc::SnapshotGrantError::UnknownGrant)
    );
}
