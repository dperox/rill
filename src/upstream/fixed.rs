//! HTTP/1 keep-alive senders for a route with one numeric plaintext peer.
use super::{CONNECT_TIMEOUT, RequestError};
use axum::{body::Body, extract::Request};
use hyper::client::conn::http1::{SendRequest, handshake};
use hyper_util::rt::TokioIo;
use std::{
    collections::VecDeque,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::TcpStream, sync::Semaphore, task::AbortHandle, time::Instant};

pub(super) const CAPACITY: usize = 48;
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Default)]
struct Idle {
    senders: VecDeque<(Instant, SendRequest<Body>)>,
    cleaner: Option<AbortHandle>,
}

impl Drop for Idle {
    fn drop(&mut self) {
        if let Some(task) = self.cleaner.take() {
            task.abort();
        }
    }
}

pub(crate) struct Fixed {
    address: SocketAddr,
    idle: Arc<Mutex<Idle>>,
    waiters: Arc<Semaphore>,
}

impl Fixed {
    pub(super) fn new(address: SocketAddr) -> Self {
        Self {
            address,
            idle: Default::default(),
            waiters: Arc::new(Semaphore::new(CAPACITY)),
        }
    }

    fn checkout(&self) -> Option<SendRequest<Body>> {
        let mut idle = self.idle.lock().unwrap();
        while let Some((at, tx)) = idle.senders.pop_back() {
            if at.elapsed() >= IDLE_TIMEOUT {
                idle.senders.clear();
                break;
            }
            if tx.is_ready() {
                return Some(tx);
            }
        }
        None
    }

    fn put(pool: &Arc<Mutex<Idle>>, tx: SendRequest<Body>) {
        let mut idle = pool.lock().unwrap();
        if idle.senders.len() >= CAPACITY {
            return;
        }
        idle.senders.push_back((Instant::now(), tx));
        if idle.cleaner.is_some() {
            return;
        }
        let weak = Arc::downgrade(pool);
        idle.cleaner = Some(
            tokio::spawn(async move {
                let mut wake = Instant::now() + IDLE_TIMEOUT;
                loop {
                    tokio::time::sleep_until(wake).await;
                    let Some(pool) = weak.upgrade() else { return };
                    let mut idle = pool.lock().unwrap();
                    while idle
                        .senders
                        .front()
                        .is_some_and(|(at, _)| at.elapsed() >= IDLE_TIMEOUT)
                    {
                        idle.senders.pop_front();
                    }
                    match idle.senders.front() {
                        Some((at, _)) => wake = *at + IDLE_TIMEOUT,
                        None => {
                            idle.cleaner = None;
                            return;
                        }
                    }
                }
            })
            .abort_handle(),
        );
    }

    fn recycle(&self, mut tx: SendRequest<Body>) {
        if tx.is_ready() {
            Self::put(&self.idle, tx);
        } else if !tx.is_closed()
            && let Ok(permit) = self.waiters.clone().try_acquire_owned()
        {
            // A streaming response may hold the connection. Never wait on the request path
            // or retain a retired configuration for the lifetime of a slow consumer.
            let weak = Arc::downgrade(&self.idle);
            tokio::spawn(async move {
                let _permit = permit;
                if matches!(
                    tokio::time::timeout(IDLE_TIMEOUT, tx.ready()).await,
                    Ok(Ok(()))
                ) && let Some(pool) = weak.upgrade()
                {
                    Self::put(&pool, tx);
                }
            });
        }
    }

    pub(crate) async fn request(
        &self,
        mut req: Request,
    ) -> Result<hyper::Response<hyper::body::Incoming>, RequestError> {
        let mut cached = self.checkout();
        loop {
            let reused = cached.is_some();
            let mut tx = match cached.take() {
                Some(tx) => tx,
                None => {
                    let stream =
                        tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(self.address))
                            .await
                            .map_err(|_| RequestError::Connect)?
                            .map_err(|_| RequestError::Connect)?;
                    stream
                        .set_nodelay(true)
                        .map_err(|_| RequestError::Connect)?;
                    let (tx, driver) = handshake(TokioIo::new(stream))
                        .await
                        .map_err(|_| RequestError::Connect)?;
                    tokio::spawn(driver);
                    tx
                }
            };
            match tx.try_send_request(req).await {
                Ok(response) => {
                    self.recycle(tx);
                    return Ok(response);
                }
                Err(mut error) if reused => {
                    // Hyper only returns the request if it was never sent. Retry once on a fresh socket.
                    req = error.take_message().ok_or(RequestError::Exchange)?;
                }
                Err(_) => return Err(RequestError::Exchange),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn checkout_discards_an_idle_socket_closed_by_the_peer() {
        let fixed = Fixed::new("127.0.0.1:80".parse().unwrap());
        let (client, peer) = tokio::io::duplex(128);
        let (mut tx, driver) = handshake(TokioIo::new(client)).await.unwrap();
        let done = tokio::spawn(driver);
        tx.ready().await.unwrap();
        fixed.recycle(tx);
        drop(peer);
        let _ = done.await.unwrap();
        assert!(fixed.checkout().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn idle_budget_and_expiry_close_the_underlying_connections() {
        let fixed = Fixed::new("127.0.0.1:80".parse().unwrap());
        let mut peers = Vec::new();
        for _ in 0..=CAPACITY {
            let (client, peer) = tokio::io::duplex(128);
            let (mut tx, driver) = handshake(TokioIo::new(client)).await.unwrap();
            tokio::spawn(driver);
            tx.ready().await.unwrap();
            fixed.recycle(tx);
            peers.push(peer);
        }
        assert_eq!(fixed.idle.lock().unwrap().senders.len(), CAPACITY);
        let mut byte = [0; 1];
        assert_eq!(peers.last_mut().unwrap().read(&mut byte).await.unwrap(), 0);
        tokio::time::advance(IDLE_TIMEOUT).await;
        tokio::task::yield_now().await;
        assert!(fixed.idle.lock().unwrap().senders.is_empty());
        for mut peer in peers {
            assert_eq!(peer.read(&mut byte).await.unwrap(), 0);
        }
    }
}
