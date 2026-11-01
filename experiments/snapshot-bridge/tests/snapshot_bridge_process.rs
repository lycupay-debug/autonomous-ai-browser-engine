#![cfg(target_os = "freebsd")]

use std::io;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aibe_broker_policy_poc::{RendererId, RendererPrincipal, SnapshotGrantRegistry};
use aibe_browser_runtime_origin_poc::OriginRealmRuntime;
use aibe_browser_runtime_origin_poc::navigation::{FrameId, NavigationController};
use aibe_control_protocol_uds_poc::bind_private_socket;
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

unsafe extern "C" {
    fn geteuid() -> u32;
    fn getegid() -> u32;
}

fn socket_path() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "aibe-full-snapshot-bridge-{}-{nonce}.sock",
        std::process::id()
    ))
}

fn accept_with_deadline(
    listener: &UnixListener,
    child: &mut Child,
) -> io::Result<std::os::unix::net::UnixStream> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match listener.accept() {
            Ok((stream, _)) => return Ok(stream),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if let Some(status) = child.try_wait()? {
                    return Err(io::Error::other(format!(
                        "Renderer exited before connect: {status}"
                    )));
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Renderer UDS connect timed out",
                    ));
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
}

#[test]
fn browser_runtime_document_to_broker_grant_to_independent_renderer_process() {
    let path = socket_path();
    let listener = bind_private_socket(&path).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut child = Command::new(
        std::env::var("CARGO_BIN_EXE_snapshot-bridge-renderer").expect("Cargo binary path"),
    )
    .arg(&path)
    .spawn()
    .unwrap();
    let renderer = RendererPrincipal::new(
        RendererId::new(0xB002).unwrap(),
        child.id(),
        unsafe { geteuid() },
        unsafe { getegid() },
    )
    .unwrap();
    let mut stream = accept_with_deadline(&listener, &mut child).unwrap();

    let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
    let frame: FrameId = controller
        .create_root_frame("https://bridge.example/")
        .unwrap();
    let document = controller.current_document_id(frame).unwrap();
    controller
        .parse_document_html(
            document,
            b"<!doctype html><html><body><p>full snapshot bridge</p></body></html>",
        )
        .unwrap();

    let mut grants = SnapshotGrantRegistry::new(8, 2).unwrap();
    let receipt =
        send_current_document_snapshot(&controller, frame, &mut stream, renderer, 71, &mut grants)
            .unwrap();
    assert_eq!(receipt.peer.pid, child.id());
    assert_eq!(receipt.document_id, document.value());
    assert_eq!(receipt.revision, 1);
    assert_eq!(grants.active_count(), 1);

    drop(stream);
    drop(listener);
    let status = child.wait().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(
        status.success(),
        "Renderer rejected Browser Runtime snapshot: {status}"
    );
}
