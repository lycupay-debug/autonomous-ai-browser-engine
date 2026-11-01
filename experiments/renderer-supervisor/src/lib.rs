//! Host-owned Renderer process lifecycle and Broker identity registration PoC.
//!
//! The supervisor creates the private UDS listener, spawns the Renderer,
//! accepts its connection, and derives RendererPrincipal only from the child
//! PID plus kernel-authenticated LOCAL_PEERCRED/getpeereid credentials. It
//! owns both Child and UnixStream and releases Snapshot grants only after
//! observing process exit.

use std::collections::{HashMap, HashSet};
use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aibe_broker_policy_poc::{GrantRegistry, RendererId, RendererPrincipal, SnapshotGrantRegistry};
use aibe_control_protocol_uds_poc::{bind_private_socket, peer_process_credentials};

pub mod fd_policy;

#[cfg(target_os = "freebsd")]
mod freebsd_jail;
#[cfg(target_os = "freebsd")]
pub use freebsd_jail::{FreeBsdJailConfig, RendererRctlLimits};

static NEXT_RENDERER_INSTANCE: AtomicU64 = AtomicU64::new(1);
static NEXT_SOCKET_NONCE: AtomicU64 = AtomicU64::new(1);
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const STREAM_TIMEOUT: Duration = Duration::from_secs(5);

struct ManagedRenderer {
    principal: RendererPrincipal,
    process: ManagedProcess,
    stream: UnixStream,
    socket_path: PathBuf,
}

enum ManagedProcess {
    Host(Child),
    #[cfg(target_os = "freebsd")]
    Jail {
        launcher: Child,
        name: String,
        jid: i32,
        renderer_pid: u32,
        process_start: (i64, i64),
        renderer_uid: u32,
        rootfs: PathBuf,
        devfs: freebsd_jail::DevfsMount,
        rctl_rules_applied: bool,
        cleanup_rootfs_on_exit: bool,
    },
}

impl ManagedProcess {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        match self {
            Self::Host(child) => child.try_wait(),
            #[cfg(target_os = "freebsd")]
            Self::Jail {
                launcher,
                name,
                jid,
                renderer_pid,
                process_start,
                renderer_uid,
                devfs,
                rctl_rules_applied,
                cleanup_rootfs_on_exit,
                rootfs,
                ..
            } => {
                if let Some(status) = launcher.try_wait()? {
                    freebsd_jail::remove_jail_if_present(name, *rctl_rules_applied)?;
                    devfs.unmount()?;
                    if *cleanup_rootfs_on_exit {
                        std::fs::remove_dir_all(rootfs)?;
                    }
                    return Ok(Some(status));
                }
                match freebsd_jail::process_jail_identity(*renderer_pid)? {
                    Some(identity)
                        if identity.jid == *jid
                            && identity.uid == *renderer_uid
                            && identity.start == *process_start =>
                    {
                        Ok(None)
                    }
                    _ => {
                        freebsd_jail::remove_jail_if_present(name, *rctl_rules_applied)?;
                        devfs.unmount()?;
                        let status = launcher.wait()?;
                        if *cleanup_rootfs_on_exit {
                            std::fs::remove_dir_all(rootfs)?;
                        }
                        Ok(Some(status))
                    }
                }
            }
        }
    }

    fn terminate(&mut self) -> io::Result<ExitStatus> {
        match self {
            Self::Host(child) => match child.try_wait()? {
                Some(status) => Ok(status),
                None => {
                    child.kill()?;
                    child.wait()
                }
            },
            #[cfg(target_os = "freebsd")]
            Self::Jail {
                launcher,
                name,
                devfs,
                rctl_rules_applied,
                cleanup_rootfs_on_exit,
                rootfs,
                ..
            } => {
                freebsd_jail::remove_jail_if_present(name, *rctl_rules_applied)?;
                devfs.unmount()?;
                let status = match launcher.try_wait()? {
                    Some(status) => status,
                    None => launcher.wait()?,
                };
                if *cleanup_rootfs_on_exit {
                    std::fs::remove_dir_all(rootfs)?;
                }
                Ok(status)
            }
        }
    }
}

#[derive(Debug)]
pub struct RendererExit {
    pub principal: RendererPrincipal,
    pub status: ExitStatus,
    pub released_storage_grants: usize,
    pub released_snapshot_grants: usize,
}

/// Owns Renderer child processes and their authenticated Broker channels.
pub struct RendererSupervisor {
    renderers: HashMap<RendererId, ManagedRenderer>,
    request_ids: HashMap<RendererId, HashSet<u64>>,
    max_total_requests: usize,
    max_per_renderer_requests: usize,
    socket_directory: PathBuf,
}

fn next_renderer_id() -> io::Result<RendererId> {
    let instance = NEXT_RENDERER_INSTANCE
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .map_err(|_| io::Error::other("Renderer instance ID exhausted"))?;
    RendererId::new(instance).ok_or_else(|| io::Error::other("Renderer instance ID is zero"))
}

impl RendererSupervisor {
    pub fn new(socket_directory: impl AsRef<Path>) -> Self {
        Self::with_request_quotas(socket_directory, 4096, 256)
            .expect("default Renderer request quotas are valid")
    }

    pub fn with_request_quotas(
        socket_directory: impl AsRef<Path>,
        max_total_requests: usize,
        max_per_renderer_requests: usize,
    ) -> io::Result<Self> {
        if max_total_requests == 0
            || max_per_renderer_requests == 0
            || max_per_renderer_requests > max_total_requests
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Renderer request quotas must be nonzero and per-Renderer quota cannot exceed global quota",
            ));
        }
        Ok(Self {
            renderers: HashMap::new(),
            request_ids: HashMap::new(),
            max_total_requests,
            max_per_renderer_requests,
            socket_directory: socket_directory.as_ref().to_path_buf(),
        })
    }

    /// Spawn a Renderer command and accept its private UDS connection.
    /// The socket path is appended as the final command-line argument.
    /// No RendererPrincipal is accepted from the caller.
    /// Unisolated host-process launch retained only for Phase 0 lifecycle tests.
    /// Never use this for page-controlled production content; use `spawn_in_jail`.
    pub fn spawn_unisolated_for_test(
        &mut self,
        mut command: Command,
        connect_timeout: Duration,
    ) -> io::Result<RendererPrincipal> {
        let renderer_id = next_renderer_id()?;
        let socket_path = self.next_socket_path()?;
        let listener = bind_private_socket(&socket_path)?;
        if let Err(error) = listener.set_nonblocking(true) {
            drop(listener);
            let _ = std::fs::remove_file(&socket_path);
            return Err(error);
        }
        command
            .arg(&socket_path)
            .env("AIBE_RENDERER_INSTANCE_ID", renderer_id.value().to_string());
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                drop(listener);
                let _ = std::fs::remove_file(&socket_path);
                return Err(error);
            }
        };

        let accepted = self.accept_child(&listener, &mut child, renderer_id, connect_timeout);
        drop(listener);
        let (stream, principal) = match accepted {
            Ok(value) => value,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(&socket_path);
                return Err(error);
            }
        };
        let _ = std::fs::remove_file(&socket_path);
        if let Err(error) = stream
            .set_read_timeout(Some(STREAM_TIMEOUT))
            .and_then(|()| stream.set_write_timeout(Some(STREAM_TIMEOUT)))
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }

        self.renderers.insert(
            principal.id(),
            ManagedRenderer {
                principal,
                process: ManagedProcess::Host(child),
                stream,
                socket_path,
            },
        );
        Ok(principal)
    }

    /// Reserve a nonzero request ID for one exact Renderer process instance.
    /// Reservations are retained after uncertain writes and released only
    /// after the supervisor confirms that the child process has exited.
    pub fn reserve_request(
        &mut self,
        principal: RendererPrincipal,
        request_id: u64,
    ) -> io::Result<()> {
        if request_id == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "request ID must be nonzero",
            ));
        }
        let Some(renderer) = self.renderers.get(&principal.id()) else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Renderer instance is not managed",
            ));
        };
        if renderer.principal != principal {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Renderer principal does not match the managed process",
            ));
        }
        if self
            .request_ids
            .get(&principal.id())
            .is_some_and(|ids| ids.contains(&request_id))
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "request ID was already used by this Renderer instance",
            ));
        }
        let per_renderer_count = self
            .request_ids
            .get(&principal.id())
            .map_or(0, HashSet::len);
        if per_renderer_count >= self.max_per_renderer_requests {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "per-Renderer request quota exceeded",
            ));
        }
        let total = self.request_ids.values().map(HashSet::len).sum::<usize>();
        if total >= self.max_total_requests {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "global Renderer request quota exceeded",
            ));
        }
        self.request_ids
            .entry(principal.id())
            .or_default()
            .insert(request_id);
        Ok(())
    }

    pub fn active_request_count(&self) -> usize {
        self.request_ids.values().map(HashSet::len).sum()
    }

    pub fn close_connection(&mut self, principal: RendererPrincipal) -> io::Result<()> {
        let renderer = self
            .renderers
            .get(&principal.id())
            .filter(|renderer| renderer.principal == principal)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "Renderer instance is not managed")
            })?;
        renderer.stream.shutdown(std::net::Shutdown::Both)
    }

    fn accept_child(
        &self,
        listener: &UnixListener,
        child: &mut Child,
        renderer_id: RendererId,
        connect_timeout: Duration,
    ) -> io::Result<(UnixStream, RendererPrincipal)> {
        let deadline = Instant::now() + connect_timeout;
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    let credentials = match peer_process_credentials(&stream) {
                        Ok(credentials) => credentials,
                        Err(_) => {
                            // An unauthenticated local connection is not the
                            // Renderer we spawned. Ignore it and keep waiting
                            // within the original bounded accept deadline.
                            drop(stream);
                            continue;
                        }
                    };
                    if credentials.pid != child.id() {
                        // Ignore an unexpected local peer; do not let it cause
                        // the supervisor to kill the child it actually spawned.
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
                    return Ok((stream, principal));
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if let Some(status) = child.try_wait()? {
                        return Err(io::Error::new(
                            io::ErrorKind::ConnectionAborted,
                            format!(
                                "Renderer exited before authenticating its UDS connection: {status}"
                            ),
                        ));
                    }
                    if Instant::now() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "Renderer did not connect before the supervisor deadline",
                        ));
                    }
                    thread::sleep(ACCEPT_POLL_INTERVAL);
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn next_socket_path(&self) -> io::Result<PathBuf> {
        let nonce = NEXT_SOCKET_NONCE
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| io::Error::other("Renderer socket nonce exhausted"))?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        Ok(self.socket_directory.join(format!(
            "aibe-renderer-{}-{timestamp}-{nonce}.sock",
            std::process::id()
        )))
    }

    pub fn principal(&self, id: RendererId) -> Option<RendererPrincipal> {
        self.renderers.get(&id).map(|renderer| renderer.principal)
    }

    pub fn stream_mut(&mut self, principal: RendererPrincipal) -> Option<&mut UnixStream> {
        self.renderers
            .get_mut(&principal.id())
            .filter(|renderer| renderer.principal == principal)
            .map(|renderer| &mut renderer.stream)
    }

    /// Reap exited children and release all grants bound to each exact process
    /// instance. PID reuse cannot match a replacement RendererId.
    pub fn poll_exits(
        &mut self,
        storage_grants: &mut GrantRegistry,
        snapshot_grants: &mut SnapshotGrantRegistry,
    ) -> io::Result<Vec<RendererExit>> {
        let mut exited = Vec::new();
        for (id, renderer) in &mut self.renderers {
            if let Some(status) = renderer.process.try_wait()? {
                exited.push((*id, renderer.principal, status));
            }
        }
        let mut results = Vec::with_capacity(exited.len());
        for (id, principal, status) in exited {
            if let Some(renderer) = self.renderers.remove(&id) {
                drop(renderer.stream);
                let _ = std::fs::remove_file(renderer.socket_path);
            }
            self.request_ids.remove(&id);
            let released_storage_grants = storage_grants.release_renderer(principal);
            let released_snapshot_grants = snapshot_grants.release_renderer(principal);
            results.push(RendererExit {
                principal,
                status,
                released_storage_grants,
                released_snapshot_grants,
            });
        }
        Ok(results)
    }

    /// Terminate one managed Renderer, wait for confirmed exit, then revoke
    /// every grant associated with that exact Renderer process instance.
    pub fn terminate(
        &mut self,
        id: RendererId,
        storage_grants: &mut GrantRegistry,
        snapshot_grants: &mut SnapshotGrantRegistry,
    ) -> io::Result<RendererExit> {
        let status = {
            let renderer = self.renderers.get_mut(&id).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "Renderer instance is not managed")
            })?;
            renderer.process.terminate()?
        };
        let renderer = self
            .renderers
            .remove(&id)
            .expect("managed Renderer retained");
        drop(renderer.stream);
        let _ = std::fs::remove_file(renderer.socket_path);
        self.request_ids.remove(&id);
        let released_storage_grants = storage_grants.release_renderer(renderer.principal);
        let released_snapshot_grants = snapshot_grants.release_renderer(renderer.principal);
        Ok(RendererExit {
            principal: renderer.principal,
            status,
            released_storage_grants,
            released_snapshot_grants,
        })
    }

    /// Gracefully close the supervisor's managed set by terminating every
    /// child, waiting for exit, and releasing both storage and Snapshot grants.
    pub fn shutdown(
        &mut self,
        storage_grants: &mut GrantRegistry,
        snapshot_grants: &mut SnapshotGrantRegistry,
    ) -> io::Result<Vec<RendererExit>> {
        let ids: Vec<_> = self.renderers.keys().copied().collect();
        let mut exits = Vec::with_capacity(ids.len());
        for id in ids {
            exits.push(self.terminate(id, storage_grants, snapshot_grants)?);
        }
        Ok(exits)
    }

    pub fn len(&self) -> usize {
        self.renderers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.renderers.is_empty()
    }
}

/// Enter FreeBSD Capsicum capability mode in the calling Renderer process.
/// All required runtime libraries and filesystem resources must be opened first.
#[cfg(target_os = "freebsd")]
pub fn enter_renderer_capability_mode() -> io::Result<()> {
    if unsafe { libc::cap_enter() } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut mode: libc::c_uint = 0;
    if unsafe { libc::cap_getmode(&mut mode) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if mode != 1 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Capsicum capability mode was not enabled",
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "freebsd"))]
pub fn enter_renderer_capability_mode() -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Capsicum capability mode requires FreeBSD",
    ))
}

impl Drop for RendererSupervisor {
    // Emergency process cleanup only. Call shutdown() when grant registries
    // are available so their per-Renderer records are also released.
    fn drop(&mut self) {
        for (_, mut renderer) in self.renderers.drain() {
            let _ = renderer.process.terminate();
            drop(renderer.stream);
            let _ = std::fs::remove_file(renderer.socket_path);
        }
    }
}
