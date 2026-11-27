//! File transfer after the HTTP/1 writer has flushed the response headers.

use std::{borrow::Borrow, fs::File, io, os::unix::fs::FileExt, sync::Arc};
use tokio::{io::Interest, net::TcpStream, task::yield_now};

pub async fn transfer(
    socket: Arc<tokio::net::tcp::OwnedWriteHalf>,
    file: impl Borrow<File> + Send + 'static,
    len: u64,
    chunk: usize,
) -> io::Result<()> {
    if len == 0 {
        return Ok(());
    }
    if chunk == 0 || len > i64::MAX as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid sendfile length or chunk size",
        ));
    }

    let socket = socket.as_ref().as_ref();
    let mut offset = 0;
    while offset < len {
        socket.writable().await?;
        let count = (len - offset).min(chunk as u64) as usize;
        // A nonblocking socket still allows a cold source page to block this
        // call. Bound each transfer and yield between successful chunks.
        match socket.try_io(Interest::WRITABLE, || {
            send(socket, file.borrow(), offset, count)
        }) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(written) => {
                offset += written as u64;
                if offset < len {
                    yield_now().await;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => (),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => yield_now().await,
            Err(error)
                if matches!(
                    error.raw_os_error(),
                    Some(libc::EINVAL | libc::ENOSYS | libc::EOPNOTSUPP | libc::EXDEV)
                ) =>
            {
                return buffered_transfer(socket, file, offset, len - offset, chunk).await;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn send(socket: &TcpStream, file: &File, offset: u64, count: usize) -> io::Result<usize> {
    let mut offset = offset;
    rustix::fs::sendfile(socket, file, Some(&mut offset), count).map_err(io::Error::from)
}

async fn buffered_transfer(
    socket: &TcpStream,
    file: impl Borrow<File> + Send + 'static,
    mut offset: u64,
    mut remaining: u64,
    chunk: usize,
) -> io::Result<()> {
    // A cached descriptor can serve several clients. Positional reads keep each
    // transfer at its own offset even after a partial sendfile or a rename.
    let mut file = file;
    let mut buffer = vec![0; remaining.min(chunk.min(64 * 1024) as u64) as usize];

    while remaining != 0 {
        let count = remaining.min(buffer.len() as u64) as usize;
        let result;
        (file, buffer, result) = tokio::task::spawn_blocking(move || {
            let read = file.borrow().read_at(&mut buffer[..count], offset);
            (file, buffer, read)
        })
        .await?;
        let read = match result {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        let mut data = &buffer[..read];
        while !data.is_empty() {
            socket.writable().await?;
            match socket.try_write(data) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(written) => data = &data[written..],
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => (),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => yield_now().await,
                Err(error) => return Err(error),
            }
        }
        offset += read as u64;
        remaining -= read as u64;
    }
    Ok(())
}
