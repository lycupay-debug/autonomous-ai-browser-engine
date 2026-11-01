#![cfg(target_os = "freebsd")]

use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use aibe_broker_policy_poc::{GrantRegistry, SnapshotGrantRegistry};
use aibe_renderer_supervisor_poc::{FreeBsdJailConfig, RendererRctlLimits, RendererSupervisor};

static NEXT_TEST_ROOT: AtomicU64 = AtomicU64::new(1);

struct TestRootfs(PathBuf);

impl TestRootfs {
    fn new() -> Self {
        let nonce = NEXT_TEST_ROOT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "aibe-supervisor-jail-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            root.join("capability-probe"),
            b"must not be opened after cap_enter",
        )
        .unwrap();
        fs::set_permissions(
            root.join("capability-probe"),
            fs::Permissions::from_mode(0o444),
        )
        .unwrap();
        fs::create_dir(root.join("run")).unwrap();
        fs::set_permissions(root.join("run"), fs::Permissions::from_mode(0o711)).unwrap();
        fs::create_dir(root.join("dev")).unwrap();
        fs::set_permissions(root.join("dev"), fs::Permissions::from_mode(0o755)).unwrap();

        let output = root.join("renderer-test");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jail-renderer.c");
        let status = Command::new("clang")
            .args(["-Wall", "-Wextra", "-Werror", "-O2", "-static"])
            .arg(fixture)
            .arg("-o")
            .arg(&output)
            .status()
            .expect("clang must be available for the FreeBSD Jail integration fixture");
        assert!(
            status.success(),
            "static Jail renderer fixture must compile"
        );
        fs::set_permissions(&output, fs::Permissions::from_mode(0o555)).unwrap();
        Self(root)
    }

    fn config(&self) -> FreeBsdJailConfig {
        let nonce = NEXT_TEST_ROOT.fetch_add(1, Ordering::Relaxed);
        FreeBsdJailConfig {
            name: format!("aibe-supervisor-test-{}-{nonce}", std::process::id()),
            rootfs: self.0.clone(),
            username: "nobody".to_owned(),
            expected_uid: 65_534,
            expected_gid: 65_534,
            executable: PathBuf::from("/renderer-test"),
            socket_path: PathBuf::from("/run/broker.sock"),
            library_path: None,
            rctl_limits: RendererRctlLimits {
                maxproc: Some(2),
                ..Default::default()
            },
            cleanup_rootfs_on_exit: true,
            renderer_idle_timeout_ms: None,
            args: Vec::new(),
        }
    }
}

impl Drop for TestRootfs {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn read_descendant_pid(stream: &mut UnixStream) -> u32 {
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    let mut fields = line.split_whitespace();
    assert_eq!(fields.next(), Some("CAPMODE"));
    assert_eq!(
        fields.next(),
        Some("1"),
        "Renderer must enter Capsicum capability mode"
    );
    assert_eq!(fields.next(), Some("DESCENDANT"));
    fields.next().unwrap().parse().unwrap()
}

fn assert_process_gone(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if result != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "descendant PID {pid} survived Jail teardown"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn supervisor_jail_binds_kernel_peer_and_terminates_descendant_tree() {
    assert_eq!(
        unsafe { libc::geteuid() },
        0,
        "Jail integration test requires root"
    );
    let rootfs = TestRootfs::new();
    let config = rootfs.config();
    let mut supervisor = RendererSupervisor::new(std::env::temp_dir());
    let principal = supervisor
        .spawn_in_jail(&config, Duration::from_secs(8))
        .unwrap();
    assert_eq!(principal.uid(), 65_534);
    assert_eq!(principal.gid(), 65_534);
    assert_ne!(principal.pid(), 0);
    let active_rules = Command::new("/usr/bin/rctl").output().unwrap();
    assert!(active_rules.status.success());
    assert!(
        String::from_utf8_lossy(&active_rules.stdout)
            .contains(&format!("jail:{}:maxproc:deny=2", config.name))
    );
    let jail_network = Command::new("/usr/sbin/jls")
        .args(["-j", &config.name, "ip4", "ip6"])
        .output()
        .unwrap();
    assert!(jail_network.status.success());
    let network_policy = String::from_utf8_lossy(&jail_network.stdout);
    assert!(
        network_policy.contains("disable"),
        "Jail must have IPv4 and IPv6 disabled: {network_policy}"
    );

    let descendant = read_descendant_pid(supervisor.stream_mut(principal).unwrap());
    let mut storage = GrantRegistry::new(4, 2).unwrap();
    let mut snapshots = SnapshotGrantRegistry::new(4, 2).unwrap();
    let exit = supervisor
        .terminate(principal.id(), &mut storage, &mut snapshots)
        .unwrap();
    assert_eq!(exit.principal, principal);
    assert!(
        !config.rootfs.exists(),
        "dedicated rootfs must be removed after termination"
    );
    assert!(supervisor.is_empty());
    assert_process_gone(descendant);
    assert!(
        !config.rootfs.exists(),
        "dedicated rootfs must be removed after observed exit"
    );
    let remaining_rules = Command::new("/usr/bin/rctl").output().unwrap();
    assert!(!String::from_utf8_lossy(&remaining_rules.stdout).contains(&config.name));
    assert!(
        !Command::new("/usr/sbin/jls")
            .args(["-j", &config.name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn supervisor_poll_exits_destroys_jail_and_releases_grants() {
    assert_eq!(
        unsafe { libc::geteuid() },
        0,
        "Jail integration test requires root"
    );
    let rootfs = TestRootfs::new();
    let config = rootfs.config();
    let mut supervisor = RendererSupervisor::new(std::env::temp_dir());
    let principal = supervisor
        .spawn_in_jail(&config, Duration::from_secs(8))
        .unwrap();
    let descendant = read_descendant_pid(supervisor.stream_mut(principal).unwrap());

    let mut storage = GrantRegistry::new(4, 2).unwrap();
    let mut snapshots = SnapshotGrantRegistry::new(4, 2).unwrap();
    storage
        .issue(
            principal,
            aibe_broker_policy_poc::OriginGroupId::new(0x8811).unwrap(),
            aibe_broker_policy_poc::StorageRights::READ_ONLY,
        )
        .unwrap();
    let mut origins = aibe_origin_policy_poc::OriginRegistry::new().unwrap();
    let origin = origins.resolve_url("https://jail-test.example/").unwrap();
    snapshots.issue(principal, origin, 0x8812, 1).unwrap();

    supervisor.close_connection(principal).unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    let exit = loop {
        if let Some(exit) = supervisor
            .poll_exits(&mut storage, &mut snapshots)
            .unwrap()
            .into_iter()
            .next()
        {
            break exit;
        }
        assert!(
            Instant::now() < deadline,
            "Supervisor did not observe the Renderer exit"
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(exit.principal, principal);
    assert_eq!(exit.released_storage_grants, 1);
    assert_eq!(exit.released_snapshot_grants, 1);
    assert_eq!(storage.active_total_count(), 0);
    assert_eq!(snapshots.active_count(), 0);
    assert_process_gone(descendant);
    let remaining_rules = Command::new("/usr/bin/rctl").output().unwrap();
    assert!(!String::from_utf8_lossy(&remaining_rules.stdout).contains(&config.name));
    assert!(supervisor.is_empty());
    assert!(
        !Command::new("/usr/sbin/jls")
            .args(["-j", &config.name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn supervisor_rejects_host_root_as_renderer_rootfs() {
    assert_eq!(
        unsafe { libc::geteuid() },
        0,
        "Jail integration test requires root"
    );
    let config = FreeBsdJailConfig {
        name: format!("aibe-invalid-root-{}", std::process::id()),
        rootfs: PathBuf::from("/"),
        username: "nobody".to_owned(),
        expected_uid: 65_534,
        expected_gid: 65_534,
        executable: PathBuf::from("/bin/sh"),
        socket_path: PathBuf::from("/run/broker.sock"),
        library_path: None,
        rctl_limits: Default::default(),
        cleanup_rootfs_on_exit: true,
        renderer_idle_timeout_ms: None,
        args: Vec::new(),
    };
    let mut supervisor = RendererSupervisor::new(std::env::temp_dir());
    let error = supervisor
        .spawn_in_jail(&config, Duration::from_secs(1))
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
}

#[test]
fn supervisor_rejects_unsafe_rootfs_and_invalid_limits_before_jail_creation() {
    assert_eq!(
        unsafe { libc::geteuid() },
        0,
        "Jail integration test requires root"
    );
    let rootfs = TestRootfs::new();
    let mut config = rootfs.config();
    let unsafe_directory = rootfs.0.join("writable-runtime");
    fs::create_dir(&unsafe_directory).unwrap();
    fs::set_permissions(&unsafe_directory, fs::Permissions::from_mode(0o777)).unwrap();

    let mut supervisor = RendererSupervisor::new(std::env::temp_dir());
    let error = supervisor
        .spawn_in_jail(&config, Duration::from_secs(1))
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    fs::set_permissions(&unsafe_directory, fs::Permissions::from_mode(0o755)).unwrap();

    config.rctl_limits.maxproc = Some(0);
    let error = supervisor
        .spawn_in_jail(&config, Duration::from_secs(1))
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    config.rctl_limits.maxproc = Some(2);
    config.renderer_idle_timeout_ms = Some(999);
    let error = supervisor
        .spawn_in_jail(&config, Duration::from_secs(1))
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    config.renderer_idle_timeout_ms = None;

    let principal = supervisor
        .spawn_in_jail(&config, Duration::from_secs(8))
        .unwrap();
    let mut duplicate = config.clone();
    duplicate.name.push_str("-duplicate");
    let error = supervisor
        .spawn_in_jail(&duplicate, Duration::from_secs(1))
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);

    let mut storage = GrantRegistry::new(4, 2).unwrap();
    let mut snapshots = SnapshotGrantRegistry::new(4, 2).unwrap();
    supervisor
        .terminate(principal.id(), &mut storage, &mut snapshots)
        .unwrap();
    assert!(
        !config.rootfs.exists(),
        "dedicated rootfs must be removed after termination"
    );
    assert!(supervisor.is_empty());
}
