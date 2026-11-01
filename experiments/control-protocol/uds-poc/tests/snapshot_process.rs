#![cfg(target_os = "freebsd")]

use std::io;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aibe_control_protocol_uds_poc::snapshot_frame::read_dom_snapshot_frame;
use aibe_control_protocol_uds_poc::{PeerAuthorizationPolicy, authorize_peer, bind_private_socket};

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
        "aibe-snapshot-cross-process-{}-{nonce}.sock",
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
                        "snapshot sender exited before connecting: {status}"
                    )));
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "timed out waiting for child-process UDS connection",
                    ));
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
}

#[test]
fn authenticated_uds_snapshot_roundtrip_between_independent_processes() {
    let path = unique_socket_path();
    let listener = bind_private_socket(&path).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut child = Command::new(
        std::env::var("CARGO_BIN_EXE_snapshot-frame-peer").expect("Cargo binary path"),
    )
    .arg("send")
    .arg(&path)
    .spawn()
    .unwrap();

    let mut stream = accept_with_deadline(&listener, &mut child).unwrap();
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let policy = unsafe { PeerAuthorizationPolicy::exact_uid_gid(geteuid(), getegid()) };
    let peer = authorize_peer(&stream, policy).unwrap();
    let frame = read_dom_snapshot_frame(&mut stream).unwrap();

    let status = child.wait().unwrap();
    drop(stream);
    drop(listener);
    std::fs::remove_file(&path).unwrap();

    assert!(status.success(), "snapshot sender failed: {status}");
    assert_eq!(peer.uid, unsafe { geteuid() });
    assert_eq!(peer.gid, unsafe { getegid() });
    assert_eq!(frame.request_id, 99);
    assert_eq!(frame.document_id, 7);
    assert_eq!(frame.revision, 3);
}
