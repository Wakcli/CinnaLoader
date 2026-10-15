//! Custom networking between mods: real peer-to-peer delivery in this process, and the
//! client's own connections refused.
//!
//! Two mods loaded into the same process can talk to each other here, and the talk is real:
//!
//! * **A peer registry.** A mod declares a peer in this process — its mod id, the API version
//!   it advertises and the capabilities it holds — and [`get_peer_mods`],
//!   [`get_peer_api_versions`] and [`get_peer_capabilities`] read it back.
//! * **Named channels.** A channel carries a name, a [`Codec`] and a payload limit.
//!   [`register`] declares one, [`register_codec`] and [`set_payload_limit`] adjust it, and
//!   [`get`]/[`get_all`] and [`get_channel_status`] report it.
//! * **Real delivery.** [`subscribe`] registers a handler for the mod that owns it, and
//!   [`send`] frames the payload with section 43's [`crate::packets::encode`]/[`decode`],
//!   decodes it again and hands it to that peer's handler, answering the count of handlers it
//!   really reached. [`broadcast`] does the same for every peer once.
//! * **Streams and requests.** [`open_stream`] opens an ordered channel to a peer,
//!   [`send_stream`] numbers each message and [`close_stream`] ends it, after which it refuses
//!   more. [`register_request_handler`] answers a [`send_request`] with bytes, through
//!   [`send_response`] or a return value.
//! * **Versions.** Two peers with different advertised versions are both visible, and a
//!   channel that requires one of them reports the mismatch through [`negotiate_version`] and
//!   [`send`] rather than passing it silently.
//!
//! What this host cannot do is leave the process. Delivering to a player's client, relaying
//! through the client's server, and reading the client's own plugin channels all need the
//! game's one connection, which lives inside the client; each is refused with its own sentence
//! in [`why_unknown`], listed by [`refusals`], and never answers a fake success.
//!
//! ```
//! use cinnaloader::channels::{self, Channel, ChannelError, Peer};
//!
//! channels::register(Channel::new("hello:telemetry").payload_limit(64));
//! channels::register_peer(Peer::new("hello:relay", 1));
//! channels::subscribe_for("hello:relay", "hello:telemetry", |msg| {
//!     assert_eq!(msg.payload, b"tick");
//! });
//!
//! assert_eq!(channels::send("hello:telemetry", "hello:relay", b"tick"), Ok(1));
//! assert_eq!(
//!     channels::send("hello:telemetry", "hello:ghost", b"tick"),
//!     Err(ChannelError::UnknownPeer("hello:ghost".to_owned()))
//! );
//! assert_eq!(
//!     channels::send("hello:telemetry", "hello:relay", &[0u8; 65]),
//!     Err(ChannelError::PayloadTooLarge { size: 65, limit: 64 })
//! );
//! assert!(!channels::send_to_player("Steve", "hello:telemetry", b"tick"));
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use crate::capability::Capability;
pub use crate::network::Codec;

/// The payload limit a channel carries until a mod sets its own.
pub const DEFAULT_PAYLOAD_LIMIT: usize = 32 * 1024;

/// A handle to a stream a mod opened to a peer.
pub type StreamId = u64;

/// Where a channel is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ChannelStatus {
    /// Not declared, or declared and unregistered: nothing travels on it.
    #[default]
    Closed,
    /// Declared, with no stream open on it.
    Registered,
    /// Declared, with at least one stream open on it.
    Open,
}

impl ChannelStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Registered => "registered",
            Self::Open => "open",
        }
    }
}

impl fmt::Display for ChannelStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Why a channel call did not deliver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelError {
    /// No channel by that name is declared.
    UnknownChannel(String),
    /// No peer by that mod id is registered in this process.
    UnknownPeer(String),
    /// No stream has that handle.
    UnknownStream(StreamId),
    /// The payload is longer than the channel allows.
    PayloadTooLarge { size: usize, limit: usize },
    /// The peer's advertised version is not the one the channel requires.
    VersionMismatch { required: u32, actual: u32 },
    /// The payload does not match the channel's codec.
    Codec(String),
    /// The payload did not survive section 43's framing.
    Framing(String),
    /// The stream is closed and refuses more messages.
    StreamClosed(StreamId),
    /// The target peer registered no handler for this channel.
    NoHandler(String),
}

impl fmt::Display for ChannelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownChannel(name) => write!(formatter, "channel {name} is not declared"),
            Self::UnknownPeer(id) => write!(formatter, "no peer {id} is registered"),
            Self::UnknownStream(id) => write!(formatter, "no stream {id} is open"),
            Self::PayloadTooLarge { size, limit } => {
                write!(formatter, "payload is {size} bytes, over the {limit}-byte limit")
            }
            Self::VersionMismatch { required, actual } => {
                write!(formatter, "channel needs version {required}, peer advertises {actual}")
            }
            Self::Codec(why) => write!(formatter, "payload does not match the codec: {why}"),
            Self::Framing(why) => write!(formatter, "framing failed: {why}"),
            Self::StreamClosed(id) => write!(formatter, "stream {id} is closed"),
            Self::NoHandler(name) => write!(formatter, "the peer registered no handler for {name}"),
        }
    }
}

impl std::error::Error for ChannelError {}

/// A named channel a mod talks on: a codec, a payload limit and, optionally, the API version
/// it requires of a peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub name: String,
    pub owner: String,
    pub codec: Codec,
    pub payload_limit: usize,
    pub required_version: Option<u32>,
}

impl Channel {
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            owner: crate::metadata::id(),
            codec: Codec::Raw,
            payload_limit: DEFAULT_PAYLOAD_LIMIT,
            required_version: None,
        }
    }

    #[must_use]
    pub fn codec(mut self, codec: Codec) -> Self {
        self.codec = codec;
        self
    }

    #[must_use]
    pub fn payload_limit(mut self, limit: usize) -> Self {
        self.payload_limit = limit;
        self
    }

    #[must_use]
    pub fn require_version(mut self, version: u32) -> Self {
        self.required_version = Some(version);
        self
    }
}

/// Another mod loaded into this process, as it advertises itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub mod_id: String,
    pub api_version: u32,
    pub capabilities: Vec<Capability>,
}

impl Peer {
    #[must_use]
    pub fn new(mod_id: &str, api_version: u32) -> Self {
        Self {
            mod_id: mod_id.to_owned(),
            api_version,
            capabilities: Vec::new(),
        }
    }

    #[must_use]
    pub fn capability(mut self, capability: Capability) -> Self {
        if !self.capabilities.contains(&capability) {
            self.capabilities.push(capability);
        }
        self
    }
}

/// One message handed to a subscribed handler, byte for byte as the sender wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub channel: String,
    pub sender: String,
    pub payload: Vec<u8>,
    pub sequence: u64,
}

/// A request handed to the target peer's request handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub id: u64,
    pub channel: String,
    pub sender: String,
    pub payload: Vec<u8>,
}

/// An ordered stream a mod opened to a peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stream {
    pub id: StreamId,
    pub channel: String,
    pub peer: String,
    pub sequence: u64,
    pub messages: u64,
    pub open: bool,
}

type Handler = Arc<dyn Fn(Message) + Send + Sync>;
type RequestHandler = Arc<dyn Fn(&Request) -> Vec<u8> + Send + Sync>;
type Subscription = (u64, String, String, Handler);
type RequestRoute = (u64, String, String, RequestHandler);

#[derive(Default)]
struct Registry {
    channels: BTreeMap<String, Channel>,
    peers: BTreeMap<String, Peer>,
    subs: Vec<Subscription>,
    routes: Vec<RequestRoute>,
    streams: BTreeMap<StreamId, Stream>,
    responses: BTreeMap<u64, Vec<u8>>,
    open_requests: BTreeSet<u64>,
    next_sub: u64,
    next_route: u64,
    next_stream: u64,
    next_request: u64,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

fn held() -> MutexGuard<'static, Registry> {
    registry().lock().unwrap_or_else(|error| error.into_inner())
}

/// Declares a channel, or replaces the one with that name. Answers whether it was new.
pub fn register(channel: Channel) -> bool {
    let name = channel.name.clone();
    let fresh = held().channels.insert(name.clone(), channel).is_none();
    crate::network::register_channel(&name);
    fresh
}

/// Forgets a channel, closes its streams, and answers whether it was there.
pub fn unregister(name: &str) -> bool {
    let removed = {
        let mut held = held();
        if held.channels.remove(name).is_none() {
            false
        } else {
            for stream in held.streams.values_mut() {
                if stream.channel == name {
                    stream.open = false;
                }
            }
            true
        }
    };
    if removed {
        crate::network::unregister_channel(name);
    }
    removed
}

/// A declared channel by name.
#[must_use]
pub fn get(name: &str) -> Option<Channel> {
    held().channels.get(name).cloned()
}

/// Every declared channel, in name order.
#[must_use]
pub fn get_all() -> Vec<Channel> {
    held().channels.values().cloned().collect()
}

/// Sets the codec a channel frames its payloads with. An unknown channel is refused.
pub fn register_codec(name: &str, codec: Codec) -> bool {
    match held().channels.get_mut(name) {
        Some(channel) => {
            channel.codec = codec;
            true
        }
        None => false,
    }
}

/// The codec a channel frames its payloads with.
#[must_use]
pub fn get_codec(name: &str) -> Option<Codec> {
    held().channels.get(name).map(|channel| channel.codec)
}

/// The payload limit a channel enforces, in bytes.
#[must_use]
pub fn get_payload_limit(name: &str) -> Option<usize> {
    held().channels.get(name).map(|channel| channel.payload_limit)
}

/// Sets a channel's payload limit. An unknown channel is refused.
pub fn set_payload_limit(name: &str, limit: usize) -> bool {
    match held().channels.get_mut(name) {
        Some(channel) => {
            channel.payload_limit = limit;
            true
        }
        None => false,
    }
}

/// Whether a channel is declared and able to carry messages.
#[must_use]
pub fn is_available(name: &str) -> bool {
    held().channels.contains_key(name)
}

/// Where a channel is in its life: `open` with a stream, `registered` without, `closed` when
/// it is not declared.
#[must_use]
pub fn get_channel_status(name: &str) -> ChannelStatus {
    let held = held();
    if !held.channels.contains_key(name) {
        return ChannelStatus::Closed;
    }
    if held.streams.values().any(|stream| stream.open && stream.channel == name) {
        ChannelStatus::Open
    } else {
        ChannelStatus::Registered
    }
}

/// Registers a peer in this process, or replaces the one with that mod id.
pub fn register_peer(peer: Peer) -> bool {
    held().peers.insert(peer.mod_id.clone(), peer).is_none()
}

/// Forgets a peer, closes its streams, and answers whether it was there.
pub fn unregister_peer(mod_id: &str) -> bool {
    let mut held = held();
    if held.peers.remove(mod_id).is_none() {
        return false;
    }
    for stream in held.streams.values_mut() {
        if stream.peer == mod_id {
            stream.open = false;
        }
    }
    true
}

/// A registered peer by mod id.
#[must_use]
pub fn get_peer(mod_id: &str) -> Option<Peer> {
    held().peers.get(mod_id).cloned()
}

/// Every peer registered in this process, by mod id.
#[must_use]
pub fn get_peer_mods() -> Vec<String> {
    held().peers.keys().cloned().collect()
}

/// The API version each peer advertises, in mod id order.
#[must_use]
pub fn get_peer_api_versions() -> Vec<(String, u32)> {
    held()
        .peers
        .values()
        .map(|peer| (peer.mod_id.clone(), peer.api_version))
        .collect()
}

/// The capabilities a peer holds.
#[must_use]
pub fn get_peer_capabilities(mod_id: &str) -> Option<Vec<Capability>> {
    held().peers.get(mod_id).map(|peer| peer.capabilities.clone())
}

/// Registers a handler for a channel under this mod's own id, and answers its handle.
pub fn subscribe<F>(channel: &str, handler: F) -> u64
where
    F: Fn(Message) + Send + Sync + 'static,
{
    subscribe_for(&crate::metadata::id(), channel, handler)
}

/// The same, attributed to an explicit owner — how a peer in this process is modelled.
pub fn subscribe_for<F>(owner: &str, channel: &str, handler: F) -> u64
where
    F: Fn(Message) + Send + Sync + 'static,
{
    let mut held = held();
    held.next_sub += 1;
    let id = held.next_sub;
    held.subs
        .push((id, owner.to_owned(), channel.to_owned(), Arc::new(handler)));
    id
}

/// Removes a subscription, and answers whether it was there.
pub fn unsubscribe(id: u64) -> bool {
    let mut held = held();
    let before = held.subs.len();
    held.subs.retain(|(held_id, _, _, _)| *held_id != id);
    held.subs.len() != before
}

fn offer(owner: &str, channel: &str, message: &Message) -> usize {
    let handlers: Vec<Handler> = held()
        .subs
        .iter()
        .filter(|(_, who, on, _)| who == owner && on == channel)
        .map(|(_, _, _, handler)| Arc::clone(handler))
        .collect();
    let mut delivered = 0;
    for handler in handlers {
        handler(message.clone());
        delivered += 1;
    }
    delivered
}

fn check_version(channel: &Channel, peer: &Peer) -> Result<u32, ChannelError> {
    match channel.required_version {
        Some(required) if required != peer.api_version => Err(ChannelError::VersionMismatch {
            required,
            actual: peer.api_version,
        }),
        Some(required) => Ok(required),
        None => Ok(peer.api_version),
    }
}

fn check_codec(codec: Codec, payload: &[u8]) -> Result<(), ChannelError> {
    match codec {
        Codec::Utf8 | Codec::Json => std::str::from_utf8(payload)
            .map(|_| ())
            .map_err(|_| ChannelError::Codec("the bytes are not UTF-8".to_owned())),
        Codec::Raw | Codec::Binary => Ok(()),
    }
}

fn build(channel: &Channel, payload: &[u8], sequence: u64) -> Result<Message, ChannelError> {
    if payload.len() > channel.payload_limit {
        return Err(ChannelError::PayloadTooLarge {
            size: payload.len(),
            limit: channel.payload_limit,
        });
    }
    check_codec(channel.codec, payload)?;
    let framed = crate::packets::decode(&crate::packets::encode(payload))
        .map_err(|error| ChannelError::Framing(error.to_string()))?;
    Ok(Message {
        channel: channel.name.clone(),
        sender: crate::metadata::id(),
        payload: framed,
        sequence,
    })
}

/// Sends one payload to a peer's handler on a channel, and answers how many handlers it
/// reached. Unknown channels and peers, a version mismatch and an over-long payload are refused
/// with their own error.
pub fn send(channel: &str, peer: &str, payload: &[u8]) -> Result<usize, ChannelError> {
    let channel = get(channel).ok_or_else(|| ChannelError::UnknownChannel(channel.to_owned()))?;
    let target = get_peer(peer).ok_or_else(|| ChannelError::UnknownPeer(peer.to_owned()))?;
    check_version(&channel, &target)?;
    let message = build(&channel, payload, 0)?;
    Ok(offer(peer, &channel.name, &message))
}

/// Sends one payload to every registered peer whose version the channel accepts, once each,
/// and answers the total number of handlers reached.
pub fn broadcast(channel: &str, payload: &[u8]) -> Result<usize, ChannelError> {
    let channel = get(channel).ok_or_else(|| ChannelError::UnknownChannel(channel.to_owned()))?;
    let message = build(&channel, payload, 0)?;
    let peers: Vec<Peer> = held().peers.values().cloned().collect();
    let mut delivered = 0;
    for peer in peers {
        if check_version(&channel, &peer).is_err() {
            continue;
        }
        delivered += offer(&peer.mod_id, &channel.name, &message);
    }
    Ok(delivered)
}

/// The API version a channel and a peer agree on, or the mismatch between them.
pub fn negotiate_version(channel: &str, peer: &str) -> Result<u32, ChannelError> {
    let channel = get(channel).ok_or_else(|| ChannelError::UnknownChannel(channel.to_owned()))?;
    let peer = get_peer(peer).ok_or_else(|| ChannelError::UnknownPeer(peer.to_owned()))?;
    check_version(&channel, &peer)
}

/// Opens an ordered stream from a channel to a peer.
pub fn open_stream(channel: &str, peer: &str) -> Result<StreamId, ChannelError> {
    let held_channel =
        get(channel).ok_or_else(|| ChannelError::UnknownChannel(channel.to_owned()))?;
    let held_peer = get_peer(peer).ok_or_else(|| ChannelError::UnknownPeer(peer.to_owned()))?;
    check_version(&held_channel, &held_peer)?;
    let mut held = held();
    held.next_stream += 1;
    let id = held.next_stream;
    held.streams.insert(
        id,
        Stream {
            id,
            channel: channel.to_owned(),
            peer: peer.to_owned(),
            sequence: 0,
            messages: 0,
            open: true,
        },
    );
    Ok(id)
}

/// Sends the next message on a stream, numbering it, and answers its sequence. A closed or
/// unknown stream is refused.
pub fn send_stream(stream: StreamId, payload: &[u8]) -> Result<u64, ChannelError> {
    let (channel_name, peer_name) = {
        let held = held();
        let live = held
            .streams
            .get(&stream)
            .ok_or(ChannelError::UnknownStream(stream))?;
        if !live.open {
            return Err(ChannelError::StreamClosed(stream));
        }
        (live.channel.clone(), live.peer.clone())
    };
    let channel =
        get(&channel_name).ok_or_else(|| ChannelError::UnknownChannel(channel_name.clone()))?;
    let peer = get_peer(&peer_name).ok_or_else(|| ChannelError::UnknownPeer(peer_name.clone()))?;
    check_version(&channel, &peer)?;
    let sequence = held()
        .streams
        .get(&stream)
        .ok_or(ChannelError::UnknownStream(stream))?
        .sequence
        + 1;
    let message = build(&channel, payload, sequence)?;
    let _ = offer(&peer_name, &channel_name, &message);
    if let Some(held_stream) = held().streams.get_mut(&stream) {
        held_stream.sequence = sequence;
        held_stream.messages += 1;
    }
    Ok(sequence)
}

/// Closes a stream so it refuses more messages, and answers whether it was open.
pub fn close_stream(stream: StreamId) -> bool {
    match held().streams.get_mut(&stream) {
        Some(held_stream) if held_stream.open => {
            held_stream.open = false;
            true
        }
        _ => false,
    }
}

/// A stream by handle.
#[must_use]
pub fn get_stream(stream: StreamId) -> Option<Stream> {
    held().streams.get(&stream).cloned()
}

/// Registers a request handler for a channel under this mod's own id, and answers its handle.
pub fn register_request_handler<F>(channel: &str, handler: F) -> u64
where
    F: Fn(&Request) -> Vec<u8> + Send + Sync + 'static,
{
    register_request_handler_for(&crate::metadata::id(), channel, handler)
}

/// The same, attributed to an explicit owner.
pub fn register_request_handler_for<F>(owner: &str, channel: &str, handler: F) -> u64
where
    F: Fn(&Request) -> Vec<u8> + Send + Sync + 'static,
{
    let mut held = held();
    held.next_route += 1;
    let id = held.next_route;
    held.routes
        .push((id, owner.to_owned(), channel.to_owned(), Arc::new(handler)));
    id
}

/// Removes a request handler, and answers whether it was there.
pub fn unregister_request_handler(id: u64) -> bool {
    let mut held = held();
    let before = held.routes.len();
    held.routes.retain(|(held_id, _, _, _)| *held_id != id);
    held.routes.len() != before
}

/// Sends a request to a peer's request handler on a channel and answers its reply. A channel
/// with no handler for that peer is refused.
pub fn send_request(channel: &str, peer: &str, payload: &[u8]) -> Result<Vec<u8>, ChannelError> {
    let held_channel =
        get(channel).ok_or_else(|| ChannelError::UnknownChannel(channel.to_owned()))?;
    let held_peer = get_peer(peer).ok_or_else(|| ChannelError::UnknownPeer(peer.to_owned()))?;
    check_version(&held_channel, &held_peer)?;
    let message = build(&held_channel, payload, 0)?;
    let handler = {
        let held = held();
        held.routes
            .iter()
            .find(|(_, who, on, _)| who == peer && on == channel)
            .map(|(_, _, _, handler)| Arc::clone(handler))
    }
    .ok_or_else(|| ChannelError::NoHandler(channel.to_owned()))?;
    let id = {
        let mut held = held();
        held.next_request += 1;
        let id = held.next_request;
        held.open_requests.insert(id);
        id
    };
    let request = Request {
        id,
        channel: channel.to_owned(),
        sender: crate::metadata::id(),
        payload: message.payload,
    };
    let returned = handler(&request);
    held().open_requests.remove(&id);
    if returned.is_empty() {
        Ok(held().responses.remove(&id).unwrap_or_default())
    } else {
        held().responses.remove(&id);
        Ok(returned)
    }
}

/// Answers the request with that id from inside its handler. `false` when the id is not being
/// handled here.
pub fn send_response(request: u64, payload: &[u8]) -> bool {
    let mut held = held();
    if !held.open_requests.contains(&request) {
        return false;
    }
    held.responses.insert(request, payload.to_vec());
    true
}

/// Why a channel question about the client's own connections has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "player" => {
            "delivering to a player puts the payload on the game's connection to its server, \
             which this host does not hold"
                .to_owned()
        }
        "server" => {
            "relaying through the client's server means writing into the client's own \
             connection, which is inside the client"
                .to_owned()
        }
        "client" => {
            "the client's plugin channels are declared inside the client's connection, and this \
             host cannot read them"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["player", "server", "client"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refuse(what: &str) -> bool {
    crate::warn!(
        "a mod asked this host to reach across the client's connections: {}",
        why_unknown(what)
    );
    false
}

fn refuse_count(what: &str) -> usize {
    crate::warn!(
        "a mod asked this host to reach across the client's connections: {}",
        why_unknown(what)
    );
    0
}

fn refuse_names(what: &str) -> Vec<String> {
    crate::warn!(
        "a mod asked this host to reach across the client's connections: {}",
        why_unknown(what)
    );
    Vec::new()
}

/// Sends to one player's client, on another machine. `false`.
#[must_use]
pub fn send_to_player(_player: &str, _channel: &str, _payload: &[u8]) -> bool {
    refuse("player")
}

/// Sends to every player's client. `0`.
#[must_use]
pub fn send_to_players(_channel: &str, _payload: &[u8]) -> usize {
    refuse_count("server")
}

/// Sends to the server the client is connected to. `false`.
#[must_use]
pub fn send_to_server(_channel: &str, _payload: &[u8]) -> bool {
    refuse("server")
}

/// The client's own plugin channels. Empty.
#[must_use]
pub fn client_channels() -> Vec<String> {
    refuse_names("client")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn serial() -> MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        *held() = Registry::default();
    }

    #[test]
    fn a_payload_reaches_its_peers_handler_byte_for_byte() {
        let _serial = serial();
        fresh();
        assert!(register(Channel::new("hello:telemetry")));
        assert!(register_peer(Peer::new("hello:alpha", 1)));
        assert!(register_peer(Peer::new("hello:beta", 1)));

        let got = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
        let into = Arc::clone(&got);
        let _sub = subscribe_for("hello:beta", "hello:telemetry", move |msg| {
            assert_eq!(msg.channel, "hello:telemetry");
            assert!(!msg.sender.is_empty(), "the sender is recorded");
            into.lock().unwrap().push(msg.payload);
        });

        let payload = [0x00u8, 0x01, 0xff, 0x7f];
        assert_eq!(send("hello:telemetry", "hello:beta", &payload), Ok(1));
        assert_eq!(*got.lock().unwrap(), vec![payload.to_vec()]);
        assert_eq!(
            send("hello:telemetry", "hello:alpha", &payload),
            Ok(0),
            "alpha subscribed to nothing"
        );
    }

    #[test]
    fn the_payload_limit_is_a_real_boundary() {
        let _serial = serial();
        fresh();
        assert!(register(Channel::new("hello:tiny").payload_limit(8)));
        assert_eq!(get_payload_limit("hello:tiny"), Some(8));
        assert!(register_peer(Peer::new("hello:beta", 1)));
        let _sub = subscribe_for("hello:beta", "hello:tiny", |_msg| {});

        assert_eq!(
            send("hello:tiny", "hello:beta", &[0u8; 8]),
            Ok(1),
            "exactly at the limit is allowed"
        );
        assert_eq!(
            send("hello:tiny", "hello:beta", &[0u8; 9]),
            Err(ChannelError::PayloadTooLarge { size: 9, limit: 8 })
        );
        assert!(set_payload_limit("hello:tiny", 16));
        assert_eq!(send("hello:tiny", "hello:beta", &[0u8; 9]), Ok(1), "now it fits");
        assert!(!set_payload_limit("hello:nothing", 16));
    }

    #[test]
    fn a_message_to_an_unknown_peer_or_channel_is_refused() {
        let _serial = serial();
        fresh();
        assert!(register(Channel::new("hello:telemetry")));
        assert_eq!(
            send("hello:telemetry", "hello:ghost", b"x"),
            Err(ChannelError::UnknownPeer("hello:ghost".to_owned()))
        );
        assert!(register_peer(Peer::new("hello:beta", 1)));
        assert_eq!(
            send("hello:none", "hello:beta", b"x"),
            Err(ChannelError::UnknownChannel("hello:none".to_owned()))
        );
    }

    #[test]
    fn a_stream_carries_ordered_messages_then_refuses_after_it_closes() {
        let _serial = serial();
        fresh();
        register(Channel::new("hello:link"));
        register_peer(Peer::new("hello:beta", 1));
        let seen = Arc::new(Mutex::new(Vec::<(u64, Vec<u8>)>::new()));
        let into = Arc::clone(&seen);
        subscribe_for("hello:beta", "hello:link", move |msg| {
            into.lock().unwrap().push((msg.sequence, msg.payload));
        });

        assert_eq!(get_channel_status("hello:link"), ChannelStatus::Registered);
        let stream = open_stream("hello:link", "hello:beta").expect("a stream");
        assert_eq!(get_channel_status("hello:link"), ChannelStatus::Open);
        assert_eq!(send_stream(stream, b"one"), Ok(1));
        assert_eq!(send_stream(stream, b"two"), Ok(2));
        assert_eq!(send_stream(stream, b"three"), Ok(3));
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                (1, b"one".to_vec()),
                (2, b"two".to_vec()),
                (3, b"three".to_vec()),
            ]
        );
        assert_eq!(get_stream(stream).map(|held| held.messages), Some(3));

        assert!(close_stream(stream));
        assert!(!close_stream(stream));
        assert_eq!(send_stream(stream, b"four"), Err(ChannelError::StreamClosed(stream)));
        assert_eq!(get_channel_status("hello:link"), ChannelStatus::Registered);

        assert!(unregister("hello:link"));
        assert_eq!(get_channel_status("hello:link"), ChannelStatus::Closed);
        assert_eq!(send_stream(stream, b"five"), Err(ChannelError::StreamClosed(stream)));
    }

    #[test]
    fn broadcast_delivers_once_per_peer() {
        let _serial = serial();
        fresh();
        register(Channel::new("hello:ping"));
        let count = Arc::new(AtomicU64::new(0));
        for name in ["hello:a", "hello:b", "hello:c"] {
            assert!(register_peer(Peer::new(name, 1)));
            let into = Arc::clone(&count);
            subscribe_for(name, "hello:ping", move |_msg| {
                into.fetch_add(1, Ordering::SeqCst);
            });
        }
        assert_eq!(broadcast("hello:ping", b"hey"), Ok(3));
        assert_eq!(count.load(Ordering::SeqCst), 3);
        assert_eq!(
            broadcast("hello:none", b"hey"),
            Err(ChannelError::UnknownChannel("hello:none".to_owned()))
        );
    }

    #[test]
    fn a_version_mismatch_on_the_channel_is_reported() {
        let _serial = serial();
        fresh();
        assert!(register(Channel::new("hello:wire").require_version(2)));
        assert!(register_peer(Peer::new("hello:new", 3)));
        assert!(register_peer(Peer::new("hello:old", 2)));
        assert_eq!(
            get_peer_api_versions(),
            vec![("hello:new".to_owned(), 3), ("hello:old".to_owned(), 2)]
        );
        subscribe_for("hello:old", "hello:wire", |_msg| {});
        subscribe_for("hello:new", "hello:wire", |_msg| {});

        assert_eq!(negotiate_version("hello:wire", "hello:old"), Ok(2));
        assert_eq!(
            negotiate_version("hello:wire", "hello:new"),
            Err(ChannelError::VersionMismatch { required: 2, actual: 3 })
        );
        assert_eq!(send("hello:wire", "hello:old", b"x"), Ok(1));
        assert_eq!(
            send("hello:wire", "hello:new", b"x"),
            Err(ChannelError::VersionMismatch { required: 2, actual: 3 })
        );
    }

    #[test]
    fn a_request_is_answered_by_the_peers_handler() {
        let _serial = serial();
        fresh();
        register(Channel::new("hello:rfq"));
        assert!(register_peer(Peer::new("hello:beta", 1)));
        assert!(register_peer(Peer::new("hello:delta", 1)));
        let _responder = register_request_handler_for("hello:beta", "hello:rfq", |request| {
            assert_eq!(request.payload, b"ping");
            assert!(send_response(request.id, b"pong"));
            Vec::new()
        });
        assert_eq!(send_request("hello:rfq", "hello:beta", b"ping"), Ok(b"pong".to_vec()));
        assert_eq!(
            send_request("hello:rfq", "hello:gamma", b"ping"),
            Err(ChannelError::UnknownPeer("hello:gamma".to_owned()))
        );
        assert_eq!(
            send_request("hello:rfq", "hello:delta", b"ping"),
            Err(ChannelError::NoHandler("hello:rfq".to_owned()))
        );
        let _direct = register_request_handler_for("hello:delta", "hello:rfq", |_req| b"ack".to_vec());
        assert_eq!(send_request("hello:rfq", "hello:delta", b"ping"), Ok(b"ack".to_vec()));
        assert!(!send_response(999, b"late"), "no request 999 is open");
    }

    #[test]
    fn channels_are_registered_read_back_and_carry_a_codec() {
        let _serial = serial();
        fresh();
        assert!(register(
            Channel::new("hello:a").payload_limit(100).require_version(4)
        ));
        assert!(
            !register(Channel::new("hello:a").payload_limit(100).require_version(4)),
            "it replaces"
        );
        assert!(register(Channel::new("hello:b")));
        assert_eq!(
            get("hello:a").expect("registered").required_version,
            Some(4)
        );
        assert_eq!(get("hello:a").expect("registered").payload_limit, 100);
        assert_eq!(get_all().len(), 2);
        assert!(is_available("hello:a"));
        assert!(!is_available("hello:missing"));
        assert_eq!(get_codec("hello:a"), Some(Codec::Raw));
        assert!(register_codec("hello:a", Codec::Utf8));
        assert_eq!(get_codec("hello:a"), Some(Codec::Utf8));
        assert!(!register_codec("hello:missing", Codec::Utf8));
        assert!(unregister("hello:b"));
        assert!(!unregister("hello:b"));
        assert_eq!(get("hello:b"), None);
        assert_eq!(get_all().len(), 1);

        assert!(register(Channel::new("hello:text").codec(Codec::Utf8)));
        assert!(register_peer(Peer::new("hello:beta", 1)));
        let _sub = subscribe_for("hello:beta", "hello:text", |_msg| {});
        assert_eq!(send("hello:text", "hello:beta", "héllo".as_bytes()), Ok(1));
        assert!(matches!(
            send("hello:text", "hello:beta", &[0xff, 0xfe]),
            Err(ChannelError::Codec(_))
        ));
    }

    #[test]
    fn every_refusal_answers_honestly_with_its_own_reason() {
        let _serial = serial();
        fresh();
        assert!(!send_to_player("Steve", "hello:t", b"x"));
        assert_eq!(send_to_players("hello:t", b"x"), 0);
        assert!(!send_to_server("hello:t", b"x"));
        assert!(client_channels().is_empty());

        assert_eq!(refusals().len(), 3);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert_ne!(why_unknown("player"), why_unknown("server"));
        assert_ne!(why_unknown("server"), why_unknown("client"));
        assert!(why_unknown("something else").contains("not something"));
    }
}
