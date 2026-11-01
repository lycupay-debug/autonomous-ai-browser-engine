#![cfg(target_os = "freebsd")]

use std::io;
use std::mem::size_of;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

use aibe_broker_policy_poc::{RendererId, RendererPrincipal};
use aibe_control_protocol_uds_poc::snapshot_descriptor::{
    SnapshotDescriptorEnvelope, create_snapshot_descriptor, receive_snapshot_descriptor,
    send_snapshot_descriptor,
};

const PAYLOAD: &[u8] = br##"{"envelopeVersion":1,"documentId":"7","revision":"3","tree":{"schemaVersion":1,"nodeCount":1,"root":1,"nodes":[{"id":1,"parent":null,"children":[],"type":"document","nodeType":9,"name":"#document","value":null,"attributes":[]}]}}"##;

#[repr(align(8))]
struct AlignedControl([u8; 128]);

fn principal() -> RendererPrincipal {
    RendererPrincipal::new(
        RendererId::new(0xA1B2).unwrap(),
        unsafe { libc::getpid() as u32 },
        unsafe { libc::geteuid() },
        unsafe { libc::getegid() },
    )
    .unwrap()
}

fn envelope(principal: RendererPrincipal) -> SnapshotDescriptorEnvelope {
    SnapshotDescriptorEnvelope {
        request_id: 71,
        renderer_instance_id: principal.id().value(),
        renderer_pid: principal.pid(),
        renderer_uid: principal.uid(),
        renderer_gid: principal.gid(),
        origin_registry_id: 11,
        origin_group_id: 12,
        grant_registry_id: 13,
        grant_id: 14,
        document_id: 7,
        revision: 3,
        payload_bytes: PAYLOAD.len() as u64,
    }
}

fn envelope_bytes(principal: RendererPrincipal) -> [u8; 104] {
    let mut bytes = [0u8; 104];
    let mut offset = 0usize;
    macro_rules! put {
        ($value:expr) => {{
            let value = $value;
            bytes[offset..offset + value.len()].copy_from_slice(&value);
            offset += value.len();
        }};
    }
    put!(0x4149_4246u32.to_le_bytes());
    put!(1u16.to_le_bytes());
    put!(104u16.to_le_bytes());
    put!(1u16.to_le_bytes());
    put!(0u16.to_le_bytes());
    put!(0u32.to_le_bytes());
    put!(71u64.to_le_bytes());
    put!(principal.id().value().to_le_bytes());
    put!(principal.pid().to_le_bytes());
    put!(principal.uid().to_le_bytes());
    put!(principal.gid().to_le_bytes());
    put!(0u32.to_le_bytes());
    for value in [11u64, 12, 13, 14, 7, 3, PAYLOAD.len() as u64] {
        put!(value.to_le_bytes());
    }
    assert_eq!(offset, bytes.len());
    bytes
}

fn open_fd_set() -> Vec<i32> {
    (0..1024)
        .filter(|fd| unsafe { libc::fcntl(*fd, libc::F_GETFD) } >= 0)
        .collect()
}

fn send_raw_with_fds(stream: &UnixStream, bytes: &[u8], fds: &[i32]) -> io::Result<()> {
    let header_bytes =
        (size_of::<libc::cmsghdr>() + size_of::<usize>() - 1) & !(size_of::<usize>() - 1);
    let data_len = std::mem::size_of_val(fds);
    let cmsg_len = header_bytes + data_len;
    let cmsg_space =
        header_bytes + ((data_len + size_of::<usize>() - 1) & !(size_of::<usize>() - 1));
    let mut control = AlignedControl([0; 128]);
    unsafe {
        std::ptr::write(
            control.0.as_mut_ptr().cast::<libc::cmsghdr>(),
            libc::cmsghdr {
                cmsg_len: cmsg_len as libc::socklen_t,
                cmsg_level: libc::SOL_SOCKET,
                cmsg_type: libc::SCM_RIGHTS,
            },
        );
        std::ptr::copy_nonoverlapping(
            fds.as_ptr().cast::<u8>(),
            control.0.as_mut_ptr().add(header_bytes),
            data_len,
        );
    }
    let mut iov = libc::iovec {
        iov_base: bytes.as_ptr() as *mut _,
        iov_len: bytes.len(),
    };
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.0.as_mut_ptr().cast();
    message.msg_controllen = cmsg_space as libc::socklen_t;
    let sent = unsafe { libc::sendmsg(stream.as_raw_fd(), &message, 0) };
    if sent < 0 {
        return Err(io::Error::last_os_error());
    }
    if sent as usize != bytes.len() {
        return Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "partial raw envelope send",
        ));
    }
    Ok(())
}

#[test]
fn rejected_snapshot_descriptor_messages_close_all_received_fds() {
    // Keep all descriptor-table comparisons in one dedicated integration-test
    // process. Unit tests in the crate share a process-wide FD table and can
    // otherwise create/close unrelated descriptors concurrently.
    let expected = principal();

    {
        let descriptor = create_snapshot_descriptor(PAYLOAD).unwrap();
        let (sender, mut receiver) = UnixStream::pair().unwrap();
        let mut wrong = envelope(expected);
        wrong.renderer_pid += 1;
        send_snapshot_descriptor(&sender, wrong, descriptor.as_raw_fd()).unwrap();
        let before = open_fd_set();
        assert_eq!(
            receive_snapshot_descriptor(&mut receiver, expected)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(open_fd_set(), before, "wrong-principal FD must close");
    }

    {
        let descriptor = create_snapshot_descriptor(PAYLOAD).unwrap();
        let (sender, mut receiver) = UnixStream::pair().unwrap();
        let mut bytes = envelope_bytes(expected);
        bytes[12] = 1; // reserved flags must remain zero
        send_raw_with_fds(&sender, &bytes, &[descriptor.as_raw_fd()]).unwrap();
        let before = open_fd_set();
        assert_eq!(
            receive_snapshot_descriptor(&mut receiver, expected)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(open_fd_set(), before, "invalid-envelope FD must close");
    }

    {
        let first = create_snapshot_descriptor(PAYLOAD).unwrap();
        let second = create_snapshot_descriptor(PAYLOAD).unwrap();
        let (sender, mut receiver) = UnixStream::pair().unwrap();
        let bytes = envelope_bytes(expected);
        send_raw_with_fds(&sender, &bytes, &[first.as_raw_fd(), second.as_raw_fd()]).unwrap();
        let before = open_fd_set();
        assert_eq!(
            receive_snapshot_descriptor(&mut receiver, expected)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            open_fd_set(),
            before,
            "all descriptors from a multi-FD message must close"
        );
    }
}
