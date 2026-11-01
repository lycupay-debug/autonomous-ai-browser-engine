use super::*;
use std::collections::HashSet;
use std::ffi::OsString;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use aibe_control_protocol_uds_poc::{bind_private_socket, peer_process_credentials};

static NEXT_DEVFS_RULESET: AtomicU64 = AtomicU64::new(0);

#[cfg(target_os = "freebsd")]
#[derive(Clone, Debug, Default)]
pub struct RendererRctlLimits {
    pub maxproc: Option<u32>,
    pub vmemoryuse_bytes: Option<u64>,
    pub memoryuse_bytes: Option<u64>,
    pub openfiles: Option<u32>,
    pub swapuse_bytes: Option<u64>,
}

#[cfg(target_os = "freebsd")]
#[derive(Clone, Debug)]
/// Configuration for a prebuilt, per-Renderer root filesystem. The rootfs must
/// be dedicated to this instance, root-owned, non-writable by untrusted users,
/// and contain the executable plus an accessible `/run` directory. `/tmp` may
/// be a per-instance sticky directory. Runtime libraries must be staged before
/// launch; this adapter does not copy host libraries into the Jail.
pub struct FreeBsdJailConfig {
    pub name: String,
    pub rootfs: PathBuf,
    pub username: String,
    pub expected_uid: u32,
    pub expected_gid: u32,
    pub executable: PathBuf,
    pub socket_path: PathBuf,
    /// Optional absolute library search directory inside the Jail.
    pub library_path: Option<PathBuf>,
    /// Optional per-Jail deny limits. Empty by default until Renderer workload
    /// measurements establish a safe production budget.
    pub rctl_limits: RendererRctlLimits,
    /// Remove this dedicated rootfs after Jail teardown. The caller retains
    /// ownership when false; true is recommended for ephemeral Renderer roots.
    pub cleanup_rootfs_on_exit: bool,
    /// Optional bounded post-ingestion idle timeout passed to the Renderer.
    /// Values are milliseconds and must be between 1s and 5m.
    pub renderer_idle_timeout_ms: Option<u64>,
    pub args: Vec<OsString>,
}

impl RendererSupervisor {
    /// Launch one Renderer in a dedicated FreeBSD Jail and bind its principal
    /// to the kernel-authenticated peer PID/UID/GID and kernel-reported Jail ID.
    #[cfg(target_os = "freebsd")]
    pub fn spawn_in_jail(
        &mut self,
        config: &FreeBsdJailConfig,
        connect_timeout: Duration,
    ) -> io::Result<RendererPrincipal> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::PermissionsExt;

        validate_jail_config(config)?;
        let renderer_id = super::next_renderer_id()?;
        if jail_id(&config.name)?.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "FreeBSD Jail name is already active",
            ));
        }
        let rootfs = config.rootfs.canonicalize()?;
        if self
            .renderers
            .values()
            .any(|renderer| match &renderer.process {
                ManagedProcess::Host(_) => false,
                #[cfg(target_os = "freebsd")]
                ManagedProcess::Jail {
                    rootfs: active_rootfs,
                    ..
                } => active_rootfs == &rootfs,
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "each Renderer must have a distinct Jail rootfs",
            ));
        }
        if rootfs == Path::new("/") {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "host root cannot be used as a Renderer Jail rootfs",
            ));
        }
        use std::os::unix::fs::MetadataExt;
        let rootfs_metadata = std::fs::metadata(&rootfs)?;
        if rootfs_metadata.uid() != 0 || rootfs_metadata.mode() & 0o022 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Renderer rootfs must be root-owned and not group/world writable",
            ));
        }
        let rootfs_parent = rootfs.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Renderer rootfs must have a parent directory",
            )
        })?;
        let parent_metadata = std::fs::metadata(rootfs_parent)?;
        let parent_is_sticky = parent_metadata.mode() & 0o1000 != 0;
        if parent_metadata.uid() != 0 || (parent_metadata.mode() & 0o022 != 0 && !parent_is_sticky)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Renderer rootfs parent must be root-owned and not untrusted-writable unless sticky",
            ));
        }
        validate_rootfs_tree(&rootfs)?;
        let devfs = DevfsMount::mount(&rootfs.join("dev"))?;
        if let Some(library_path) = &config.library_path {
            if !library_path.is_absolute()
                || library_path.components().any(|part| {
                    matches!(
                        part,
                        std::path::Component::ParentDir | std::path::Component::CurDir
                    )
                })
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Jail library path must be a normalized absolute path",
                ));
            }
            let library_host = rootfs.join(
                library_path
                    .strip_prefix("/")
                    .expect("validated absolute path"),
            );
            if !library_host.is_dir() || !library_host.canonicalize()?.starts_with(&rootfs) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Jail library path must exist inside the dedicated rootfs",
                ));
            }
        }
        let executable_host = rootfs.join(
            config
                .executable
                .strip_prefix("/")
                .expect("validated absolute path"),
        );
        let socket_host = rootfs.join(
            config
                .socket_path
                .strip_prefix("/")
                .expect("validated absolute path"),
        );
        let socket_parent = socket_host
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "UDS path has no parent"))?;
        if !socket_parent.canonicalize()?.starts_with(&rootfs)
            || !executable_host.is_file()
            || !executable_host.canonicalize()?.starts_with(&rootfs)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "Jail executable or UDS parent escapes the dedicated rootfs: rootfs={} socket_parent={} executable={} executable_canonical={:?}",
                    rootfs.display(),
                    socket_parent.display(),
                    executable_host.display(),
                    executable_host.canonicalize().ok()
                ),
            ));
        }
        for protected_path in [
            socket_parent,
            executable_host.parent().expect("executable has parent"),
            &executable_host,
        ] {
            let metadata = std::fs::metadata(protected_path)?;
            if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Renderer rootfs runtime paths must be root-owned and not group/world writable",
                ));
            }
        }
        let listener = bind_private_socket(&socket_host)?;
        if let Err(error) = listener.set_nonblocking(true) {
            drop(listener);
            let _ = std::fs::remove_file(&socket_host);
            return Err(error);
        }
        let socket_setup = (|| -> io::Result<()> {
            let socket_c = std::ffi::CString::new(socket_host.as_os_str().as_encoded_bytes())
                .map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidInput, "UDS path contains NUL")
                })?;
            if unsafe { libc::chown(socket_c.as_ptr(), config.expected_uid, config.expected_gid) }
                != 0
            {
                return Err(io::Error::last_os_error());
            }
            std::fs::set_permissions(&socket_host, std::fs::Permissions::from_mode(0o600))?;
            let fd_flags = unsafe { libc::fcntl(listener.as_raw_fd(), libc::F_GETFD) };
            if fd_flags < 0
                || unsafe {
                    libc::fcntl(
                        listener.as_raw_fd(),
                        libc::F_SETFD,
                        fd_flags | libc::FD_CLOEXEC,
                    )
                } < 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        })();
        if let Err(error) = socket_setup {
            drop(listener);
            let _ = std::fs::remove_file(&socket_host);
            return Err(error);
        }

        let jail_create = Command::new("/usr/sbin/jail")
            .arg("-q")
            .arg("-c")
            .arg(format!("name={}", config.name))
            .arg(format!("path={}", rootfs.display()))
            .arg(format!("host.hostname={}", config.name))
            .arg("ip4=disable")
            .arg("ip6=disable")
            .arg("enforce_statfs=2")
            .arg("persist")
            .status();
        let jail_create = match jail_create {
            Ok(status) => status,
            Err(error) => {
                drop(listener);
                let _ = std::fs::remove_file(&socket_host);
                return Err(error);
            }
        };
        if !jail_create.success() {
            let _ = remove_jail_if_present(&config.name, false);
            drop(listener);
            let _ = std::fs::remove_file(&socket_host);
            return Err(io::Error::other(
                "failed to create persistent Renderer Jail",
            ));
        }
        let rctl_rules_applied = match apply_rctl_limits(&config.name, &config.rctl_limits) {
            Ok(applied) => applied,
            Err(error) => {
                let _ = remove_jail_if_present(&config.name, true);
                drop(listener);
                let _ = std::fs::remove_file(&socket_host);
                return Err(error);
            }
        };

        let mut launcher_command = Command::new("/usr/sbin/jexec");
        launcher_command
            .arg("-u")
            .arg(&config.username)
            .arg(&config.name)
            .arg(&config.executable)
            .args(&config.args)
            .arg(&config.socket_path)
            .env_clear()
            .env("PATH", "/bin:/usr/bin:/usr/sbin")
            .env("HOME", "/nonexistent")
            .env("AIBE_RENDERER_INSTANCE_ID", renderer_id.value().to_string());
        if let Some(timeout_ms) = config.renderer_idle_timeout_ms {
            launcher_command.env("AIBE_RENDERER_IDLE_TIMEOUT_MS", timeout_ms.to_string());
        }
        if let Some(library_path) = &config.library_path {
            launcher_command.env("LD_LIBRARY_PATH", library_path);
        }
        let mut launcher = match launcher_command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let _ = remove_jail_if_present(&config.name, rctl_rules_applied);
                let _ = std::fs::remove_file(&socket_host);
                return Err(error);
            }
        };
        let accepted = accept_jail_child(
            &listener,
            &mut launcher,
            &config.name,
            config.expected_uid,
            config.expected_gid,
            renderer_id,
            connect_timeout,
        );
        drop(listener);
        let (stream, principal, jid, process_start) = match accepted {
            Ok(value) => value,
            Err(error) => {
                let _ = remove_jail_if_present(&config.name, rctl_rules_applied);
                let _ = launcher.kill();
                let _ = launcher.wait();
                let _ = std::fs::remove_file(&socket_host);
                return Err(error);
            }
        };
        let _ = std::fs::remove_file(&socket_host);
        if let Err(error) = stream
            .set_read_timeout(Some(STREAM_TIMEOUT))
            .and_then(|()| stream.set_write_timeout(Some(STREAM_TIMEOUT)))
        {
            let mut process = ManagedProcess::Jail {
                launcher,
                name: config.name.clone(),
                jid,
                renderer_pid: principal.pid(),
                process_start,
                renderer_uid: config.expected_uid,
                rootfs: rootfs.clone(),
                devfs,
                rctl_rules_applied,
                cleanup_rootfs_on_exit: config.cleanup_rootfs_on_exit,
            };
            let _ = process.terminate();
            return Err(error);
        }
        self.renderers.insert(
            principal.id(),
            ManagedRenderer {
                principal,
                process: ManagedProcess::Jail {
                    launcher,
                    name: config.name.clone(),
                    jid,
                    renderer_pid: principal.pid(),
                    process_start,
                    renderer_uid: config.expected_uid,
                    rootfs: rootfs.clone(),
                    devfs,
                    rctl_rules_applied,
                    cleanup_rootfs_on_exit: config.cleanup_rootfs_on_exit,
                },
                stream,
                socket_path: socket_host,
            },
        );
        Ok(principal)
    }
}

#[cfg(target_os = "freebsd")]
pub(super) struct DevfsMount {
    mountpoint: PathBuf,
    ruleset: u64,
    mounted: bool,
}

#[cfg(target_os = "freebsd")]
impl DevfsMount {
    fn mount(mountpoint: &Path) -> io::Result<Self> {
        if !mountpoint.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Renderer rootfs must contain a /dev mountpoint directory",
            ));
        }
        let configured = Command::new("/sbin/devfs")
            .arg("rule")
            .arg("showsets")
            .output()?;
        if !configured.status.success() {
            return Err(io::Error::other(
                "unable to inspect existing devfs rulesets",
            ));
        }
        let existing: HashSet<u64> = String::from_utf8_lossy(&configured.stdout)
            .split_whitespace()
            .filter_map(|value| value.parse().ok())
            .collect();
        let base = 10_000 + (std::process::id() as u64 % 50_000);
        let offset = NEXT_DEVFS_RULESET.fetch_add(1, Ordering::Relaxed) % 50_000;
        let first = 10_000 + ((base - 10_000 + offset) % 50_000);
        let mut ruleset = first;
        while existing.contains(&ruleset) {
            ruleset = 10_000 + ((ruleset - 10_000 + 1) % 50_000);
            if ruleset == first {
                return Err(io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    "no unused devfs ruleset ID is available",
                ));
            }
        }
        let mut mounted = Self {
            mountpoint: mountpoint.to_path_buf(),
            ruleset,
            mounted: false,
        };
        for arguments in [
            vec!["add", "hide"],
            vec!["add", "path", "null", "unhide"],
            vec!["add", "path", "zero", "unhide"],
            vec!["add", "path", "random", "unhide"],
            vec!["add", "path", "urandom", "unhide"],
            vec!["add", "path", "fd", "unhide"],
            vec!["add", "path", "fd/*", "unhide"],
            vec!["add", "path", "stdin", "unhide"],
            vec!["add", "path", "stdout", "unhide"],
            vec!["add", "path", "stderr", "unhide"],
        ] {
            let mut command = Command::new("/sbin/devfs");
            command
                .arg("rule")
                .arg("-s")
                .arg(ruleset.to_string())
                .args(arguments);
            let output = command.output()?;
            if !output.status.success() {
                let _ = mounted.remove_ruleset();
                return Err(io::Error::other(format!(
                    "failed to configure restricted Renderer devfs ruleset: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }
        }
        let output = Command::new("/sbin/mount")
            .args(["-t", "devfs", "-o"])
            .arg(format!("ruleset={ruleset}"))
            .arg("devfs")
            .arg(mountpoint)
            .output()?;
        if !output.status.success() {
            let _ = mounted.remove_ruleset();
            return Err(io::Error::other(format!(
                "failed to mount restricted devfs: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        mounted.mounted = true;
        Ok(mounted)
    }

    fn remove_ruleset(&mut self) -> io::Result<()> {
        let output = Command::new("/sbin/devfs")
            .arg("rule")
            .arg("-s")
            .arg(self.ruleset.to_string())
            .arg("delset")
            .output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "failed to remove temporary devfs ruleset {}: {}",
                self.ruleset,
                String::from_utf8_lossy(&output.stderr)
            )))
        }
    }

    pub(super) fn unmount(&mut self) -> io::Result<()> {
        if self.mounted {
            let output = Command::new("/sbin/umount")
                .arg(&self.mountpoint)
                .output()?;
            if !output.status.success() {
                return Err(io::Error::other(format!(
                    "failed to unmount Renderer devfs: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }
            self.mounted = false;
        }
        self.remove_ruleset()
    }
}

#[cfg(target_os = "freebsd")]
impl Drop for DevfsMount {
    fn drop(&mut self) {
        let _ = self.unmount();
    }
}

#[cfg(target_os = "freebsd")]
fn validate_jail_config(config: &FreeBsdJailConfig) -> io::Result<()> {
    if config
        .renderer_idle_timeout_ms
        .is_some_and(|value| !(1_000..=300_000).contains(&value))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Renderer idle timeout must be between 1000 and 300000 ms",
        ));
    }
    let valid_name = !config.name.is_empty()
        && config.name.len() <= 63
        && config
            .name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    if !valid_name
        || config.username.is_empty()
        || config.username.contains('/')
        || config.expected_uid == 0
        || config.expected_gid == 0
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid Jail name or non-root Renderer identity is required",
        ));
    }
    if [
        config.rctl_limits.maxproc.map(u64::from),
        config.rctl_limits.vmemoryuse_bytes,
        config.rctl_limits.memoryuse_bytes,
        config.rctl_limits.openfiles.map(u64::from),
        config.rctl_limits.swapuse_bytes,
    ]
    .into_iter()
    .flatten()
    .any(|value| value == 0)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Renderer Jail RCTL limits must be nonzero",
        ));
    }
    for path in [&config.executable, &config.socket_path] {
        if !path.is_absolute()
            || path.components().any(|part| {
                matches!(
                    part,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
            || path == Path::new("/")
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Jail executable and UDS paths must be normalized absolute paths",
            ));
        }
    }
    Ok(())
}

#[cfg(target_os = "freebsd")]
fn validate_rootfs_tree(rootfs: &Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let mut pending = vec![rootfs.to_path_buf()];
    let mut visited_directories = HashSet::new();
    while let Some(directory) = pending.pop() {
        let canonical_directory = directory.canonicalize()?;
        if !visited_directories.insert(canonical_directory.clone()) {
            continue;
        }
        for entry in std::fs::read_dir(&canonical_directory)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                let target = path.canonicalize()?;
                if metadata.uid() != 0 || !target.starts_with(rootfs) {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "Renderer rootfs symlink must be root-owned and resolve inside its root",
                    ));
                }
                if target.is_dir() {
                    pending.push(target);
                }
                continue;
            }
            let relative = path.strip_prefix(rootfs).unwrap_or(&path);
            let is_private_tmp = relative == Path::new("tmp")
                && metadata.is_dir()
                && metadata.mode() & 0o1777 == 0o1777;
            if metadata.uid() != 0 || (metadata.mode() & 0o022 != 0 && !is_private_tmp) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "Renderer rootfs entry is writable by a non-root user or not root-owned: {}",
                        path.display()
                    ),
                ));
            }
            if metadata.is_dir() && !is_private_tmp {
                pending.push(path);
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "freebsd")]
fn jail_id(name: &str) -> io::Result<Option<i32>> {
    let output = Command::new("/usr/sbin/jls")
        .args(["-j", name, "jid"])
        .output()?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        if error.contains("not found") {
            return Ok(None);
        }
        return Err(io::Error::other(format!(
            "jls failed while checking Jail {name}: {error}"
        )));
    }
    let value = String::from_utf8(output.stdout)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    value
        .split_whitespace()
        .next()
        .and_then(|item| item.parse::<i32>().ok())
        .filter(|jid| *jid > 0)
        .map(Some)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "jls returned an invalid Jail ID",
            )
        })
}

#[cfg(target_os = "freebsd")]
#[derive(Clone, Copy)]
pub(super) struct KernelProcessIdentity {
    pub(super) jid: i32,
    pub(super) uid: u32,
    ppid: i32,
    pub(super) start: (i64, i64),
}

#[cfg(target_os = "freebsd")]
pub(super) fn process_jail_identity(pid: u32) -> io::Result<Option<KernelProcessIdentity>> {
    use std::mem::MaybeUninit;
    let pid = i32::try_from(pid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Renderer PID exceeds pid_t"))?;
    let mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PID, pid];
    let mut info = MaybeUninit::<libc::kinfo_proc>::zeroed();
    let mut length = std::mem::size_of::<libc::kinfo_proc>();
    let result = unsafe {
        libc::sysctl(
            mib.as_ptr(),
            mib.len() as libc::c_uint,
            info.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null(),
            0,
        )
    };
    if result != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(None);
        }
        return Err(error);
    }
    if length != std::mem::size_of::<libc::kinfo_proc>() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "KERN_PROC_PID returned unexpected kinfo_proc size",
        ));
    }
    let info = unsafe { info.assume_init() };
    if info.ki_pid != pid {
        return Ok(None);
    }
    Ok(Some(KernelProcessIdentity {
        jid: info.ki_jid,
        uid: info.ki_uid,
        ppid: info.ki_ppid,
        start: (info.ki_start.tv_sec, info.ki_start.tv_usec),
    }))
}

#[cfg(target_os = "freebsd")]
fn apply_rctl_limits(name: &str, limits: &RendererRctlLimits) -> io::Result<bool> {
    let mut rules = Vec::new();
    if let Some(value) = limits.maxproc {
        if value == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "maxproc limit must be nonzero",
            ));
        }
        rules.push(format!("jail:{name}:maxproc:deny={value}/jail"));
    }
    if let Some(value) = limits.vmemoryuse_bytes {
        if value == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "vmemoryuse limit must be nonzero",
            ));
        }
        rules.push(format!("jail:{name}:vmemoryuse:deny={value}/jail"));
    }
    if let Some(value) = limits.memoryuse_bytes {
        if value == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "memoryuse limit must be nonzero",
            ));
        }
        rules.push(format!("jail:{name}:memoryuse:deny={value}/jail"));
    }
    if let Some(value) = limits.openfiles {
        if value == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "openfiles limit must be nonzero",
            ));
        }
        rules.push(format!("jail:{name}:openfiles:deny={value}/jail"));
    }
    if let Some(value) = limits.swapuse_bytes {
        if value == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "swapuse limit must be nonzero",
            ));
        }
        rules.push(format!("jail:{name}:swapuse:deny={value}/jail"));
    }
    let mut applied = false;
    for rule in rules {
        let output = Command::new("/usr/bin/rctl")
            .arg("-a")
            .arg(&rule)
            .output()?;
        if !output.status.success() {
            if applied {
                let _ = remove_rctl_rules(name);
            }
            return Err(io::Error::other(format!(
                "failed to apply Renderer Jail RCTL rule {rule}: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        applied = true;
    }
    Ok(applied)
}

#[cfg(target_os = "freebsd")]
fn remove_rctl_rules(name: &str) -> io::Result<()> {
    let output = Command::new("/usr/bin/rctl")
        .arg("-r")
        .arg(format!("jail:{name}"))
        .output()?;
    if output.status.success()
        || String::from_utf8_lossy(&output.stderr).contains("No such process")
    {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "failed to remove Renderer Jail RCTL rules: {}",
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

#[cfg(target_os = "freebsd")]
pub(super) fn remove_jail_if_present(name: &str, rctl_rules_applied: bool) -> io::Result<()> {
    if jail_id(name)?.is_some() {
        let output = Command::new("/usr/sbin/jail").args(["-r", name]).output()?;
        if !output.status.success() && jail_id(name)?.is_some() {
            return Err(io::Error::other(format!(
                "failed to destroy Renderer Jail {name}: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
    }
    if rctl_rules_applied {
        remove_rctl_rules(name)?;
    }
    Ok(())
}

#[cfg(target_os = "freebsd")]
fn accept_jail_child(
    listener: &UnixListener,
    launcher: &mut Child,
    jail_name: &str,
    expected_uid: u32,
    expected_gid: u32,
    renderer_id: RendererId,
    connect_timeout: Duration,
) -> io::Result<(UnixStream, RendererPrincipal, i32, (i64, i64))> {
    let deadline = Instant::now() + connect_timeout;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let credentials = match peer_process_credentials(&stream) {
                    Ok(value) => value,
                    Err(_) => {
                        drop(stream);
                        continue;
                    }
                };
                if credentials.uid != expected_uid || credentials.gid != expected_gid {
                    drop(stream);
                    continue;
                }
                let Some(jid) = jail_id(jail_name)? else {
                    drop(stream);
                    continue;
                };
                let Some(identity) = process_jail_identity(credentials.pid)? else {
                    drop(stream);
                    continue;
                };
                if identity.jid != jid
                    || identity.uid != expected_uid
                    || (credentials.pid != launcher.id() && identity.ppid != launcher.id() as i32)
                {
                    drop(stream);
                    continue;
                }
                let principal = RendererPrincipal::new(
                    renderer_id,
                    credentials.pid,
                    credentials.uid,
                    credentials.gid,
                )
                .ok_or_else(|| io::Error::other("kernel returned an invalid Renderer PID"))?;
                return Ok((stream, principal, jid, identity.start));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if let Some(status) = launcher.try_wait()? {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionAborted,
                        format!("Renderer Jail launcher exited before authentication: {status}"),
                    ));
                }
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Renderer did not connect from its expected Jail before deadline",
                    ));
                }
                thread::sleep(ACCEPT_POLL_INTERVAL);
            }
            Err(error) => return Err(error),
        }
    }
}
