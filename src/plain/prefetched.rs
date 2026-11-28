//! Replay bytes read ahead before handing a connection to the streaming driver.
use bytes::{Buf, Bytes};
use http_kit_io::{BufResult, IoBufMut, Read, ReadOwned, Received, SpliceRead};
use std::io;

pub(super) struct Reader<R> {
    pending: Bytes,
    reader: R,
}

impl<R> Reader<R> {
    pub(super) fn new(pending: Bytes, reader: R) -> Self {
        Self { pending, reader }
    }
}

impl<R: Read> Read for Reader<R> {
    async fn read<B: IoBufMut>(&mut self, mut buf: B) -> BufResult<usize, B> {
        if self.pending.is_empty() {
            return self.reader.read(buf).await;
        }
        let n = self.pending.len().min(buf.buf_capacity());
        buf.ensure_init()[..n].copy_from_slice(&self.pending[..n]);
        // SAFETY: all n bytes were initialized above and fit in the buffer.
        unsafe {
            buf.set_len(n);
        }
        self.pending.advance(n);
        BufResult(Ok(n), buf)
    }
}

impl<R: Read> ReadOwned for Reader<R> {
    type Buf = Bytes;
    async fn read_owned(&mut self) -> io::Result<Received<Bytes>> {
        Ok(if self.pending.is_empty() {
            Received::ReadInstead
        } else {
            Received::Lent(std::mem::take(&mut self.pending))
        })
    }
}

impl<R: SpliceRead> SpliceRead for Reader<R> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn can_splice(&self) -> bool {
        self.pending.is_empty() && self.reader.can_splice()
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    async fn splice_to_pipe(
        &mut self,
        pipe: http_kit_io::splice::Pipe,
        max: usize,
    ) -> Result<http_kit_io::splice::Spliced, (io::Error, http_kit_io::splice::Pipe)> {
        if !self.pending.is_empty() {
            return Err((
                io::Error::new(io::ErrorKind::Unsupported, "prefetched bytes remain"),
                pipe,
            ));
        }
        self.reader.splice_to_pipe(pipe, max).await
    }
}
