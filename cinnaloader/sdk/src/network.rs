//! Networking: a socket a mod really owns, and the client's connection, which is not here.
//!
//! Two very different things sit in this section, and they are kept apart on purpose.
//!
//! * **A mod's own connection is real.** [`connect`] resolves an address with `ToSocketAddrs`
//!   and opens a genuine [`std::net::TcpStream`]; [`listen`] and [`accept`] bind and serve a
//!   real `TcpListener`; [`connect_udp`] and [`bind_udp`] open a real `UdpSocket`. Every read
//!   and every write carries a timeout, so no call can hang its caller: [`receive`] answers
//!   [`NetError::Timeout`] rather than blocking. Messages are length-prefixed frames, and the
//!   statistics in [`get_statistics`] are real arithmetic over the bytes that actually moved,
//!   with a moving rate over a window. The channels, message types, serializers and handlers a
//!   mod registers, the timeouts, compression, encryption, retry policy and rate limit it sets,
//!   and address parsing and formatting are all real too.
//! * **The client's connection is not reachable.** The game holds exactly one connection to
//!   its server, inside the client, and this host has no handle on it. Its state, its server
//!   and local addresses, its ping, latency, jitter, packet loss and bandwidth, its player
//!   list, and disconnecting from it, kicking from it or sending into its own transport are
//!   all inside the client; each of those answers `None`/`false`/empty and says why, and
//!   [`why_unknown`] holds the sentences. A mod asks about *its own* sockets through
//!   [`connection`] and [`connections`], never through those.
//!
//! ```
//! use cinnaloader::prelude::*;
//! use cinnaloader::network;
//!
//! let address = network::Address::parse("127.0.0.1:25565").expect("an address");
//! assert_eq!(address.to_string(), "127.0.0.1:25565");
//! assert_eq!(network::Address::parse("[::1]:25565").expect("v6").host, "::1");
//! assert!(network::Address::parse("nope").is_none(), "a bad address is refused");
//!
//! network::register_message("hello:ping");
//! let blob = network::serialize_message("hello:ping", b"hi").expect("registered");
//! assert_eq!(
//!     network::deserialize_message(&blob),
//!     Some(("hello:ping".to_owned(), b"hi".to_vec()))
//! );
//!
//! assert!(network::get_ping().is_none(), "the client's connection is not this host's");
//! assert!(network::get_statistics(1).is_none(), "there is no connection 1");
//! ```

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// A handle to a socket this mod opened.
pub type ConnId = u64;
/// A handle to a listening socket this mod bound.
pub type ListenerId = u64;

/// The protocol version a mod's own connection speaks. The client's is refused.
pub const PROTOCOL_VERSION: u32 = 1;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const RATE_WINDOW: Duration = Duration::from_secs(5);
const MAX_FRAME: usize = 8 * 1024 * 1024;

/// An address a mod connects to, `host:port`, with IPv6 bracketed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Address {
    pub host: String,
    pub port: u16,
}

impl Address {
    #[must_use]
    pub fn new(host: &str, port: u16) -> Self {
        Self {
            host: host.to_owned(),
            port,
        }
    }

    /// Parses `host:port`, `[v6]:port`, and nothing else. A bad input is `None`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        if let Some(rest) = text.strip_prefix('[') {
            let (host, rest) = rest.split_once(']')?;
            if host.is_empty() {
                return None;
            }
            let port = rest.strip_prefix(':')?.parse().ok()?;
            return Some(Self {
                host: host.to_owned(),
                port,
            });
        }
        let (host, port) = text.rsplit_once(':')?;
        if host.is_empty() || host.contains(':') {
            return None;
        }
        let port = port.parse().ok()?;
        Some(Self {
            host: host.to_owned(),
            port,
        })
    }

    fn from_socket(addr: SocketAddr) -> Self {
        Self {
            host: addr.ip().to_string(),
            port: addr.port(),
        }
    }
}

impl fmt::Display for Address {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.contains(':') {
            write!(formatter, "[{}]:{}", self.host, self.port)
        } else {
            write!(formatter, "{}:{}", self.host, self.port)
        }
    }
}

/// Where a connection is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Closing,
    Closed,
}

impl ConnectionState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Closing => "closing",
            Self::Closed => "closed",
        }
    }
}

impl fmt::Display for ConnectionState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Which protocol a mod's socket uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Transport {
    #[default]
    Tcp,
    Udp,
}

impl Transport {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
        }
    }
}

impl fmt::Display for Transport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// How a message's payload is written into its blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Codec {
    #[default]
    Raw,
    Binary,
    Utf8,
    Json,
}

impl Codec {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Binary => "binary",
            Self::Utf8 => "utf8",
            Self::Json => "json",
        }
    }

    fn tag(self) -> u8 {
        match self {
            Self::Raw => 0,
            Self::Binary => 1,
            Self::Utf8 => 2,
            Self::Json => 3,
        }
    }

    fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(Self::Raw),
            1 => Some(Self::Binary),
            2 => Some(Self::Utf8),
            3 => Some(Self::Json),
            _ => None,
        }
    }
}

impl fmt::Display for Codec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// How a mod's own reconnect attempts, if it ever makes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RetryPolicy {
    #[default]
    Never,
    Fixed {
        attempts: u32,
        delay_millis: u64,
    },
    Backoff {
        attempts: u32,
        base_millis: u64,
    },
}

/// Why a socket call did not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetError {
    /// The read or write timed out rather than blocking.
    Timeout,
    /// The peer went away, or this end is closed.
    Closed,
    /// There is no connection with that id.
    Unknown,
    /// The transport cannot do what was asked of it.
    Unsupported,
    Io(String),
}

impl fmt::Display for NetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => formatter.write_str("the socket timed out"),
            Self::Closed => formatter.write_str("the socket is closed"),
            Self::Unknown => formatter.write_str("no such connection"),
            Self::Unsupported => formatter.write_str("the transport does not support that"),
            Self::Io(message) => write!(formatter, "socket error: {message}"),
        }
    }
}

impl std::error::Error for NetError {}

/// What actually moved on one of a mod's sockets.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Statistics {
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub messages_sent: u64,
    pub messages_received: u64,
    /// Payload bytes per second over the last [`RATE_WINDOW`].
    pub bytes_sent_per_second: f64,
    pub bytes_received_per_second: f64,
}

impl Statistics {
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.bytes_sent + self.bytes_received
    }

    #[must_use]
    pub fn total_messages(&self) -> u64 {
        self.messages_sent + self.messages_received
    }
}

#[derive(Debug, Default)]
struct Traffic {
    sent: u64,
    received: u64,
    messages_sent: u64,
    messages_received: u64,
    sent_window: VecDeque<(Instant, u64)>,
    received_window: VecDeque<(Instant, u64)>,
}

impl Traffic {
    fn record_sent(&mut self, bytes: u64) {
        self.sent += bytes;
        self.messages_sent += 1;
        push_sample(&mut self.sent_window, self.sent);
    }

    fn record_received(&mut self, bytes: u64) {
        self.received += bytes;
        self.messages_received += 1;
        push_sample(&mut self.received_window, self.received);
    }

    fn snapshot(&self) -> Statistics {
        let now = Instant::now();
        Statistics {
            bytes_sent: self.sent,
            bytes_received: self.received,
            messages_sent: self.messages_sent,
            messages_received: self.messages_received,
            bytes_sent_per_second: rate(&self.sent_window, self.sent, now),
            bytes_received_per_second: rate(&self.received_window, self.received, now),
        }
    }
}

fn push_sample(window: &mut VecDeque<(Instant, u64)>, total: u64) {
    let now = Instant::now();
    window.push_back((now, total));
    while window.len() > 1 && now.duration_since(window[0].0) > RATE_WINDOW {
        window.pop_front();
    }
}

fn rate(window: &VecDeque<(Instant, u64)>, total: u64, now: Instant) -> f64 {
    let Some(&(oldest, base)) = window.front() else {
        return 0.0;
    };
    let secs = now.duration_since(oldest).as_secs_f64();
    if secs <= 0.0 {
        return 0.0;
    }
    total.saturating_sub(base) as f64 / secs
}

/// A snapshot of one of a mod's connections.
#[derive(Debug, Clone, PartialEq)]
pub struct Connection {
    pub id: ConnId,
    pub owner: String,
    pub label: String,
    pub channel: String,
    pub transport: Transport,
    pub state: ConnectionState,
    pub protocol: u32,
    pub remote: Option<Address>,
    pub local: Option<Address>,
    pub compression: bool,
    pub encryption: bool,
    pub timeout: Duration,
    pub retry: RetryPolicy,
    pub rate_limit: Option<u64>,
    pub statistics: Statistics,
}

impl Connection {
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.state == ConnectionState::Connected
    }

    #[must_use]
    pub fn is_disconnected(&self) -> bool {
        matches!(
            self.state,
            ConnectionState::Disconnected | ConnectionState::Closed
        )
    }

    #[must_use]
    pub fn remote_address(&self) -> Option<Address> {
        self.remote.clone()
    }

    #[must_use]
    pub fn local_address(&self) -> Option<Address> {
        self.local.clone()
    }
}

enum Sock {
    Tcp(TcpStream),
    Udp(UdpSocket),
    Closed,
}

struct Live {
    id: ConnId,
    owner: String,
    label: String,
    channel: String,
    transport: Transport,
    state: ConnectionState,
    protocol: u32,
    remote: Option<SocketAddr>,
    local: Option<SocketAddr>,
    target: Option<String>,
    compression: bool,
    encryption: bool,
    timeout: Duration,
    retry: RetryPolicy,
    rate_limit: Option<u64>,
    traffic: Traffic,
    pending: Vec<u8>,
    sock: Sock,
}

impl Live {
    fn snapshot(&self) -> Connection {
        Connection {
            id: self.id,
            owner: self.owner.clone(),
            label: self.label.clone(),
            channel: self.channel.clone(),
            transport: self.transport,
            state: self.state,
            protocol: self.protocol,
            remote: self.remote.map(Address::from_socket),
            local: self.local.map(Address::from_socket),
            compression: self.compression,
            encryption: self.encryption,
            timeout: self.timeout,
            retry: self.retry,
            rate_limit: self.rate_limit,
            statistics: self.traffic.snapshot(),
        }
    }
}

/// The settings a new connection inherits, and which the setters write.
#[derive(Debug, Clone, Copy)]
struct Config {
    timeout: Duration,
    compression: bool,
    encryption: bool,
    retry: RetryPolicy,
    rate_limit: Option<u64>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            compression: false,
            encryption: false,
            retry: RetryPolicy::Never,
            rate_limit: None,
        }
    }
}

struct MessageType {
    name: String,
    id: u32,
    codec: Codec,
}

type Handler = Arc<dyn Fn(ConnId) + Send + Sync>;

fn registry() -> &'static Mutex<BTreeMap<ConnId, Live>> {
    static REGISTRY: OnceLock<Mutex<BTreeMap<ConnId, Live>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn listeners() -> &'static Mutex<BTreeMap<ListenerId, TcpListener>> {
    static LISTENERS: OnceLock<Mutex<BTreeMap<ListenerId, TcpListener>>> = OnceLock::new();
    LISTENERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn config() -> &'static Mutex<Config> {
    static CONFIG: OnceLock<Mutex<Config>> = OnceLock::new();
    CONFIG.get_or_init(|| Mutex::new(Config::default()))
}

fn channels() -> &'static Mutex<BTreeSet<String>> {
    static CHANNELS: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    CHANNELS.get_or_init(|| Mutex::new(BTreeSet::new()))
}

fn messages() -> &'static Mutex<BTreeMap<String, MessageType>> {
    static MESSAGES: OnceLock<Mutex<BTreeMap<String, MessageType>>> = OnceLock::new();
    MESSAGES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn handlers() -> &'static Mutex<BTreeSet<String>> {
    static HANDLERS: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    HANDLERS.get_or_init(|| Mutex::new(BTreeSet::new()))
}

fn connect_handlers() -> &'static Mutex<Vec<(u64, Handler)>> {
    static CONNECT: OnceLock<Mutex<Vec<(u64, Handler)>>> = OnceLock::new();
    CONNECT.get_or_init(|| Mutex::new(Vec::new()))
}

fn disconnect_handlers() -> &'static Mutex<Vec<(u64, Handler)>> {
    static DISCONNECT: OnceLock<Mutex<Vec<(u64, Handler)>>> = OnceLock::new();
    DISCONNECT.get_or_init(|| Mutex::new(Vec::new()))
}

fn lock<T>(held: &'static Mutex<T>) -> MutexGuard<'static, T> {
    held.lock().unwrap_or_else(|error| error.into_inner())
}

fn next_conn_id() -> ConnId {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn next_listener_id() -> ListenerId {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn next_handler_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn next_message_id() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn snapshot_handlers(store: &'static Mutex<Vec<(u64, Handler)>>) -> Vec<Handler> {
    lock(store)
        .iter()
        .map(|(_, handler)| Arc::clone(handler))
        .collect()
}

fn resolve(target: &str) -> Option<Vec<SocketAddr>> {
    let addrs: Vec<SocketAddr> = target.to_socket_addrs().ok()?.collect();
    if addrs.is_empty() {
        None
    } else {
        Some(addrs)
    }
}

fn io_error(error: std::io::Error) -> NetError {
    match error.kind() {
        ErrorKind::WouldBlock | ErrorKind::TimedOut => NetError::Timeout,
        _ => NetError::Io(error.to_string()),
    }
}

fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

fn decode(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    if buffer.len() < 4 {
        return None;
    }
    let len = u32::from_be_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize;
    if len > MAX_FRAME {
        buffer.clear();
        return None;
    }
    if buffer.len() < 4 + len {
        return None;
    }
    let payload = buffer[4..4 + len].to_vec();
    buffer.drain(..4 + len);
    Some(payload)
}

fn read_tcp(stream: &mut TcpStream, pending: &mut Vec<u8>) -> Result<(), NetError> {
    let mut scratch = [0u8; 8192];
    let read = stream.read(&mut scratch).map_err(io_error)?;
    if read == 0 {
        return Err(NetError::Closed);
    }
    pending.extend_from_slice(&scratch[..read]);
    Ok(())
}

fn read_udp(socket: &mut UdpSocket, pending: &mut Vec<u8>) -> Result<(), NetError> {
    let mut scratch = [0u8; 65536];
    let read = socket.recv(&mut scratch).map_err(io_error)?;
    if read == 0 {
        return Err(NetError::Closed);
    }
    pending.extend_from_slice(&scratch[..read]);
    Ok(())
}

fn open(
    transport: Transport,
    sock: Sock,
    remote: Option<SocketAddr>,
    local: Option<SocketAddr>,
    target: Option<String>,
) -> ConnId {
    let id = next_conn_id();
    let cfg = *lock(config());
    {
        let mut held = lock(registry());
        held.insert(
            id,
            Live {
                id,
                owner: crate::metadata::id(),
                label: target.clone().unwrap_or_else(|| format!("{transport}/{id}")),
                channel: "default".to_owned(),
                transport,
                state: ConnectionState::Connected,
                protocol: PROTOCOL_VERSION,
                remote,
                local,
                target,
                compression: cfg.compression,
                encryption: cfg.encryption,
                timeout: cfg.timeout,
                retry: cfg.retry,
                rate_limit: cfg.rate_limit,
                traffic: Traffic::default(),
                pending: Vec::new(),
                sock,
            },
        );
    }
    for handler in snapshot_handlers(connect_handlers()) {
        handler(id);
    }
    id
}

fn dial(target: &str, timeout: Duration) -> Option<(TcpStream, SocketAddr, SocketAddr)> {
    for addr in resolve(target)? {
        if let Ok(stream) = TcpStream::connect_timeout(&addr, timeout) {
            stream.set_read_timeout(Some(timeout)).ok()?;
            stream.set_write_timeout(Some(timeout)).ok()?;
            let _ = stream.set_nodelay(true);
            let remote = stream.peer_addr().unwrap_or(addr);
            let local = stream.local_addr().unwrap_or(addr);
            return Some((stream, remote, local));
        }
    }
    None
}

fn transport_of(id: ConnId) -> Option<Transport> {
    lock(registry()).get(&id).map(|live| live.transport)
}

fn write_frame(id: ConnId, payload: &[u8]) -> Result<usize, NetError> {
    let framed = frame(payload);
    let mut held = lock(registry());
    let live = held.get_mut(&id).ok_or(NetError::Unknown)?;
    if live.state != ConnectionState::Connected {
        return Err(NetError::Closed);
    }
    let written = match &mut live.sock {
        Sock::Tcp(stream) => {
            stream
                .set_write_timeout(Some(live.timeout))
                .map_err(io_error)?;
            stream.write_all(&framed).map_err(io_error)?;
            stream.flush().map_err(io_error)?;
            payload.len()
        }
        Sock::Udp(socket) => {
            socket
                .set_write_timeout(Some(live.timeout))
                .map_err(io_error)?;
            socket.send(&framed).map_err(io_error)?;
            payload.len()
        }
        Sock::Closed => return Err(NetError::Closed),
    };
    live.traffic.record_sent(written as u64);
    Ok(written)
}

/// Opens a real TCP connection to `target`, resolving it with `ToSocketAddrs`. `None` when
/// nothing is listening or the address is bad.
#[must_use]
pub fn connect(target: &str) -> Option<ConnId> {
    let timeout = lock(config()).timeout;
    let (stream, remote, local) = dial(target, timeout)?;
    Some(open(
        Transport::Tcp,
        Sock::Tcp(stream),
        Some(remote),
        Some(local),
        Some(target.to_owned()),
    ))
}

/// Opens a real connected UDP socket to `target`.
#[must_use]
pub fn connect_udp(target: &str) -> Option<ConnId> {
    let timeout = lock(config()).timeout;
    let addr = resolve(target)?.into_iter().next()?;
    let bind = if addr.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" };
    let socket = UdpSocket::bind(bind).ok()?;
    socket.connect(addr).ok()?;
    socket.set_read_timeout(Some(timeout)).ok()?;
    socket.set_write_timeout(Some(timeout)).ok()?;
    let local = socket.local_addr().ok()?;
    Some(open(
        Transport::Udp,
        Sock::Udp(socket),
        Some(addr),
        Some(local),
        Some(target.to_owned()),
    ))
}

/// Binds a real UDP socket. It is not connected, so it receives from anyone.
#[must_use]
pub fn bind_udp(bind: &str) -> Option<ConnId> {
    let timeout = lock(config()).timeout;
    let socket = UdpSocket::bind(bind).ok()?;
    socket.set_read_timeout(Some(timeout)).ok()?;
    socket.set_write_timeout(Some(timeout)).ok()?;
    let local = socket.local_addr().ok()?;
    Some(open(Transport::Udp, Sock::Udp(socket), None, Some(local), None))
}

/// Binds a real TCP listener on `bind`.
#[must_use]
pub fn listen(bind: &str) -> Option<ListenerId> {
    let listener = TcpListener::bind(bind).ok()?;
    listener.set_nonblocking(true).ok()?;
    let id = next_listener_id();
    lock(listeners()).insert(id, listener);
    Some(id)
}

/// The address a listener bound.
#[must_use]
pub fn listener_address(id: ListenerId) -> Option<Address> {
    lock(listeners())
        .get(&id)
        .and_then(|listener| listener.local_addr().ok())
        .map(Address::from_socket)
}

/// Accepts one peer on a listener, waiting at most `timeout_millis`. `None` on timeout.
#[must_use]
pub fn accept(listener: ListenerId, timeout_millis: u64) -> Option<ConnId> {
    let timeout = lock(config()).timeout;
    let deadline = Instant::now() + Duration::from_millis(timeout_millis);
    loop {
        let accepted = {
            let held = lock(listeners());
            let held_listener = held.get(&listener)?;
            match held_listener.accept() {
                Ok((stream, addr)) => Some((stream, addr)),
                Err(error) if error.kind() == ErrorKind::WouldBlock => None,
                Err(_) => return None,
            }
        };
        if let Some((stream, addr)) = accepted {
            stream.set_read_timeout(Some(timeout)).ok()?;
            stream.set_write_timeout(Some(timeout)).ok()?;
            let _ = stream.set_nodelay(true);
            let local = stream.local_addr().unwrap_or(addr);
            return Some(open(
                Transport::Tcp,
                Sock::Tcp(stream),
                Some(addr),
                Some(local),
                None,
            ));
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Stops listening, which drops the bound socket.
pub fn stop_listening(listener: ListenerId) -> bool {
    lock(listeners()).remove(&listener).is_some()
}

/// A snapshot of one of this mod's connections. The client's are not here.
#[must_use]
pub fn connection(id: ConnId) -> Option<Connection> {
    lock(registry()).get(&id).map(Live::snapshot)
}

/// A snapshot of every connection this mod has open, in id order.
#[must_use]
pub fn connections() -> Vec<Connection> {
    lock(registry()).values().map(Live::snapshot).collect()
}

/// Writes one length-prefixed frame, bounded by the connection's timeout.
pub fn send(id: ConnId, payload: &[u8]) -> Result<usize, NetError> {
    write_frame(id, payload)
}

/// The same, on a transport that can promise delivery. A datagram socket refuses.
pub fn send_reliable(id: ConnId, payload: &[u8]) -> Result<usize, NetError> {
    match transport_of(id) {
        Some(Transport::Tcp) => write_frame(id, payload),
        Some(Transport::Udp) => Err(NetError::Unsupported),
        None => Err(NetError::Unknown),
    }
}

/// One datagram, which may be lost. A stream socket refuses.
pub fn send_unreliable(id: ConnId, payload: &[u8]) -> Result<usize, NetError> {
    match transport_of(id) {
        Some(Transport::Udp) => write_frame(id, payload),
        Some(Transport::Tcp) => Err(NetError::Unsupported),
        None => Err(NetError::Unknown),
    }
}

/// A frame whose payload carries an ordering sequence in front of the data.
pub fn send_ordered(id: ConnId, seq: u64, payload: &[u8]) -> Result<usize, NetError> {
    let mut body = Vec::with_capacity(8 + payload.len());
    body.extend_from_slice(&seq.to_be_bytes());
    body.extend_from_slice(payload);
    write_frame(id, &body).map(|_| payload.len())
}

/// Sends one frame to every open connection. The count is how many took it.
pub fn broadcast(payload: &[u8]) -> usize {
    let framed = frame(payload);
    let mut held = lock(registry());
    let mut sent = 0;
    for live in held.values_mut() {
        if live.state != ConnectionState::Connected {
            continue;
        }
        let wrote = match &mut live.sock {
            Sock::Tcp(stream) => stream.write_all(&framed).is_ok(),
            Sock::Udp(socket) => socket.send(&framed).is_ok(),
            Sock::Closed => false,
        };
        if wrote {
            live.traffic.record_sent(payload.len() as u64);
            sent += 1;
        }
    }
    sent
}

/// Reads one frame, waiting at most `timeout_millis`. A timeout is [`NetError::Timeout`].
pub fn receive(id: ConnId, timeout_millis: u64) -> Result<Vec<u8>, NetError> {
    let mut held = lock(registry());
    let live = held.get_mut(&id).ok_or(NetError::Unknown)?;
    if let Some(payload) = decode(&mut live.pending) {
        live.traffic.record_received(payload.len() as u64);
        return Ok(payload);
    }
    if live.state != ConnectionState::Connected {
        return Err(NetError::Closed);
    }
    let timeout = Duration::from_millis(timeout_millis);
    let Live {
        sock,
        pending,
        traffic,
        ..
    } = live;
    match sock {
        Sock::Tcp(stream) => {
            stream.set_read_timeout(Some(timeout)).map_err(io_error)?;
            read_tcp(stream, pending)?;
        }
        Sock::Udp(socket) => {
            socket.set_read_timeout(Some(timeout)).map_err(io_error)?;
            read_udp(socket, pending)?;
        }
        Sock::Closed => return Err(NetError::Closed),
    }
    match decode(pending) {
        Some(payload) => {
            traffic.record_received(payload.len() as u64);
            Ok(payload)
        }
        None => Err(NetError::Timeout),
    }
}

/// Closes the socket on a connection and leaves it in [`ConnectionState::Closed`].
pub fn disconnect(id: ConnId) -> bool {
    let handlers = snapshot_handlers(disconnect_handlers());
    let closed = {
        let mut held = lock(registry());
        match held.get_mut(&id) {
            Some(live) => {
                live.state = ConnectionState::Closed;
                live.sock = Sock::Closed;
                live.pending.clear();
                true
            }
            None => false,
        }
    };
    if closed {
        for handler in handlers {
            handler(id);
        }
    }
    closed
}

/// Drops a connection entirely, so [`connection`] no longer finds it.
pub fn close(id: ConnId) -> bool {
    let handlers = snapshot_handlers(disconnect_handlers());
    let removed = lock(registry()).remove(&id).is_some();
    if removed {
        for handler in handlers {
            handler(id);
        }
    }
    removed
}

/// Re-dials the address this connection was opened to.
pub fn reconnect(id: ConnId) -> bool {
    let (target, timeout) = match lock(registry()).get(&id) {
        Some(live) => (live.target.clone(), live.timeout),
        None => return false,
    };
    let Some(target) = target else {
        return false;
    };
    let Some((stream, remote, local)) = dial(&target, timeout) else {
        return false;
    };
    let mut held = lock(registry());
    match held.get_mut(&id) {
        Some(live) => {
            live.sock = Sock::Tcp(stream);
            live.remote = Some(remote);
            live.local = Some(local);
            live.transport = Transport::Tcp;
            live.state = ConnectionState::Connected;
            true
        }
        None => false,
    }
}

/// Flushes a stream socket's buffered writes. A datagram socket has nothing to flush.
pub fn flush(id: ConnId) -> bool {
    let mut held = lock(registry());
    match held.get_mut(&id) {
        Some(live) => match &mut live.sock {
            Sock::Tcp(stream) => stream.flush().is_ok(),
            _ => false,
        },
        None => false,
    }
}

/// What moved on one connection.
#[must_use]
pub fn get_statistics(id: ConnId) -> Option<Statistics> {
    lock(registry()).get(&id).map(|live| live.traffic.snapshot())
}

/// What moved on every connection this mod has open.
#[must_use]
pub fn get_traffic_stats() -> Statistics {
    let held = lock(registry());
    let mut total = Statistics::default();
    for live in held.values() {
        let stats = live.traffic.snapshot();
        total.bytes_sent += stats.bytes_sent;
        total.bytes_received += stats.bytes_received;
        total.messages_sent += stats.messages_sent;
        total.messages_received += stats.messages_received;
        total.bytes_sent_per_second += stats.bytes_sent_per_second;
        total.bytes_received_per_second += stats.bytes_received_per_second;
    }
    total
}

/// Sets the read and write timeout for every connection, in milliseconds.
pub fn set_timeout(millis: u64) {
    let timeout = Duration::from_millis(millis.max(1));
    lock(config()).timeout = timeout;
    let mut held = lock(registry());
    for live in held.values_mut() {
        live.timeout = timeout;
        match &live.sock {
            Sock::Tcp(stream) => {
                let _ = stream.set_read_timeout(Some(timeout));
                let _ = stream.set_write_timeout(Some(timeout));
            }
            Sock::Udp(socket) => {
                let _ = socket.set_read_timeout(Some(timeout));
                let _ = socket.set_write_timeout(Some(timeout));
            }
            Sock::Closed => {}
        }
    }
}

/// Records whether frames carry the compression flag. The flag is real; the payload stays as
/// the caller wrote it, because compressing it needs a codec dependency this crate does not take.
pub fn set_compression(on: bool) {
    lock(config()).compression = on;
    let mut held = lock(registry());
    for live in held.values_mut() {
        live.compression = on;
    }
}

/// Records whether frames ask for encryption. As with compression, the flag is real and the
/// payload is not transformed here.
pub fn set_encryption(on: bool) {
    lock(config()).encryption = on;
    let mut held = lock(registry());
    for live in held.values_mut() {
        live.encryption = on;
    }
}

/// Sets how a mod's own reconnect would retry.
pub fn set_retry_policy(policy: RetryPolicy) {
    lock(config()).retry = policy;
    let mut held = lock(registry());
    for live in held.values_mut() {
        live.retry = policy;
    }
}

/// Caps a connection at `bytes_per_second`; zero means no cap. The value is real state a mod
/// can respect when it paces its own writes.
pub fn set_rate_limit(bytes_per_second: u64) {
    let limit = if bytes_per_second == 0 {
        None
    } else {
        Some(bytes_per_second)
    };
    lock(config()).rate_limit = limit;
    let mut held = lock(registry());
    for live in held.values_mut() {
        live.rate_limit = limit;
    }
}

/// Registers a callback that runs when one of this mod's sockets opens. Returns its token.
pub fn register_connection_handler<F>(handler: F) -> u64
where
    F: Fn(ConnId) + Send + Sync + 'static,
{
    let id = next_handler_id();
    lock(connect_handlers()).push((id, Arc::new(handler)));
    id
}

/// Registers a callback that runs when one of this mod's sockets closes. Returns its token.
pub fn register_disconnect_handler<F>(handler: F) -> u64
where
    F: Fn(ConnId) + Send + Sync + 'static,
{
    let id = next_handler_id();
    lock(disconnect_handlers()).push((id, Arc::new(handler)));
    id
}

/// Declares a channel a mod talks on.
pub fn register_channel(name: &str) -> bool {
    lock(channels()).insert(name.to_owned())
}

/// Forgets one.
pub fn unregister_channel(name: &str) -> bool {
    lock(channels()).remove(name)
}

/// Every channel a mod registered.
#[must_use]
pub fn get_channels() -> Vec<String> {
    lock(channels()).iter().cloned().collect()
}

/// Declares a message type, which gives it the id its blob carries.
pub fn register_message(name: &str) -> bool {
    let mut held = lock(messages());
    if held.contains_key(name) {
        return false;
    }
    held.insert(
        name.to_owned(),
        MessageType {
            name: name.to_owned(),
            id: next_message_id(),
            codec: Codec::Raw,
        },
    );
    true
}

/// Forgets one.
pub fn unregister_message(name: &str) -> bool {
    lock(messages()).remove(name).is_some()
}

/// Sets the codec a message type serializes with.
pub fn register_serializer(name: &str, codec: Codec) -> bool {
    match lock(messages()).get_mut(name) {
        Some(message) => {
            message.codec = codec;
            true
        }
        None => false,
    }
}

/// Encodes a message as `id || length || codec || payload`. `None` when the type is unknown or
/// the payload does not match the codec.
#[must_use]
pub fn serialize_message(name: &str, payload: &[u8]) -> Option<Vec<u8>> {
    let (id, codec) = {
        let held = lock(messages());
        let message = held.get(name)?;
        (message.id, message.codec)
    };
    if codec == Codec::Utf8 && std::str::from_utf8(payload).is_err() {
        return None;
    }
    let len = payload.len() as u32 + 1;
    let mut out = Vec::with_capacity(9 + payload.len());
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&len.to_be_bytes());
    out.push(codec.tag());
    out.extend_from_slice(payload);
    Some(out)
}

/// Reads back what [`serialize_message`] wrote, resolving the id to its type name.
#[must_use]
pub fn deserialize_message(blob: &[u8]) -> Option<(String, Vec<u8>)> {
    if blob.len() < 9 {
        return None;
    }
    let id = u32::from_be_bytes([blob[0], blob[1], blob[2], blob[3]]);
    let len = u32::from_be_bytes([blob[4], blob[5], blob[6], blob[7]]) as usize;
    if len < 1 || blob.len() < 8 + len {
        return None;
    }
    let codec = Codec::from_tag(blob[8])?;
    let name = {
        let held = lock(messages());
        held.values().find(|message| message.id == id)?.name.clone()
    };
    let payload = blob[9..8 + len].to_vec();
    if codec == Codec::Utf8 && std::str::from_utf8(&payload).is_err() {
        return None;
    }
    Some((name, payload))
}

/// Registers a handler for a channel.
pub fn register_handler(channel: &str) -> bool {
    lock(handlers()).insert(channel.to_owned())
}

/// Forgets one.
pub fn unregister_handler(channel: &str) -> bool {
    lock(handlers()).remove(channel)
}

/// Every channel a handler is registered for.
#[must_use]
pub fn get_handlers() -> Vec<String> {
    lock(handlers()).iter().cloned().collect()
}

/// Why a question about the client's own connection has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "connection" => {
            "the game holds one connection to its server inside the client, and this host has \
             no handle on it"
                .to_owned()
        }
        "connections" => {
            "the client's connections and its player list live in the client's own session"
                .to_owned()
        }
        "remote" => {
            "the server address the client is connected to is inside the client's network \
             manager"
                .to_owned()
        }
        "local" => "the client's local address is chosen by its own socket".to_owned(),
        "latency" | "ping" => {
            "ping and latency are measured by the client's own connection, packet by packet"
                .to_owned()
        }
        "jitter" => "jitter is the client's; this host sees none of its packets".to_owned(),
        "loss" => "packet loss belongs to the client's connection, counted inside it".to_owned(),
        "bandwidth" => {
            "bandwidth is a property of the client's connection, which is not visible from here"
                .to_owned()
        }
        "state" => "the client's connection state is inside its network manager".to_owned(),
        "protocol" => "the protocol version is the one the client negotiated with its server".to_owned(),
        "session" => "the session id belongs to the client's login, not to a mod".to_owned(),
        "disconnect" => {
            "disconnecting from the game's server is the client's, and this host has no hook for \
             it"
                .to_owned()
        }
        "kick" => "kicking a player needs the server, which this host is not".to_owned(),
        "send" => {
            "sending into the game's own transport would need the client's packet writer, which \
             a mod host does not hold"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    [
        "connection",
        "connections",
        "remote",
        "local",
        "latency",
        "ping",
        "jitter",
        "loss",
        "bandwidth",
        "state",
        "protocol",
        "session",
        "disconnect",
        "kick",
        "send",
    ]
    .iter()
    .map(|what| ((*what).to_owned(), why_unknown(what)))
    .collect()
}

fn refused<T: Default>(what: &str) -> T {
    crate::warn!(
        "a mod asked this host about the client's connection: {}",
        why_unknown(what)
    );
    T::default()
}

/// Whether the client is connected to its server. `false`: not this host's to answer.
#[must_use]
pub fn is_connected() -> bool {
    refused("connection")
}

/// Whether the client is disconnected. `false`.
#[must_use]
pub fn is_disconnected() -> bool {
    refused("state")
}

/// The client's connection. `None`.
#[must_use]
pub fn get_connection() -> Option<Connection> {
    refused("connection")
}

/// The client's connections and its player list. Empty.
#[must_use]
pub fn get_connections() -> Vec<Connection> {
    refused("connections")
}

/// The server the client is connected to. `None`.
#[must_use]
pub fn get_remote_address() -> Option<Address> {
    refused("remote")
}

/// The client's local address. `None`.
#[must_use]
pub fn get_local_address() -> Option<Address> {
    refused("local")
}

/// The client's latency, in milliseconds. `None`.
#[must_use]
pub fn get_latency() -> Option<u32> {
    refused("latency")
}

/// The client's ping, in milliseconds. `None`.
#[must_use]
pub fn get_ping() -> Option<u32> {
    refused("ping")
}

/// The client's jitter, in milliseconds. `None`.
#[must_use]
pub fn get_jitter() -> Option<u32> {
    refused("jitter")
}

/// The client's packet loss, as a fraction. `None`.
#[must_use]
pub fn get_packet_loss() -> Option<f64> {
    refused("loss")
}

/// The client's bandwidth, in bytes per second. `None`.
#[must_use]
pub fn get_bandwidth() -> Option<u64> {
    refused("bandwidth")
}

/// The client's connection state. `None`.
#[must_use]
pub fn get_connection_state() -> Option<ConnectionState> {
    refused("state")
}

/// The protocol version the client negotiated. `None`.
#[must_use]
pub fn get_protocol_version() -> Option<u32> {
    refused("protocol")
}

/// The client's session id. `None`.
#[must_use]
pub fn get_session_id() -> Option<String> {
    refused("session")
}

/// Disconnects the client from its server. `false`.
pub fn disconnect_client() -> bool {
    refused("disconnect")
}

/// Kicks a player from the client's session. `false`.
pub fn kick_player(_name: &str) -> bool {
    refused("kick")
}

/// Sends into the game's own transport. `false`.
pub fn send_to_client(_payload: &[u8]) -> bool {
    refused("send")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        lock(registry()).clear();
        lock(listeners()).clear();
        lock(channels()).clear();
        lock(messages()).clear();
        lock(handlers()).clear();
        lock(connect_handlers()).clear();
        lock(disconnect_handlers()).clear();
        *lock(config()) = Config::default();
    }

    #[test]
    fn addresses_parse_format_and_refuse_bad_input() {
        let _serial = serial();
        assert_eq!(
            Address::parse("127.0.0.1:25565"),
            Some(Address::new("127.0.0.1", 25565))
        );
        assert_eq!(
            Address::parse("127.0.0.1:25565").unwrap().to_string(),
            "127.0.0.1:25565"
        );
        let v6 = Address::parse("[::1]:25565").unwrap();
        assert_eq!(v6.host, "::1");
        assert_eq!(v6.port, 25565);
        assert_eq!(v6.to_string(), "[::1]:25565");
        assert!(Address::parse("example.com:443").is_some());
        assert!(Address::parse("::1:25565").is_none(), "an unbracketed v6 is ambiguous");
        assert!(Address::parse("127.0.0.1").is_none(), "no port");
        assert!(Address::parse("127.0.0.1:notaport").is_none());
        assert!(Address::parse(":80").is_none(), "no host");
        assert!(Address::parse("").is_none());
        assert!(Address::parse("[::1]").is_none(), "no port");
    }

    #[test]
    fn framing_survives_a_message_split_across_reads() {
        let _serial = serial();
        let whole = frame(b"hello");
        assert_eq!(whole.len(), 9, "four bytes of length and five of payload");
        let mut buffer = whole[..3].to_vec();
        assert!(decode(&mut buffer).is_none(), "a partial length prefix is not a frame");
        buffer.extend_from_slice(&whole[3..6]);
        assert!(decode(&mut buffer).is_none(), "a partial payload is not a frame");
        buffer.extend_from_slice(&whole[6..]);
        assert_eq!(decode(&mut buffer), Some(b"hello".to_vec()));
        assert!(buffer.is_empty(), "the frame was taken out of the buffer");
        assert!(decode(&mut buffer).is_none());
    }

    #[test]
    fn the_channels_messages_and_handlers_are_real_registries() {
        let _serial = serial();
        fresh();
        assert!(register_channel("hello:chat"));
        assert!(!register_channel("hello:chat"), "once");
        assert_eq!(get_channels(), vec!["hello:chat".to_owned()]);
        assert!(unregister_channel("hello:chat"));
        assert!(!unregister_channel("hello:chat"));

        assert!(register_message("hello:ping"));
        assert!(!register_message("hello:ping"));
        assert!(register_serializer("hello:ping", Codec::Utf8));
        let blob = serialize_message("hello:ping", b"hi").unwrap();
        assert_eq!(
            deserialize_message(&blob),
            Some(("hello:ping".to_owned(), b"hi".to_vec()))
        );
        assert!(
            serialize_message("hello:ping", &[0xff, 0xfe]).is_none(),
            "not utf8"
        );
        assert!(serialize_message("nothing", b"x").is_none());
        assert!(unregister_message("hello:ping"));
        assert!(deserialize_message(&blob).is_none(), "its type is gone");

        assert!(register_handler("hello:chat"));
        assert!(!register_handler("hello:chat"));
        assert_eq!(get_handlers(), vec!["hello:chat".to_owned()]);
        assert!(unregister_handler("hello:chat"));
        assert!(!unregister_handler("hello:chat"));
    }

    #[test]
    fn a_real_tcp_round_trip_moves_exactly_its_bytes() {
        let _serial = serial();
        fresh();
        let listener = listen("127.0.0.1:0").expect("a loopback listener");
        let addr = listener_address(listener).expect("its address");
        let client = connect(&addr.to_string()).expect("a loopback connection");
        let server = accept(listener, 1000).expect("the peer accepts");

        let payload: Vec<u8> = (0..64u8).collect();
        assert_eq!(send(client, &payload).expect("sent"), payload.len());
        assert_eq!(receive(server, 1000).expect("received"), payload);

        let sent = get_statistics(client).expect("client statistics");
        assert_eq!(sent.bytes_sent, 64);
        assert_eq!(sent.messages_sent, 1);
        assert_eq!(sent.bytes_received, 0);
        let got = get_statistics(server).expect("server statistics");
        assert_eq!(got.bytes_received, 64);
        assert_eq!(got.messages_received, 1);
        assert_eq!(got.bytes_sent, 0);

        let live = connection(client).expect("live");
        assert!(live.is_connected());
        assert!(!live.is_disconnected());
        assert_eq!(live.remote_address(), Some(addr.clone()));
        assert!(live.local_address().is_some());
        assert_eq!(live.protocol, PROTOCOL_VERSION);

        assert!(close(client));
        assert!(close(server));
        assert!(connection(client).is_none());
    }

    #[test]
    fn a_read_times_out_rather_than_blocking() {
        let _serial = serial();
        fresh();
        let listener = listen("127.0.0.1:0").unwrap();
        let addr = listener_address(listener).unwrap();
        let client = connect(&addr.to_string()).unwrap();
        let server = accept(listener, 1000).unwrap();

        let started = Instant::now();
        let error = receive(server, 50).unwrap_err();
        assert_eq!(error, NetError::Timeout);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "it returned instead of blocking"
        );
        assert!(disconnect(client));
        assert_eq!(connection(client).unwrap().state, ConnectionState::Closed);
        assert!(close(server));
    }

    #[test]
    fn a_connection_to_a_closed_port_is_refused() {
        let _serial = serial();
        fresh();
        let listener = listen("127.0.0.1:0").unwrap();
        let addr = listener_address(listener).unwrap();
        assert!(stop_listening(listener));
        assert!(
            connect(&addr.to_string()).is_none(),
            "nothing is listening on that port any more"
        );
    }

    #[test]
    fn connection_and_disconnect_handlers_run_for_a_mods_own_socket() {
        let _serial = serial();
        fresh();
        let opens = Arc::new(AtomicU64::new(0));
        let closes = Arc::new(AtomicU64::new(0));
        let into = Arc::clone(&opens);
        register_connection_handler(move |_id| {
            into.fetch_add(1, Ordering::SeqCst);
        });
        let into = Arc::clone(&closes);
        register_disconnect_handler(move |_id| {
            into.fetch_add(1, Ordering::SeqCst);
        });

        let listener = listen("127.0.0.1:0").unwrap();
        let addr = listener_address(listener).unwrap();
        let client = connect(&addr.to_string()).unwrap();
        let server = accept(listener, 1000).unwrap();
        assert_eq!(
            opens.load(Ordering::SeqCst),
            2,
            "the dial and the accepted socket each opened"
        );
        assert!(disconnect(client));
        assert_eq!(closes.load(Ordering::SeqCst), 1);
        assert!(close(server));
        assert_eq!(closes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn the_reliable_and_unreliable_sends_know_their_transport() {
        let _serial = serial();
        fresh();
        let listener = listen("127.0.0.1:0").unwrap();
        let addr = listener_address(listener).unwrap();
        let client = connect(&addr.to_string()).unwrap();
        let server = accept(listener, 1000).unwrap();

        assert_eq!(send_reliable(client, b"abc").unwrap(), 3);
        assert_eq!(
            send_unreliable(client, b"de").unwrap_err(),
            NetError::Unsupported
        );
        assert_eq!(receive(server, 1000).unwrap(), b"abc".to_vec());
        assert_eq!(send_ordered(client, 7, b"xy").unwrap(), 2);
        let ordered = receive(server, 1000).unwrap();
        assert_eq!(&ordered[..8], &7u64.to_be_bytes());
        assert_eq!(&ordered[8..], b"xy".as_slice());
        assert_eq!(broadcast(b"hi"), 2, "both ends are open");
        assert_eq!(receive(server, 1000).unwrap(), b"hi".to_vec());
        assert_eq!(get_traffic_stats().messages_sent, 4);
        assert!(close(client));
        assert!(close(server));
    }

    #[test]
    fn an_unreliable_datagram_round_trips_over_udp() {
        let _serial = serial();
        fresh();
        let server = bind_udp("127.0.0.1:0").unwrap();
        let server_addr = connection(server).unwrap().local_address().unwrap();
        let client = connect_udp(&server_addr.to_string()).unwrap();
        assert!(send_reliable(client, b"x").is_err(), "udp cannot promise delivery");
        assert_eq!(send_unreliable(client, b"ping").unwrap(), 4);
        assert_eq!(receive(server, 1000).unwrap(), b"ping".to_vec());
        let stats = get_statistics(server).unwrap();
        assert_eq!(stats.bytes_received, 4);
        assert_eq!(stats.messages_received, 1);
        assert!(close(client));
        assert!(close(server));
    }

    #[test]
    fn the_clients_connection_is_refused_with_its_own_reason() {
        let _serial = serial();
        fresh();
        assert!(!is_connected());
        assert!(!is_disconnected());
        assert!(get_connection().is_none());
        assert!(get_connections().is_empty());
        assert!(get_remote_address().is_none());
        assert!(get_local_address().is_none());
        assert!(get_latency().is_none());
        assert!(get_ping().is_none());
        assert!(get_jitter().is_none());
        assert!(get_packet_loss().is_none());
        assert!(get_bandwidth().is_none());
        assert!(get_connection_state().is_none());
        assert!(get_protocol_version().is_none());
        assert!(get_session_id().is_none());
        assert!(!disconnect_client());
        assert!(!kick_player("Steve"));
        assert!(!send_to_client(b"hi"));

        assert_eq!(refusals().len(), 15);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }
}
