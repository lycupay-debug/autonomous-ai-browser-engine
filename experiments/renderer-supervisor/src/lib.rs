//! Host-owned Renderer process lifecycle and Broker identity registration PoC.
//!
//! The supervisor creates the private UDS listener, spawns the Renderer,
//! accepts its connection, and derives RendererPrincipal only from the child
//! PID plus kernel-authenticated LOCAL_PEERCRED/getpeereid credentials. It
//! owns both Child and UnixStream and releases Snapshot grants only after
//! observing process exit.

use std::collections::HashMap;
use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aibe_broker_policy_poc::{GrantRegistry, RendererId, RendererPrincipal, SnapshotGrantRegistry};
use aibe_control_protocol_uds_poc::{bind_private_socket, peer_process_credentials};

static NEXT_RENDERER_INSTANCE: AtomicU64 = AtomicU64::new(1);
static NEXT_SOCKET_NONCE: AtomicU64 = AtomicU64::new(1);
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const STREAM_TIMEOUT: Duration = Duration::from_secs(5);

struct ManagedRenderer {
    principal: RendererPrincipal,
    child: Child,
    stream: UnixStream,
    socket_path: PathBuf,
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
    socket_directory: PathBuf,
}

impl RendererSupervisor {
    pub fn new(socket_directory: impl AsRef<Path>) -> Self {
        Self {
            renderers: HashMap::new(),
            socket_directory: socket_directory.as_ref().to_path_buf(),
        }
    }

    /// Spawn a Renderer command and accept its private UDS connection.
    /// The socket path is appended as the final command-line argument.
    /// No RendererPrincipal is accepted from the caller.
    pub fn spawn(
        &mut self,
        mut command: Command,
        connect_timeout: Duration,
    ) -> io::Result<RendererPrincipal> {
        let socket_path = self.next_socket_path()?;
        let listener = bind_private_socket(&socket_path)?;
        listener.set_nonblocking(true)?;
        command.arg(&socket_path);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                drop(listener);
                let _ = std::fs::remove_file(&socket_path);
                return Err(error);
            }
        };

        let accepted = self.accept_child(&listener, &mut child, connect_timeout);
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
                child,
                stream,
                socket_path,
            },
        );
        Ok(principal)
    }

    fn accept_child(
        &self,
        listener: &UnixListener,
        child: &mut Child,
        connect_timeout: Duration,
    ) -> io::Result<(UnixStream, RendererPrincipal)> {
        let deadline = Instant::now() + connect_timeout;
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    let credentials = peer_process_credentials(&stream)?;
                    if credentials.pid != child.id() {
                        // Ignore an unexpected local peer; do not let it cause
                        // the supervisor to kill the child it actually spawned.
                        drop(stream);
                        continue;
                    }
                    let instance = NEXT_RENDERER_INSTANCE
                        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                            current.checked_add(1)
                        })
                        .map_err(|_| io::Error::other("Renderer instance ID exhausted"))?;
                    let id = RendererId::new(instance)
                        .ok_or_else(|| io::Error::other("Renderer instance ID is zero"))?;
                    let principal = RendererPrincipal::new(
                        id,
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
            if let Some(status) = renderer.child.try_wait()? {
                exited.push((*id, renderer.principal, status));
            }
        }
        let mut results = Vec::with_capacity(exited.len());
        for (id, principal, status) in exited {
            if let Some(renderer) = self.renderers.remove(&id) {
                drop(renderer.stream);
                let _ = std::fs::remove_file(renderer.socket_path);
            }
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
            match renderer.child.try_wait()? {
                Some(status) => status,
                None => {
                    renderer.child.kill()?;
                    renderer.child.wait()?
                }
            }
        };
        let renderer = self
            .renderers
            .remove(&id)
            .expect("managed Renderer retained");
        drop(renderer.stream);
        let _ = std::fs::remove_file(renderer.socket_path);
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

impl Drop for RendererSupervisor {
    // Emergency process cleanup only. Call shutdown() when grant registries
    // are available so their per-Renderer records are also released.
    fn drop(&mut self) {
        for (_, mut renderer) in self.renderers.drain() {
            if renderer.child.try_wait().ok().flatten().is_none() {
                let _ = renderer.child.kill();
                let _ = renderer.child.wait();
            }
            drop(renderer.stream);
            let _ = std::fs::remove_file(renderer.socket_path);
        }
    }
}
