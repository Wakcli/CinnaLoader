//! Redstone: the network a mod builds, and the client's own which it cannot touch.
//!
//! Redstone here is data and a computation, and both are real. A mod declares the pieces of a
//! network — signal sources, receivers and the blocks between them — each with a name, an id
//! and a position, and [`Signal`], a strength clamped to `0..=15`. [`SignalNetwork`] is the
//! graph those pieces form, and [`solve`] is a real propagation: it walks out from every
//! source, loses one per step along a wire (or the wire behaviour's own decay), stops when the
//! signal reaches zero, and gives each position the **maximum** that reaches it rather than the
//! sum. A ring of wire terminates because every step is strictly weaker and a position is only
//! revisited when the signal improves.
//!
//! A block has two numbers, and they are not the same question. A block's **signal** is what it
//! emits — a redstone block emits 15, a lamp emits nothing. Its **power** is what the network
//! delivers to its position. [`SignalGraph::signal_at`] answers the first,
//! [`SignalGraph::power_at`] and [`SignalGraph::has_signal_at`] answer the second.
//!
//! Updates are a queue a mod drives: [`schedule_update`] and [`notify_neighbors`] put positions
//! in it, and [`drain_updates`] recomputes them in the order a change would reach them, adding a
//! position's neighbours only when its power really changed. A change that reaches nothing
//! enqueues nothing new.
//!
//! Refused, each with a sentence from [`why_unknown`] and a line in [`refusals`]: the client's
//! own redstone — its signal, its direct signal, its direction, its block power and powered bit,
//! its neighbours, and its update queue. Reading or writing any of those needs the client's
//! world, which sections 10 and 51 put out of reach, so every one of them answers
//! `None`/`false`/empty and says why.
//!
//! ```
//! use cinnaloader::redstone::{Component, SignalConsumer, SignalNetwork, SignalSource};
//!
//! let net = SignalNetwork::new()
//!     .source(SignalSource::new("lever", "Lever", (0, 0, 0)).strength(15))
//!     .push(Component::wire("wire", "Wire", (1, 0, 0)))
//!     .consumer(SignalConsumer::new("lamp", "Lamp", (2, 0, 0)));
//! let graph = net.solve();
//! assert_eq!(graph.power_at((0, 0, 0)).get(), 15, "the source is full strength");
//! assert_eq!(graph.power_at((1, 0, 0)).get(), 14, "one step loses one");
//! assert_eq!(graph.power_at((2, 0, 0)).get(), 13, "two steps lose two");
//! assert!(graph.has_signal_at((2, 0, 0)));
//! ```

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::{Mutex, OnceLock};

/// The strongest a signal can be, what a source emits at full power.
pub const MAX_SIGNAL: u8 = 15;
/// The weakest, which is no signal at all.
pub const MIN_SIGNAL: u8 = 0;

/// A redstone strength, clamped into `0..=15` at both ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Signal {
    strength: u8,
}

impl Signal {
    /// A strength, clamped into range; `-3` is nothing and `99` is full.
    #[must_use]
    pub fn new(strength: i32) -> Self {
        Self {
            strength: strength.clamp(i32::from(MIN_SIGNAL), i32::from(MAX_SIGNAL)) as u8,
        }
    }

    /// The strength, `0..=15`.
    #[must_use]
    pub fn get(self) -> u8 {
        self.strength
    }

    /// Whether this is no signal.
    #[must_use]
    pub fn is_zero(self) -> bool {
        self.strength == MIN_SIGNAL
    }

    /// Weaker by `by`, floored at [`MIN_SIGNAL`] rather than wrapping.
    #[must_use]
    pub fn decayed(self, by: u8) -> Self {
        Self {
            strength: self.strength.saturating_sub(by),
        }
    }

    /// Shifted by `amount`, clamped; a repeater boosts, a drain cuts.
    #[must_use]
    pub fn offset(self, amount: i32) -> Self {
        Self::new(i32::from(self.strength) + amount)
    }
}

impl fmt::Display for Signal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.strength)
    }
}

impl From<u8> for Signal {
    fn from(strength: u8) -> Self {
        Self::new(i32::from(strength))
    }
}

/// What a piece of the network is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignalKind {
    Source,
    Consumer,
    Wire,
    Block,
    Comparator,
}

impl SignalKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Consumer => "consumer",
            Self::Wire => "wire",
            Self::Block => "block",
            Self::Comparator => "comparator",
        }
    }
}

/// A signal source: a lever, a button, a redstone block. It emits `strength` at its position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalSource {
    pub id: String,
    pub name: String,
    pub position: (i32, i32, i32),
    pub strength: Signal,
    /// A [`crate::blocks`] id when this source is a placed block, rather than a bare emitter.
    pub block: Option<String>,
}

impl SignalSource {
    /// A source at full strength.
    #[must_use]
    pub fn new(id: &str, name: &str, position: (i32, i32, i32)) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            position,
            strength: Signal::new(i32::from(MAX_SIGNAL)),
            block: None,
        }
    }

    #[must_use]
    pub fn strength(mut self, strength: i32) -> Self {
        self.strength = Signal::new(strength);
        self
    }

    #[must_use]
    pub fn block(mut self, block_id: &str) -> Self {
        self.block = Some(block_id.to_owned());
        self
    }
}

/// A signal receiver: a lamp, a door, a piston. It emits nothing and reads the power delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalConsumer {
    pub id: String,
    pub name: String,
    pub position: (i32, i32, i32),
    /// A [`crate::blocks`] id when this receiver is a placed block.
    pub block: Option<String>,
}

impl SignalConsumer {
    #[must_use]
    pub fn new(id: &str, name: &str, position: (i32, i32, i32)) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            position,
            block: None,
        }
    }

    #[must_use]
    pub fn block(mut self, block_id: &str) -> Self {
        self.block = Some(block_id.to_owned());
        self
    }
}

/// A block between sources and receivers: wire, a comparator, or a solid block that relays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    pub id: String,
    pub name: String,
    pub position: (i32, i32, i32),
    pub kind: SignalKind,
    /// What this component emits by itself, which is nothing for plain wire.
    pub signal: Signal,
    /// A [`crate::blocks`] id, so a wire behaviour can find it.
    pub block: Option<String>,
}

impl Component {
    fn of(id: &str, name: &str, position: (i32, i32, i32), kind: SignalKind, block: Option<&str>) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            position,
            kind,
            signal: Signal::default(),
            block: block.map(str::to_owned),
        }
    }

    /// Redstone wire, which relays with the per-step decay.
    #[must_use]
    pub fn wire(id: &str, name: &str, position: (i32, i32, i32)) -> Self {
        Self::of(id, name, position, SignalKind::Wire, None)
    }

    /// A solid block.
    #[must_use]
    pub fn block(id: &str, name: &str, position: (i32, i32, i32), block_id: &str) -> Self {
        Self::of(id, name, position, SignalKind::Block, Some(block_id))
    }

    /// A comparator.
    #[must_use]
    pub fn comparator(id: &str, name: &str, position: (i32, i32, i32)) -> Self {
        Self::of(id, name, position, SignalKind::Comparator, None)
    }

    /// The signal this component emits by itself.
    #[must_use]
    pub fn signal(mut self, signal: i32) -> Self {
        self.signal = Signal::new(signal);
        self
    }

    #[must_use]
    pub fn block_id(mut self, block_id: &str) -> Self {
        self.block = Some(block_id.to_owned());
        self
    }
}

/// A component that changes the signal as it passes: a repeater boosts, a drain cuts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalModifier {
    pub id: String,
    pub name: String,
    pub position: (i32, i32, i32),
    /// Added to the signal leaving this position, then clamped.
    pub adjustment: i32,
}

impl SignalModifier {
    #[must_use]
    pub fn new(id: &str, name: &str, position: (i32, i32, i32)) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            position,
            adjustment: 0,
        }
    }

    #[must_use]
    pub fn adjustment(mut self, adjustment: i32) -> Self {
        self.adjustment = adjustment;
        self
    }
}

/// How a comparator compares its two inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ComparatorMode {
    #[default]
    Compare,
    Subtract,
}

impl ComparatorMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Compare => "compare",
            Self::Subtract => "subtract",
        }
    }
}

/// A comparator's behaviour. Recorded and read back; the solver does not apply it, because it
/// needs the comparator's two inputs and this host only has the network a mod built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparatorBehavior {
    pub id: String,
    pub name: String,
    pub mode: ComparatorMode,
}

impl ComparatorBehavior {
    #[must_use]
    pub fn new(id: &str, name: &str, mode: ComparatorMode) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            mode,
        }
    }
}

/// The decay a wire of a given block loses per step. The game's wire loses one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireBehavior {
    pub id: String,
    pub name: String,
    /// The block id this applies to, a [`crate::blocks`] id.
    pub block: String,
    pub decay: u8,
}

impl WireBehavior {
    /// The game's own wire: one per step.
    #[must_use]
    pub fn new(id: &str, name: &str, block: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            block: block.to_owned(),
            decay: 1,
        }
    }

    #[must_use]
    pub fn decay(mut self, decay: u8) -> Self {
        self.decay = decay;
        self
    }
}

/// A network a mod assembles and solves for itself, before it is ever registered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SignalNetwork {
    pub sources: Vec<SignalSource>,
    pub consumers: Vec<SignalConsumer>,
    pub components: Vec<Component>,
    pub wires: Vec<WireBehavior>,
    pub modifiers: Vec<SignalModifier>,
    pub comparators: Vec<ComparatorBehavior>,
}

impl SignalNetwork {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn source(mut self, source: SignalSource) -> Self {
        self.sources.push(source);
        self
    }

    #[must_use]
    pub fn consumer(mut self, consumer: SignalConsumer) -> Self {
        self.consumers.push(consumer);
        self
    }

    #[must_use]
    pub fn push(mut self, component: Component) -> Self {
        self.components.push(component);
        self
    }

    #[must_use]
    pub fn wire(mut self, wire: WireBehavior) -> Self {
        self.wires.push(wire);
        self
    }

    #[must_use]
    pub fn modifier(mut self, modifier: SignalModifier) -> Self {
        self.modifiers.push(modifier);
        self
    }

    #[must_use]
    pub fn comparator(mut self, comparator: ComparatorBehavior) -> Self {
        self.comparators.push(comparator);
        self
    }

    /// Solves this network now.
    #[must_use]
    pub fn solve(&self) -> SignalGraph {
        solve(self)
    }
}

/// One position in a solved graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalNode {
    pub id: String,
    pub position: (i32, i32, i32),
    pub kind: SignalKind,
    /// What this node emits by itself.
    pub signal: Signal,
    /// What the network delivers to this position.
    pub power: Signal,
}

/// The result of solving a network: what every position emits and receives.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SignalGraph {
    pub nodes: Vec<SignalNode>,
}

impl SignalGraph {
    #[must_use]
    pub fn node_at(&self, position: (i32, i32, i32)) -> Option<&SignalNode> {
        self.nodes.iter().find(|node| node.position == position)
    }

    /// What the block at a position emits, or nothing when no component is there.
    #[must_use]
    pub fn signal_at(&self, position: (i32, i32, i32)) -> Signal {
        self.node_at(position).map_or(Signal::default(), |node| node.signal)
    }

    /// What the network delivers to a position, or nothing when no component is there.
    #[must_use]
    pub fn power_at(&self, position: (i32, i32, i32)) -> Signal {
        self.node_at(position).map_or(Signal::default(), |node| node.power)
    }

    /// Whether anything reaches a position.
    #[must_use]
    pub fn is_powered_at(&self, position: (i32, i32, i32)) -> bool {
        !self.power_at(position).is_zero()
    }

    /// The same question as [`is_powered_at`](Self::is_powered_at), under the wishlist's name.
    #[must_use]
    pub fn has_signal_at(&self, position: (i32, i32, i32)) -> bool {
        !self.power_at(position).is_zero()
    }

    /// What a registered receiver reads, by id.
    #[must_use]
    pub fn receiver_power(&self, id: &str) -> Option<Signal> {
        self.nodes
            .iter()
            .find(|node| node.id == id && node.kind == SignalKind::Consumer)
            .map(|node| node.power)
    }

    /// The sum of every position's power. `std`'s `Sum` seeds with `-0.0`, so an empty graph
    /// would answer `-0`; this normalises it to `0`.
    #[must_use]
    pub fn total_power(&self) -> f64 {
        let total: f64 = self.nodes.iter().map(|node| f64::from(node.power.get())).sum();
        if total == 0.0 {
            0.0
        } else {
            total
        }
    }

    /// The mean power over the nodes that exist.
    #[must_use]
    pub fn average_power(&self) -> f64 {
        if self.nodes.is_empty() {
            return 0.0;
        }
        self.total_power() / self.nodes.len() as f64
    }
}

#[derive(Debug, Clone)]
struct Node {
    id: String,
    position: (i32, i32, i32),
    kind: SignalKind,
    signal: Signal,
    decay: u8,
}

fn wire_decay(network: &SignalNetwork, block: &Option<String>) -> u8 {
    block
        .as_ref()
        .and_then(|id| network.wires.iter().find(|wire| wire.block == *id))
        .map_or(1, |wire| wire.decay)
}

fn adjacent((x, y, z): (i32, i32, i32)) -> [(i32, i32, i32); 6] {
    [
        (x + 1, y, z),
        (x - 1, y, z),
        (x, y + 1, z),
        (x, y - 1, z),
        (x, y, z + 1),
        (x, y, z - 1),
    ]
}

/// Walks out from every source, losing each step's decay until the signal dies, and answers the
/// power at every position. A position is only revisited when the signal reaching it improves,
/// so a ring settles instead of spinning.
#[must_use]
pub fn solve(network: &SignalNetwork) -> SignalGraph {
    let mut nodes = Vec::new();
    for source in &network.sources {
        nodes.push(Node {
            id: source.id.clone(),
            position: source.position,
            kind: SignalKind::Source,
            signal: source.strength,
            decay: wire_decay(network, &source.block),
        });
    }
    for consumer in &network.consumers {
        nodes.push(Node {
            id: consumer.id.clone(),
            position: consumer.position,
            kind: SignalKind::Consumer,
            signal: Signal::default(),
            decay: wire_decay(network, &consumer.block),
        });
    }
    for component in &network.components {
        nodes.push(Node {
            id: component.id.clone(),
            position: component.position,
            kind: component.kind,
            signal: component.signal,
            decay: wire_decay(network, &component.block),
        });
    }

    let positions: Vec<(i32, i32, i32)> = nodes.iter().map(|node| node.position).collect();
    let mut best: BTreeMap<(i32, i32, i32), Signal> = BTreeMap::new();
    let mut queue: VecDeque<((i32, i32, i32), Signal)> = VecDeque::new();

    for node in &nodes {
        if node.signal.is_zero() {
            continue;
        }
        let entry = best.entry(node.position).or_default();
        if node.signal > *entry {
            *entry = node.signal;
            queue.push_back((node.position, node.signal));
        }
    }

    while let Some((position, signal)) = queue.pop_front() {
        let Some(node) = nodes.iter().find(|node| node.position == position) else {
            continue;
        };
        if node.kind == SignalKind::Consumer {
            continue;
        }
        let adjusted = network
            .modifiers
            .iter()
            .find(|modifier| modifier.position == position)
            .map_or(signal, |modifier| signal.offset(modifier.adjustment));
        let out = adjusted.decayed(node.decay);
        if out.is_zero() {
            continue;
        }
        for next in adjacent(position) {
            if !positions.contains(&next) {
                continue;
            }
            let entry = best.entry(next).or_default();
            if out > *entry {
                *entry = out;
                queue.push_back((next, out));
            }
        }
    }

    let solved = nodes
        .into_iter()
        .map(|node| {
            let power = best.get(&node.position).copied().unwrap_or_default();
            SignalNode {
                id: node.id,
                position: node.position,
                kind: node.kind,
                signal: node.signal,
                power,
            }
        })
        .collect();

    SignalGraph { nodes: solved }
}

#[derive(Default)]
struct Registry {
    sources: BTreeMap<String, SignalSource>,
    consumers: BTreeMap<String, SignalConsumer>,
    components: BTreeMap<String, Component>,
    modifiers: BTreeMap<String, SignalModifier>,
    comparators: BTreeMap<String, ComparatorBehavior>,
    wires: BTreeMap<String, WireBehavior>,
    updates: Vec<(i32, i32, i32)>,
    cache: Option<SignalGraph>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

fn with<T>(body: impl FnOnce(&mut Registry) -> T) -> T {
    let mut guard = registry().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn network_of(reg: &Registry) -> SignalNetwork {
    SignalNetwork {
        sources: reg.sources.values().cloned().collect(),
        consumers: reg.consumers.values().cloned().collect(),
        components: reg.components.values().cloned().collect(),
        wires: reg.wires.values().cloned().collect(),
        modifiers: reg.modifiers.values().cloned().collect(),
        comparators: reg.comparators.values().cloned().collect(),
    }
}

fn registry_positions(reg: &Registry) -> Vec<(i32, i32, i32)> {
    reg.sources
        .values()
        .map(|source| source.position)
        .chain(reg.consumers.values().map(|consumer| consumer.position))
        .chain(reg.components.values().map(|component| component.position))
        .collect()
}

fn neighbors_of(reg: &Registry, position: (i32, i32, i32)) -> Vec<(i32, i32, i32)> {
    let positions = registry_positions(reg);
    adjacent(position)
        .into_iter()
        .filter(|next| positions.contains(next))
        .collect()
}

/// Declares a signal source, or replaces the one with that id.
pub fn register_signal_source(source: SignalSource) -> bool {
    with(|reg| { reg.cache = None; reg.sources.insert(source.id.clone(), source).is_none() })
}

/// Declares a signal receiver, or replaces the one with that id.
pub fn register_signal_consumer(consumer: SignalConsumer) -> bool {
    with(|reg| { reg.cache = None; reg.consumers.insert(consumer.id.clone(), consumer).is_none() })
}

/// Declares a block or wire between them, or replaces the one with that id.
pub fn register_component(component: Component) -> bool {
    with(|reg| { reg.cache = None; reg.components.insert(component.id.clone(), component).is_none() })
}

/// Declares a signal modifier.
pub fn register_signal_modifier(modifier: SignalModifier) -> bool {
    with(|reg| { reg.cache = None; reg.modifiers.insert(modifier.id.clone(), modifier).is_none() })
}

/// Declares a comparator's behaviour. It is read back; the solver does not apply it.
pub fn register_comparator_behavior(behavior: ComparatorBehavior) -> bool {
    with(|reg| reg.comparators.insert(behavior.id.clone(), behavior).is_none())
}

/// Declares the decay a wire of a given block loses per step.
pub fn register_wire_behavior(behavior: WireBehavior) -> bool {
    with(|reg| { reg.cache = None; reg.wires.insert(behavior.id.clone(), behavior).is_none() })
}

/// Forgets any declared source, receiver, component, modifier, comparator or wire, by id.
pub fn unregister(id: &str) -> bool {
    with(|reg| {
        reg.cache = None;
        [
            reg.sources.remove(id).is_some(),
            reg.consumers.remove(id).is_some(),
            reg.components.remove(id).is_some(),
            reg.modifiers.remove(id).is_some(),
            reg.comparators.remove(id).is_some(),
            reg.wires.remove(id).is_some(),
        ]
        .into_iter()
        .any(|removed| removed)
    })
}

/// A declared signal source, by id.
#[must_use]
pub fn get_signal_source(id: &str) -> Option<SignalSource> {
    with(|reg| reg.sources.get(id).cloned())
}

/// A declared signal receiver, by id.
#[must_use]
pub fn get_signal_consumer(id: &str) -> Option<SignalConsumer> {
    with(|reg| reg.consumers.get(id).cloned())
}

/// A declared component, by id.
#[must_use]
pub fn get_component(id: &str) -> Option<Component> {
    with(|reg| reg.components.get(id).cloned())
}

/// A declared modifier, by id.
#[must_use]
pub fn get_signal_modifier(id: &str) -> Option<SignalModifier> {
    with(|reg| reg.modifiers.get(id).cloned())
}

/// A declared comparator behaviour, by id.
#[must_use]
pub fn get_comparator_behavior(id: &str) -> Option<ComparatorBehavior> {
    with(|reg| reg.comparators.get(id).cloned())
}

/// A declared wire behaviour, by id.
#[must_use]
pub fn get_wire_behavior(id: &str) -> Option<WireBehavior> {
    with(|reg| reg.wires.get(id).cloned())
}

/// Drops the solved graph so the next question recomputes it. Answers whether there was one.
pub fn invalidate_cache() -> bool {
    with(|reg| reg.cache.take().is_some())
}

/// Solves the registered network now and caches it.
pub fn update_network() -> SignalGraph {
    with(|reg| {
        let graph = solve(&network_of(reg));
        reg.cache = Some(graph.clone());
        graph
    })
}

/// The registered network's solved graph, cached between changes.
#[must_use]
pub fn get_signal_graph() -> SignalGraph {
    with(|reg| {
        if let Some(graph) = &reg.cache {
            return graph.clone();
        }
        let graph = solve(&network_of(reg));
        reg.cache = Some(graph.clone());
        graph
    })
}

/// What the block at a position emits, from the registered network.
#[must_use]
pub fn signal_at(x: i32, y: i32, z: i32) -> Option<Signal> {
    get_signal_graph().node_at((x, y, z)).map(|node| node.signal)
}

/// What the registered network delivers to a position.
#[must_use]
pub fn power_at(x: i32, y: i32, z: i32) -> Option<Signal> {
    get_signal_graph().node_at((x, y, z)).map(|node| node.power)
}

/// Whether anything reaches a position in the registered network.
#[must_use]
pub fn is_powered_at(x: i32, y: i32, z: i32) -> bool {
    get_signal_graph().is_powered_at((x, y, z))
}

/// The wishlist's spelling for [`is_powered_at`], over the registered network.
#[must_use]
pub fn has_signal_at(x: i32, y: i32, z: i32) -> bool {
    get_signal_graph().has_signal_at((x, y, z))
}

/// Changes the signal a declared source or component emits, queues its position, and answers the
/// new strength. A receiver has no signal to change and answers `None`.
pub fn modify_signal(id: &str, strength: i32) -> Option<Signal> {
    with(|reg| {
        let signal = Signal::new(strength);
        let position = if let Some(source) = reg.sources.get_mut(id) {
            source.strength = signal;
            Some(source.position)
        } else if let Some(component) = reg.components.get_mut(id) {
            component.signal = signal;
            Some(component.position)
        } else {
            None
        };
        let position = position?;
        reg.cache = None;
        if !reg.updates.contains(&position) {
            reg.updates.push(position);
        }
        Some(signal)
    })
}

/// Queues a position for the next [`drain_updates`]. Answers whether it was not already queued.
pub fn schedule_update(x: i32, y: i32, z: i32) -> bool {
    with(|reg| {
        let position = (x, y, z);
        if reg.updates.contains(&position) {
            return false;
        }
        reg.updates.push(position);
        true
    })
}

/// Queues every registered neighbour of a position, and answers how many were newly queued.
pub fn notify_neighbors(x: i32, y: i32, z: i32) -> usize {
    with(|reg| {
        let neighbors = neighbors_of(reg, (x, y, z));
        let mut queued = 0;
        for position in neighbors {
            if !reg.updates.contains(&position) {
                reg.updates.push(position);
                queued += 1;
            }
        }
        queued
    })
}

/// The positions waiting for the next drain, in the order they were queued.
#[must_use]
pub fn pending_updates() -> Vec<(i32, i32, i32)> {
    with(|reg| reg.updates.clone())
}

/// Recomputes the queued positions in the order a change reaches them, adding a position's
/// neighbours only when its power really moved. A change that reaches nothing queues nothing new.
pub fn drain_updates() -> Vec<(i32, i32, i32)> {
    with(|reg| {
        let queued = std::mem::take(&mut reg.updates);
        let before = reg.cache.clone();
        let graph = solve(&network_of(reg));
        let mut order: Vec<(i32, i32, i32)> = Vec::new();
        let mut queue: VecDeque<(i32, i32, i32)> = queued.into();
        while let Some(position) = queue.pop_front() {
            if order.contains(&position) {
                continue;
            }
            order.push(position);
            let was = before.as_ref().map_or(0, |solved| solved.power_at(position).get());
            let now = graph.power_at(position).get();
            if now != was {
                for next in neighbors_of(reg, position) {
                    if !order.contains(&next) && !queue.contains(&next) {
                        queue.push_back(next);
                    }
                }
            }
        }
        reg.cache = Some(graph);
        order
    })
}

/// Why a redstone question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "signal" => {
            "the signal at a position is the client's redstone; reading it needs the hooks \
             section 10 lists as never"
                .to_owned()
        }
        "write" => {
            "changing the client's redstone is its world state; it needs the hooks section 10 \
             lists as never"
                .to_owned()
        }
        "direct_signal" => {
            "the direct signal a block gives its neighbour is computed inside the client".to_owned()
        }
        "power" => "a block's power is the client's redstone state, not this host's network".to_owned(),
        "powered" => "whether the client's block is powered is its own state".to_owned(),
        "neighbors" => "the client's neighbouring blocks are its world state".to_owned(),
        "direction" => "the direction a signal arrives from is the client's own propagation".to_owned(),
        "update" => {
            "the client's update queue is its scheduler; a mod drives this host's queue with \
             schedule_update and drain_updates instead"
                .to_owned()
        }
        "world" => "anything needing the client's world is out of reach here".to_owned(),
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot do about the client's redstone, for a panel or a report.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    [
        "signal",
        "write",
        "direct_signal",
        "power",
        "powered",
        "neighbors",
        "direction",
        "update",
        "world",
    ]
    .iter()
    .map(|what| ((*what).to_owned(), why_unknown(what)))
    .collect()
}

/// The client half: every one of these is the client's own redstone, and answers honestly rather
/// than inventing a signal.
pub mod known_only_to_the_client {
    use super::*;

    #[must_use]
    pub fn get_signal(_x: i32, _y: i32, _z: i32) -> Option<Signal> {
        crate::warn!("{}", why_unknown("signal"));
        None
    }

    pub fn set_signal(_x: i32, _y: i32, _z: i32, _strength: i32) -> bool {
        crate::warn!("{}", why_unknown("write"));
        false
    }

    #[must_use]
    pub fn get_direct_signal(_x: i32, _y: i32, _z: i32) -> Option<Signal> {
        crate::warn!("{}", why_unknown("direct_signal"));
        None
    }

    #[must_use]
    pub fn get_power(_x: i32, _y: i32, _z: i32) -> Option<Signal> {
        crate::warn!("{}", why_unknown("power"));
        None
    }

    #[must_use]
    pub fn is_powered(_x: i32, _y: i32, _z: i32) -> bool {
        crate::warn!("{}", why_unknown("powered"));
        false
    }

    #[must_use]
    pub fn get_neighbors(_x: i32, _y: i32, _z: i32) -> Vec<(i32, i32, i32)> {
        crate::warn!("{}", why_unknown("neighbors"));
        Vec::new()
    }

    #[must_use]
    pub fn get_signal_direction(_x: i32, _y: i32, _z: i32) -> Option<(i32, i32, i32)> {
        crate::warn!("{}", why_unknown("direction"));
        None
    }

    #[must_use]
    pub fn get_update_queue() -> Vec<(i32, i32, i32)> {
        crate::warn!("{}", why_unknown("update"));
        Vec::new()
    }
}

pub use known_only_to_the_client::{
    get_direct_signal, get_neighbors, get_power, get_signal, get_signal_direction, is_powered,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|reg| *reg = Registry::default());
    }

    #[test]
    fn a_signal_clamps_at_both_ends() {
        assert_eq!(Signal::new(-3).get(), 0);
        assert_eq!(Signal::new(0).get(), 0);
        assert_eq!(Signal::new(7).get(), 7);
        assert_eq!(Signal::new(15).get(), 15);
        assert_eq!(Signal::new(99).get(), 15);
        assert_eq!(Signal::new(1).decayed(1).get(), 0);
        assert_eq!(Signal::new(3).decayed(9).get(), 0, "it floors rather than wraps");
        assert_eq!(Signal::new(20).offset(-7).get(), 8);
        assert_eq!(Signal::new(5).to_string(), "5");
        assert!(
            SignalGraph::default().total_power().is_sign_positive(),
            "an empty sum must not answer with a negative zero"
        );
        assert_eq!(SignalGraph::default().total_power(), 0.0);
    }

    #[test]
    fn power_decays_one_per_step_from_a_source() {
        let net = SignalNetwork::new()
            .source(SignalSource::new("lever", "Lever", (0, 0, 0)).strength(15))
            .push(Component::wire("a", "Wire", (1, 0, 0)))
            .push(Component::wire("b", "Wire", (2, 0, 0)))
            .push(Component::wire("c", "Wire", (3, 0, 0)))
            .consumer(SignalConsumer::new("lamp", "Lamp", (4, 0, 0)));
        let graph = net.solve();
        assert_eq!(graph.power_at((0, 0, 0)).get(), 15, "the source is full strength");
        assert_eq!(graph.power_at((1, 0, 0)).get(), 14);
        assert_eq!(graph.power_at((2, 0, 0)).get(), 13);
        assert_eq!(graph.power_at((3, 0, 0)).get(), 12);
        assert_eq!(graph.power_at((4, 0, 0)).get(), 11);
        assert_eq!(graph.signal_at((0, 0, 0)).get(), 15, "the source emits");
        assert_eq!(graph.signal_at((1, 0, 0)).get(), 0, "a wire emits nothing");
        assert!(graph.has_signal_at((4, 0, 0)));
        assert_eq!(graph.receiver_power("lamp").map(Signal::get), Some(11));
    }

    #[test]
    fn a_receiver_takes_the_maximum_of_two_paths_not_the_sum() {
        let net = SignalNetwork::new()
            .source(SignalSource::new("strong", "Strong", (0, 0, 0)).strength(15))
            .source(SignalSource::new("weak", "Weak", (1, 0, 0)).strength(8))
            .consumer(SignalConsumer::new("lamp", "Lamp", (2, 0, 0)));
        let graph = net.solve();
        assert_eq!(graph.power_at((2, 0, 0)).get(), 13, "15 loses one a step, not 15 + 8");
    }

    #[test]
    fn a_ring_of_wire_terminates_and_settles() {
        let net = SignalNetwork::new()
            .source(SignalSource::new("lever", "Lever", (0, 0, -1)).strength(15))
            .push(Component::wire("a", "Wire", (0, 0, 0)))
            .push(Component::wire("b", "Wire", (1, 0, 0)))
            .push(Component::wire("c", "Wire", (1, 0, 1)))
            .push(Component::wire("d", "Wire", (0, 0, 1)));
        let graph = net.solve();
        assert_eq!(graph.power_at((0, 0, 0)).get(), 14);
        assert_eq!(graph.power_at((1, 0, 0)).get(), 13);
        assert_eq!(graph.power_at((0, 0, 1)).get(), 13);
        assert_eq!(graph.power_at((1, 0, 1)).get(), 12, "the far corner is fed from both sides");
    }

    #[test]
    fn a_wire_behaviour_changes_the_per_step_decay() {
        let net = SignalNetwork::new()
            .source(SignalSource::new("lever", "Lever", (0, 0, 0)).strength(15))
            .push(Component::block("a", "Wire", (1, 0, 0), "hello:slow_wire"))
            .push(Component::block("b", "Wire", (2, 0, 0), "hello:slow_wire"))
            .consumer(SignalConsumer::new("lamp", "Lamp", (3, 0, 0)))
            .wire(WireBehavior::new("slow", "Slow wire", "hello:slow_wire").decay(2));
        let graph = net.solve();
        assert_eq!(graph.power_at((1, 0, 0)).get(), 14, "the source still gives one step");
        assert_eq!(graph.power_at((2, 0, 0)).get(), 12, "the slow wire loses two");
        assert_eq!(graph.power_at((3, 0, 0)).get(), 10);
    }

    #[test]
    fn the_registered_network_solves_with_a_cache() {
        let _serial = serial();
        fresh();
        assert!(register_signal_source(SignalSource::new("lever", "Lever", (0, 0, 0)).strength(15)));
        assert!(register_component(Component::wire("wire", "Wire", (1, 0, 0))));
        assert!(register_signal_consumer(SignalConsumer::new("lamp", "Lamp", (2, 0, 0))));
        let graph = update_network();
        assert_eq!(graph.power_at((2, 0, 0)).get(), 13);
        assert_eq!(signal_at(0, 0, 0).map(Signal::get), Some(15));
        assert_eq!(power_at(2, 0, 0).map(Signal::get), Some(13));
        assert!(is_powered_at(2, 0, 0));
        assert!(has_signal_at(2, 0, 0));
        assert_eq!(signal_at(9, 9, 9), None, "no component, no answer");
        assert_eq!(get_signal_graph().receiver_power("lamp").map(Signal::get), Some(13));
        assert!(invalidate_cache(), "there was a cache");
        assert!(!invalidate_cache(), "and now there is not");
    }

    #[test]
    fn an_update_drains_in_order_and_a_change_that_reaches_nothing_stops() {
        let _serial = serial();
        fresh();
        register_signal_source(SignalSource::new("lever", "Lever", (0, 0, 0)).strength(15));
        register_component(Component::wire("wire", "Wire", (1, 0, 0)));
        register_signal_consumer(SignalConsumer::new("lamp", "Lamp", (2, 0, 0)));
        assert!(schedule_update(0, 0, 0));
        assert!(!schedule_update(0, 0, 0), "the same position is not queued twice");
        assert_eq!(drain_updates(), vec![(0, 0, 0), (1, 0, 0), (2, 0, 0)]);
        assert!(pending_updates().is_empty());
        // The lamp's power has not moved, so re-scheduling it reaches nothing new.
        assert!(schedule_update(2, 0, 0));
        assert_eq!(drain_updates(), vec![(2, 0, 0)]);
        assert!(pending_updates().is_empty());
    }

    #[test]
    fn sources_receivers_blocks_and_behaviours_are_declared_and_read_back() {
        let _serial = serial();
        fresh();
        assert!(register_signal_source(
            SignalSource::new("lever", "Lever", (0, 0, 0)).strength(15).block("minecraft:lever")
        ));
        assert!(register_signal_consumer(
            SignalConsumer::new("lamp", "Lamp", (2, 0, 0)).block("minecraft:redstone_lamp")
        ));
        assert!(register_component(Component::block("block", "Block", (1, 0, 0), "minecraft:redstone_block")));
        assert!(register_signal_modifier(SignalModifier::new("repeater", "Repeater", (1, 0, 0)).adjustment(1)));
        assert!(register_comparator_behavior(ComparatorBehavior::new(
            "comparator",
            "Comparator",
            ComparatorMode::Subtract
        )));
        assert!(register_wire_behavior(WireBehavior::new("wire", "Wire", "minecraft:redstone_wire").decay(2)));

        assert_eq!(get_signal_source("lever").map(|source| source.name), Some("Lever".to_owned()));
        assert_eq!(
            get_signal_consumer("lamp").and_then(|consumer| consumer.block),
            Some("minecraft:redstone_lamp".to_owned())
        );
        assert_eq!(get_component("block").map(|component| component.kind), Some(SignalKind::Block));
        assert_eq!(get_signal_modifier("repeater").map(|modifier| modifier.adjustment), Some(1));
        assert_eq!(
            get_comparator_behavior("comparator").map(|behavior| behavior.mode),
            Some(ComparatorMode::Subtract)
        );
        assert_eq!(get_wire_behavior("wire").map(|wire| wire.decay), Some(2));
        assert_eq!(ComparatorMode::Compare.as_str(), "compare");
        assert_eq!(SignalKind::Wire.as_str(), "wire");

        assert_eq!(modify_signal("lever", 7), Some(Signal::new(7)));
        assert!(pending_updates().contains(&(0, 0, 0)), "the change is queued");
        assert_eq!(modify_signal("lamp", 3), None, "a receiver emits nothing to change");
        assert_eq!(notify_neighbors(0, 0, 0), 1, "only the wire touches the lever");
        assert!(!schedule_update(1, 0, 0), "notify already queued it");

        assert!(unregister("lever"));
        assert!(!unregister("lever"));
    }

    #[test]
    fn every_client_redstone_question_is_refused_with_a_reason() {
        assert!(get_signal(0, 64, 0).is_none());
        assert!(get_direct_signal(0, 64, 0).is_none());
        assert!(get_power(0, 64, 0).is_none());
        assert!(!is_powered(0, 64, 0));
        assert!(get_neighbors(0, 64, 0).is_empty());
        assert!(get_signal_direction(0, 64, 0).is_none());
        assert!(!known_only_to_the_client::set_signal(0, 64, 0, 15));
        assert!(known_only_to_the_client::get_update_queue().is_empty());
        let lines = refusals();
        assert_eq!(lines.len(), 9);
        for (what, why) in lines {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }
}
