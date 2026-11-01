#![cfg(target_os = "freebsd")]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use aibe_broker_policy_poc::{GrantRegistry, SnapshotGrantRegistry};
use aibe_browser_runtime_origin_poc::OriginRealmRuntime;
use aibe_browser_runtime_origin_poc::navigation::{FrameId, NavigationController};
use aibe_renderer_supervisor_poc::{FreeBsdJailConfig, RendererSupervisor};
use aibe_snapshot_bridge_poc::send_supervised_document_snapshot;
use std::fmt;

static NEXT_ROOTFS: AtomicU64 = AtomicU64::new(1);

struct RendererRootfs(PathBuf);

impl RendererRootfs {
    fn build(binary: &Path, runtime_library_dir: &Path) -> Self {
        let nonce = NEXT_ROOTFS.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("aibe-stress-rootfs-{}-{nonce}", std::process::id()));
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let script = repository.join("scripts/build-renderer-rootfs.sh");
        let status = Command::new(script)
            .arg(binary)
            .arg(&root)
            .arg(runtime_library_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("Renderer rootfs builder must execute");
        assert!(status.success(), "Renderer rootfs assembly failed");
        Self(root)
    }

    fn config(&self, index: usize) -> FreeBsdJailConfig {
        let nonce = NEXT_ROOTFS.fetch_add(1, Ordering::Relaxed);
        FreeBsdJailConfig {
            name: format!("aibe-stress-{}-{nonce}-{index}", std::process::id()),
            rootfs: self.0.clone(),
            username: "nobody".to_owned(),
            expected_uid: 65_534,
            expected_gid: 65_534,
            executable: PathBuf::from("/renderer"),
            socket_path: PathBuf::from("/run/broker.sock"),
            library_path: Some(PathBuf::from("/usr/local/lib")),
            rctl_limits: Default::default(),
            cleanup_rootfs_on_exit: true,
            renderer_idle_timeout_ms: Some(300_000),
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

fn system_reclaimable_bytes() -> u64 {
    let output = Command::new("/sbin/sysctl")
        .args([
            "-n",
            "vm.stats.vm.v_free_count",
            "vm.stats.vm.v_inactive_count",
        ])
        .output()
        .expect("sysctl VM counters");
    assert!(output.status.success());
    let pages: u64 = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse::<u64>().ok())
        .sum();
    pages * 4096
}

fn swap_used_bytes() -> u64 {
    let output = Command::new("/usr/sbin/swapinfo")
        .arg("-k")
        .output()
        .expect("swapinfo");
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().nth(2)?.parse::<u64>().ok())
        .sum::<u64>()
        * 1024
}

fn sample_processes(pids: &HashSet<u32>) -> (usize, u64, u64, u64) {
    let output = Command::new("/bin/ps")
        .args(["-axo", "pid,jid,rss,vsz,nlwp,comm"])
        .output()
        .expect("ps process snapshot");
    assert!(output.status.success());
    let mut alive = 0usize;
    let mut rss_kb = 0u64;
    let mut vsz_kb = 0u64;
    let mut threads = 0u64;
    for line in String::from_utf8_lossy(&output.stdout).lines().skip(1) {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 6 {
            continue;
        }
        let Ok(pid) = fields[0].parse::<u32>() else {
            continue;
        };
        if !pids.contains(&pid) {
            continue;
        }
        alive += 1;
        rss_kb += fields[2].parse::<u64>().unwrap_or(0);
        vsz_kb += fields[3].parse::<u64>().unwrap_or(0);
        threads += fields[4].parse::<u64>().unwrap_or(0);
    }
    (alive, rss_kb, vsz_kb, threads)
}

fn parse_quantity(value: &str) -> Option<u64> {
    let suffix = value.chars().last()?;
    let (number, multiplier) = match suffix {
        'K' | 'k' => (&value[..value.len() - 1], 1024u64),
        'M' | 'm' => (&value[..value.len() - 1], 1024u64.pow(2)),
        'G' | 'g' => (&value[..value.len() - 1], 1024u64.pow(3)),
        'T' | 't' => (&value[..value.len() - 1], 1024u64.pow(4)),
        _ => (value, 1u64),
    };
    number.parse::<u64>().ok()?.checked_mul(multiplier)
}

fn run_stage(count: usize, hold: Duration, renderer_binary: &Path, runtime_library_dir: &Path) {
    assert_eq!(unsafe { libc::geteuid() }, 0, "stress test requires root");
    println!("STAGE_BEGIN count={count} hold_secs={}", hold.as_secs());

    // Build all isolated root filesystems before launching processes. This
    // keeps rootfs assembly cost out of the Renderer startup measurements.
    let build_started = Instant::now();
    let rootfs: Vec<_> = (0..count)
        .map(|_| RendererRootfs::build(renderer_binary, runtime_library_dir))
        .collect();
    println!(
        "ROOTFS_READY count={count} build_ms={}",
        build_started.elapsed().as_millis()
    );

    let configs: Vec<_> = rootfs
        .iter()
        .enumerate()
        .map(|(i, fs)| fs.config(i))
        .collect();
    let mut supervisor = RendererSupervisor::new(std::env::temp_dir());
    let mut storage = GrantRegistry::new(count * 4 + 8, count.max(2)).unwrap();
    let mut grants = SnapshotGrantRegistry::new(count * 4 + 8, count.max(2)).unwrap();

    let run_result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let mut controller = NavigationController::new(MockRuntime::default())?;
        let frame: FrameId = controller.create_root_frame("https://stress.example/")?;
        let document = controller
            .current_document_id(frame)
            .ok_or("current Document is missing")?;
        controller.parse_document_html(
            document,
            b"<!doctype html><html><head><title>AIBE stress</title></head><body><main><h1>Renderer stress</h1><p>Isolated V8 Snapshot ingestion under concurrency.</p></main></body></html>",
        )?;
        let snapshot = controller.current_document_snapshot(frame)?;

        let mut principals = Vec::with_capacity(count);
        let mut startup_ms = Vec::with_capacity(count);
        let total_start = Instant::now();
        for (index, config) in configs.iter().enumerate() {
            let started = Instant::now();
            let principal = supervisor.spawn_in_jail(config, Duration::from_secs(30))?;
            let spawn_ms = started.elapsed().as_millis() as u64;
            let receipt = send_supervised_document_snapshot(
                &controller,
                frame,
                &mut supervisor,
                principal,
                (index as u64) + 1,
                &mut grants,
            )?;
            assert_eq!(receipt.peer.pid, principal.pid());
            assert_eq!(receipt.document_id, snapshot.document_id());
            assert_eq!(receipt.revision, snapshot.revision());
            principals.push(principal);
            startup_ms.push(spawn_ms);
            if index % 10 == 9 || index + 1 == count {
                println!("PROGRESS count={count} launched={}", index + 1);
            }
        }

        startup_ms.sort_unstable();
        let percentile = |p: usize| -> u64 {
            let index = ((startup_ms.len() - 1) * p).div_ceil(100);
            startup_ms[index]
        };
        let startup_sum: u64 = startup_ms.iter().sum();
        println!(
            "STARTUP_METRICS count={count} total_launch_window_ms={} mean_spawn_ms={} p50_spawn_ms={} p95_spawn_ms={} max_spawn_ms={}",
            total_start.elapsed().as_millis(),
            startup_sum / startup_ms.len() as u64,
            percentile(50),
            percentile(95),
            startup_ms.last().copied().unwrap_or(0)
        );

        let pids: HashSet<u32> = principals.iter().map(|p| p.pid()).collect();
        let started = Instant::now();
        while started.elapsed() < hold {
            let (alive, rss_kb, vsz_kb, threads) = sample_processes(&pids);
            let reclaimable = system_reclaimable_bytes();
            let swap = swap_used_bytes();
            println!(
                "SAMPLE count={count} elapsed_ms={} alive={alive} rss_mib={:.1} vsz_tib={:.2} threads={threads} reclaimable_mib={:.1} swap_used_mib={:.1}",
                started.elapsed().as_millis(),
                rss_kb as f64 / 1024.0,
                vsz_kb as f64 / (1024.0 * 1024.0 * 1024.0),
                reclaimable as f64 / (1024.0 * 1024.0),
                swap as f64 / (1024.0 * 1024.0)
            );
            assert_eq!(alive, count, "a Renderer exited during the hold window");
            if reclaimable < 1536 * 1024 * 1024 || swap > 1024 * 1024 * 1024 {
                return Err(format!(
                    "safety stop: reclaimable={} MiB swap={} MiB",
                    reclaimable / 1024 / 1024,
                    swap / 1024 / 1024
                )
                .into());
            }
            thread::sleep(Duration::from_secs(2));
        }

        // One deliberate Renderer crash at the 100-instance tier validates
        // supervisor detection and Jail/grant cleanup under load.
        if count >= 100 {
            let victim = principals[0];
            let status = Command::new("/bin/kill")
                .args(["-KILL", &victim.pid().to_string()])
                .status()?;
            assert!(status.success());
            let deadline = Instant::now() + Duration::from_secs(20);
            let exit = loop {
                if let Some(exit) = supervisor
                    .poll_exits(&mut storage, &mut grants)?
                    .into_iter()
                    .find(|exit| exit.principal == victim)
                {
                    break exit;
                }
                if Instant::now() >= deadline {
                    return Err(
                        "Supervisor failed to reap crashed Renderer within 20 seconds".into(),
                    );
                }
                thread::sleep(Duration::from_millis(20));
            };
            println!(
                "CRASH_RECOVERY pid={} exit_success={} released_snapshot_grants={}",
                victim.pid(),
                exit.status.success(),
                exit.released_snapshot_grants
            );
            assert_eq!(exit.released_snapshot_grants, 1);
        }

        // Aggregate per-Jail RACCT samples. The vmemoryuse/swapuse counters
        // are reported separately from host swapinfo because V8 reserves a
        // very large virtual address range that is not resident swap usage.
        let mut resource_values: HashMap<&'static str, Vec<(u64, usize)>> = HashMap::new();
        for (index, config) in configs.iter().enumerate() {
            let output = Command::new("/usr/bin/rctl")
                .arg("-hu")
                .arg(format!("jail:{}", config.name))
                .output()?;
            assert!(output.status.success());
            let report = String::from_utf8_lossy(&output.stdout);
            for key in [
                "memoryuse",
                "vmemoryuse",
                "swapuse",
                "openfiles",
                "maxproc",
                "nthr",
            ] {
                if let Some(value) = report.lines().find_map(|line| {
                    let (name, value) = line.trim().split_once('=')?;
                    if name == key {
                        parse_quantity(value).map(|n| (n, index))
                    } else {
                        None
                    }
                }) {
                    resource_values.entry(key).or_default().push(value);
                }
            }
        }
        for (key, values) in &mut resource_values {
            values.sort_by_key(|(value, _)| *value);
            let max = values.last().copied().unwrap_or((0, 0));
            let p95 = values[((values.len() - 1) * 95).div_ceil(100)];
            println!(
                "RCTL_AGGREGATE resource={key} samples={} p95={} max={} max_jail={} ",
                values.len(),
                p95.0,
                max.0,
                configs[max.1].name
            );
            if *key == "openfiles" && max.0 > 64 {
                let pid = principals[max.1].pid();
                let procstat = Command::new("/usr/bin/procstat")
                    .args(["-f", &pid.to_string()])
                    .output()?;
                println!(
                    "PROCSTAT_FD_OUTLIER pid={pid}\n{}",
                    String::from_utf8_lossy(&procstat.stdout)
                );
            }
        }

        let remaining = supervisor.len();
        println!(
            "SHUTDOWN_BEGIN count={count} remaining={remaining} active_snapshot_grants={}",
            grants.active_count()
        );
        Ok(())
    })();

    let cleanup_started = Instant::now();
    let shutdown = supervisor.shutdown(&mut storage, &mut grants);
    println!(
        "SHUTDOWN_METRICS count={count} elapsed_ms={} remaining_supervisor={} storage_grants={} snapshot_grants={}",
        cleanup_started.elapsed().as_millis(),
        supervisor.len(),
        storage.active_total_count(),
        grants.active_count()
    );
    if let Err(error) = shutdown {
        panic!("Renderer Supervisor shutdown failed: {error}");
    }
    if let Err(error) = run_result {
        panic!("Renderer stress stage {count} failed: {error}");
    }

    assert!(supervisor.is_empty());
    assert_eq!(storage.active_total_count(), 0);
    assert_eq!(grants.active_count(), 0);
    for config in &configs {
        assert!(
            !config.rootfs.exists(),
            "rootfs remained after Jail cleanup"
        );
        let jail = Command::new("/usr/sbin/jls")
            .args(["-j", &config.name])
            .output()
            .unwrap();
        assert!(
            !jail.status.success(),
            "Jail remained after cleanup: {}",
            config.name
        );
        let rules = Command::new("/usr/bin/rctl").output().unwrap();
        assert!(
            !String::from_utf8_lossy(&rules.stdout).contains(&config.name),
            "RCTL rules remained after cleanup: {}",
            config.name
        );
    }
    println!(
        "STAGE_END count={count} cleanup_ms={}",
        cleanup_started.elapsed().as_millis()
    );
}

#[test]
fn staged_v8_renderer_concurrency_and_lifecycle_stress() {
    let renderer_binary = Path::new(env!("CARGO_BIN_EXE_snapshot-bridge-renderer"));
    let runtime_library_dir = std::env::var_os("AIBE_RUNTIME_PREFIX")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/aibe-runtime-root/usr/local"))
        .join("lib");
    assert!(
        runtime_library_dir.is_dir(),
        "V8 runtime library directory unavailable"
    );

    let counts = std::env::var("AIBE_STRESS_COUNTS").unwrap_or_else(|_| "10,50,100".to_owned());
    let hold = std::env::var("AIBE_STRESS_HOLD_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(15);
    for count in counts
        .split(',')
        .map(|value| value.trim().parse::<usize>().expect("numeric stage count"))
    {
        assert!(
            [10, 50, 100].contains(&count),
            "only the planned 10/50/100 tiers are valid"
        );
        run_stage(
            count,
            Duration::from_secs(hold),
            renderer_binary,
            &runtime_library_dir,
        );
    }
}
