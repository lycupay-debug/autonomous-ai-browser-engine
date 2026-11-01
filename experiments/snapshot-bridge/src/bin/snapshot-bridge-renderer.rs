use std::env;
use std::io::{self, Read};
use std::os::unix::net::UnixStream;
use std::time::Duration;

#[cfg(target_os = "freebsd")]
use aibe_broker_policy_poc::{RendererId, RendererPrincipal};
#[cfg(target_os = "freebsd")]
use aibe_control_protocol_uds_poc::snapshot_descriptor::receive_snapshot_descriptor;
#[cfg(not(target_os = "freebsd"))]
use aibe_control_protocol_uds_poc::snapshot_frame::read_dom_snapshot_frame;

#[cfg(target_os = "freebsd")]
fn ingest_snapshot_into_v8(
    runtime: &mut aibe_js_runtime_ffi_poc::V8Runtime,
    realm: aibe_js_runtime_ffi_poc::JsRealmId,
    envelope: &serde_json::Value,
) -> io::Result<()> {
    use aibe_js_runtime_ffi_poc::{JsRuntime, JsValue};

    let tree_json = serde_json::to_string(&envelope["tree"]).map_err(io::Error::other)?;
    // JSON-encode the complete JSON string before embedding it in JavaScript.
    // Snapshot content is data; it must never become executable source.
    let encoded_tree = serde_json::to_string(&tree_json).map_err(io::Error::other)?;
    let script = format!(
        r#"(() => {{
            const tree = JSON.parse({encoded_tree});
            if (!Array.isArray(tree.nodes) || tree.nodes.length === 0) {{
                throw new Error("empty DOM snapshot");
            }}
            const wrappers = new Map();
            for (const record of tree.nodes) {{
                const wrapper = new Node();
                wrapper.snapshotId = record.id;
                wrapper.snapshotType = record.type;
                wrapper.snapshotNodeType = record.nodeType;
                wrapper.snapshotName = record.name ?? record.type;
                wrapper.snapshotValue = record.value ?? null;
                wrapper.snapshotAttributes = record.attributes ?? [];
                wrappers.set(record.id, wrapper);
            }}
            for (const record of tree.nodes) {{
                const parent = wrappers.get(record.id);
                for (const childId of record.children) {{
                    const child = wrappers.get(childId);
                    if (!child) throw new Error("snapshot child wrapper missing");
                    parent.appendChild(child);
                }}
                if (record.templateContents !== null && record.templateContents !== undefined) {{
                    const template = wrappers.get(record.templateContents);
                    if (!template) throw new Error("template content wrapper missing");
                    parent.templateContent = template;
                }}
            }}
            const root = wrappers.get(tree.root);
            const htmlRecord = tree.nodes.find(record =>
                record.type === "element" && record.name === "html");
            if (!root || !htmlRecord) throw new Error("Document or HTML root missing");
            const html = wrappers.get(htmlRecord.id);
            document.documentElement = html;
            globalThis.__snapshotNodes = wrappers;
            globalThis.__snapshotRoot = root;
            const firstChildRecord = tree.nodes.find(record => record.parent === tree.root);
            const stableIdentity = firstChildRecord
                ? root.firstChild() === wrappers.get(firstChildRecord.id)
                : root.firstChild() === null;
            return [
                wrappers.size,
                root.childCount(),
                html.childCount(),
                document.documentElement === html,
                stableIdentity
            ].join(":");
        }})()"#
    );

    let result = runtime
        .evaluate_value_in_realm(realm, &script)
        .map_err(io::Error::other)?;
    let JsValue::String(summary) = result else {
        return Err(io::Error::other(
            "V8 snapshot ingestion returned a non-string result",
        ));
    };
    let expected_nodes = envelope["tree"]["nodes"]
        .as_array()
        .ok_or_else(|| io::Error::other("snapshot node list is missing"))?
        .len();
    let fields: Vec<_> = summary.split(':').collect();
    if fields.len() != 5
        || fields[0].parse::<usize>().ok() != Some(expected_nodes)
        || fields[1].parse::<usize>().is_err()
        || fields[2].parse::<usize>().is_err()
        || fields[3] != "true"
        || fields[4] != "true"
    {
        return Err(io::Error::other(format!(
            "V8 Native Node snapshot verification failed: {summary}"
        )));
    }
    runtime
        .collect_garbage()
        .map_err(|code| io::Error::other(format!("V8 garbage collection failed: {code}")))?;
    runtime.destroy_realm(realm).map_err(io::Error::other)?;
    Ok(())
}

#[cfg(not(target_os = "freebsd"))]
fn ingest_snapshot_into_v8(_envelope: &serde_json::Value) -> io::Result<()> {
    // Hosted Linux CI compiles the process helper; the selected V8 SDK and
    // native embedding library are validated on the FreeBSD target.
    Ok(())
}

fn main() -> io::Result<()> {
    let socket_path = env::args_os()
        .nth(1)
        .ok_or_else(|| io::Error::other("usage: snapshot-bridge-renderer <socket-path>"))?;
    let idle_timeout_ms = env::var("AIBE_RENDERER_IDLE_TIMEOUT_MS")
        .ok()
        .map(|value| value.parse::<u64>().map_err(io::Error::other))
        .transpose()?
        .unwrap_or(5_000);
    if !(1_000..=300_000).contains(&idle_timeout_ms) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Renderer idle timeout must be between 1000 and 300000 ms",
        ));
    }
    let mut stream = UnixStream::connect(socket_path)?;
    stream.set_read_timeout(Some(Duration::from_millis(idle_timeout_ms)))?;

    #[cfg(target_os = "freebsd")]
    let mut runtime = {
        use aibe_js_runtime_ffi_poc::{JsRuntime, V8Runtime};
        let mut runtime = V8Runtime::new().map_err(io::Error::other)?;
        let origin = runtime.create_origin().map_err(io::Error::other)?;
        let realm = runtime
            .create_realm_for_origin(origin)
            .map_err(io::Error::other)?;
        runtime
            .initialize_document(realm)
            .map_err(io::Error::other)?;
        (runtime, realm)
    };

    // Load V8, create the Realm and initialize trusted host objects before
    // capability mode. Untrusted Snapshot parsing and JS evaluation happen
    // only after the Renderer has entered Capsicum.
    #[cfg(target_os = "freebsd")]
    let (grant_envelope, payload, expected_principal) = {
        let instance_id = env::var("AIBE_RENDERER_INSTANCE_ID")
            .map_err(io::Error::other)?
            .parse::<u64>()
            .map_err(io::Error::other)?;
        let id = RendererId::new(instance_id)
            .ok_or_else(|| io::Error::other("Renderer instance ID must be nonzero"))?;
        let principal = RendererPrincipal::new(
            id,
            unsafe { libc::getpid() as u32 },
            unsafe { libc::geteuid() },
            unsafe { libc::getegid() },
        )
        .ok_or_else(|| io::Error::other("Renderer process identity is invalid"))?;
        aibe_renderer_supervisor_poc::fd_policy::apply_renderer_fd_allowlist(&mut stream)?;
        aibe_renderer_supervisor_poc::enter_renderer_capability_mode()?;
        let (grant, payload) =
            receive_snapshot_descriptor(&mut stream, principal).map_err(|error| {
                eprintln!("Renderer SCM_RIGHTS receive failed: {error}");
                error
            })?;
        (grant, payload, principal)
    };

    #[cfg(target_os = "freebsd")]
    let envelope: serde_json::Value = serde_json::from_slice(&payload).map_err(io::Error::other)?;

    #[cfg(not(target_os = "freebsd"))]
    let envelope: serde_json::Value = {
        let frame = read_dom_snapshot_frame(&mut stream)?;
        if frame.request_id != 71 || frame.document_id != 1 || frame.revision != 1 {
            return Err(io::Error::other(
                "unexpected Browser Runtime snapshot identity",
            ));
        }
        let value: serde_json::Value =
            serde_json::from_slice(&frame.payload).map_err(io::Error::other)?;
        let payload_document_id = value["documentId"]
            .as_str()
            .and_then(|value| value.parse::<u64>().ok());
        let payload_revision = value["revision"]
            .as_str()
            .and_then(|value| value.parse::<u64>().ok());
        if payload_document_id != Some(frame.document_id)
            || payload_revision != Some(frame.revision)
        {
            return Err(io::Error::other(
                "Snapshot frame identity does not match its payload envelope",
            ));
        }
        value
    };

    #[cfg(target_os = "freebsd")]
    if grant_envelope.request_id == 0
        || grant_envelope.renderer_pid != expected_principal.pid()
        || grant_envelope.renderer_uid != expected_principal.uid()
        || grant_envelope.renderer_gid != expected_principal.gid()
        || grant_envelope.document_id == 0
        || grant_envelope.revision == 0
    {
        return Err(io::Error::other(
            "Snapshot FD grant envelope failed Renderer identity validation",
        ));
    }
    if envelope["tree"]["nodes"]
        .as_array()
        .is_none_or(|nodes| nodes.is_empty())
    {
        return Err(io::Error::other("Renderer received an empty DOM tree"));
    }

    #[cfg(target_os = "freebsd")]
    ingest_snapshot_into_v8(&mut runtime.0, runtime.1, &envelope)?;
    #[cfg(not(target_os = "freebsd"))]
    ingest_snapshot_into_v8(&envelope)?;

    // Keep the Renderer alive until Broker explicitly closes the channel.
    // This lets the integration test prove replay rejection for one live PID.
    let mut trailing = [0_u8; 1];
    if stream.read(&mut trailing)? != 0 {
        return Err(io::Error::other("unexpected trailing Snapshot bytes"));
    }
    Ok(())
}
