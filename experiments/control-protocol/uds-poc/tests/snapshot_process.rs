#![cfg(target_os = "freebsd")]

use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aibe_broker_policy_poc::{OriginGroupId, RendererId, RendererPrincipal, SnapshotGrantRegistry};
use aibe_control_protocol_uds_poc::bind_private_socket;
use aibe_control_protocol_uds_poc::snapshot_endpoint::send_authorized_snapshot;
use aibe_control_protocol_uds_poc::snapshot_frame::DomSnapshotFrame;
use serde_json::json;

unsafe extern "C" {
    fn geteuid() -> u32;
    fn getegid() -> u32;
}

fn unique_socket_path() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "aibe-snapshot-broker-to-renderer-{}-{nonce}.sock",
        std::process::id()
    ))
}

fn accept_with_deadline(listener: &UnixListener, child: &mut Child) -> io::Result<UnixStream> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match listener.accept() {
            Ok((stream, _)) => return Ok(stream),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if let Some(status) = child.try_wait()? {
                    return Err(io::Error::other(format!(
                        "Renderer receiver exited before connecting: {status}"
                    )));
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "timed out waiting for Renderer UDS connection",
                    ));
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
}

fn spawn_renderer(path: &PathBuf) -> Child {
    Command::new(std::env::var("CARGO_BIN_EXE_snapshot-frame-peer").expect("Cargo binary path"))
        .arg("receive")
        .arg(path)
        .spawn()
        .unwrap()
}

fn make_renderer(pid: u32, instance: u64) -> RendererPrincipal {
    RendererPrincipal::new(
        RendererId::new(instance).unwrap(),
        pid,
        unsafe { geteuid() },
        unsafe { getegid() },
    )
    .unwrap()
}

fn test_frame() -> DomSnapshotFrame {
    let payload = serde_json::to_vec(&json!({
        "envelopeVersion": 1,
        "documentId": "7",
        "revision": "3",
        "tree": {
            "schemaVersion": 1,
            "root": 0,
            "nodeCount": 1,
            "nodes": [
                {"id": 0, "parent": null, "children": [], "type": "document", "nodeType": 9}
            ]
        }
    }))
    .unwrap();
    DomSnapshotFrame {
        request_id: 99,
        document_id: 7,
        revision: 3,
        payload,
    }
}

struct TestConnection {
    path: PathBuf,
    listener: UnixListener,
    child: Child,
    stream: UnixStream,
    renderer: RendererPrincipal,
}

fn test_connection() -> TestConnection {
    let path = unique_socket_path();
    let listener = bind_private_socket(&path).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut child = spawn_renderer(&path);
    let renderer = make_renderer(child.id(), 0xA1B2_C3D4);
    let stream = accept_with_deadline(&listener, &mut child).unwrap();
    TestConnection {
        path,
        listener,
        child,
        stream,
        renderer,
    }
}

fn cleanup(mut connection: TestConnection) {
    drop(connection.stream);
    drop(connection.listener);
    let status = connection.child.wait().unwrap();
    std::fs::remove_file(connection.path).unwrap();
    assert!(status.success(), "Renderer receiver failed: {status}");
}

#[test]
fn broker_authenticates_renderer_and_sends_granted_snapshot_across_processes() {
    let mut connection = test_connection();
    let origin = OriginGroupId::new(0x7711).unwrap();
    let mut grants = SnapshotGrantRegistry::new(16, 4).unwrap();
    let grant = grants.issue(connection.renderer, origin, 7, 3).unwrap();
    let receipt = send_authorized_snapshot(
        &mut connection.stream,
        connection.renderer,
        origin,
        grant,
        &mut grants,
        &test_frame(),
    )
    .unwrap();
    assert_eq!(receipt.peer.pid, connection.child.id());
    assert_eq!(receipt.peer.uid, unsafe { geteuid() });
    assert_eq!(receipt.peer.gid, unsafe { getegid() });
    assert_eq!(receipt.peer.origin, origin);
    assert_eq!(receipt.request_id, 99);
    assert_eq!(receipt.document_id, 7);
    assert_eq!(receipt.revision, 3);
    assert!(receipt.payload_bytes > 0);
    cleanup(connection);
}

#[test]
fn broker_endpoint_rejects_wrong_kernel_peer_pid_before_sending() {
    let mut connection = test_connection();
    let wrong_renderer = make_renderer(connection.child.id().saturating_add(1), 0xA1B2_C3D4);
    let origin = OriginGroupId::new(0x7711).unwrap();
    let mut grants = SnapshotGrantRegistry::new(8, 2).unwrap();
    let grant = grants.issue(wrong_renderer, origin, 7, 3).unwrap();
    let result = send_authorized_snapshot(
        &mut connection.stream,
        wrong_renderer,
        origin,
        grant,
        &mut grants,
        &test_frame(),
    );
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(grants.active_count(), 1);
    cleanup(connection);
}

#[test]
fn broker_endpoint_rejects_wrong_renderer_instance_after_peer_authentication() {
    let mut connection = test_connection();
    let grant_owner = connection.renderer;
    let replacement = make_renderer(connection.child.id(), 0xA1B2_C3D5);
    let origin = OriginGroupId::new(0x7711).unwrap();
    let mut grants = SnapshotGrantRegistry::new(8, 2).unwrap();
    let grant = grants.issue(grant_owner, origin, 7, 3).unwrap();
    let result = send_authorized_snapshot(
        &mut connection.stream,
        replacement,
        origin,
        grant,
        &mut grants,
        &test_frame(),
    );
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(
        grants.active_count(),
        1,
        "mismatched Renderer must not consume the grant"
    );
    cleanup(connection);
}

#[test]
fn broker_endpoint_rejects_wrong_origin_and_preserves_grant() {
    let mut connection = test_connection();
    let grant_origin = OriginGroupId::new(0x7711).unwrap();
    let wrong_origin = OriginGroupId::new(0x7712).unwrap();
    let mut grants = SnapshotGrantRegistry::new(8, 2).unwrap();
    let grant = grants
        .issue(connection.renderer, grant_origin, 7, 3)
        .unwrap();
    let result = send_authorized_snapshot(
        &mut connection.stream,
        connection.renderer,
        wrong_origin,
        grant,
        &mut grants,
        &test_frame(),
    );
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(
        grants.active_count(),
        1,
        "wrong Origin must not consume the grant"
    );
    cleanup(connection);
}

#[test]
fn broker_endpoint_rejects_stale_document_revision() {
    let mut connection = test_connection();
    let origin = OriginGroupId::new(0x7711).unwrap();
    let mut grants = SnapshotGrantRegistry::new(8, 2).unwrap();
    let grant = grants.issue(connection.renderer, origin, 7, 2).unwrap();
    let result = send_authorized_snapshot(
        &mut connection.stream,
        connection.renderer,
        origin,
        grant,
        &mut grants,
        &test_frame(),
    );
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(
        grants.active_count(),
        1,
        "stale revision must not consume the grant"
    );
    cleanup(connection);
}

#[test]
fn broker_endpoint_rejects_wrong_document_grant_binding() {
    let mut connection = test_connection();
    let origin = OriginGroupId::new(0x7711).unwrap();
    let mut grants = SnapshotGrantRegistry::new(8, 2).unwrap();
    let grant = grants.issue(connection.renderer, origin, 7001, 3).unwrap();
    let result = send_authorized_snapshot(
        &mut connection.stream,
        connection.renderer,
        origin,
        grant,
        &mut grants,
        &test_frame(),
    );
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(
        grants.active_count(),
        1,
        "wrong Document must not consume the grant"
    );
    cleanup(connection);
}

#[test]
fn broker_endpoint_rejects_mismatch_between_frame_and_embedded_envelope() {
    let mut connection = test_connection();
    let origin = OriginGroupId::new(0x7711).unwrap();
    let mut grants = SnapshotGrantRegistry::new(8, 2).unwrap();
    let grant = grants.issue(connection.renderer, origin, 7, 3).unwrap();
    let mut frame = test_frame();
    frame.document_id = 8;
    let result = send_authorized_snapshot(
        &mut connection.stream,
        connection.renderer,
        origin,
        grant,
        &mut grants,
        &frame,
    );
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
    assert_eq!(
        grants.active_count(),
        1,
        "invalid envelope must not consume the grant"
    );
    cleanup(connection);
}

#[test]
fn broker_endpoint_rejects_zero_request_id_without_consuming_grant() {
    let mut connection = test_connection();
    let origin = OriginGroupId::new(0x7711).unwrap();
    let mut grants = SnapshotGrantRegistry::new(8, 2).unwrap();
    let grant = grants.issue(connection.renderer, origin, 7, 3).unwrap();
    let mut frame = test_frame();
    frame.request_id = 0;
    let result = send_authorized_snapshot(
        &mut connection.stream,
        connection.renderer,
        origin,
        grant,
        &mut grants,
        &frame,
    );
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
    assert_eq!(
        grants.active_count(),
        1,
        "invalid request ID must not consume the grant"
    );
    cleanup(connection);
}
