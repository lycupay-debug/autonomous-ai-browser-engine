//! Bounded binary framing for immutable DOM Snapshot messages.
//!
//! This codec is deliberately separate from the existing 64 KiB newline JSON
//! control RPC. It does not authenticate peers or authorize a Document/Origin;
//! callers must establish those properties before accepting a decoded message.

use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};

use serde_json::Value;

pub const DOM_SNAPSHOT_PROTOCOL_VERSION: u16 = 1;
pub const DOM_SNAPSHOT_MESSAGE_TYPE: u16 = 1;
pub const MAX_DOM_SNAPSHOT_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;
const FRAME_MAGIC: [u8; 4] = *b"AIBS";
const HEADER_BYTES: usize = 20;
const MAX_DOM_NODES: usize = 65_536;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomSnapshotFrame {
    pub request_id: u64,
    pub document_id: u64,
    pub revision: u64,
    pub payload: Vec<u8>,
}

fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn parse_decimal_u64(value: &Value, field: &'static str) -> io::Result<u64> {
    let text = value
        .as_str()
        .ok_or_else(|| invalid_data("Document ID and revision must be decimal strings"))?;
    if text.is_empty() || text.len() > 20 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid_data(
            "Document ID or revision is not a canonical decimal u64",
        ));
    }
    let parsed = text
        .parse::<u64>()
        .map_err(|_| invalid_data("Document ID or revision exceeds u64"))?;
    if parsed == 0 {
        return Err(if field == "documentId" {
            invalid_data("Document ID must be nonzero")
        } else {
            invalid_data("Document revision must be nonzero")
        });
    }
    if parsed.to_string() != text {
        return Err(invalid_data(
            "Document ID or revision is not canonical decimal",
        ));
    }
    Ok(parsed)
}

/// Validate the versioned Browser Runtime envelope and its flat node graph.
/// Returns the host-issued Document ID and revision as exact u64 values.
pub fn validate_dom_snapshot_payload(payload: &[u8]) -> io::Result<(u64, u64)> {
    if payload.is_empty() || payload.len() > MAX_DOM_SNAPSHOT_PAYLOAD_BYTES {
        return Err(invalid_data(
            "DOM Snapshot payload length is outside the permitted range",
        ));
    }
    let envelope: Value = serde_json::from_slice(payload)
        .map_err(|_| invalid_data("DOM Snapshot payload is not valid JSON"))?;
    if envelope.get("envelopeVersion").and_then(Value::as_u64) != Some(1) {
        return Err(invalid_data("unsupported DOM Snapshot envelope version"));
    }
    let document_id = parse_decimal_u64(
        envelope
            .get("documentId")
            .ok_or_else(|| invalid_data("missing Document ID"))?,
        "documentId",
    )?;
    let revision = parse_decimal_u64(
        envelope
            .get("revision")
            .ok_or_else(|| invalid_data("missing Document revision"))?,
        "revision",
    )?;
    let tree = envelope
        .get("tree")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid_data("DOM Snapshot tree must be an object"))?;
    if tree.get("schemaVersion").and_then(Value::as_u64) != Some(1) {
        return Err(invalid_data("unsupported DOM tree snapshot schema version"));
    }
    let node_count = tree
        .get("nodeCount")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0 && *value <= MAX_DOM_NODES)
        .ok_or_else(|| invalid_data("DOM Snapshot node count is invalid"))?;
    let root_id = tree
        .get("root")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid_data("DOM Snapshot root ID is invalid"))?;
    let nodes = tree
        .get("nodes")
        .and_then(Value::as_array)
        .filter(|nodes| nodes.len() == node_count)
        .ok_or_else(|| invalid_data("DOM Snapshot node array does not match node count"))?;

    let mut parents: HashMap<u64, Option<u64>> = HashMap::with_capacity(nodes.len());
    let mut child_lists: HashMap<u64, Vec<u64>> = HashMap::with_capacity(nodes.len());
    for node in nodes {
        let object = node
            .as_object()
            .ok_or_else(|| invalid_data("DOM Snapshot node record must be an object"))?;
        let id = object
            .get("id")
            .and_then(Value::as_u64)
            .ok_or_else(|| invalid_data("DOM Snapshot node ID must be an unsigned integer"))?;
        if id > MAX_DOM_NODES as u64 {
            return Err(invalid_data(
                "DOM Snapshot node ID exceeds the parser node budget",
            ));
        }
        if parents.contains_key(&id) {
            return Err(invalid_data("DOM Snapshot contains a duplicate node ID"));
        }
        let parent = match object.get("parent") {
            Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_u64()
                    .ok_or_else(|| invalid_data("DOM Snapshot parent ID is invalid"))?,
            ),
            None => return Err(invalid_data("DOM Snapshot node is missing parent field")),
        };
        let children = object
            .get("children")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid_data("DOM Snapshot children must be an array"))?
            .iter()
            .map(|child| {
                child
                    .as_u64()
                    .ok_or_else(|| invalid_data("DOM Snapshot child ID is invalid"))
            })
            .collect::<io::Result<Vec<_>>>()?;
        let unique_children: HashSet<u64> = children.iter().copied().collect();
        if unique_children.len() != children.len() {
            return Err(invalid_data(
                "DOM Snapshot node contains duplicate child references",
            ));
        }
        if let Some(template_contents) = object.get("templateContents")
            && !template_contents.is_null()
            && !template_contents
                .as_u64()
                .is_some_and(|value| value <= MAX_DOM_NODES as u64)
        {
            return Err(invalid_data(
                "DOM Snapshot template-content reference is invalid",
            ));
        }
        parents.insert(id, parent);
        child_lists.insert(id, children);
    }

    for node in nodes {
        if let Some(template_contents) = node.get("templateContents")
            && !template_contents.is_null()
        {
            let template_id = template_contents.as_u64().ok_or_else(|| {
                invalid_data("DOM Snapshot template-content reference is invalid")
            })?;
            if !parents.contains_key(&template_id) {
                return Err(invalid_data(
                    "DOM Snapshot references unknown template contents",
                ));
            }
        }
    }

    if parents.get(&root_id) != Some(&None) {
        return Err(invalid_data(
            "DOM Snapshot root must exist and have no parent",
        ));
    }
    let root = nodes
        .iter()
        .find(|node| node.get("id").and_then(Value::as_u64) == Some(root_id))
        .ok_or_else(|| invalid_data("DOM Snapshot root node is missing"))?;
    if root.get("type").and_then(Value::as_str) != Some("document") {
        return Err(invalid_data("DOM Snapshot root node must be a Document"));
    }

    for (id, parent) in &parents {
        if let Some(parent_id) = parent {
            let siblings = child_lists
                .get(parent_id)
                .ok_or_else(|| invalid_data("DOM Snapshot references an unknown parent"))?;
            if !siblings.contains(id) {
                return Err(invalid_data(
                    "DOM Snapshot parent/child references are inconsistent",
                ));
            }
        }
        for child_id in child_lists
            .get(id)
            .expect("child list inserted with parent")
        {
            if parents.get(child_id) != Some(&Some(*id)) {
                return Err(invalid_data(
                    "DOM Snapshot child/parent references are inconsistent",
                ));
            }
        }
    }

    // Reject cycles in parent links. The iterative color walk avoids recursion
    // on attacker-controlled DOM depth and is O(nodes + edges).
    let mut colors: HashMap<u64, u8> = HashMap::with_capacity(parents.len());
    for start in parents.keys().copied() {
        let mut path = Vec::new();
        let mut current = start;
        loop {
            match colors.get(&current).copied() {
                Some(2) => break,
                Some(1) => return Err(invalid_data("DOM Snapshot parent graph contains a cycle")),
                _ => {}
            }
            colors.insert(current, 1);
            path.push(current);
            match parents.get(&current).copied().flatten() {
                Some(parent) => current = parent,
                None => break,
            }
        }
        for id in path {
            colors.insert(id, 2);
        }
    }

    Ok((document_id, revision))
}

/// Write one DOM Snapshot frame using a fixed 20-byte big-endian header.
/// Header fields: magic, protocol version, message type, request ID, payload length.
pub fn write_dom_snapshot_frame<W: Write>(
    writer: &mut W,
    request_id: u64,
    payload: &[u8],
) -> io::Result<()> {
    if request_id == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "request ID must be nonzero",
        ));
    }
    validate_dom_snapshot_payload(payload)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let payload_length = u32::try_from(payload.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "payload length exceeds u32"))?;
    let mut header = [0_u8; HEADER_BYTES];
    header[0..4].copy_from_slice(&FRAME_MAGIC);
    header[4..6].copy_from_slice(&DOM_SNAPSHOT_PROTOCOL_VERSION.to_be_bytes());
    header[6..8].copy_from_slice(&DOM_SNAPSHOT_MESSAGE_TYPE.to_be_bytes());
    header[8..16].copy_from_slice(&request_id.to_be_bytes());
    header[16..20].copy_from_slice(&payload_length.to_be_bytes());
    writer.write_all(&header)?;
    writer.write_all(payload)
}

/// Read and validate one bounded DOM Snapshot frame. No allocation occurs until
/// the fixed header's payload length has passed the 8 MiB limit check.
pub fn read_dom_snapshot_frame<R: Read>(reader: &mut R) -> io::Result<DomSnapshotFrame> {
    let mut header = [0_u8; HEADER_BYTES];
    reader.read_exact(&mut header)?;
    if header[0..4] != FRAME_MAGIC {
        return Err(invalid_data("invalid DOM Snapshot frame magic"));
    }
    let version = u16::from_be_bytes([header[4], header[5]]);
    if version != DOM_SNAPSHOT_PROTOCOL_VERSION {
        return Err(invalid_data(
            "unsupported DOM Snapshot transport protocol version",
        ));
    }
    let message_type = u16::from_be_bytes([header[6], header[7]]);
    if message_type != DOM_SNAPSHOT_MESSAGE_TYPE {
        return Err(invalid_data(
            "unexpected UDS message type for DOM Snapshot decoder",
        ));
    }
    let request_id = u64::from_be_bytes(header[8..16].try_into().expect("fixed header slice"));
    if request_id == 0 {
        return Err(invalid_data("request ID must be nonzero"));
    }
    let payload_length =
        u32::from_be_bytes(header[16..20].try_into().expect("fixed header slice")) as usize;
    if payload_length == 0 || payload_length > MAX_DOM_SNAPSHOT_PAYLOAD_BYTES {
        return Err(invalid_data(
            "DOM Snapshot frame payload length exceeds the permitted range",
        ));
    }
    let mut payload = vec![0_u8; payload_length];
    reader.read_exact(&mut payload)?;
    let (document_id, revision) = validate_dom_snapshot_payload(&payload)?;
    Ok(DomSnapshotFrame {
        request_id,
        document_id,
        revision,
        payload,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        DOM_SNAPSHOT_MESSAGE_TYPE, DOM_SNAPSHOT_PROTOCOL_VERSION, DomSnapshotFrame,
        MAX_DOM_SNAPSHOT_PAYLOAD_BYTES, read_dom_snapshot_frame, validate_dom_snapshot_payload,
        write_dom_snapshot_frame,
    };
    use serde_json::json;
    use std::io::Cursor;

    fn valid_payload() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "envelopeVersion": 1,
            "documentId": "18446744073709551615",
            "revision": "7",
            "tree": {
                "schemaVersion": 1,
                "root": 0,
                "nodeCount": 3,
                "nodes": [
                    {"id": 0, "parent": null, "children": [2], "type": "document", "nodeType": 9},
                    {"id": 2, "parent": 0, "children": [3], "type": "element", "nodeType": 1, "name": "p"},
                    {"id": 3, "parent": 2, "children": [], "type": "text", "nodeType": 3, "value": "snapshot"}
                ]
            }
        })).unwrap()
    }

    fn frame_bytes(payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        write_dom_snapshot_frame(&mut bytes, 19, payload).unwrap();
        bytes
    }

    #[test]
    fn binary_frame_roundtrips_document_identity_and_u64_precision() {
        let payload = valid_payload();
        let bytes = frame_bytes(&payload);
        let mut cursor = Cursor::new(bytes);
        let frame = read_dom_snapshot_frame(&mut cursor).unwrap();
        assert_eq!(
            frame,
            DomSnapshotFrame {
                request_id: 19,
                document_id: u64::MAX,
                revision: 7,
                payload,
            }
        );
        assert_eq!(cursor.position() as usize, cursor.get_ref().len());
    }

    #[test]
    fn unix_stream_roundtrip_uses_length_prefixed_frames() {
        let (mut sender, mut receiver) = std::os::unix::net::UnixStream::pair().unwrap();
        let payload = valid_payload();
        let writer =
            std::thread::spawn(move || write_dom_snapshot_frame(&mut sender, 20, &payload));
        let frame = read_dom_snapshot_frame(&mut receiver).unwrap();
        writer.join().unwrap().unwrap();
        assert_eq!(frame.request_id, 20);
        assert_eq!(frame.document_id, u64::MAX);
        assert_eq!(frame.revision, 7);
    }

    #[test]
    fn rejects_bad_magic_protocol_version_and_message_type() {
        let valid = frame_bytes(&valid_payload());
        let mut bad_magic = valid.clone();
        bad_magic[0] = b'X';
        assert!(read_dom_snapshot_frame(&mut Cursor::new(bad_magic)).is_err());
        let mut bad_version = valid.clone();
        bad_version[4..6].copy_from_slice(&(DOM_SNAPSHOT_PROTOCOL_VERSION + 1).to_be_bytes());
        assert!(read_dom_snapshot_frame(&mut Cursor::new(bad_version)).is_err());
        let mut bad_type = valid;
        bad_type[6..8].copy_from_slice(&(DOM_SNAPSHOT_MESSAGE_TYPE + 1).to_be_bytes());
        assert!(read_dom_snapshot_frame(&mut Cursor::new(bad_type)).is_err());
    }

    #[test]
    fn rejects_zero_request_id_and_zero_payload_length() {
        let valid = frame_bytes(&valid_payload());
        let mut zero_request = valid.clone();
        zero_request[8..16].copy_from_slice(&0_u64.to_be_bytes());
        assert!(read_dom_snapshot_frame(&mut Cursor::new(zero_request)).is_err());
        let mut zero_length = valid;
        zero_length[16..20].copy_from_slice(&0_u32.to_be_bytes());
        assert!(read_dom_snapshot_frame(&mut Cursor::new(zero_length)).is_err());
    }

    #[test]
    fn rejects_oversized_payload_length_before_reading_or_allocating() {
        let mut header = [0_u8; 20];
        header[0..4].copy_from_slice(b"AIBS");
        header[4..6].copy_from_slice(&DOM_SNAPSHOT_PROTOCOL_VERSION.to_be_bytes());
        header[6..8].copy_from_slice(&DOM_SNAPSHOT_MESSAGE_TYPE.to_be_bytes());
        header[8..16].copy_from_slice(&1_u64.to_be_bytes());
        header[16..20]
            .copy_from_slice(&((MAX_DOM_SNAPSHOT_PAYLOAD_BYTES + 1) as u32).to_be_bytes());
        assert!(read_dom_snapshot_frame(&mut Cursor::new(header)).is_err());
    }

    #[test]
    fn rejects_truncated_header_and_payload() {
        let valid = frame_bytes(&valid_payload());
        assert!(read_dom_snapshot_frame(&mut Cursor::new(&valid[..10])).is_err());
        assert!(read_dom_snapshot_frame(&mut Cursor::new(&valid[..valid.len() - 1])).is_err());
    }

    #[test]
    fn rejects_invalid_envelope_version_identity_and_revision() {
        let mut value: serde_json::Value = serde_json::from_slice(&valid_payload()).unwrap();
        value["envelopeVersion"] = json!(2);
        assert!(validate_dom_snapshot_payload(&serde_json::to_vec(&value).unwrap()).is_err());
        value["envelopeVersion"] = json!(1);
        value["documentId"] = json!("0");
        assert!(validate_dom_snapshot_payload(&serde_json::to_vec(&value).unwrap()).is_err());
        value["documentId"] = json!("01");
        assert!(validate_dom_snapshot_payload(&serde_json::to_vec(&value).unwrap()).is_err());
        value["documentId"] = json!("1");
        value["revision"] = json!("0");
        assert!(validate_dom_snapshot_payload(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn rejects_invalid_node_counts_duplicate_ids_and_dangling_edges() {
        let mut value: serde_json::Value = serde_json::from_slice(&valid_payload()).unwrap();
        value["tree"]["nodeCount"] = json!(4);
        assert!(validate_dom_snapshot_payload(&serde_json::to_vec(&value).unwrap()).is_err());
        value["tree"]["nodeCount"] = json!(3);
        value["tree"]["nodes"][2]["id"] = json!(2);
        assert!(validate_dom_snapshot_payload(&serde_json::to_vec(&value).unwrap()).is_err());
        value = serde_json::from_slice(&valid_payload()).unwrap();
        value["tree"]["nodes"][0]["children"][0] = json!(99);
        assert!(validate_dom_snapshot_payload(&serde_json::to_vec(&value).unwrap()).is_err());
        value = serde_json::from_slice(&valid_payload()).unwrap();
        value["tree"]["nodes"][1]["templateContents"] = json!(999);
        assert!(validate_dom_snapshot_payload(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn rejects_inconsistent_parent_edges_and_cycles() {
        let mut value: serde_json::Value = serde_json::from_slice(&valid_payload()).unwrap();
        value["tree"]["nodes"][2]["parent"] = json!(0);
        assert!(validate_dom_snapshot_payload(&serde_json::to_vec(&value).unwrap()).is_err());

        value = serde_json::from_slice(&valid_payload()).unwrap();
        value["tree"]["nodeCount"] = json!(5);
        value["tree"]["nodes"].as_array_mut().unwrap().extend([
            json!({"id": 4, "parent": 5, "children": [5], "type": "document-fragment", "nodeType": 11}),
            json!({"id": 5, "parent": 4, "children": [4], "type": "document-fragment", "nodeType": 11}),
        ]);
        assert!(validate_dom_snapshot_payload(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn refuses_to_write_invalid_or_oversized_payloads() {
        let mut output = Vec::new();
        assert!(write_dom_snapshot_frame(&mut output, 1, b"not json").is_err());
        assert!(output.is_empty());
        let oversized = vec![b'x'; MAX_DOM_SNAPSHOT_PAYLOAD_BYTES + 1];
        assert!(write_dom_snapshot_frame(&mut output, 1, &oversized).is_err());
        assert!(output.is_empty());
        assert!(write_dom_snapshot_frame(&mut output, 0, &valid_payload()).is_err());
        assert!(output.is_empty());
    }
}
