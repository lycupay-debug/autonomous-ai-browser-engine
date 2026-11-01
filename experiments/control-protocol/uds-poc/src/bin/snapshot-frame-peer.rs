use std::env;
use std::io::{self, Read};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use aibe_control_protocol_uds_poc::snapshot_frame::read_dom_snapshot_frame;

fn main() -> io::Result<()> {
    let mut arguments = env::args_os().skip(1);
    let mode = arguments
        .next()
        .ok_or_else(|| io::Error::other("missing mode"))?;
    let socket_path = arguments
        .next()
        .ok_or_else(|| io::Error::other("missing socket path"))?;
    if arguments.next().is_some() || mode != "receive" {
        return Err(io::Error::other(
            "usage: snapshot-frame-peer receive <socket-path>",
        ));
    }

    let mut stream = UnixStream::connect(socket_path)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut first_byte = [0_u8; 1];
    match stream.read(&mut first_byte)? {
        0 => return Ok(()), // Broker deliberately sent no frame after denying authorization.
        1 => {}
        _ => unreachable!("one-byte read cannot return more than one byte"),
    }
    // Preserve the first byte so read_dom_snapshot_frame can distinguish a
    // clean EOF (handled above) from a truncated header or payload (an error).
    let mut prefixed = std::io::Cursor::new(first_byte).chain(&mut stream);
    let frame = read_dom_snapshot_frame(&mut prefixed)?;
    if frame.request_id != 99 || frame.document_id != 7 || frame.revision != 3 {
        return Err(io::Error::other(
            "received snapshot identity did not match the test contract",
        ));
    }
    Ok(())
}
