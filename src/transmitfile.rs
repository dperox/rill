//! Bounded overlapped file sends for plaintext HTTP/1 on Windows.
use std::{
    fs::File,
    io,
    os::windows::io::{AsRawHandle, AsRawSocket, FromRawHandle, OwnedHandle},
    ptr,
    sync::{Arc, Mutex, OnceLock, mpsc},
    thread,
};
use tokio::{
    net::tcp::OwnedWriteHalf,
    sync::{Semaphore, SemaphorePermit, oneshot},
};
use windows_sys::Win32::{
    Foundation::{ERROR_IO_PENDING, WAIT_TIMEOUT},
    Networking::WinSock::{TransmitFile, WSAGetLastError, WSAGetOverlappedResult},
    System::{
        IO::{CancelIoEx, OVERLAPPED},
        Threading::{CreateEventW, WaitForSingleObject},
    },
};

const LIMIT: usize = 2;
static SLOTS: Semaphore = Semaphore::const_new(LIMIT);
static WORKERS: OnceLock<Option<mpsc::Sender<Job>>> = OnceLock::new();

pub struct Slot {
    permit: SemaphorePermit<'static>,
    queue: &'static mpsc::Sender<Job>,
}
struct Job {
    _permit: SemaphorePermit<'static>,
    socket: Arc<OwnedWriteHalf>,
    file: File,
    len: u64,
    done: oneshot::Sender<io::Result<u64>>,
}

pub fn reserve() -> Option<Slot> {
    let permit = SLOTS.try_acquire().ok()?;
    let queue = WORKERS.get_or_init(workers).as_ref()?;
    Some(Slot { permit, queue })
}

fn workers() -> Option<mpsc::Sender<Job>> {
    let (tx, rx) = mpsc::channel::<Job>();
    let rx = Arc::new(Mutex::new(rx));
    for _ in 0..LIMIT {
        let rx = rx.clone();
        thread::Builder::new()
            .name("rill-transmitfile".into())
            .spawn(move || {
                loop {
                    let next = rx.lock().unwrap().recv();
                    let Ok(job) = next else { break };
                    let result = transfer(&job);
                    let _ = job.done.send(result);
                }
            })
            .ok()?;
    }
    Some(tx)
}

impl Slot {
    pub fn body(
        self,
        socket: Arc<OwnedWriteHalf>,
        file: File,
        len: u64,
    ) -> hyper::ext::Http1DirectBody {
        hyper::ext::Http1DirectBody::new(len, async move {
            let (done, completion) = oneshot::channel();
            self.queue
                .send(Job {
                    _permit: self.permit,
                    socket,
                    file,
                    len,
                    done,
                })
                .map_err(|_| io::Error::other("TransmitFile workers stopped"))?;
            completion.await.map_err(io::Error::other)?
        })
    }
}

fn transfer(job: &Job) -> io::Result<u64> {
    let socket = job.socket.as_ref().as_ref().as_raw_socket() as usize;
    let mut offset = 0;
    while offset < job.len && !job.done.is_closed() {
        // Leave room below Winsock's signed 2 GiB per-call ceiling.
        let chunk = (job.len - offset).min(1 << 30) as u32;
        // SAFETY: no name or custom security attributes. Own the returned event until
        // the operation has completed, including on cancellation.
        let handle = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let event = unsafe { OwnedHandle::from_raw_handle(handle) };
        let mut op = OVERLAPPED::default();
        op.Anonymous.Anonymous.Offset = offset as u32;
        op.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
        // The low bit suppresses completion-port delivery for this operation.
        // Tokio owns the socket's IOCP and must never receive our OVERLAPPED.
        op.hEvent = event.as_raw_handle().map_addr(|a| a | 1);
        // SAFETY: this worker owns op and the event. Job keeps both socket and
        // file alive. Every pending operation is drained before leaving this scope.
        unsafe {
            if TransmitFile(
                socket,
                job.file.as_raw_handle(),
                chunk,
                0,
                &mut op,
                ptr::null(),
                0,
            ) == 0
            {
                let error = WSAGetLastError();
                if error != ERROR_IO_PENDING as i32 {
                    return Err(io::Error::from_raw_os_error(error));
                }
                while WaitForSingleObject(event.as_raw_handle(), 100) == WAIT_TIMEOUT {
                    if job.done.is_closed() {
                        CancelIoEx(socket as _, &op);
                        break;
                    }
                }
            }
            let mut sent = 0;
            let mut flags = 0;
            if WSAGetOverlappedResult(socket, &op, &mut sent, 1, &mut flags) == 0 {
                return Err(io::Error::from_raw_os_error(WSAGetLastError()));
            }
            if sent != chunk {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "short TransmitFile transfer",
                ));
            }
        }
        offset += u64::from(chunk);
    }
    Ok(offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserving_and_releasing_direct_slots() {
        let first = reserve().expect("first transfer");
        let second = reserve().expect("second transfer");
        assert!(reserve().is_none());
        drop(first);
        assert!(reserve().is_some());
        drop(second);
    }
}
