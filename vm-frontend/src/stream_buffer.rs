use std::io::{ErrorKind, Read, Write};

pub(crate) const DEFAULT_STREAM_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PendingBufferLimit {
    pub limit: usize,
    pub attempted: usize,
}

pub(crate) fn check_pending_buffer_limit(
    current: usize,
    incoming: usize,
    limit: usize,
) -> Result<usize, PendingBufferLimit> {
    let attempted = current.saturating_add(incoming);
    if attempted > limit {
        Err(PendingBufferLimit { limit, attempted })
    } else {
        Ok(attempted)
    }
}

pub(crate) fn extend_pending_buffer(
    pending: &mut Vec<u8>,
    bytes: &[u8],
    limit: usize,
) -> Result<(), PendingBufferLimit> {
    check_pending_buffer_limit(pending.len(), bytes.len(), limit)?;
    pending.extend_from_slice(bytes);
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NonblockingRead {
    Payload(Vec<u8>),
    Closed,
    WouldBlock,
}

pub(crate) fn read_nonblocking_chunk(
    connection: &mut impl Read,
    max_bytes: usize,
) -> Result<NonblockingRead, String> {
    if max_bytes == 0 {
        return Ok(NonblockingRead::WouldBlock);
    }
    let mut buffer = vec![0; max_bytes.min(DEFAULT_STREAM_CHUNK_BYTES)];
    match connection.read(&mut buffer) {
        Ok(0) => Ok(NonblockingRead::Closed),
        Ok(count) => {
            buffer.truncate(count);
            Ok(NonblockingRead::Payload(buffer))
        }
        Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(NonblockingRead::WouldBlock),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn write_pending_best_effort(
    connection: &mut impl Write,
    pending: &mut Vec<u8>,
    max_bytes: usize,
) -> Result<usize, String> {
    let mut written = 0;
    while !pending.is_empty() && written < max_bytes {
        let writable = (max_bytes - written).min(pending.len());
        match connection.write(&pending[..writable]) {
            Ok(0) => break,
            Ok(count) => {
                written += count;
                pending.drain(..count);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(written)
}

pub(crate) fn append_and_write_pending_best_effort(
    connection: &mut impl Write,
    pending: &mut Vec<u8>,
    bytes: &[u8],
) -> Result<usize, String> {
    pending.extend_from_slice(bytes);
    write_pending_best_effort(connection, pending, usize::MAX)
}
