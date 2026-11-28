//! Preserve platform-specific transfer limits and filesystem fallbacks.
use http_kit_io::{BufResult, IoBuf, IoVectoredBuf, SendFile, SpliceWrite, Write};
use std::{borrow::Borrow, fs::File, io};

pub(super) struct FileWriter<W> {
    pub inner: W,
    #[cfg(windows)]
    pub socket: std::sync::Arc<socket2::Socket>,
}

impl<W: Write> Write for FileWriter<W> {
    async fn write<B: IoBuf>(&mut self, buf: B) -> BufResult<usize, B> {
        self.inner.write(buf).await
    }
    async fn write_vectored<B: IoVectoredBuf>(&mut self, buf: B) -> BufResult<usize, B> {
        self.inner.write_vectored(buf).await
    }
    async fn flush(&mut self) -> io::Result<()> {
        self.inner.flush().await
    }
    async fn shutdown(&mut self) -> io::Result<()> {
        self.inner.shutdown().await
    }
}

impl<W: SpliceWrite> SpliceWrite for FileWriter<W> {
    #[cfg(target_os = "linux")]
    fn can_splice(&self) -> bool {
        self.inner.can_splice()
    }
    #[cfg(target_os = "linux")]
    async fn splice_from_pipe(
        &mut self,
        filled: http_kit_io::splice::FilledPipe,
    ) -> io::Result<http_kit_io::splice::Pipe> {
        self.inner.splice_from_pipe(filled).await
    }
}

impl<W: Write + SendFile> SendFile for FileWriter<W> {
    async fn send_file<F: Borrow<File> + 'static>(
        &mut self,
        file: F,
        offset: u64,
        len: usize,
    ) -> BufResult<usize, F> {
        #[cfg(windows)]
        return crate::transmitfile::send(&mut self.inner, &self.socket, file, offset, len).await;
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::FileExt;
            let BufResult(result, file) =
                self.inner.send_file(file, offset, len.min(1 << 20)).await;
            if !matches!(
                result.as_ref().err().and_then(io::Error::raw_os_error),
                Some(libc::EINVAL | libc::ENOSYS | libc::EOPNOTSUPP | libc::EXDEV)
            ) {
                if result.is_ok() && len > 1 << 20 {
                    tokio::task::yield_now().await;
                }
                return BufResult(result, file);
            }
            let result = async {
                let handle = file.borrow().try_clone()?;
                let bytes = tokio::task::spawn_blocking(move || {
                    let mut buf = vec![0; len.min(64 * 1024)];
                    let read = handle.read_at(&mut buf, offset)?;
                    buf.truncate(read);
                    Ok::<_, io::Error>(buf)
                })
                .await
                .map_err(io::Error::other)??;
                let n = bytes.len();
                self.inner.write_all(bytes).await.0?;
                Ok(n)
            }
            .await;
            BufResult(result, file)
        }
    }
}
