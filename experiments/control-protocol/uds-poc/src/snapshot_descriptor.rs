//! FreeBSD SCM_RIGHTS transport for immutable Browser DOM Snapshot payloads.
//!
//! The Broker sends a fixed-size grant envelope plus exactly one read-only
//! anonymous shared-memory descriptor over the authenticated UnixStream. The
//! descriptor carries the bounded Snapshot bytes; no path or pathname lookup
//! is required in Renderer capability mode.

#[cfg(target_os = "freebsd")]
mod freebsd {
    use std::fs::File;
    use std::io::{self, Read, Seek, SeekFrom, Write};
    use std::mem::{MaybeUninit, size_of};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::net::UnixStream;

    use aibe_broker_policy_poc::RendererPrincipal;

    use crate::snapshot_frame::{MAX_DOM_SNAPSHOT_PAYLOAD_BYTES, validate_dom_snapshot_payload};

    const MAGIC: u32 = 0x4149_4246; // AIBF
    const VERSION: u16 = 1;
    const HEADER_BYTES: usize = 104;
    const CONTROL_BYTES: usize = 128;
    const READ_ONLY_FD_RIGHTS: u64 = libc::CAP_READ | libc::CAP_FSTAT;

    #[repr(align(8))]
    pub(super) struct AlignedControl(pub(super) [u8; CONTROL_BYTES]);

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct SnapshotDescriptorEnvelope {
        pub request_id: u64,
        pub renderer_instance_id: u64,
        pub renderer_pid: u32,
        pub renderer_uid: u32,
        pub renderer_gid: u32,
        pub origin_registry_id: u64,
        pub origin_group_id: u64,
        pub grant_registry_id: u64,
        pub grant_id: u64,
        pub document_id: u64,
        pub revision: u64,
        pub payload_bytes: u64,
    }

    impl SnapshotDescriptorEnvelope {
        pub(super) fn encode(self) -> io::Result<[u8; HEADER_BYTES]> {
            if self.request_id == 0
                || self.renderer_instance_id == 0
                || self.renderer_pid == 0
                || self.origin_registry_id == 0
                || self.origin_group_id == 0
                || self.grant_registry_id == 0
                || self.grant_id == 0
                || self.document_id == 0
                || self.revision == 0
                || self.payload_bytes == 0
                || self.payload_bytes > MAX_DOM_SNAPSHOT_PAYLOAD_BYTES as u64
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid Snapshot FD grant envelope",
                ));
            }
            let mut bytes = [0_u8; HEADER_BYTES];
            let mut offset = 0;
            put(&mut bytes, &mut offset, &MAGIC.to_le_bytes());
            put(&mut bytes, &mut offset, &VERSION.to_le_bytes());
            put(
                &mut bytes,
                &mut offset,
                &(HEADER_BYTES as u16).to_le_bytes(),
            );
            put(&mut bytes, &mut offset, &1_u16.to_le_bytes()); // exactly one FD
            put(&mut bytes, &mut offset, &0_u16.to_le_bytes());
            put(&mut bytes, &mut offset, &0_u32.to_le_bytes());
            put(&mut bytes, &mut offset, &self.request_id.to_le_bytes());
            put(
                &mut bytes,
                &mut offset,
                &self.renderer_instance_id.to_le_bytes(),
            );
            put(&mut bytes, &mut offset, &self.renderer_pid.to_le_bytes());
            put(&mut bytes, &mut offset, &self.renderer_uid.to_le_bytes());
            put(&mut bytes, &mut offset, &self.renderer_gid.to_le_bytes());
            put(&mut bytes, &mut offset, &0_u32.to_le_bytes());
            for value in [
                self.origin_registry_id,
                self.origin_group_id,
                self.grant_registry_id,
                self.grant_id,
                self.document_id,
                self.revision,
                self.payload_bytes,
            ] {
                put(&mut bytes, &mut offset, &value.to_le_bytes());
            }
            if offset != HEADER_BYTES {
                return Err(io::Error::other(
                    "Snapshot FD envelope encoder length mismatch",
                ));
            }
            Ok(bytes)
        }

        fn decode(bytes: &[u8; HEADER_BYTES]) -> io::Result<Self> {
            let mut offset = 0;
            let magic = take_u32(bytes, &mut offset)?;
            let version = take_u16(bytes, &mut offset)?;
            let header_bytes = take_u16(bytes, &mut offset)?;
            let descriptor_count = take_u16(bytes, &mut offset)?;
            let reserved = take_u16(bytes, &mut offset)?;
            let flags = take_u32(bytes, &mut offset)?;
            if magic != MAGIC
                || version != VERSION
                || header_bytes as usize != HEADER_BYTES
                || descriptor_count != 1
                || reserved != 0
                || flags != 0
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid Snapshot FD envelope header",
                ));
            }
            let request_id = take_u64(bytes, &mut offset)?;
            let renderer_instance_id = take_u64(bytes, &mut offset)?;
            let renderer_pid = take_u32(bytes, &mut offset)?;
            let renderer_uid = take_u32(bytes, &mut offset)?;
            let renderer_gid = take_u32(bytes, &mut offset)?;
            if take_u32(bytes, &mut offset)? != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Snapshot FD envelope reserved field is nonzero",
                ));
            }
            let envelope = Self {
                request_id,
                renderer_instance_id,
                renderer_pid,
                renderer_uid,
                renderer_gid,
                origin_registry_id: take_u64(bytes, &mut offset)?,
                origin_group_id: take_u64(bytes, &mut offset)?,
                grant_registry_id: take_u64(bytes, &mut offset)?,
                grant_id: take_u64(bytes, &mut offset)?,
                document_id: take_u64(bytes, &mut offset)?,
                revision: take_u64(bytes, &mut offset)?,
                payload_bytes: take_u64(bytes, &mut offset)?,
            };
            let _ = envelope.encode()?;
            Ok(envelope)
        }
    }

    fn put<const N: usize>(out: &mut [u8; HEADER_BYTES], offset: &mut usize, bytes: &[u8; N]) {
        out[*offset..*offset + N].copy_from_slice(bytes);
        *offset += N;
    }
    fn take<const N: usize>(input: &[u8; HEADER_BYTES], offset: &mut usize) -> io::Result<[u8; N]> {
        let end = offset
            .checked_add(N)
            .ok_or_else(|| io::Error::other("Snapshot FD header offset overflow"))?;
        let slice = input.get(*offset..end).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "truncated Snapshot FD envelope",
            )
        })?;
        let mut out = [0_u8; N];
        out.copy_from_slice(slice);
        *offset = end;
        Ok(out)
    }
    fn take_u16(input: &[u8; HEADER_BYTES], offset: &mut usize) -> io::Result<u16> {
        Ok(u16::from_le_bytes(take(input, offset)?))
    }
    fn take_u32(input: &[u8; HEADER_BYTES], offset: &mut usize) -> io::Result<u32> {
        Ok(u32::from_le_bytes(take(input, offset)?))
    }
    fn take_u64(input: &[u8; HEADER_BYTES], offset: &mut usize) -> io::Result<u64> {
        Ok(u64::from_le_bytes(take(input, offset)?))
    }

    fn aligned(value: usize) -> usize {
        let alignment = size_of::<usize>();
        (value + alignment - 1) & !(alignment - 1)
    }

    fn restrict_read_only(fd: RawFd) -> io::Result<()> {
        let mut rights = MaybeUninit::<libc::cap_rights_t>::uninit();
        if unsafe {
            libc::__cap_rights_init(
                libc::CAP_RIGHTS_VERSION,
                rights.as_mut_ptr(),
                READ_ONLY_FD_RIGHTS,
                0_u64,
            )
        }
        .is_null()
        {
            return Err(io::Error::last_os_error());
        }
        let rights = unsafe { rights.assume_init() };
        if unsafe { libc::cap_rights_limit(fd, &rights) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut actual = MaybeUninit::<libc::cap_rights_t>::uninit();
        if unsafe { libc::__cap_rights_get(libc::CAP_RIGHTS_VERSION, fd, actual.as_mut_ptr()) } != 0
        {
            return Err(io::Error::last_os_error());
        }
        let actual = unsafe { actual.assume_init() };
        if !unsafe { libc::cap_rights_contains(&rights, &actual) }
            || !unsafe { libc::cap_rights_contains(&actual, &rights) }
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Snapshot FD rights do not match the read-only policy",
            ));
        }
        Ok(())
    }

    /// Create an anonymous shared-memory descriptor containing an immutable
    /// Snapshot payload. The descriptor is rewound and restricted before send.
    pub fn create_snapshot_descriptor(payload: &[u8]) -> io::Result<OwnedFd> {
        if payload.is_empty() || payload.len() > MAX_DOM_SNAPSHOT_PAYLOAD_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Snapshot payload length is outside the permitted range",
            ));
        }
        let fd = unsafe {
            libc::shm_open(
                libc::SHM_ANON,
                libc::O_CREAT | libc::O_RDWR | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        if unsafe { libc::ftruncate(fd, payload.len() as libc::off_t) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut file = File::from(owned);
        file.write_all(payload)?;
        file.seek(SeekFrom::Start(0))?;
        if unsafe { libc::fchmod(file.as_raw_fd(), 0o400) } != 0 {
            return Err(io::Error::last_os_error());
        }
        restrict_read_only(file.as_raw_fd())?;
        Ok(file.into())
    }

    /// Send one fixed grant envelope and one descriptor. The first byte carries
    /// SCM_RIGHTS; a stream receiver must use recvmsg for that byte before
    /// reading the remainder of the fixed-size envelope.
    pub fn send_snapshot_descriptor(
        stream: &UnixStream,
        envelope: SnapshotDescriptorEnvelope,
        descriptor: RawFd,
    ) -> io::Result<()> {
        restrict_read_only(descriptor)?;
        let bytes = envelope.encode()?;
        let mut control = AlignedControl([0; CONTROL_BYTES]);
        let header_bytes = aligned(size_of::<libc::cmsghdr>());
        let cmsg_len = header_bytes + size_of::<RawFd>();
        let cmsg_space = header_bytes + aligned(size_of::<RawFd>());
        let header = libc::cmsghdr {
            cmsg_len: cmsg_len as libc::socklen_t,
            cmsg_level: libc::SOL_SOCKET,
            cmsg_type: libc::SCM_RIGHTS,
        };
        unsafe {
            std::ptr::write(control.0.as_mut_ptr().cast::<libc::cmsghdr>(), header);
            std::ptr::copy_nonoverlapping(
                (&descriptor as *const RawFd).cast::<u8>(),
                control.0.as_mut_ptr().add(header_bytes),
                size_of::<RawFd>(),
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
        let sent = loop {
            let result = unsafe { libc::sendmsg(stream.as_raw_fd(), &message, 0) };
            if result < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            break result;
        };
        if sent <= 0 {
            return Err(if sent == 0 {
                io::Error::new(io::ErrorKind::WriteZero, "SCM_RIGHTS send returned zero")
            } else {
                io::Error::last_os_error()
            });
        }
        if sent as usize > bytes.len() {
            return Err(io::Error::other("SCM_RIGHTS send exceeded envelope length"));
        }
        if sent as usize != bytes.len() {
            let mut remainder = &bytes[sent as usize..];
            while !remainder.is_empty() {
                match (&mut &*stream).write(remainder) {
                    Ok(0) => {
                        return Err(io::Error::new(
                            io::ErrorKind::WriteZero,
                            "Snapshot FD envelope write returned zero",
                        ));
                    }
                    Ok(count) => remainder = &remainder[count..],
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(())
    }

    fn close_control_fds(control: &AlignedControl, controllen: usize) {
        let base = control.0.as_ptr();
        let header_bytes = aligned(size_of::<libc::cmsghdr>());
        let mut offset = 0;
        while offset + size_of::<libc::cmsghdr>() <= controllen {
            let header =
                unsafe { std::ptr::read_unaligned(base.add(offset).cast::<libc::cmsghdr>()) };
            let length = header.cmsg_len as usize;
            if length < header_bytes || offset + length > controllen {
                break;
            }
            if header.cmsg_level == libc::SOL_SOCKET && header.cmsg_type == libc::SCM_RIGHTS {
                let data_len = length - header_bytes;
                for index in 0..(data_len / size_of::<RawFd>()) {
                    let fd = unsafe {
                        std::ptr::read_unaligned(
                            base.add(offset + header_bytes + index * size_of::<RawFd>())
                                .cast::<RawFd>(),
                        )
                    };
                    if fd >= 0 {
                        unsafe {
                            libc::close(fd);
                        }
                    }
                }
            }
            offset += aligned(length);
        }
    }

    /// Receive exactly one Broker-granted descriptor and verify the complete
    /// envelope against the current process identity before exposing payload.
    pub fn receive_snapshot_descriptor(
        stream: &mut UnixStream,
        expected: RendererPrincipal,
    ) -> io::Result<(SnapshotDescriptorEnvelope, Vec<u8>)> {
        let mut control = AlignedControl([0; CONTROL_BYTES]);
        let mut first_byte = 0_u8;
        let mut iov = libc::iovec {
            iov_base: (&mut first_byte as *mut u8).cast(),
            iov_len: 1,
        };
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.0.as_mut_ptr().cast();
        message.msg_controllen = control.0.len() as libc::socklen_t;
        let received = loop {
            let result =
                unsafe { libc::recvmsg(stream.as_raw_fd(), &mut message, libc::MSG_CMSG_CLOEXEC) };
            if result < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            break result;
        };
        if received != 1 || message.msg_flags & (libc::MSG_CTRUNC | libc::MSG_TRUNC) != 0 {
            close_control_fds(&control, message.msg_controllen as usize);
            return Err(if received < 0 {
                io::Error::last_os_error()
            } else {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "SCM_RIGHTS message was truncated or empty",
                )
            });
        }
        let controllen = message.msg_controllen as usize;
        let base = control.0.as_ptr();
        let header_bytes = aligned(size_of::<libc::cmsghdr>());
        let mut offset = 0;
        let mut received_fds = Vec::new();
        let mut ancillary_count = 0;
        while offset + size_of::<libc::cmsghdr>() <= controllen {
            let header =
                unsafe { std::ptr::read_unaligned(base.add(offset).cast::<libc::cmsghdr>()) };
            let length = header.cmsg_len as usize;
            if length < header_bytes || offset + length > controllen {
                close_control_fds(&control, controllen);
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "malformed SCM_RIGHTS control header",
                ));
            }
            ancillary_count += 1;
            if header.cmsg_level == libc::SOL_SOCKET && header.cmsg_type == libc::SCM_RIGHTS {
                let data_len = length - header_bytes;
                if !data_len.is_multiple_of(size_of::<RawFd>()) {
                    close_control_fds(&control, controllen);
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "SCM_RIGHTS descriptor payload is misaligned",
                    ));
                }
                for index in 0..(data_len / size_of::<RawFd>()) {
                    let fd = unsafe {
                        std::ptr::read_unaligned(
                            base.add(offset + header_bytes + index * size_of::<RawFd>())
                                .cast::<RawFd>(),
                        )
                    };
                    received_fds.push(fd);
                }
            } else {
                close_control_fds(&control, controllen);
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unexpected ancillary message on Renderer Broker channel",
                ));
            }
            offset += aligned(length);
        }
        if ancillary_count != 1 || received_fds.len() != 1 || received_fds[0] < 0 {
            close_control_fds(&control, controllen);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "SCM_RIGHTS must contain exactly one descriptor",
            ));
        }
        let descriptor = unsafe { OwnedFd::from_raw_fd(received_fds[0]) };
        let mut bytes = [0_u8; HEADER_BYTES];
        bytes[0] = first_byte;
        if let Err(error) = stream.read_exact(&mut bytes[1..]) {
            drop(descriptor);
            return Err(error);
        }
        let envelope = match SnapshotDescriptorEnvelope::decode(&bytes) {
            Ok(value) => value,
            Err(error) => {
                drop(descriptor);
                return Err(error);
            }
        };
        if envelope.renderer_instance_id != expected.id().value()
            || envelope.renderer_pid != expected.pid()
            || envelope.renderer_uid != expected.uid()
            || envelope.renderer_gid != expected.gid()
        {
            drop(descriptor);
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Snapshot FD grant targets another Renderer principal",
            ));
        }
        restrict_read_only(descriptor.as_raw_fd())?;
        let mut stat = MaybeUninit::<libc::stat>::zeroed();
        if unsafe { libc::fstat(descriptor.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let stat = unsafe { stat.assume_init() };
        if stat.st_size < 0
            || stat.st_size as u64 != envelope.payload_bytes
            || stat.st_mode & libc::S_IFMT != libc::S_IFREG
            || stat.st_mode & 0o222 != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Snapshot FD type, mode, or size does not match the read-only grant",
            ));
        }
        let write_probe = [0_u8; 1];
        let write_result = unsafe {
            libc::write(
                descriptor.as_raw_fd(),
                write_probe.as_ptr().cast(),
                write_probe.len(),
            )
        };
        if write_result >= 0 || io::Error::last_os_error().raw_os_error() != Some(libc::ENOTCAPABLE)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Snapshot descriptor unexpectedly permits writes",
            ));
        }
        let mut file = File::from(descriptor);
        let mut payload = Vec::with_capacity(envelope.payload_bytes as usize);
        file.read_to_end(&mut payload)?;
        if payload.len() != envelope.payload_bytes as usize {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Snapshot FD payload length mismatch",
            ));
        }
        let (document_id, revision) = validate_dom_snapshot_payload(&payload)?;
        if document_id != envelope.document_id || revision != envelope.revision {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Snapshot payload identity does not match the Broker grant envelope",
            ));
        }
        Ok((envelope, payload))
    }
}

#[cfg(target_os = "freebsd")]
pub use freebsd::{
    SnapshotDescriptorEnvelope, create_snapshot_descriptor, receive_snapshot_descriptor,
    send_snapshot_descriptor,
};

#[cfg(all(test, target_os = "freebsd"))]
mod tests {
    use super::freebsd::{
        SnapshotDescriptorEnvelope, create_snapshot_descriptor, receive_snapshot_descriptor,
        send_snapshot_descriptor,
    };
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;

    use aibe_broker_policy_poc::{RendererId, RendererPrincipal};

    const PAYLOAD: &[u8] = br##"{"envelopeVersion":1,"documentId":"7","revision":"3","tree":{"schemaVersion":1,"nodeCount":1,"root":1,"nodes":[{"id":1,"parent":null,"children":[],"type":"document","nodeType":9,"name":"#document","value":null,"attributes":[]}]}}"##;

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

    #[test]
    fn anonymous_snapshot_fd_round_trips_with_exact_renderer_identity_and_read_only_rights() {
        let expected = principal();
        let descriptor = create_snapshot_descriptor(PAYLOAD).unwrap();
        let (sender, mut receiver) = UnixStream::pair().unwrap();
        send_snapshot_descriptor(&sender, envelope(expected), descriptor.as_raw_fd()).unwrap();
        let (received, payload) = receive_snapshot_descriptor(&mut receiver, expected).unwrap();
        assert_eq!(received, envelope(expected));
        assert_eq!(payload, PAYLOAD);
    }
}
