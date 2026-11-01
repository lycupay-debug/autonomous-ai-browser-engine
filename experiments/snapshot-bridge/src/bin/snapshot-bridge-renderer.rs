use std::env;
use std::io;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use aibe_control_protocol_uds_poc::snapshot_frame::read_dom_snapshot_frame;

fn main() -> io::Result<()> {
    let socket_path = env::args_os()
        .nth(1)
        .ok_or_else(|| io::Error::other("usage: snapshot-bridge-renderer <socket-path>"))?;
    let mut stream = UnixStream::connect(socket_path)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let frame = read_dom_snapshot_frame(&mut stream)?;
    if frame.request_id != 71 || frame.document_id != 1 || frame.revision != 1 {
        return Err(io::Error::other(
            "unexpected Browser Runtime snapshot identity",
        ));
    }
    let envelope: serde_json::Value =
        serde_json::from_slice(&frame.payload).map_err(io::Error::other)?;
    let payload_document_id = envelope["documentId"]
        .as_str()
        .and_then(|value| value.parse::<u64>().ok());
    let payload_revision = envelope["revision"]
        .as_str()
        .and_then(|value| value.parse::<u64>().ok());
    if payload_document_id != Some(frame.document_id) || payload_revision != Some(frame.revision) {
        return Err(io::Error::other(
            "Snapshot frame identity does not match its payload envelope",
        ));
    }
    if envelope["tree"]["nodes"]
        .as_array()
        .is_none_or(|nodes| nodes.is_empty())
    {
        return Err(io::Error::other("Renderer received an empty DOM tree"));
    }
    Ok(())
}
