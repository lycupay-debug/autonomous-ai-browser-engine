//! Least-privilege descriptor policy for the FreeBSD Renderer process.
//!
//! Call after V8/runtime initialization and UDS connection, but before
//! `cap_enter()`. Only standard input/output/error and the authenticated Broker
//! socket survive. Rights are reduced monotonically and verified by reading
//! them back from the kernel.

use std::io;
#[cfg(target_os = "freebsd")]
use std::mem::MaybeUninit;
#[cfg(target_os = "freebsd")]
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;

#[cfg(target_os = "freebsd")]
fn rights_for(mask: u64) -> io::Result<libc::cap_rights_t> {
    let mut rights = MaybeUninit::<libc::cap_rights_t>::uninit();
    let initialized = unsafe {
        libc::__cap_rights_init(libc::CAP_RIGHTS_VERSION, rights.as_mut_ptr(), mask, 0_u64)
    };
    if initialized.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { rights.assume_init() })
}

#[cfg(target_os = "freebsd")]
fn restrict_and_verify(fd: libc::c_int, mask: u64) -> io::Result<()> {
    let expected = rights_for(mask)?;
    if unsafe { libc::cap_rights_limit(fd, &expected) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut actual = MaybeUninit::<libc::cap_rights_t>::uninit();
    if unsafe { libc::__cap_rights_get(libc::CAP_RIGHTS_VERSION, fd, actual.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let actual = unsafe { actual.assume_init() };
    if !unsafe { libc::cap_rights_contains(&expected, &actual) }
        || !unsafe { libc::cap_rights_contains(&actual, &expected) }
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("descriptor {fd} does not have the exact requested Capsicum rights"),
        ));
    }
    Ok(())
}

/// Close every non-standard descriptor except the connected Broker UDS, then
/// apply and read back the exact rights allowlist. No path, device, directory,
/// process, or network descriptor is retained implicitly.
#[cfg(target_os = "freebsd")]
pub fn apply_renderer_fd_allowlist(stream: &mut UnixStream) -> io::Result<()> {
    const BROKER_IPC_FD: libc::c_int = 3;
    let original_fd = stream.as_raw_fd();
    if original_fd < 3 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Renderer Broker socket must not overlap standard descriptors",
        ));
    }

    if original_fd != BROKER_IPC_FD {
        if unsafe { libc::dup2(original_fd, BROKER_IPC_FD) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // Replacing the stream closes the old descriptor and makes ownership
        // of the fixed Broker FD explicit.
        *stream = unsafe { UnixStream::from_raw_fd(BROKER_IPC_FD) };
    }
    let ipc_fd = BROKER_IPC_FD;
    // FreeBSD closefrom closes every descriptor above the one allowlisted IPC
    // FD without depending on the current soft RLIMIT_NOFILE value.
    unsafe { libc::closefrom(BROKER_IPC_FD + 1) };
    let flags = unsafe { libc::fcntl(ipc_fd, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(ipc_fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }

    // stdin is read-only; stdout/stderr are write-only. The Broker channel
    // receives only stream read/write rights, with no ioctl, fcntl, connect,
    // accept, or pathname authority.
    for (fd, mask) in [
        (libc::STDIN_FILENO, libc::CAP_READ),
        (libc::STDOUT_FILENO, libc::CAP_WRITE),
        (libc::STDERR_FILENO, libc::CAP_WRITE),
        (ipc_fd, libc::CAP_READ | libc::CAP_WRITE),
    ] {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EBADF) {
                if fd == ipc_fd {
                    return Err(error);
                }
                continue;
            }
            return Err(error);
        }
        restrict_and_verify(fd, mask)?;
    }

    // `closefrom(4)` is the authoritative close operation; verify its boundary.
    if unsafe { libc::fcntl(BROKER_IPC_FD + 1, libc::F_GETFD) } >= 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unexpected Renderer descriptor survived closefrom(4)",
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "freebsd"))]
pub fn apply_renderer_fd_allowlist(_stream: &mut UnixStream) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Renderer descriptor allowlist requires FreeBSD Capsicum",
    ))
}
