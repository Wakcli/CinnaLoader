//! Packets: a real binary codec and a real dispatch path, and the client's protocol refused.
//!
//! A packet here is bytes, and bytes are something this host can really handle, so what a mod
//! builds on is genuinely real:
//!
//! * **A registry** of packet types: a numeric id, a name, a direction and a protocol state,
//!   read back by id and by name. Registering a duplicate id or name is refused.
//! * **A binary [`Buffer`]**: a read cursor with explicit errors, writing and reading `u8`,
//!   `u16`, `u32`, `u64`, `i32`, `f32`, `f64`, `bool`, a varint (the game's 7-bit continuation
//!   form), a length-prefixed UTF-8 string and a raw byte block. Reading past the end is an
//!   error, never a panic and never a silent zero.
//! * **Framing** shared with the caller: [`encode`] puts a length prefix on a payload and
//!   [`decode`] takes it off, refusing a truncated frame rather than guessing.
//! * **A handler registry and a dispatch path**: a handler is registered per packet type and
//!   optionally per direction, the caller [`feed`]s bytes in, and the module decodes and
//!   dispatches in registration order, answering how many handlers ran and recording what was
//!   dispatched for [`get_trace`].
//!
//! What is *not* real is the client's own protocol. This host cannot see the client's
//! connection, so intercepting, reading, rewriting or cancelling a packet the client sends or
//! receives, claiming one of its ids, or sending into its transport is refused with its own
//! sentence in [`why_unknown`] and listed by [`refusals`] — never a fake success.
//!
//! ```
//! use cinnaloader::packets::{self, Buffer, Direction, Packet};
//!
//! let mut buf = Buffer::new();
//! buf.write_u32(7);
//! buf.write_varint(300);
//! buf.write_string("hello");
//! let frame = packets::encode(buf.bytes());
//! assert_eq!(packets::decode(&frame).expect("a whole frame"), buf.bytes());
//!
//! packets::register(Packet::new(7, "hello:ping").direction(Direction::ToServer))
//!     .expect("a free id");
//! assert_eq!(packets::get_packet_id("hello:ping"), Some(7));
//! let _handler = packets::register_handler(7, None, |packet| {
//!     packet.metadata.insert("seen".to_owned(), "yes".to_owned());
//!     true
//! });
//! assert_eq!(packets::feed(7, &frame).expect("a whole frame"), 1);
//! assert_eq!(packets::get_trace().len(), 1);
//! # assert!(packets::unregister(7));
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::events;

/// The largest payload this host will frame or accept, so a hostile length prefix cannot make
/// it allocate without bound.
pub const MAX_PAYLOAD: usize = 2 * 1024 * 1024;

/// Which way a packet type travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    ToServer,
    ToClient,
    Both,
}

impl Direction {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ToServer => "to_server",
            Self::ToClient => "to_client",
            Self::Both => "both",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "to_server" => Some(Self::ToServer),
            "to_client" => Some(Self::ToClient),
            "both" => Some(Self::Both),
            _ => None,
        }
    }

    /// Whether a handler registered for `self` should see a packet going `other`'s way.
    #[must_use]
    pub fn matches(self, other: Self) -> bool {
        self == Self::Both || other == Self::Both || self == other
    }
}

impl fmt::Display for Direction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The protocol state a packet belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PacketState {
    Handshake,
    Status,
    Login,
    #[default]
    Play,
}

impl PacketState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Handshake => "handshake",
            Self::Status => "status",
            Self::Login => "login",
            Self::Play => "play",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "handshake" => Some(Self::Handshake),
            "status" => Some(Self::Status),
            "login" => Some(Self::Login),
            "play" => Some(Self::Play),
            _ => None,
        }
    }
}

impl fmt::Display for PacketState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One packet type, and optionally the bytes of one packet of that type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub id: u32,
    pub name: String,
    pub direction: Direction,
    pub state: PacketState,
    pub payload: Vec<u8>,
    pub metadata: BTreeMap<String, String>,
}

impl Packet {
    #[must_use]
    pub fn new(id: u32, name: &str) -> Self {
        Self {
            id,
            name: name.to_owned(),
            direction: Direction::Both,
            state: PacketState::Play,
            payload: Vec::new(),
            metadata: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn direction(mut self, direction: Direction) -> Self {
        self.direction = direction;
        self
    }

    #[must_use]
    pub fn state(mut self, state: PacketState) -> Self {
        self.state = state;
        self
    }

    #[must_use]
    pub fn payload(mut self, payload: &[u8]) -> Self {
        self.payload = payload.to_vec();
        self
    }

    #[must_use]
    pub fn metadata(mut self, key: &str, value: &str) -> Self {
        self.metadata.insert(key.to_owned(), value.to_owned());
        self
    }

    /// The size of the payload in bytes.
    #[must_use]
    pub fn size(&self) -> usize {
        self.payload.len()
    }
}

/// Anything that can go wrong reading bytes or naming a packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PacketError {
    OutOfBounds { wanted: usize, remaining: usize },
    VarintTooLong,
    InvalidUtf8,
    Truncated { declared: usize, available: usize },
    DuplicateId(u32),
    DuplicateName(String),
    UnknownPacket(u32),
    Malformed(String),
}

impl fmt::Display for PacketError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfBounds { wanted, remaining } => {
                write!(formatter, "need {wanted} bytes, {remaining} left")
            }
            Self::VarintTooLong => formatter.write_str("varint runs past the fifth continuation byte"),
            Self::InvalidUtf8 => formatter.write_str("length-prefixed bytes are not UTF-8"),
            Self::Truncated { declared, available } => {
                write!(formatter, "frame declares {declared} bytes, {available} present")
            }
            Self::DuplicateId(id) => write!(formatter, "packet id {id} is already registered"),
            Self::DuplicateName(name) => write!(formatter, "packet name {name} is already registered"),
            Self::UnknownPacket(id) => write!(formatter, "packet id {id} is not registered"),
            Self::Malformed(what) => write!(formatter, "malformed packet: {what}"),
        }
    }
}

impl std::error::Error for PacketError {}

/// A byte buffer with a read cursor, big-endian fields and the game's varint form.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Buffer {
    bytes: Vec<u8>,
    position: usize,
}

impl Buffer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity),
            position: 0,
        }
    }

    #[must_use]
    pub fn from_slice(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.to_vec(),
            position: 0,
        }
    }

    #[must_use]
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self { bytes, position: 0 }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    #[must_use]
    pub fn position(&self) -> usize {
        self.position
    }

    #[must_use]
    pub fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Puts the cursor back at the start, keeping the bytes.
    pub fn rewind(&mut self) {
        self.position = 0;
    }

    /// Throws the bytes away and starts over.
    pub fn clear(&mut self) {
        self.bytes.clear();
        self.position = 0;
    }

    pub fn seek(&mut self, position: usize) -> Result<(), PacketError> {
        if position > self.bytes.len() {
            return Err(PacketError::OutOfBounds {
                wanted: position,
                remaining: self.bytes.len(),
            });
        }
        self.position = position;
        Ok(())
    }

    fn push(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
        self.position = self.bytes.len();
    }

    fn take(&mut self, count: usize) -> Result<&[u8], PacketError> {
        let remaining = self.bytes.len() - self.position;
        if count > remaining {
            return Err(PacketError::OutOfBounds {
                wanted: count,
                remaining,
            });
        }
        let start = self.position;
        self.position += count;
        Ok(&self.bytes[start..start + count])
    }

    pub fn write_u8(&mut self, value: u8) {
        self.push(&[value]);
    }

    pub fn write_u16(&mut self, value: u16) {
        self.push(&value.to_be_bytes());
    }

    pub fn write_u32(&mut self, value: u32) {
        self.push(&value.to_be_bytes());
    }

    pub fn write_u64(&mut self, value: u64) {
        self.push(&value.to_be_bytes());
    }

    pub fn write_i32(&mut self, value: i32) {
        self.push(&value.to_be_bytes());
    }

    pub fn write_f32(&mut self, value: f32) {
        self.push(&value.to_bits().to_be_bytes());
    }

    pub fn write_f64(&mut self, value: f64) {
        self.push(&value.to_bits().to_be_bytes());
    }

    pub fn write_bool(&mut self, value: bool) {
        self.push(&[u8::from(value)]);
    }

    /// A UTF-8 string with a varint length in front of it, the way the game frames one.
    pub fn write_string(&mut self, text: &str) {
        self.write_varint(text.len() as u32);
        self.push(text.as_bytes());
    }

    /// A 7-bit continuation varint: `0x80` set means another byte follows.
    pub fn write_varint(&mut self, value: u32) {
        let mut value = value;
        while value & !0x7f != 0 {
            self.push(&[(value as u8 & 0x7f) | 0x80]);
            value >>= 7;
        }
        self.push(&[value as u8]);
    }

    /// A raw byte block with no length in front of it; the reader supplies the count.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.push(bytes);
    }

    /// A byte block with a varint length in front of it, so it can be read back.
    pub fn write_block(&mut self, bytes: &[u8]) {
        self.write_varint(bytes.len() as u32);
        self.push(bytes);
    }

    pub fn read_u8(&mut self) -> Result<u8, PacketError> {
        Ok(self.take(1)?[0])
    }

    pub fn read_u16(&mut self) -> Result<u16, PacketError> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    pub fn read_u32(&mut self) -> Result<u32, PacketError> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub fn read_u64(&mut self) -> Result<u64, PacketError> {
        let bytes = self.take(8)?;
        Ok(u64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    pub fn read_i32(&mut self) -> Result<i32, PacketError> {
        let bytes = self.take(4)?;
        Ok(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub fn read_f32(&mut self) -> Result<f32, PacketError> {
        Ok(f32::from_bits(self.read_u32()?))
    }

    pub fn read_f64(&mut self) -> Result<f64, PacketError> {
        Ok(f64::from_bits(self.read_u64()?))
    }

    pub fn read_bool(&mut self) -> Result<bool, PacketError> {
        Ok(self.read_u8()? != 0)
    }

    pub fn read_string(&mut self) -> Result<String, PacketError> {
        let length = self.read_varint()? as usize;
        let bytes = self.take(length)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| PacketError::InvalidUtf8)
    }

    pub fn read_varint(&mut self) -> Result<u32, PacketError> {
        let mut value = 0u32;
        for shift in 0..5 {
            let byte = self.read_u8()?;
            let part = u32::from(byte & 0x7f);
            // The fifth byte carries only the top four bits of a u32.
            if shift == 4 && part > 0x0f {
                return Err(PacketError::VarintTooLong);
            }
            value |= part << (7 * shift);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(PacketError::VarintTooLong)
    }

    pub fn read_bytes(&mut self, count: usize) -> Result<Vec<u8>, PacketError> {
        Ok(self.take(count)?.to_vec())
    }

    pub fn read_block(&mut self) -> Result<Vec<u8>, PacketError> {
        let length = self.read_varint()? as usize;
        Ok(self.take(length)?.to_vec())
    }
}

/// Puts a length prefix on a payload: the whole frame a caller hands to [`decode`].
#[must_use]
pub fn encode(payload: &[u8]) -> Vec<u8> {
    let mut frame = Buffer::new();
    frame.write_varint(payload.len() as u32);
    frame.write_bytes(payload);
    frame.into_bytes()
}

/// Takes the length prefix off a frame, refusing one that declares more than it carries.
pub fn decode(frame: &[u8]) -> Result<Vec<u8>, PacketError> {
    let mut buf = Buffer::from_slice(frame);
    let declared = buf.read_varint()? as usize;
    let available = buf.remaining();
    if declared > available {
        return Err(PacketError::Truncated { declared, available });
    }
    buf.read_bytes(declared)
}

/// One packet's text form: id, name, direction, state, payload as hex, then metadata.
#[must_use]
pub fn serialize(packet: &Packet) -> String {
    let payload = if packet.payload.is_empty() {
        "-".to_owned()
    } else {
        hex_encode(&packet.payload)
    };
    let mut line = format!(
        "{} {} {} {} {}",
        packet.id,
        packet.name,
        packet.direction.as_str(),
        packet.state.as_str(),
        payload
    );
    for (key, value) in &packet.metadata {
        line.push(' ');
        line.push_str(key);
        line.push('=');
        line.push_str(value);
    }
    line
}

/// Reads back what [`serialize`] wrote.
pub fn deserialize(text: &str) -> Result<Packet, PacketError> {
    let malformed = |what: &str| PacketError::Malformed(format!("{what} in {text:?}"));
    let mut tokens = text.split_whitespace();
    let id = tokens
        .next()
        .and_then(|token| token.parse::<u32>().ok())
        .ok_or_else(|| malformed("no numeric id"))?;
    let name = tokens
        .next()
        .ok_or_else(|| malformed("no name"))?
        .to_owned();
    let direction = tokens
        .next()
        .and_then(Direction::parse)
        .ok_or_else(|| malformed("no known direction"))?;
    let state = tokens
        .next()
        .and_then(PacketState::parse)
        .ok_or_else(|| malformed("no known state"))?;
    let payload_field = tokens.next().ok_or_else(|| malformed("no payload"))?;
    let payload = if payload_field == "-" {
        Vec::new()
    } else {
        hex_decode(payload_field).ok_or_else(|| malformed("payload is not hex"))?
    };
    let mut metadata = BTreeMap::new();
    for token in tokens {
        let (key, value) = token
            .split_once('=')
            .ok_or_else(|| malformed("metadata is not key=value"))?;
        metadata.insert(key.to_owned(), value.to_owned());
    }
    Ok(Packet {
        id,
        name,
        direction,
        state,
        payload,
        metadata,
    })
}

/// A copy of a packet, which is the wishlist's name for [`Clone`].
#[must_use]
pub fn clone(packet: &Packet) -> Packet {
    packet.clone()
}

/// What is wrong with a packet, if anything: a name the text form can carry, a sane size.
#[must_use]
pub fn validate(packet: &Packet) -> Vec<String> {
    let mut problems = Vec::new();
    if packet.name.is_empty() {
        problems.push(format!("packet {} has no name", packet.id));
    }
    if packet.name.chars().any(char::is_whitespace) {
        problems.push(format!("packet name {:?} contains whitespace", packet.name));
    }
    for (key, value) in &packet.metadata {
        if key.chars().any(char::is_whitespace) || value.chars().any(char::is_whitespace) {
            problems.push(format!(
                "packet {} metadata {key:?} contains whitespace",
                packet.id
            ));
        }
    }
    if packet.payload.len() > MAX_PAYLOAD {
        problems.push(format!(
            "packet {} payload is {} bytes, over the {MAX_PAYLOAD}-byte maximum",
            packet.id,
            packet.payload.len()
        ));
    }
    problems
}

/// One line describing a packet, for a log or a panel.
#[must_use]
pub fn inspect(packet: &Packet) -> String {
    format!(
        "packet {} {} ({}, {}, {} bytes, {} metadata)",
        packet.id,
        packet.name,
        packet.direction,
        packet.state,
        packet.size(),
        packet.metadata.len()
    )
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    out
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let digit = |byte: u8| (byte as char).to_digit(16).map(|value| value as u8);
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks(2) {
        out.push((digit(pair[0])? << 4) | digit(pair[1])?);
    }
    Some(out)
}

/// A codec a mod names for a packet type it has registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Codec {
    pub packet: u32,
    pub name: String,
}

impl Codec {
    #[must_use]
    pub fn new(packet: u32, name: &str) -> Self {
        Self {
            packet,
            name: name.to_owned(),
        }
    }
}

/// What one [`dispatch`] did, kept so a caller can assert on it.
#[derive(Debug, Clone, PartialEq)]
pub struct TraceEntry {
    pub packet: Packet,
    pub handlers: usize,
}

type Handler = Box<dyn FnMut(&mut Packet) -> bool + Send + 'static>;

struct Held {
    by_id: BTreeMap<u32, Packet>,
    by_name: BTreeMap<String, u32>,
    codecs: BTreeMap<u32, Codec>,
    handlers: Vec<(u64, u32, Option<Direction>, Handler)>,
    next_handler: u64,
    trace: Vec<TraceEntry>,
    tracing: bool,
}

impl Default for Held {
    fn default() -> Self {
        Self {
            by_id: BTreeMap::new(),
            by_name: BTreeMap::new(),
            codecs: BTreeMap::new(),
            handlers: Vec::new(),
            next_handler: 1,
            trace: Vec::new(),
            tracing: true,
        }
    }
}

fn held() -> &'static Mutex<Held> {
    static HELD: OnceLock<Mutex<Held>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(Held::default()))
}

fn with<T>(body: impl FnOnce(&mut Held) -> T) -> T {
    let mut guard = held().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Declares a packet type. A duplicate id or name is refused, not overwritten.
pub fn register(packet: Packet) -> Result<(), PacketError> {
    with(|held| {
        if held.by_id.contains_key(&packet.id) {
            return Err(PacketError::DuplicateId(packet.id));
        }
        if held.by_name.contains_key(&packet.name) {
            return Err(PacketError::DuplicateName(packet.name.clone()));
        }
        held.by_name.insert(packet.name.clone(), packet.id);
        held.by_id.insert(packet.id, packet);
        Ok(())
    })
}

/// Forgets a packet type, its codec and its handlers, and answers whether it was there.
pub fn unregister(id: u32) -> bool {
    with(|held| match held.by_id.remove(&id) {
        Some(packet) => {
            held.by_name.remove(&packet.name);
            held.codecs.remove(&id);
            held.handlers.retain(|(_, packet, _, _)| *packet != id);
            true
        }
        None => false,
    })
}

/// Every declared packet type, in id order.
#[must_use]
pub fn get_registry() -> Vec<Packet> {
    with(|held| held.by_id.values().cloned().collect())
}

/// A declared packet type by its numeric id.
#[must_use]
pub fn get_packet_type(id: u32) -> Option<Packet> {
    with(|held| held.by_id.get(&id).cloned())
}

/// The id a name was registered under.
#[must_use]
pub fn get_packet_id(name: &str) -> Option<u32> {
    with(|held| held.by_name.get(name).copied())
}

/// The direction a declared type travels.
#[must_use]
pub fn get_direction(id: u32) -> Option<Direction> {
    get_packet_type(id).map(|packet| packet.direction)
}

/// The protocol state a declared type belongs to.
#[must_use]
pub fn get_state(id: u32) -> Option<PacketState> {
    get_packet_type(id).map(|packet| packet.state)
}

/// The payload size of a declared type.
#[must_use]
pub fn get_size(id: u32) -> Option<usize> {
    get_packet_type(id).map(|packet| packet.size())
}

/// The metadata of a declared type.
#[must_use]
pub fn get_metadata(id: u32) -> Option<BTreeMap<String, String>> {
    get_packet_type(id).map(|packet| packet.metadata)
}

/// Whether this host's registry knows a packet id, which is all "supports" can mean here.
#[must_use]
pub fn supports_packet(id: u32) -> bool {
    with(|held| held.by_id.contains_key(&id))
}

/// Names a codec for a packet type that is registered. An unknown type is refused.
pub fn register_codec(codec: Codec) -> bool {
    with(|held| {
        if !held.by_id.contains_key(&codec.packet) {
            return false;
        }
        held.codecs.insert(codec.packet, codec).is_none()
    })
}

/// The codec named for a packet type.
#[must_use]
pub fn get_codec(packet: u32) -> Option<Codec> {
    with(|held| held.codecs.get(&packet).cloned())
}

/// Registers a handler for a packet type, optionally only for one direction. The handler
/// returns whether it consumed the packet, which stops the rest of the chain.
pub fn register_handler<F>(packet: u32, direction: Option<Direction>, handler: F) -> u64
where
    F: FnMut(&mut Packet) -> bool + Send + 'static,
{
    with(|held| {
        let id = held.next_handler;
        held.next_handler += 1;
        held.handlers.push((id, packet, direction, Box::new(handler)));
        id
    })
}

/// Removes a handler, and answers whether it was there.
pub fn unregister_handler(id: u64) -> bool {
    with(|held| {
        let before = held.handlers.len();
        held.handlers.retain(|(held, _, _, _)| *held != id);
        held.handlers.len() != before
    })
}

/// Runs every handler for a packet's type and direction, in registration order, and answers
/// how many ran. A handler that returns `true` consumed the packet and stops the chain.
#[must_use]
pub fn dispatch(packet: &mut Packet) -> usize {
    with(|held| {
        let mut ran = 0;
        for (_, packet_id, wanted, handler) in &mut held.handlers {
            if *packet_id != packet.id {
                continue;
            }
            if wanted.is_some_and(|wanted| !wanted.matches(packet.direction)) {
                continue;
            }
            ran += 1;
            if handler(packet) {
                break;
            }
        }
        if held.tracing {
            held.trace.push(TraceEntry {
                packet: packet.clone(),
                handlers: ran,
            });
        }
        ran
    })
}

/// The path a caller really has: it hands bytes in, the module decodes the frame, builds the
/// registered type and dispatches.
pub fn feed(packet: u32, frame: &[u8]) -> Result<usize, PacketError> {
    let payload = decode(frame)?;
    let mut packet = with(|held| held.by_id.get(&packet).cloned())
        .ok_or(PacketError::UnknownPacket(packet))?;
    packet.payload = payload;
    Ok(dispatch(&mut packet))
}

/// What has been dispatched, so a caller can assert on it.
#[must_use]
pub fn get_trace() -> Vec<TraceEntry> {
    with(|held| held.trace.clone())
}

/// Turns recording on or off, and answers whether it was on before.
pub fn set_trace_enabled(enabled: bool) -> bool {
    with(|held| std::mem::replace(&mut held.tracing, enabled))
}

/// Forgets the trace, and answers how many entries it dropped.
pub fn clear_trace() -> usize {
    with(|held| std::mem::take(&mut held.trace).len())
}

/// The trace as text, one [`serialize`]d packet per line.
#[must_use]
pub fn export_recording() -> String {
    with(|held| {
        held.trace
            .iter()
            .map(|entry| serialize(&entry.packet))
            .collect::<Vec<_>>()
            .join("\n")
    })
}

/// Decodes a recording and dispatches each packet again, answering how many it replayed.
pub fn replay_recording(recording: &str) -> Result<usize, PacketError> {
    let mut replayed = 0;
    for line in recording.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut packet = deserialize(line)?;
        let _ran = dispatch(&mut packet);
        replayed += 1;
    }
    Ok(replayed)
}

/// Why a packet question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "intercept" => {
            "observing the client's packet flow means intercepting its protocol, and this host \
             never sees the client's connection"
                .to_owned()
        }
        "read" => {
            "the client decodes its own packets as the connection runs; this host has no handle \
             on that traffic, so it cannot read it"
                .to_owned()
        }
        "rewrite" => {
            "rewriting a packet the client sends or receives would desynchronise it from the \
             server, so it is refused"
                .to_owned()
        }
        "cancel" => {
            "cancelling a packet in the client's protocol would desynchronise it, and this host \
             has no handle on that flow"
                .to_owned()
        }
        "claim" => {
            "claiming an id in the client's protocol would hand a mod an id the client already \
             uses, and this host cannot see those ids"
                .to_owned()
        }
        "send" => {
            "sending into the client's transport would put a packet on the wire as the client, \
             which this host cannot do"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach about packets.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["intercept", "read", "rewrite", "cancel", "claim", "send"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refused(what: &str) -> bool {
    crate::warn!(
        "a mod asked this host to touch the client's packets: {}",
        why_unknown(what)
    );
    false
}

/// Cancelling a packet in the client's flow. `false`, with [`why_unknown`].
pub fn cancel(_packet: u32) -> bool {
    refused("cancel")
}

/// Replacing a packet the client handles. `false`.
pub fn replace(_packet: &Packet) -> bool {
    refused("rewrite")
}

/// Rewriting a packet in place. `false`.
pub fn modify(_packet: &mut Packet) -> bool {
    refused("rewrite")
}

/// Sending a packet as the client. `false`.
pub fn send(_packet: &Packet) -> bool {
    refused("send")
}

/// Sending to the server as the client. `false`.
pub fn send_to_server(_packet: &Packet) -> bool {
    refused("send")
}

/// Sending to the client. `false`.
pub fn send_to_client(_packet: &Packet) -> bool {
    refused("send")
}

/// Sending to everyone. `false`.
pub fn send_to_all(_packet: &Packet) -> bool {
    refused("send")
}

/// Watching the client's packet flow. `false`.
pub fn set_observer<F>(_observer: F) -> bool
where
    F: FnMut(&Packet) + Send + 'static,
{
    refused("intercept")
}

/// Claims an id in the client's protocol. `false`.
pub fn register_protocol_mapping(_name: &str, _id: u32) -> bool {
    refused("claim")
}

/// The client's unknown packets. Empty, with [`why_unknown`].
#[must_use]
pub fn get_unknown_packets() -> Vec<u32> {
    crate::warn!(
        "a mod asked this host to touch the client's packets: {}",
        why_unknown("read")
    );
    Vec::new()
}

/// The client's protocol version. `None`.
#[must_use]
pub fn get_protocol_version() -> Option<u32> {
    crate::warn!(
        "a mod asked this host to touch the client's packets: {}",
        why_unknown("read")
    );
    None
}

/// A name's id under the client's protocol. `None`.
#[must_use]
pub fn get_protocol_mapping(_name: &str) -> Option<u32> {
    crate::warn!(
        "a mod asked this host to touch the client's packets: {}",
        why_unknown("read")
    );
    None
}

/// The packet events this host would have to receive from the client to fire.
pub const PACKET_EVENTS: [&str; 12] = [
    "packet_receive",
    "packet_received",
    "packet_decode",
    "packet_encode",
    "packet_send",
    "packet_sent",
    "packet_cancelled",
    "packet_modified",
    "connection_open",
    "connection_close",
    "connection_timeout",
    "protocol_negotiation",
];

/// Teaches the event bus each packet event name.
pub fn register_event_kinds() -> usize {
    PACKET_EVENTS
        .iter()
        .filter(|name| events::register_event_type(name))
        .count()
}

/// The event kinds this host would need from the client to fire one.
#[must_use]
pub fn event_kinds() -> Vec<&'static str> {
    PACKET_EVENTS.to_vec()
}

/// Whether a packet event kind is one the bus knows.
#[must_use]
pub fn has_event_kind(kind: &str) -> bool {
    PACKET_EVENTS.contains(&kind) && events::is_registered_kind(kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|held| {
            held.by_id.clear();
            held.by_name.clear();
            held.codecs.clear();
            held.handlers.clear();
            held.next_handler = 1;
            held.trace.clear();
            held.tracing = true;
        });
    }

    #[test]
    fn a_registered_type_is_found_both_ways_and_a_duplicate_is_refused() {
        let _serial = serial();
        fresh();
        assert!(register(Packet::new(5, "hello:ping").direction(Direction::ToServer)).is_ok());
        assert_eq!(
            register(Packet::new(5, "hello:other")),
            Err(PacketError::DuplicateId(5))
        );
        assert_eq!(
            register(Packet::new(6, "hello:ping")),
            Err(PacketError::DuplicateName("hello:ping".to_owned()))
        );
        assert_eq!(get_packet_id("hello:ping"), Some(5));
        assert_eq!(get_packet_type(5).expect("declared").name, "hello:ping");
        assert_eq!(get_direction(5), Some(Direction::ToServer));
        assert_eq!(get_state(5), Some(PacketState::Play));
        assert!(supports_packet(5));
        assert!(!supports_packet(99));

        assert!(register(Packet::new(9, "hello:meta").metadata("k", "v")).is_ok());
        assert_eq!(
            get_metadata(9).expect("declared").get("k").map(String::as_str),
            Some("v")
        );
        assert_eq!(get_size(9), Some(0));
        assert_eq!(get_registry().len(), 2);

        assert!(register_codec(Codec::new(5, "hello:codec")));
        assert_eq!(get_codec(5).map(|codec| codec.name), Some("hello:codec".to_owned()));
        assert!(!register_codec(Codec::new(99, "hello:none")));

        assert!(unregister(5));
        assert!(!unregister(5));
        assert_eq!(get_packet_type(5), None);
        assert_eq!(get_packet_id("hello:ping"), None);
    }

    #[test]
    fn every_scalar_round_trips_with_exact_bytes() {
        let mut buf = Buffer::new();
        buf.write_u8(0xab);
        buf.write_u16(0xbeef);
        buf.write_u32(0xdead_beef);
        buf.write_u64(0x0102_0304_0506_0708);
        buf.write_i32(-2);
        buf.write_bool(true);
        buf.write_bool(false);
        assert_eq!(buf.len(), 1 + 2 + 4 + 8 + 4 + 1 + 1);
        assert_eq!(&buf.bytes()[0..1], &[0xab]);
        assert_eq!(&buf.bytes()[1..3], &[0xbe, 0xef]);
        assert_eq!(&buf.bytes()[3..7], &[0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(&buf.bytes()[7..15], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&buf.bytes()[15..19], &[0xff, 0xff, 0xff, 0xfe]);

        buf.rewind();
        assert_eq!(buf.read_u8(), Ok(0xab));
        assert_eq!(buf.read_u16(), Ok(0xbeef));
        assert_eq!(buf.read_u32(), Ok(0xdead_beef));
        assert_eq!(buf.read_u64(), Ok(0x0102_0304_0506_0708));
        assert_eq!(buf.read_i32(), Ok(-2));
        assert_eq!(buf.read_bool(), Ok(true));
        assert_eq!(buf.read_bool(), Ok(false));
        assert_eq!(buf.remaining(), 0);
    }

    #[test]
    fn varints_round_trip_at_their_boundaries() {
        for (value, length) in [
            (0u32, 1usize),
            (127, 1),
            (128, 2),
            (255, 2),
            (16_383, 2),
            (16_384, 3),
            (u32::MAX, 5),
        ] {
            let mut buf = Buffer::new();
            buf.write_varint(value);
            assert_eq!(buf.len(), length, "{value} should take {length} bytes");
            buf.rewind();
            assert_eq!(buf.read_varint(), Ok(value));
        }
        let mut buf = Buffer::new();
        buf.write_varint(300);
        assert_eq!(buf.bytes(), &[0xac, 0x02]);
    }

    #[test]
    fn floats_strings_and_blocks_round_trip() {
        let mut buf = Buffer::new();
        buf.write_f32(-1.5);
        buf.write_f32(f32::MIN_POSITIVE);
        buf.write_f64(1.0e-300);
        buf.write_f64(f64::NEG_INFINITY);
        buf.write_string("");
        buf.write_string("héllo→");
        buf.write_block(&[]);
        buf.write_block(&[0xa5; 300]);
        buf.rewind();

        assert_eq!(buf.read_f32(), Ok(-1.5));
        assert_eq!(buf.read_f32(), Ok(f32::MIN_POSITIVE));
        assert_eq!(buf.read_f64(), Ok(1.0e-300));
        assert_eq!(buf.read_f64(), Ok(f64::NEG_INFINITY));
        assert_eq!(buf.read_string(), Ok(String::new()));
        assert_eq!(buf.read_string(), Ok("héllo→".to_owned()));
        assert_eq!(buf.read_block(), Ok(Vec::new()));
        assert_eq!(buf.read_block(), Ok(vec![0xa5; 300]));
        assert_eq!(buf.remaining(), 0);
    }

    #[test]
    fn the_cursor_advances_by_what_was_written_and_overreads_are_refused() {
        let mut buf = Buffer::new();
        buf.write_u32(1);
        buf.write_varint(300);
        assert_eq!(buf.len(), 6);
        buf.rewind();
        assert_eq!(buf.position(), 0);
        assert_eq!(buf.read_u32(), Ok(1));
        assert_eq!(buf.position(), 4);
        assert_eq!(buf.read_varint(), Ok(300));
        assert_eq!(buf.position(), 6);
        assert_eq!(buf.remaining(), 0);
        assert_eq!(
            buf.read_u8(),
            Err(PacketError::OutOfBounds {
                wanted: 1,
                remaining: 0
            })
        );
    }

    #[test]
    fn a_truncated_frame_and_a_degenerate_varint_are_refused() {
        let frame = encode(&[1, 2, 3, 4, 5]);
        assert_eq!(frame, vec![5, 1, 2, 3, 4, 5]);
        assert_eq!(decode(&frame), Ok(vec![1, 2, 3, 4, 5]));
        assert_eq!(
            decode(&frame[..4]),
            Err(PacketError::Truncated {
                declared: 5,
                available: 3
            })
        );

        let mut bad = Buffer::from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0x01]);
        assert_eq!(bad.read_varint(), Err(PacketError::VarintTooLong));
    }

    #[test]
    fn handlers_run_in_registration_order_for_their_own_type_only() {
        let _serial = serial();
        fresh();
        register(Packet::new(1, "hello:one")).expect("a free id");
        register(Packet::new(2, "hello:two")).expect("a free id");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let into = Arc::clone(&calls);
        let _one = register_handler(1, None, move |_| {
            into.lock().unwrap().push("a");
            false
        });
        let into = Arc::clone(&calls);
        let _two = register_handler(1, None, move |_| {
            into.lock().unwrap().push("b");
            false
        });
        let into = Arc::clone(&calls);
        let _three = register_handler(2, None, move |_| {
            into.lock().unwrap().push("other");
            false
        });

        assert_eq!(feed(1, &encode(b"x")), Ok(2));
        assert_eq!(*calls.lock().unwrap(), vec!["a", "b"]);
        assert_eq!(feed(2, &encode(b"y")), Ok(1));
        assert_eq!(*calls.lock().unwrap(), vec!["a", "b", "other"]);
    }

    #[test]
    fn a_handler_can_consume_a_packet_and_direction_filters_the_chain() {
        let _serial = serial();
        fresh();
        register(Packet::new(3, "hello:three").direction(Direction::ToServer)).expect("a free id");
        let ran = Arc::new(Mutex::new(0));
        let into = Arc::clone(&ran);
        let _first = register_handler(3, None, move |_| {
            *into.lock().unwrap() += 1;
            true
        });
        let into = Arc::clone(&ran);
        let _second = register_handler(3, None, move |_| {
            *into.lock().unwrap() += 1;
            false
        });
        assert_eq!(feed(3, &encode(b"y")), Ok(1));
        assert_eq!(*ran.lock().unwrap(), 1, "the second handler never ran");

        register(Packet::new(4, "hello:four").direction(Direction::ToClient)).expect("a free id");
        let to_server = Arc::new(Mutex::new(0));
        let into = Arc::clone(&to_server);
        let _only_server = register_handler(4, Some(Direction::ToServer), move |_| {
            *into.lock().unwrap() += 1;
            false
        });
        assert_eq!(feed(4, &encode(b"z")), Ok(0), "the packet goes the other way");
        assert_eq!(*to_server.lock().unwrap(), 0);
    }

    #[test]
    fn the_trace_records_what_was_dispatched_and_replays_it() {
        let _serial = serial();
        fresh();
        register(Packet::new(8, "hello:trace")).expect("a free id");
        let _handler = register_handler(8, None, |_| false);
        assert_eq!(feed(8, &encode(b"abc")), Ok(1));
        assert_eq!(feed(8, &encode(b"de")), Ok(1));

        let trace = get_trace();
        assert_eq!(trace.len(), 2);
        assert_eq!(trace[0].packet.payload, b"abc");
        assert_eq!(trace[0].packet.size(), 3);
        assert_eq!(trace[0].handlers, 1);

        let recording = export_recording();
        assert_eq!(clear_trace(), 2);
        assert_eq!(replay_recording(&recording), Ok(2));
        assert_eq!(get_trace().len(), 2);

        assert!(set_trace_enabled(false), "recording was on");
        assert!(!set_trace_enabled(true), "recording was off");
    }

    #[test]
    fn serialize_validate_inspect_and_clone_are_real() {
        let packet = Packet::new(11, "hello:wire")
            .direction(Direction::ToServer)
            .state(PacketState::Login)
            .payload(&[0xde, 0xad])
            .metadata("a", "b");
        let text = serialize(&packet);
        assert_eq!(text, "11 hello:wire to_server login dead a=b");
        assert_eq!(deserialize(&text), Ok(packet.clone()));
        assert_eq!(clone(&packet), packet);
        assert!(validate(&packet).is_empty());
        assert!(inspect(&packet).contains("hello:wire"));
        assert!(inspect(&packet).contains("2 bytes"));

        let bad = Packet::new(12, "hello bad");
        assert!(validate(&bad).iter().any(|problem| problem.contains("whitespace")));
        assert!(deserialize("not a packet").is_err());
        assert!(deserialize("5 hello:x sideways play -").is_err());
    }

    #[test]
    fn the_client_protocol_is_refused_with_its_own_reason() {
        let _serial = serial();
        let probe = Packet::new(1, "hello:probe");
        assert!(!cancel(1));
        assert!(!replace(&probe));
        assert!(!modify(&mut probe.clone()));
        assert!(!send(&probe));
        assert!(!send_to_server(&probe));
        assert!(!send_to_client(&probe));
        assert!(!send_to_all(&probe));
        assert!(!set_observer(|_| {}));
        assert!(!register_protocol_mapping("hello:x", 1));
        assert!(get_unknown_packets().is_empty());
        assert!(get_protocol_version().is_none());
        assert!(get_protocol_mapping("hello:x").is_none());

        assert_eq!(refusals().len(), 6);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }
}
