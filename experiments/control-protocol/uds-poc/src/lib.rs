//! Versioned Unix Domain Socket control-protocol feasibility prototype.
//!
//! This is a local protocol/state-machine PoC, not a production broker. It has
//! a FreeBSD kernel-authenticated UDS accept helper used by the test server,
//! but does not yet execute browser tasks or enforce the complete capability
//! and policy model.

#[cfg(test)]
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
#[cfg(test)]
use std::sync::Mutex;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::Value;
#[cfg(test)]
use serde_json::json;

pub const PROTOCOL_VERSION: u64 = 1;
const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
#[cfg(test)]
const MAX_IDENTIFIER_BYTES: usize = 256;
const STREAM_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerCredentials {
    pub uid: u32,
    pub gid: u32,
}

/// Trusted local-peer identity policy. GID matching is optional for callers
/// that intentionally authorize an entire UID regardless of its primary GID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerAuthorizationPolicy {
    expected_uid: u32,
    expected_gid: Option<u32>,
}

impl PeerAuthorizationPolicy {
    pub const fn uid_only(expected_uid: u32) -> Self {
        Self {
            expected_uid,
            expected_gid: None,
        }
    }

    pub const fn exact_uid_gid(expected_uid: u32, expected_gid: u32) -> Self {
        Self {
            expected_uid,
            expected_gid: Some(expected_gid),
        }
    }
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
/// Authorize a UDS connection against kernel-reported UID and optional GID.
pub fn authorize_peer(
    stream: &UnixStream,
    policy: PeerAuthorizationPolicy,
) -> std::io::Result<PeerCredentials> {
    let credentials = peer_credentials(stream)?;
    if credentials.uid != policy.expected_uid {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "UDS peer UID {} does not match expected UID {}",
                credentials.uid, policy.expected_uid
            ),
        ));
    }
    if let Some(expected_gid) = policy.expected_gid
        && credentials.gid != expected_gid
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "UDS peer GID {} does not match expected GID {expected_gid}",
                credentials.gid
            ),
        ));
    }
    Ok(credentials)
}

/// Compatibility helper for policies that authorize every process of one UID.
pub fn authorize_peer_uid(
    stream: &UnixStream,
    expected_uid: u32,
) -> std::io::Result<PeerCredentials> {
    authorize_peer(stream, PeerAuthorizationPolicy::uid_only(expected_uid))
}

/// Authorize both the kernel-reported effective UID and effective GID.
pub fn authorize_peer_uid_gid(
    stream: &UnixStream,
    expected_uid: u32,
    expected_gid: u32,
) -> std::io::Result<PeerCredentials> {
    authorize_peer(
        stream,
        PeerAuthorizationPolicy::exact_uid_gid(expected_uid, expected_gid),
    )
}

/// Accept a UDS client only when FreeBSD reports the expected peer identity.
pub fn accept_authorized_peer_with_policy(
    listener: &UnixListener,
    policy: PeerAuthorizationPolicy,
) -> std::io::Result<(UnixStream, PeerCredentials)> {
    let (stream, _) = listener.accept()?;
    let credentials = authorize_peer(&stream, policy)?;
    Ok((stream, credentials))
}

/// Accept a UDS client only when FreeBSD reports the expected peer UID.
pub fn accept_authorized_peer(
    listener: &UnixListener,
    expected_uid: u32,
) -> std::io::Result<(UnixStream, PeerCredentials)> {
    accept_authorized_peer_with_policy(listener, PeerAuthorizationPolicy::uid_only(expected_uid))
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

struct BoundedBuffer {
    bytes: Vec<u8>,
    maximum_bytes: usize,
}

impl BoundedBuffer {
    fn new(maximum_bytes: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(maximum_bytes.min(4096)),
            maximum_bytes,
        }
    }

    fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

impl Write for BoundedBuffer {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().saturating_add(buffer.len()) > self.maximum_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "serialized frame exceeds configured byte limit",
            ));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn read_bounded_line<R: BufRead>(reader: R, maximum_bytes: usize) -> std::io::Result<Vec<u8>> {
    let mut limited = reader.take(maximum_bytes.saturating_add(1) as u64);
    let mut line = Vec::with_capacity(maximum_bytes.min(4096));
    limited.read_until(b'\n', &mut line)?;
    if line.len() > maximum_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "newline-delimited frame exceeds configured byte limit",
        ));
    }
    if !line.ends_with(b"\n") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "newline-delimited frame ended before its delimiter",
        ));
    }
    Ok(line)
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
    if !request.is_object() {
        return error_response(None, "invalid_request", "request must be a JSON object");
    }
    if let Some(request_id) = request.get("request_id") {
        let valid_string = request_id
            .as_str()
            .is_some_and(|value| !value.is_empty() && value.len() <= MAX_IDENTIFIER_BYTES);
        if !request_id.is_null() && !request_id.is_number() && !valid_string {
            return error_response(
                None,
                "invalid_request",
                "request_id must be a number, null, or a short non-empty string",
            );
        }
    }
    let request_id = request.get("request_id");
    if request.get("protocol_version").and_then(Value::as_u64) != Some(PROTOCOL_VERSION) {
        return error_response(
            request_id,
            "unsupported_protocol_version",
            "protocol_version 1 is required",
        );
    }

    let Some(method) = request
        .get("method")
        .and_then(Value::as_str)
        .filter(|method| !method.is_empty() && method.len() <= MAX_IDENTIFIER_BYTES)
    else {
        return error_response(
            request_id,
            "invalid_request",
            "method is required and must be short",
        );
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
                .filter(|task_id| !task_id.is_empty() && task_id.len() <= MAX_IDENTIFIER_BYTES)
            else {
                return error_response(
                    Some(&request_id),
                    "invalid_params",
                    "task_id is required and must be short",
                );
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
                .filter(|task_id| !task_id.is_empty() && task_id.len() <= MAX_IDENTIFIER_BYTES)
            else {
                return error_response(
                    Some(&request_id),
                    "invalid_params",
                    "task_id is required and must be short",
                );
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
                fn getegid() -> c_uint;
            }
            let expected_uid = unsafe { geteuid() };
            let expected_gid = unsafe { getegid() };
            match accept_authorized_peer_with_policy(
                &listener,
                PeerAuthorizationPolicy::exact_uid_gid(expected_uid, expected_gid),
            ) {
                Ok((stream, _credentials)) => stream,
                Err(_) => continue,
            }
        };
        #[cfg(not(target_os = "freebsd"))]
        let stream = listener.accept().expect("accept UDS connection").0;
        let state = &state;
        let _ = handle_stream(stream, state);
    }
}

#[cfg(test)]
fn handle_stream(stream: UnixStream, state: &Mutex<BrokerState>) -> std::io::Result<()> {
    stream.set_read_timeout(Some(STREAM_TIMEOUT))?;
    stream.set_write_timeout(Some(STREAM_TIMEOUT))?;
    let reader_stream = stream.try_clone()?;
    let response = match read_bounded_line(BufReader::new(reader_stream), MAX_REQUEST_BYTES) {
        Ok(line) => match serde_json::from_slice::<Value>(&line[..line.len() - 1]) {
            Ok(request) => handle_request(request, state),
            Err(_) => error_response(
                None,
                "invalid_json",
                "request must be one JSON object per line",
            ),
        },
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => error_response(
            None,
            "request_too_large",
            "request exceeds the 64 KiB limit",
        ),
        Err(_) => error_response(None, "request_read_failed", "request could not be read"),
    };
    let mut response_buffer = BoundedBuffer::new(MAX_RESPONSE_BYTES - 1);
    serde_json::to_writer(&mut response_buffer, &response)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let mut response_bytes = response_buffer.into_inner();
    response_bytes.push(b'\n');
    let mut writer = stream;
    writer.write_all(&response_bytes)
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
    let mut request_buffer = BoundedBuffer::new(MAX_REQUEST_BYTES - 1);
    serde_json::to_writer(&mut request_buffer, request)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    let mut request_bytes = request_buffer.into_inner();
    request_bytes.push(b'\n');

    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(STREAM_TIMEOUT))?;
    stream.set_write_timeout(Some(STREAM_TIMEOUT))?;
    stream.write_all(&request_bytes)?;

    let response_line = read_bounded_line(BufReader::new(stream), MAX_RESPONSE_BYTES)?;
    let response = serde_json::from_slice::<Value>(&response_line[..response_line.len() - 1])
        .map_err(std::io::Error::other)?;
    if !response.is_object() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "broker response must be a JSON object",
        ));
    }
    Ok(response)
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
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream;

        let payload = "x".repeat(70 * 1024);
        let mut client = UnixStream::connect(&path).expect("connect raw oversized client");
        writeln!(
            client,
            "{{\"protocol_version\":1,\"request_id\":\"oversized-1\",\"method\":\"health.get\",\"params\":{{\"payload\":\"{payload}\"}}}}"
        )
        .expect("send oversized raw request");
        let mut response_line = String::new();
        BufReader::new(client)
            .read_line(&mut response_line)
            .expect("read oversized response");
        let response: Value =
            serde_json::from_str(&response_line).expect("parse oversized response");
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
    fn authenticated_accept_rejects_a_peer_with_the_wrong_gid() {
        use super::{PeerAuthorizationPolicy, accept_authorized_peer_with_policy};
        use std::ffi::c_uint;
        use std::os::unix::net::UnixStream;

        unsafe extern "C" {
            fn geteuid() -> c_uint;
            fn getegid() -> c_uint;
        }

        let path = socket_path("wrong-gid");
        let listener = bind_private_socket(&path).expect("bind private UDS");
        let expected_uid = unsafe { geteuid() };
        let expected_gid = unsafe { getegid() }.wrapping_add(1);
        let server = thread::spawn(move || {
            accept_authorized_peer_with_policy(
                &listener,
                PeerAuthorizationPolicy::exact_uid_gid(expected_uid, expected_gid),
            )
            .expect_err("peer with a different GID must be rejected")
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
        use super::{authorize_peer_uid, authorize_peer_uid_gid, peer_credentials};
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
        assert!(authorize_peer_uid_gid(&server, credentials.uid, credentials.gid).is_ok());
        assert_eq!(
            authorize_peer_uid_gid(&server, credentials.uid, credentials.gid.wrapping_add(1))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            authorize_peer_uid(&server, credentials.uid.wrapping_add(1))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        drop(client);
    }

    #[test]
    fn non_object_request_and_oversized_identifiers_are_rejected() {
        let state = super::Mutex::new(super::BrokerState::default());
        let non_object = super::handle_request(json!(["health.get"]), &state);
        assert_eq!(non_object["error"]["code"], "invalid_request");

        let oversized_id = super::handle_request(
            json!({
                "protocol_version": PROTOCOL_VERSION,
                "request_id": "x".repeat(300),
                "method": "health.get"
            }),
            &state,
        );
        assert_eq!(oversized_id["error"]["code"], "invalid_request");
        assert!(oversized_id["request_id"].is_null());
    }

    #[test]
    fn client_rejects_oversized_request_before_connecting() {
        let path = socket_path("client-oversized-request");
        let error = request_once(
            &path,
            &json!({
                "protocol_version": PROTOCOL_VERSION,
                "request_id": "large",
                "method": "health.get",
                "params": { "payload": "x".repeat(70 * 1024) }
            }),
        )
        .expect_err("oversized request must be rejected before connect");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn client_rejects_oversized_broker_response() {
        use std::io::Write;
        use std::os::unix::net::UnixListener;

        let path = socket_path("client-oversized-response");
        let listener = UnixListener::bind(&path).expect("bind malicious response listener");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept client");
            let payload = vec![b'x'; super::MAX_RESPONSE_BYTES + 1];
            stream
                .write_all(&payload)
                .expect("write oversized response");
        });
        let error = request_once(
            &path,
            &json!({
                "protocol_version": PROTOCOL_VERSION,
                "request_id": "response-large",
                "method": "health.get"
            }),
        )
        .expect_err("oversized response must be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        server.join().expect("malicious response server exits");
        fs::remove_file(path).expect("remove socket");
    }

    #[test]
    fn client_rejects_response_without_newline_terminator() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixListener;

        let path = socket_path("truncated");
        let listener = UnixListener::bind(&path).expect("bind truncated response listener");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept client");
            let mut request = Vec::new();
            BufReader::new(stream.try_clone().expect("clone client stream"))
                .read_until(b'\n', &mut request)
                .expect("read complete client request before closing");
            stream
                .write_all(b"{\"result\":true}")
                .expect("write partial response");
        });
        let error = request_once(
            &path,
            &json!({
                "protocol_version": PROTOCOL_VERSION,
                "request_id": "truncated",
                "method": "health.get"
            }),
        )
        .expect_err("truncated response must be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
        server.join().expect("truncated response server exits");
        fs::remove_file(path).expect("remove socket");
    }

    #[test]
    fn bounded_buffer_rejects_writes_that_cross_its_limit() {
        use std::io::Write;

        let mut buffer = super::BoundedBuffer::new(3);
        buffer.write_all(b"abc").expect("write exact limit");
        assert_eq!(
            buffer.write_all(b"d").unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        assert_eq!(buffer.into_inner(), b"abc");
    }

    #[test]
    fn bounded_line_reader_accepts_exact_limit_and_rejects_one_byte_over() {
        let mut exact = vec![b'a'; super::MAX_RESPONSE_BYTES - 1];
        exact.push(b'\n');
        assert_eq!(
            super::read_bounded_line(
                std::io::Cursor::new(exact.clone()),
                super::MAX_RESPONSE_BYTES
            )
            .unwrap(),
            exact
        );

        let mut oversized = vec![b'a'; super::MAX_RESPONSE_BYTES];
        oversized.push(b'\n');
        assert_eq!(
            super::read_bounded_line(std::io::Cursor::new(oversized), super::MAX_RESPONSE_BYTES)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn server_handler_returns_io_error_instead_of_panicking_on_client_disconnect() {
        use std::io::Write;
        use std::os::unix::net::UnixStream;

        let (mut client, server) = UnixStream::pair().expect("create local socketpair");
        client
            .write_all(br#"{"protocol_version":1,"request_id":"disconnect","method":"health.get"}"#)
            .expect("send request body");
        client.write_all(b"\n").expect("terminate request");
        drop(client);
        let state = super::Mutex::new(super::BrokerState::default());
        assert!(super::handle_stream(server, &state).is_err());
    }

    #[test]
    fn invalid_json_response_has_no_request_id() {
        let response: Value = super::error_response(None, "invalid_json", "bad request");
        assert!(response["request_id"].is_null());
    }
}
