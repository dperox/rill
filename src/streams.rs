use std::{
    collections::HashMap,
    env, io,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream, UdpSocket},
    sync::mpsc,
    task::{JoinHandle, JoinSet},
    time::{Instant, sleep_until, timeout},
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const TCP_CONNECTION_LIMIT: usize = 4096;
const UDP_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const UDP_UNANSWERED_TIMEOUT: Duration = Duration::from_secs(5);
const UDP_SESSION_LIMIT: usize = 1024;
const UDP_PER_IP_LIMIT: usize = 32;
const UDP_QUEUE_LIMIT: usize = 16;
const UDP_BUFFER_SIZE: usize = 65_536;

#[derive(Clone, Copy)]
enum Balance {
    RoundRobin,
    LeastConnections,
}

struct Pool {
    addresses: Vec<SocketAddr>,
    balance: Balance,
    state: Mutex<PoolState>,
}

struct PoolState {
    cursor: usize,
    active: Vec<usize>,
}

struct Lease {
    pool: Arc<Pool>,
    index: usize,
}

impl Pool {
    fn new(addresses: Vec<SocketAddr>, balance: Balance) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(PoolState {
                cursor: 0,
                active: vec![0; addresses.len()],
            }),
            addresses,
            balance,
        })
    }

    fn acquire(self: &Arc<Self>) -> Lease {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let count = self.addresses.len();
        let index = match self.balance {
            Balance::RoundRobin => state.cursor,
            Balance::LeastConnections => (0..count)
                .map(|offset| (state.cursor + offset) % count)
                .min_by_key(|&index| state.active[index])
                .unwrap(),
        };
        state.cursor = (index + 1) % count;
        state.active[index] += 1;
        Lease {
            pool: Arc::clone(self),
            index,
        }
    }
}

impl Lease {
    fn address(&self) -> SocketAddr {
        self.pool.addresses[self.index]
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.pool
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active[self.index] -= 1;
    }
}

enum Transport {
    Tcp(TcpListener),
    Udp(UdpSocket),
}

pub(crate) struct BoundListener {
    transport: Transport,
    pool: Arc<Pool>,
}

impl BoundListener {
    pub(crate) fn spawn(self) -> JoinHandle<io::Result<()>> {
        tokio::spawn(async move {
            match self.transport {
                Transport::Tcp(listener) => serve_tcp(listener, self.pool).await,
                Transport::Udp(socket) => serve_udp(socket, self.pool).await,
            }
        })
    }
}

pub(crate) struct ListenerConfig {
    pub(crate) tcp: bool,
    pub(crate) address: SocketAddr,
    pool: Arc<Pool>,
}

pub(crate) fn configured() -> io::Result<Vec<ListenerConfig>> {
    let balance = match env_setting("RILL_STREAM_BALANCE")?.as_deref() {
        None | Some("round_robin") => Balance::RoundRobin,
        Some("least_connections") => Balance::LeastConnections,
        Some(_) => {
            return Err(invalid(
                "RILL_STREAM_BALANCE must be round_robin or least_connections",
            ));
        }
    };
    let mut listeners = Vec::new();
    for (name, tcp) in [("RILL_TCP_LISTENERS", true), ("RILL_UDP_LISTENERS", false)] {
        let setting = env_setting(name)?.unwrap_or_default();
        for entry in setting.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            let (address, upstreams) = entry
                .split_once('=')
                .ok_or_else(|| invalid(format!("{name}: expected BIND=UPSTREAM in {entry}")))?;
            let address = parse_address(name, address)?;
            let upstreams = upstreams
                .split(',')
                .map(|upstream| parse_address(name, upstream))
                .collect::<io::Result<Vec<_>>>()?;
            listeners.push(ListenerConfig {
                tcp,
                address,
                pool: Pool::new(upstreams, balance),
            });
        }
    }
    Ok(listeners)
}

pub(crate) async fn bind(config: Vec<ListenerConfig>) -> io::Result<Vec<BoundListener>> {
    let mut listeners = Vec::new();
    for ListenerConfig { tcp, address, pool } in config {
        let transport = if tcp {
            TcpListener::bind(address).await.map(Transport::Tcp)
        } else {
            UdpSocket::bind(address).await.map(Transport::Udp)
        }
        .map_err(|e| io::Error::new(e.kind(), format!("bind {address}: {e}")))?;
        listeners.push(BoundListener { transport, pool });
    }
    Ok(listeners)
}

fn env_setting(name: &str) -> io::Result<Option<String>> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(err) => Err(invalid(format!("{name}: {err}"))),
    }
}

fn parse_address(name: &str, address: &str) -> io::Result<SocketAddr> {
    address
        .trim()
        .parse()
        .map_err(|_| invalid(format!("{name}: invalid socket address {address:?}")))
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

async fn serve_tcp(listener: TcpListener, pool: Arc<Pool>) -> io::Result<()> {
    let mut connections = JoinSet::new();
    loop {
        while connections.try_join_next().is_some() {}
        tokio::select! {
            result = listener.accept() => {
                let (client, peer) = match result {
                    Ok(accepted) => accepted,
                    Err(error) if crate::plain::is_transient_accept_error(&error) => continue,
                    Err(error) => {
                        eprintln!("TCP stream accept error: {error}");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                if connections.len() >= TCP_CONNECTION_LIMIT {
                    drop(client);
                    continue;
                }
                let lease = pool.acquire();
                connections.spawn(async move {
                    let upstream = lease.address();
                    if let Err(err) = forward_tcp(client, upstream).await {
                        eprintln!("TCP proxy {peer} -> {upstream}: {err}");
                    }
                    drop(lease);
                });
            }
            _ = connections.join_next(), if !connections.is_empty() => {}
        }
    }
}

async fn forward_tcp(mut client: TcpStream, upstream: SocketAddr) -> io::Result<()> {
    let mut server = timeout(CONNECT_TIMEOUT, TcpStream::connect(upstream)).await??;
    tokio::io::copy_bidirectional(&mut client, &mut server).await?;
    Ok(())
}

fn reap_udp_sessions(
    sessions: &mut JoinSet<SocketAddr>,
    clients: &mut HashMap<SocketAddr, mpsc::Sender<Vec<u8>>>,
    per_ip: &mut HashMap<IpAddr, usize>,
) {
    while let Some(result) = sessions.try_join_next() {
        if let Ok(peer) = result
            && clients.get(&peer).is_some_and(mpsc::Sender::is_closed)
        {
            clients.remove(&peer);
            let ip = peer.ip().to_canonical();
            if let Some(count) = per_ip.get_mut(&ip) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    per_ip.remove(&ip);
                }
            }
        }
    }
    clients.retain(|peer, sender| {
        if sender.is_closed() {
            let ip = peer.ip().to_canonical();
            if let Some(count) = per_ip.get_mut(&ip) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    per_ip.remove(&ip);
                }
            }
            false
        } else {
            true
        }
    });
}

async fn serve_udp(socket: UdpSocket, pool: Arc<Pool>) -> io::Result<()> {
    let socket = Arc::new(socket);
    let mut buffer = vec![0; UDP_BUFFER_SIZE];
    let mut sessions = JoinSet::new();
    let mut clients: HashMap<SocketAddr, mpsc::Sender<Vec<u8>>> = HashMap::new();
    let mut per_ip: HashMap<IpAddr, usize> = HashMap::new();
    loop {
        reap_udp_sessions(&mut sessions, &mut clients, &mut per_ip);
        tokio::select! {
            result = socket.recv_from(&mut buffer) => {
                let (size, peer) = match result {
                    Ok(received) => received,
                    Err(error) if crate::plain::is_transient_accept_error(&error) => continue,
                    Err(error) => {
                        eprintln!("UDP stream recv error: {error}");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                if clients.get(&peer).is_none_or(mpsc::Sender::is_closed) {
                    let ip = peer.ip().to_canonical();
                    let ip_sessions = per_ip.get(&ip).copied().unwrap_or(0);
                    if sessions.len() < UDP_SESSION_LIMIT && ip_sessions < UDP_PER_IP_LIMIT {
                        let (sender, receiver) = mpsc::channel(UDP_QUEUE_LIMIT);
                        let lease = pool.acquire();
                        let socket = Arc::clone(&socket);
                        sessions.spawn(async move {
                            let upstream = lease.address();
                            if let Err(err) = forward_udp(socket, peer, upstream, receiver).await {
                                eprintln!("UDP proxy {peer} -> {upstream}: {err}");
                            }
                            drop(lease);
                            peer
                        });
                        if clients.insert(peer, sender).is_none() {
                            *per_ip.entry(ip).or_insert(0) += 1;
                        }
                    }
                }
                if let Some(permit) = clients.get(&peer).and_then(|sender| sender.try_reserve().ok()) {
                    permit.send(buffer[..size].to_vec());
                }
            }
            Some(result) = sessions.join_next(), if !sessions.is_empty() => {
                if let Ok(peer) = result
                    && clients.get(&peer).is_some_and(mpsc::Sender::is_closed)
                {
                    clients.remove(&peer);
                    let ip = peer.ip().to_canonical();
                    if let Some(count) = per_ip.get_mut(&ip) {
                        *count = count.saturating_sub(1);
                        if *count == 0 {
                            per_ip.remove(&ip);
                        }
                    }
                }
            }
        }
    }
}

async fn forward_udp(
    frontend: Arc<UdpSocket>,
    peer: SocketAddr,
    upstream: SocketAddr,
    mut packets: mpsc::Receiver<Vec<u8>>,
) -> io::Result<()> {
    let local = if upstream.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let backend = UdpSocket::bind(local).await?;
    backend.connect(upstream).await?;
    let mut buffer = vec![0; UDP_BUFFER_SIZE];
    let mut answered = false;
    let idle = sleep_until(Instant::now() + UDP_UNANSWERED_TIMEOUT);
    tokio::pin!(idle);
    loop {
        tokio::select! {
            packet = packets.recv() => {
                let Some(packet) = packet else { return Ok(()) };
                backend.send(&packet).await?;
                if answered {
                    idle.as_mut().reset(Instant::now() + UDP_IDLE_TIMEOUT);
                }
            }
            reply = backend.recv(&mut buffer) => {
                frontend.send_to(&buffer[..reply?], peer).await?;
                answered = true;
                idle.as_mut().reset(Instant::now() + UDP_IDLE_TIMEOUT);
            }
            _ = &mut idle => return Ok(()),
        }
    }
}
