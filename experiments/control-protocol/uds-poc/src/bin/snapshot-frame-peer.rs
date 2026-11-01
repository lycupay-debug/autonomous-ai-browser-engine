use std::env;
use std::io;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use aibe_control_protocol_uds_poc::snapshot_frame::write_dom_snapshot_frame;
use serde_json::json;

fn main() -> io::Result<()> {
    let mut arguments = env::args_os().skip(1);
    let mode = arguments
        .next()
        .ok_or_else(|| io::Error::other("missing mode"))?;
    let socket_path = arguments
        .next()
        .ok_or_else(|| io::Error::other("missing socket path"))?;
    if arguments.next().is_some() || mode != "send" {
        return Err(io::Error::other(
            "usage: snapshot-frame-peer send <socket-path>",
        ));
    }

    let mut stream = UnixStream::connect(socket_path)?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let payload = serde_json::to_vec(&json!({
        "envelopeVersion": 1,
        "documentId": "7",
        "revision": "3",
        "tree": {
            "schemaVersion": 1,
            "root": 0,
            "nodeCount": 3,
            "nodes": [
                {"id": 0, "parent": null, "children": [2], "type": "document", "nodeType": 9},
                {"id": 2, "parent": 0, "children": [3], "type": "element", "nodeType": 1, "name": "p"},
                {"id": 3, "parent": 2, "children": [], "type": "text", "nodeType": 3, "value": "from child process"}
            ]
        }
    }))
    .map_err(io::Error::other)?;
    write_dom_snapshot_frame(&mut stream, 99, &payload)?;
    stream.shutdown(std::net::Shutdown::Write)
}
