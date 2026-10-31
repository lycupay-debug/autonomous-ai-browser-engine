//! Versioned Unix Domain Socket control-protocol feasibility prototype.
//!
//! This is a local protocol/state-machine PoC, not a production broker. It does
//! not authenticate peer credentials, execute browser tasks, or enforce policy.

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

use serde_json::Value;
#[cfg(test)]
use serde_json::json;

pub const PROTOCOL_VERSION: u64 = 1;
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
        let (stream, _) = listener.accept().expect("accept UDS connection");
        let state = &state;
        handle_stream(stream, state);
    }
}

#[cfg(test)]
fn handle_stream(stream: UnixStream, state: &Mutex<BrokerState>) {
    let reader_stream = stream.try_clone().expect("clone UDS stream");
    let mut reader = BufReader::new(reader_stream);
    let mut line = String::new();
    reader.read_line(&mut line).expect("read request line");
    let response = match serde_json::from_str::<Value>(&line) {
        Ok(request) => handle_request(request, state),
        Err(_) => error_response(
            None,
            "invalid_json",
            "request must be one JSON object per line",
        ),
    };
    let mut writer = stream;
    serde_json::to_writer(&mut writer, &response).expect("write JSON response");
    writer.write_all(b"\n").expect("terminate response line");
}

/// Bind a local socket and restrict its filesystem mode to owner read/write.
pub fn bind_private_socket(path: &Path) -> std::io::Result<UnixListener> {
    if path.exists() {
        fs::remove_file(path)?;
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
    fn invalid_json_response_has_no_request_id() {
        let response: Value = super::error_response(None, "invalid_json", "bad request");
        assert!(response["request_id"].is_null());
    }
}
