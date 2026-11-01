//! Versioned Unix Domain Socket control-protocol feasibility prototype.
//!
//! This is a local protocol/state-machine PoC, not a production broker. It has
//! a FreeBSD kernel-authenticated UDS accept helper used by the test server,
//! but does not yet execute browser tasks or enforce the complete capability
//! and policy model.

#[cfg(test)]
use std::collections::HashMap;
use std::fs;
#[cfg(test)]
use std::io::Read;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
#[cfg(test)]
use std::sync::Mutex;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::time::Duration;

use serde_json::Value;
#[cfg(test)]
use serde_json::json;

pub const PROTOCOL_VERSION: u64 = 1;
#[cfg(test)]
const MAX_REQUEST_BYTES: u64 = 64 * 1024;
#[cfg(test)]
const STREAM_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerCredentials {
    pub uid: u32,
    pub gid: u32,
}

/// Read kernel-authenticated Unix-socket peer credentials on FreeBSD.
/// Other platforms return Unsupported rather than silently trusting a peer.
#[cfg(target_os = "freebsd")]
pub fn peer_credentials(stream: &UnixStream) -> std::io::Result<PeerCredentials> {
    use std::ffi::c_int;
    use std::os::fd::AsRawFd;

    unsafe extern "C" {
        fn getpeereid(socket: c_int, euid: *mut u32, egid: *mut u32) -> c_int;
    }

    let mut uid = 0_u32;
    let mut gid = 0_u32;
    let result = unsafe { getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(PeerCredentials { uid, gid })
}

#[cfg(not(target_os = "freebsd"))]
pub fn peer_credentials(_stream: &UnixStream) -> std::io::Result<PeerCredentials> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "peer credential implementation is FreeBSD-specific",
    ))
}

/// Authorize a UDS connection only when the kernel-reported peer UID matches.
pub fn authorize_peer_uid(
    stream: &UnixStream,
    expected_uid: u32,
) -> std::io::Result<PeerCredentials> {
    let credentials = peer_credentials(stream)?;
    if credentials.uid != expected_uid {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "UDS peer UID {} does not match expected UID {expected_uid}",
                credentials.uid
            ),
        ));
    }
    Ok(credentials)
}

/// Accept one UDS client only when FreeBSD reports the expected peer UID.
/// The returned credentials are kernel supplied, not fields from the request.
pub fn accept_authorized_peer(
    listener: &UnixListener,
    expected_uid: u32,
) -> std::io::Result<(UnixStream, PeerCredentials)> {
    let (stream, _) = listener.accept()?;
    let credentials = authorize_peer_uid(&stream, expected_uid)?;
    Ok((stream, credentials))
}
#[cfg(test)]
static NEXT_TASK_ID: AtomicU64 = AtomicU64::new(1);

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TaskState {
    Queued,
    Running,
    CancelRequested,
    Cancelled,
    Completed,
    Failed,
}

#[cfg(test)]
impl TaskState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::CancelRequested => "cancel_requested",
            Self::Cancelled => "cancelled",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

#[cfg(test)]
#[derive(Default)]
struct BrokerState {
    tasks: HashMap<String, TaskState>,
}

#[cfg(test)]
fn error_response(request_id: Option<&Value>, code: &str, message: &str) -> Value {
    json!({
        "protocol_version": PROTOCOL_VERSION,
        "request_id": request_id.cloned().unwrap_or(Value::Null),
        "error": { "code": code, "message": message }
    })
}

#[cfg(test)]
fn handle_request(request: Value, state: &Mutex<BrokerState>) -> Value {
    let request_id = request.get("request_id");
    if request.get("protocol_version").and_then(Value::as_u64) != Some(PROTOCOL_VERSION) {
        return error_response(
            request_id,
            "unsupported_protocol_version",
            "protocol_version 1 is required",
        );
    }

    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return error_response(request_id, "invalid_request", "method is required");
    };

    let request_id = request_id.cloned().unwrap_or(Value::Null);
    let result = match method {
        "health.get" => json!({ "service": "browser-broker-poc", "state": "healthy" }),
        "task.create" => {
            let task_id = format!("task-{}", NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed));
            state
                .lock()
                .expect("broker state lock")
                .tasks
                .insert(task_id.clone(), TaskState::Queued);
            json!({ "task_id": task_id, "state": TaskState::Queued.as_str() })
        }
        "task.get" => {
            let Some(task_id) = request
                .get("params")
                .and_then(|params| params.get("task_id"))
                .and_then(Value::as_str)
            else {
                return error_response(Some(&request_id), "invalid_params", "task_id is required");
            };
            let state = state.lock().expect("broker state lock");
            let Some(task_state) = state.tasks.get(task_id) else {
                return error_response(Some(&request_id), "not_found", "task does not exist");
            };
            json!({ "task_id": task_id, "state": task_state.as_str() })
        }
        "task.cancel" => {
            let Some(task_id) = request
                .get("params")
                .and_then(|params| params.get("task_id"))
                .and_then(Value::as_str)
            else {
                return error_response(Some(&request_id), "invalid_params", "task_id is required");
            };
            let mut state = state.lock().expect("broker state lock");
            let Some(task_state) = state.tasks.get_mut(task_id) else {
                return error_response(Some(&request_id), "not_found", "task does not exist");
            };
            *task_state = match *task_state {
                TaskState::Queued => TaskState::Cancelled,
                TaskState::Running => TaskState::CancelRequested,
                terminal => terminal,
            };
            json!({ "task_id": task_id, "state": task_state.as_str() })
        }
        _ => return error_response(Some(&request_id), "method_not_found", "unknown method"),
    };

    json!({
        "protocol_version": PROTOCOL_VERSION,
        "request_id": request_id,
        "result": result
    })
}

#[cfg(test)]
fn serve_connections(listener: UnixListener, connection_count: usize) {
    let state = Mutex::new(BrokerState::default());
    for _ in 0..connection_count {
        #[cfg(target_os = "freebsd")]
        let stream = {
            use std::ffi::c_uint;
            unsafe extern "C" {
                fn geteuid() -> c_uint;
            }
            let expected_uid = unsafe { geteuid() };
            match accept_authorized_peer(&listener, expected_uid) {
                Ok((stream, _credentials)) => stream,
                Err(_) => continue,
            }
        };
        #[cfg(not(target_os = "freebsd"))]
        let stream = listener.accept().expect("accept UDS connection").0;
        let state = &state;
        handle_stream(stream, state);
    }
}

#[cfg(test)]
fn handle_stream(stream: UnixStream, state: &Mutex<BrokerState>) {
    let _ = stream.set_read_timeout(Some(STREAM_TIMEOUT));
    let _ = stream.set_write_timeout(Some(STREAM_TIMEOUT));
    let reader_stream = stream.try_clone().expect("clone UDS stream");
    let mut reader = BufReader::new(reader_stream).take(MAX_REQUEST_BYTES + 1);
    let mut line = String::new();
    let response = match reader.read_line(&mut line) {
        Ok(bytes) if bytes as u64 > MAX_REQUEST_BYTES => error_response(
            None,
            "request_too_large",
            "request exceeds the 64 KiB limit",
        ),
        Err(_) => error_response(None, "request_read_failed", "request could not be read"),
        Ok(_) => match serde_json::from_str::<Value>(&line) {
            Ok(request) => handle_request(request, state),
            Err(_) => error_response(
                None,
                "invalid_json",
                "request must be one JSON object per line",
            ),
        },
    };
    let mut writer = stream;
    serde_json::to_writer(&mut writer, &response).expect("write JSON response");
    writer.write_all(b"\n").expect("terminate response line");
}

/// Bind a local socket and restrict its filesystem mode to owner read/write.
pub fn bind_private_socket(path: &Path) -> std::io::Result<UnixListener> {
    // Never unlink an arbitrary pre-existing file or symlink at the socket path.
    if path.symlink_metadata().is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            "refusing to replace an existing filesystem entry at the UDS path",
        ));
    }
    let listener = UnixListener::bind(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Send one newline-delimited JSON request and parse its response.
pub fn request_once(path: &Path, request: &Value) -> std::io::Result<Value> {
    let mut stream = UnixStream::connect(path)?;
    serde_json::to_writer(&mut stream, request)?;
    stream.write_all(b"\n")?;
    let mut response = String::new();
    BufReader::new(stream).read_line(&mut response)?;
    serde_json::from_str(&response).map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::{PROTOCOL_VERSION, bind_private_socket, request_once, serve_connections};
    use serde_json::Value;
    #[cfg(test)]
    use serde_json::json;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::thread;

    fn socket_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "aibe-uds-{test_name}-{}-{}.sock",
            std::process::id(),
            thread::current().name().unwrap_or("test")
        ))
    }

    fn start_server(path: &Path, connections: usize) -> thread::JoinHandle<()> {
        let listener = bind_private_socket(path).expect("bind private UDS");
        thread::spawn(move || serve_connections(listener, connections))
    }

    #[test]
    fn health_roundtrip_and_socket_permissions() {
        let path = socket_path("health");
        let server = start_server(&path, 1);
        let mode = fs::metadata(&path)
            .expect("socket metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);

        let response = request_once(
            &path,
            &json!({
                "protocol_version": PROTOCOL_VERSION,
                "request_id": "health-1",
                "method": "health.get",
                "params": {}
            }),
        )
        .expect("health request roundtrip");
        assert_eq!(response["request_id"], "health-1");
        assert_eq!(response["result"]["state"], "healthy");

        server.join().expect("server exits");
        fs::remove_file(path).expect("remove socket");
    }

    #[test]
    fn version_mismatch_is_rejected_and_request_id_is_preserved() {
        let path = socket_path("version");
        let server = start_server(&path, 1);
        let response = request_once(
            &path,
            &json!({
                "protocol_version": 999,
                "request_id": "version-1",
                "method": "health.get",
                "params": {}
            }),
        )
        .expect("version request roundtrip");
        assert_eq!(response["request_id"], "version-1");
        assert_eq!(response["error"]["code"], "unsupported_protocol_version");
        server.join().expect("server exits");
        fs::remove_file(path).expect("remove socket");
    }

    #[test]
    fn task_create_and_cancel_state_transition() {
        let path = socket_path("task");
        let server = start_server(&path, 3);
        let create = request_once(
            &path,
            &json!({
                "protocol_version": PROTOCOL_VERSION,
                "request_id": "task-create",
                "method": "task.create",
                "params": { "operation": "navigate" }
            }),
        )
        .expect("task create request");
        let task_id = create["result"]["task_id"]
            .as_str()
            .expect("task id")
            .to_string();
        assert_eq!(create["result"]["state"], "queued");

        let cancel = request_once(
            &path,
            &json!({
                "protocol_version": PROTOCOL_VERSION,
                "request_id": "task-cancel",
                "method": "task.cancel",
                "params": { "task_id": task_id }
            }),
        )
        .expect("task cancel request");
        assert_eq!(cancel["result"]["state"], "cancelled");

        let get = request_once(
            &path,
            &json!({
                "protocol_version": PROTOCOL_VERSION,
                "request_id": "task-get",
                "method": "task.get",
                "params": { "task_id": task_id }
            }),
        )
        .expect("task get request");
        assert_eq!(get["result"]["state"], "cancelled");

        server.join().expect("server exits");
        fs::remove_file(path).expect("remove socket");
    }

    #[test]
    fn running_task_requests_cancellation_and_terminal_states_do_not_change() {
        let state = super::Mutex::new(super::BrokerState::default());
        {
            let mut broker = state.lock().expect("broker state lock");
            broker
                .tasks
                .insert("running".into(), super::TaskState::Running);
            broker
                .tasks
                .insert("completed".into(), super::TaskState::Completed);
            broker
                .tasks
                .insert("failed".into(), super::TaskState::Failed);
        }

        let running = super::handle_request(
            json!({
                "protocol_version": PROTOCOL_VERSION,
                "request_id": "cancel-running",
                "method": "task.cancel",
                "params": { "task_id": "running" }
            }),
            &state,
        );
        assert_eq!(running["result"]["state"], "cancel_requested");

        for (task_id, expected) in [("completed", "completed"), ("failed", "failed")] {
            let response = super::handle_request(
                json!({
                    "protocol_version": PROTOCOL_VERSION,
                    "request_id": format!("cancel-{task_id}"),
                    "method": "task.cancel",
                    "params": { "task_id": task_id }
                }),
                &state,
            );
            assert_eq!(response["result"]["state"], expected);
        }
    }

    #[test]
    fn bind_refuses_to_remove_an_existing_regular_file() {
        let path = socket_path("existing-file");
        fs::write(&path, b"preserve-me").expect("create sentinel file");
        let error = bind_private_socket(&path).expect_err("existing file must be preserved");
        assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
        assert_eq!(fs::read(&path).expect("sentinel remains"), b"preserve-me");
        fs::remove_file(path).expect("remove sentinel file");
    }

    #[test]
    fn oversized_request_is_rejected_before_json_parsing() {
        let path = socket_path("oversized");
        let server = start_server(&path, 1);
        let payload = "x".repeat(70 * 1024);
        let response = request_once(
            &path,
            &json!({
                "protocol_version": PROTOCOL_VERSION,
                "request_id": "oversized-1",
                "method": "health.get",
                "params": { "payload": payload }
            }),
        )
        .expect("oversized response is returned");
        assert_eq!(response["error"]["code"], "request_too_large");
        server.join().expect("server exits");
        fs::remove_file(path).expect("remove socket");
    }

    #[cfg(target_os = "freebsd")]
    #[test]
    fn authenticated_accept_rejects_a_peer_with_the_wrong_uid() {
        use super::accept_authorized_peer;
        use std::ffi::c_uint;
        use std::os::unix::net::UnixStream;

        unsafe extern "C" {
            fn geteuid() -> c_uint;
        }

        let path = socket_path("wrong-uid");
        let listener = bind_private_socket(&path).expect("bind private UDS");
        let expected_uid = unsafe { geteuid() }.wrapping_add(1);
        let server = thread::spawn(move || {
            accept_authorized_peer(&listener, expected_uid)
                .expect_err("peer with a different UID must be rejected")
                .kind()
        });
        let client = UnixStream::connect(&path).expect("connect local peer");
        assert_eq!(
            server.join().expect("server exits"),
            std::io::ErrorKind::PermissionDenied
        );
        drop(client);
        fs::remove_file(path).expect("remove socket");
    }

    #[cfg(target_os = "freebsd")]
    #[test]
    fn kernel_peer_credentials_match_the_current_process() {
        use super::{authorize_peer_uid, peer_credentials};
        use std::ffi::c_uint;
        use std::os::unix::net::UnixStream;

        unsafe extern "C" {
            fn geteuid() -> c_uint;
            fn getegid() -> c_uint;
        }

        let (client, server) = UnixStream::pair().expect("create local socketpair");
        let credentials = peer_credentials(&server).expect("kernel reports peer credentials");
        assert_eq!(credentials.uid, unsafe { geteuid() });
        assert_eq!(credentials.gid, unsafe { getegid() });
        assert!(authorize_peer_uid(&server, credentials.uid).is_ok());
        assert_eq!(
            authorize_peer_uid(&server, credentials.uid.wrapping_add(1))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        drop(client);
    }

    #[test]
    fn invalid_json_response_has_no_request_id() {
        let response: Value = super::error_response(None, "invalid_json", "bad request");
        assert!(response["request_id"].is_null());
    }
}
