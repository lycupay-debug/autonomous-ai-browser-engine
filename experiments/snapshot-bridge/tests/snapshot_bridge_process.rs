#![cfg(target_os = "freebsd")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use aibe_broker_policy_poc::{GrantRegistry, OriginGroupId, SnapshotGrantRegistry, StorageRights};
use aibe_browser_runtime_origin_poc::OriginRealmRuntime;
use aibe_browser_runtime_origin_poc::navigation::{FrameId, NavigationController};
use aibe_renderer_supervisor_poc::{FreeBsdJailConfig, RendererSupervisor};
use aibe_snapshot_bridge_poc::send_supervised_document_snapshot;
use std::collections::HashMap;
use std::fmt;

static NEXT_JAIL_ROOTFS: AtomicU64 = AtomicU64::new(1);

struct RendererRootfs(PathBuf);

impl RendererRootfs {
    fn from_v8_renderer(binary: &Path, runtime_library_dir: &Path) -> Self {
        let nonce = NEXT_JAIL_ROOTFS.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "aibe-v8-renderer-rootfs-{}-{nonce}",
            std::process::id()
        ));
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let script = repository.join("scripts/build-renderer-rootfs.sh");
        let status = Command::new(script)
            .arg(binary)
            .arg(&root)
            .arg(runtime_library_dir)
            .status()
            .expect("Renderer rootfs builder must be executable");
        assert!(status.success(), "Renderer rootfs assembly must succeed");
        Self(root)
    }

    fn config(&self) -> FreeBsdJailConfig {
        let nonce = NEXT_JAIL_ROOTFS.fetch_add(1, Ordering::Relaxed);
        FreeBsdJailConfig {
            name: format!("aibe-v8-renderer-jail-{}-{nonce}", std::process::id()),
            rootfs: self.0.clone(),
            username: "nobody".to_owned(),
            expected_uid: 65_534,
            expected_gid: 65_534,
            executable: PathBuf::from("/renderer"),
            socket_path: PathBuf::from("/run/broker.sock"),
            library_path: Some(PathBuf::from("/usr/local/lib")),
            rctl_limits: Default::default(),
            cleanup_rootfs_on_exit: true,
            renderer_idle_timeout_ms: None,
            args: Vec::new(),
        }
    }
}

impl Drop for RendererRootfs {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

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

#[test]
fn supervisor_enforces_global_and_per_renderer_request_quotas() {
    let renderer_binary = env!("CARGO_BIN_EXE_snapshot-bridge-renderer");
    assert!(
        RendererSupervisor::with_request_quotas(std::env::temp_dir(), 0, 0).is_err(),
        "zero quotas must be rejected"
    );
    let mut supervisor =
        RendererSupervisor::with_request_quotas(std::env::temp_dir(), 1, 1).unwrap();
    let first = supervisor
        .spawn_unisolated_for_test(Command::new(renderer_binary), Duration::from_secs(5))
        .unwrap();
    let second = supervisor
        .spawn_unisolated_for_test(Command::new(renderer_binary), Duration::from_secs(5))
        .unwrap();

    supervisor.reserve_request(first, 101).unwrap();
    let per_renderer_error = supervisor.reserve_request(first, 102).unwrap_err();
    assert_eq!(per_renderer_error.kind(), std::io::ErrorKind::OutOfMemory);
    assert!(
        per_renderer_error.to_string().contains("per-Renderer"),
        "per-Renderer quota must be enforced before the global quota"
    );
    let global_error = supervisor.reserve_request(second, 201).unwrap_err();
    assert_eq!(global_error.kind(), std::io::ErrorKind::OutOfMemory);
    assert!(
        global_error.to_string().contains("global"),
        "global quota must be enforced across Renderer instances"
    );
    assert_eq!(supervisor.active_request_count(), 1);

    let mut storage = GrantRegistry::new(8, 2).unwrap();
    let mut snapshots = SnapshotGrantRegistry::new(8, 2).unwrap();
    let exits = supervisor.shutdown(&mut storage, &mut snapshots).unwrap();
    assert_eq!(exits.len(), 2);
    assert_eq!(supervisor.active_request_count(), 0);
    assert!(supervisor.is_empty());
}

#[test]
fn supervisor_binds_renderer_pid_and_releases_grants_after_exit_and_restart() {
    let mut supervisor = RendererSupervisor::new(std::env::temp_dir());
    let renderer_binary = env!("CARGO_BIN_EXE_snapshot-bridge-renderer");
    let principal = supervisor
        .spawn_unisolated_for_test(Command::new(renderer_binary), Duration::from_secs(5))
        .unwrap();
    assert_eq!(supervisor.principal(principal.id()), Some(principal));

    let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
    let frame: FrameId = controller
        .create_root_frame("https://bridge.example/")
        .unwrap();
    let document = controller.current_document_id(frame).unwrap();
    controller
        .parse_document_html(
            document,
            b"<!doctype html><html><body><p>supervised snapshot bridge</p></body></html>",
        )
        .unwrap();
    let snapshot = controller.current_document_snapshot(frame).unwrap();

    let mut storage_grants = GrantRegistry::new(8, 2).unwrap();
    let mut grants = SnapshotGrantRegistry::new(8, 2).unwrap();
    storage_grants
        .issue(
            principal,
            OriginGroupId::new(0x7711).unwrap(),
            StorageRights::READ_ONLY,
        )
        .unwrap();
    let receipt = send_supervised_document_snapshot(
        &controller,
        frame,
        &mut supervisor,
        principal,
        71,
        &mut grants,
    )
    .unwrap();
    assert_eq!(receipt.peer.pid, principal.pid());
    assert_eq!(receipt.document_id, snapshot.document_id());
    assert_eq!(receipt.revision, snapshot.revision());
    assert_eq!(grants.active_count(), 1);
    assert_eq!(
        send_supervised_document_snapshot(
            &controller,
            frame,
            &mut supervisor,
            principal,
            71,
            &mut grants,
        )
        .unwrap_err()
        .kind(),
        std::io::ErrorKind::AlreadyExists,
        "a duplicate request ID must be rejected before a second grant or frame is issued"
    );
    assert_eq!(supervisor.active_request_count(), 1);
    supervisor.close_connection(principal).unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let first_exit = loop {
        let exits = supervisor
            .poll_exits(&mut storage_grants, &mut grants)
            .unwrap();
        if let Some(exit) = exits.into_iter().next() {
            break exit;
        }
        assert!(
            Instant::now() < deadline,
            "supervisor did not observe Renderer exit"
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert!(first_exit.status.success());
    assert_eq!(first_exit.principal, principal);
    assert_eq!(first_exit.released_storage_grants, 1);
    assert_eq!(first_exit.released_snapshot_grants, 1);
    assert_eq!(storage_grants.active_total_count(), 0);
    assert_eq!(grants.active_count(), 0);
    assert_eq!(supervisor.active_request_count(), 0);
    assert!(supervisor.is_empty());

    // Restart creates a fresh Broker-issued Renderer instance even if the OS
    // later reuses the old PID. Grants are bound to the complete principal.
    let replacement = supervisor
        .spawn_unisolated_for_test(Command::new(renderer_binary), Duration::from_secs(5))
        .unwrap();
    assert_ne!(replacement.id(), principal.id());
    assert_eq!(
        replacement.pid(),
        supervisor.principal(replacement.id()).unwrap().pid()
    );
    storage_grants
        .issue(
            replacement,
            OriginGroupId::new(0x7712).unwrap(),
            StorageRights::READ_ONLY,
        )
        .unwrap();
    supervisor.reserve_request(replacement, 72).unwrap();
    let replacement_grant = grants
        .issue(
            replacement,
            snapshot.origin(),
            snapshot.document_id(),
            snapshot.revision(),
        )
        .unwrap();
    let replacement_exit = supervisor
        .terminate(replacement.id(), &mut storage_grants, &mut grants)
        .unwrap();
    assert_eq!(replacement_exit.principal, replacement);
    assert_eq!(replacement_exit.released_storage_grants, 1);
    assert_eq!(replacement_exit.released_snapshot_grants, 1);
    assert_eq!(supervisor.active_request_count(), 0);
    assert_eq!(storage_grants.active_total_count(), 0);
    assert_eq!(grants.active_count(), 0);
    assert_eq!(
        grants.authorize(
            replacement_grant,
            replacement,
            snapshot.origin(),
            snapshot.document_id(),
            snapshot.revision(),
        ),
        Err(aibe_broker_policy_poc::SnapshotGrantError::UnknownGrant)
    );
}

#[test]
fn concurrent_v8_renderers_run_in_distinct_jails_and_capsicum() {
    let renderer_binary = Path::new(env!("CARGO_BIN_EXE_snapshot-bridge-renderer"));
    let runtime_library_dir = std::env::var_os("AIBE_RUNTIME_PREFIX")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/aibe-runtime-root/usr/local"))
        .join("lib");
    if !runtime_library_dir.is_dir() {
        eprintln!(
            "SKIP: V8 runtime library prefix is unavailable: {}",
            runtime_library_dir.display()
        );
        return;
    }

    let first_rootfs = RendererRootfs::from_v8_renderer(renderer_binary, &runtime_library_dir);
    let second_rootfs = RendererRootfs::from_v8_renderer(renderer_binary, &runtime_library_dir);
    let first_config = first_rootfs.config();
    let second_config = second_rootfs.config();
    assert_ne!(first_config.rootfs, second_config.rootfs);
    assert_ne!(first_config.name, second_config.name);

    let mut supervisor = RendererSupervisor::new(std::env::temp_dir());
    let first = supervisor
        .spawn_in_jail(&first_config, Duration::from_secs(10))
        .unwrap();
    let second = supervisor
        .spawn_in_jail(&second_config, Duration::from_secs(10))
        .unwrap();
    assert_ne!(first.pid(), second.pid());
    assert_eq!(first.uid(), 65_534);
    assert_eq!(second.uid(), 65_534);

    for config in [&first_config, &second_config] {
        let jail_network = Command::new("/usr/sbin/jls")
            .args(["-j", &config.name, "ip4", "ip6"])
            .output()
            .unwrap();
        assert!(jail_network.status.success());
        let network_policy = String::from_utf8_lossy(&jail_network.stdout);
        assert_eq!(
            network_policy.split_whitespace().collect::<Vec<_>>(),
            ["disable", "disable"]
        );
    }

    let mut controller = NavigationController::new(MockRuntime::default()).unwrap();
    let frame: FrameId = controller
        .create_root_frame("https://jail-bridge.example/")
        .unwrap();
    let document = controller.current_document_id(frame).unwrap();
    controller.parse_document_html(
        document,
        b"<!doctype html><html><body><p>Two independent Renderer Jails plus Capsicum</p></body></html>",
    ).unwrap();
    let snapshot = controller.current_document_snapshot(frame).unwrap();
    let mut storage_grants = GrantRegistry::new(8, 2).unwrap();
    let mut snapshot_grants = SnapshotGrantRegistry::new(8, 2).unwrap();
    for principal in [first, second] {
        storage_grants
            .issue(
                principal,
                OriginGroupId::new(0x9911).unwrap(),
                StorageRights::READ_ONLY,
            )
            .unwrap();
        let receipt = send_supervised_document_snapshot(
            &controller,
            frame,
            &mut supervisor,
            principal,
            71,
            &mut snapshot_grants,
        )
        .unwrap();
        assert_eq!(receipt.peer.pid, principal.pid());
        assert_eq!(receipt.document_id, snapshot.document_id());
        assert_eq!(receipt.revision, snapshot.revision());
    }
    assert_eq!(snapshot_grants.active_count(), 2);
    thread::sleep(Duration::from_millis(100));
    for config in [&first_config, &second_config] {
        let filter = format!("jail:{}", config.name);
        let usage = Command::new("/usr/bin/rctl")
            .arg("-hu")
            .arg(&filter)
            .output()
            .unwrap();
        assert!(usage.status.success());
        println!(
            "Renderer Jail short resource sample [{}]: {}",
            config.name,
            String::from_utf8_lossy(&usage.stdout).trim()
        );
        let process_filter = format!(
            "process:{}",
            if config.name == first_config.name {
                first.pid()
            } else {
                second.pid()
            }
        );
        let process_usage = Command::new("/usr/bin/rctl")
            .arg("-hu")
            .arg(&process_filter)
            .output()
            .unwrap();
        println!(
            "Renderer process resource sample [{}]: {}",
            process_filter,
            String::from_utf8_lossy(&process_usage.stdout).trim()
        );
        let process_id = if config.name == first_config.name {
            first.pid()
        } else {
            second.pid()
        };
        let ps = Command::new("/bin/ps")
            .args(["-o", "pid,ppid,rss,vsz,comm", "-p"])
            .arg(process_id.to_string())
            .output()
            .unwrap();
        println!(
            "Renderer process table [{}]: {}",
            process_id,
            String::from_utf8_lossy(&ps.stdout).trim()
        );
    }
    supervisor.close_connection(first).unwrap();
    supervisor.close_connection(second).unwrap();

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut exits = HashMap::new();
    while exits.len() < 2 {
        for exit in supervisor
            .poll_exits(&mut storage_grants, &mut snapshot_grants)
            .unwrap()
        {
            exits.insert(exit.principal.id(), exit);
        }
        assert!(
            Instant::now() < deadline,
            "both Jailed V8 Renderers must exit after Broker closes their UDS channels"
        );
        if exits.len() < 2 {
            thread::sleep(Duration::from_millis(10));
        }
    }
    for principal in [first, second] {
        let exit = exits.get(&principal.id()).unwrap();
        assert_eq!(exit.principal, principal);
        assert_eq!(exit.released_storage_grants, 1);
        assert_eq!(exit.released_snapshot_grants, 1);
    }
    assert_eq!(storage_grants.active_total_count(), 0);
    assert_eq!(snapshot_grants.active_count(), 0);
    assert!(!first_config.rootfs.exists());
    assert!(!second_config.rootfs.exists());
    assert!(supervisor.is_empty());
}
