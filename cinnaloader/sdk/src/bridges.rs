//! Plugin bridges: a real adapter between a foreign API's call shape and this mod's own.
//!
//! Where [`crate::interop`] connects two mods that already speak the same shape, this connects
//! a *foreign* shape — another loader's API, a server plugin API, a script or a third-party
//! content format — to local functions. What a mod builds on is real:
//!
//! * **A declaration.** [`Bridge`] is a foreign API's name and [`Version`], an
//!   [`AdapterKind`], the [`Capability`]s it offers and the [`MethodMap`]s it exposes. Each map
//!   binds a foreign method name to a local function name and declares the shape of its
//!   arguments and its return as [`ArgKind`]s.
//! * **A registry.** [`register`] publishes a bridge; the same name twice is refused, and
//!   [`unregister`] takes it back. [`get`], [`get_all`], [`get_version`], [`get_capabilities`]
//!   and [`is_available`] read it back. [`require`] applies [`Version::satisfies`] — the
//!   interop rule, not a second copy of it — so a caller asking for a version it cannot get is
//!   refused naming both.
//! * **A type mapping table.** Each foreign [`ArgKind`] maps to a [`ConfigValue`] and back:
//!   `String` to `Text`, `Integer` to `Integer`, `Float` to `Float`, `Boolean` to `Boolean`,
//!   `List` to `List` and `Map` to `Map`. A foreign value that does not fit the declared kind
//!   is refused with a [`BridgeError::TypeMismatch`] naming both kinds, and so is a local
//!   return that does not fit the declared return kind.
//! * **A real call.** [`call`] maps the arguments in, invokes the local implementation that
//!   [`register_adapter`] bound, maps the result back and answers it — or a real error:
//!   unknown bridge, unknown method, wrong arity, a value that does not map, no implementation,
//!   or the implementation's own failure.
//! * **A translation report.** Every call records a [`Translation`] — what went in and what
//!   came out — so [`get_diagnostics`] can explain a mismatch rather than only refuse it.
//!
//! What this host cannot do is bridge into the client's own API: its classes and its native
//! bindings live inside the client's address space. Each is refused with its own sentence in
//! [`why_unknown`], listed by [`refusals`], and never answers a fake success.
//!
//! ```
//! use cinnaloader::bridges::{self, Adapter, ArgKind, Bridge, Foreign, MethodMap};
//! use cinnaloader::config::ConfigValue;
//! use cinnaloader::interop::Version;
//!
//! let bridge = Bridge::new("legacy:math", Version::new(1, 0, 0)).method(
//!     MethodMap::new("add", "math_add", ArgKind::Integer)
//!         .arg("a", ArgKind::Integer)
//!         .arg("b", ArgKind::Integer),
//! );
//! assert!(bridges::register(bridge));
//! assert!(bridges::register_adapter(Adapter::new("legacy:math", "math_add", |args| {
//!     match args {
//!         [ConfigValue::Integer(a), ConfigValue::Integer(b)] => Ok(ConfigValue::Integer(a + b)),
//!         _ => Err("math_add wants two integers".to_owned()),
//!     }
//! })));
//! assert!(bridges::is_available("legacy:math"));
//! assert_eq!(
//!     bridges::call("legacy:math", "add", &[Foreign::Integer(2), Foreign::Integer(3)]),
//!     Ok(Foreign::Integer(5))
//! );
//! assert!(matches!(
//!     bridges::call("legacy:math", "add", &[Foreign::Text("two".into()), Foreign::Integer(3)]),
//!     Err(bridges::BridgeError::TypeMismatch { .. })
//! ));
//! assert!(!bridges::register(Bridge::new("legacy:math", Version::new(1, 0, 0))));
//! # assert!(bridges::unregister("legacy:math"));
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use crate::capability::Capability;
use crate::config::ConfigValue;
use crate::interop::Version;

/// How a bridge adapts the foreign shape to local functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdapterKind {
    /// The foreign method and the local function take the same shape.
    Direct,
    /// A translation layer changes the shape on the way through.
    Shim,
    /// A script host interprets the local side.
    Script,
}

impl AdapterKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Shim => "shim",
            Self::Script => "script",
        }
    }
}

/// The kind of value a foreign method's argument or return is, which is what the mapping table
/// is keyed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArgKind {
    String,
    Integer,
    Float,
    Boolean,
    List,
    Map,
}

impl ArgKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Integer => "integer",
            Self::Float => "float",
            Self::Boolean => "boolean",
            Self::List => "list",
            Self::Map => "map",
        }
    }
}

/// A value as the foreign API presents it, before the mapping table turns it into a
/// [`ConfigValue`].
#[derive(Debug, Clone, PartialEq)]
pub enum Foreign {
    Text(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    List(Vec<Foreign>),
    Map(BTreeMap<String, Foreign>),
}

impl Foreign {
    /// Which [`ArgKind`] it is, for a mismatch to name.
    #[must_use]
    pub fn kind(&self) -> ArgKind {
        match self {
            Self::Text(_) => ArgKind::String,
            Self::Integer(_) => ArgKind::Integer,
            Self::Float(_) => ArgKind::Float,
            Self::Boolean(_) => ArgKind::Boolean,
            Self::List(_) => ArgKind::List,
            Self::Map(_) => ArgKind::Map,
        }
    }

    /// The value as a [`ConfigValue`], by its own structure rather than a declared kind.
    #[must_use]
    pub fn to_config(&self) -> ConfigValue {
        match self {
            Self::Text(text) => ConfigValue::Text(text.clone()),
            Self::Integer(number) => ConfigValue::Integer(*number),
            Self::Float(number) => ConfigValue::Float(*number),
            Self::Boolean(flag) => ConfigValue::Boolean(*flag),
            Self::List(items) => ConfigValue::List(items.iter().map(Self::to_config).collect()),
            Self::Map(fields) => ConfigValue::Map(
                fields
                    .iter()
                    .map(|(key, value)| (key.clone(), value.to_config()))
                    .collect(),
            ),
        }
    }
}

/// One declared argument: the name a mismatch reports it by and the kind it must be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgMap {
    pub name: String,
    pub kind: ArgKind,
}

impl ArgMap {
    #[must_use]
    pub fn new(name: &str, kind: ArgKind) -> Self {
        Self {
            name: name.to_owned(),
            kind,
        }
    }
}

/// One method a bridge maps: a foreign name, the local function it calls, the shape of its
/// arguments and the kind it returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodMap {
    pub foreign: String,
    pub local: String,
    pub args: Vec<ArgMap>,
    pub returns: ArgKind,
}

impl MethodMap {
    #[must_use]
    pub fn new(foreign: &str, local: &str, returns: ArgKind) -> Self {
        Self {
            foreign: foreign.to_owned(),
            local: local.to_owned(),
            args: Vec::new(),
            returns,
        }
    }

    #[must_use]
    pub fn arg(mut self, name: &str, kind: ArgKind) -> Self {
        self.args.push(ArgMap::new(name, kind));
        self
    }
}

/// A foreign API, its version, how it adapts, what it may do and the methods it maps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bridge {
    pub name: String,
    pub version: Version,
    pub kind: AdapterKind,
    pub capabilities: Vec<Capability>,
    pub methods: Vec<MethodMap>,
}

impl Bridge {
    #[must_use]
    pub fn new(name: &str, version: Version) -> Self {
        Self {
            name: name.to_owned(),
            version,
            kind: AdapterKind::Direct,
            capabilities: Vec::new(),
            methods: Vec::new(),
        }
    }

    #[must_use]
    pub fn kind(mut self, kind: AdapterKind) -> Self {
        self.kind = kind;
        self
    }

    #[must_use]
    pub fn capability(mut self, capability: Capability) -> Self {
        if !self.capabilities.contains(&capability) {
            self.capabilities.push(capability);
        }
        self
    }

    #[must_use]
    pub fn method(mut self, method: MethodMap) -> Self {
        self.methods.retain(|held| held.foreign != method.foreign);
        self.methods.push(method);
        self
    }

    #[must_use]
    pub fn get_method(&self, foreign: &str) -> Option<&MethodMap> {
        self.methods.iter().find(|method| method.foreign == foreign)
    }
}

type Handler = Arc<dyn Fn(&[ConfigValue]) -> Result<ConfigValue, String> + Send + Sync>;

/// The local implementation bound to one bridge method's local function name.
pub struct Adapter {
    pub bridge: String,
    pub local: String,
    handler: Handler,
}

impl Adapter {
    #[must_use]
    pub fn new<F>(bridge: &str, local: &str, handler: F) -> Self
    where
        F: Fn(&[ConfigValue]) -> Result<ConfigValue, String> + Send + Sync + 'static,
    {
        Self {
            bridge: bridge.to_owned(),
            local: local.to_owned(),
            handler: Arc::new(handler),
        }
    }
}

/// Why a bridge did not do what it was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeError {
    /// No bridge is registered by that name.
    UnknownBridge(String),
    /// The bridge declares no method by that foreign name.
    UnknownMethod { bridge: String, method: String },
    /// The call's argument count does not match the method's declared shape.
    WrongArity {
        bridge: String,
        method: String,
        expected: usize,
        got: usize,
    },
    /// A value's kind does not fit the declared kind, named on both sides.
    TypeMismatch {
        method: String,
        argument: String,
        wanted: String,
        found: String,
    },
    /// The method declares a local function nothing has implemented.
    NoImplementation { bridge: String, local: String },
    /// The local implementation refused the call.
    Implementation { method: String, why: String },
    /// The bridge's version does not satisfy the one a caller required.
    VersionMismatch {
        bridge: String,
        requested: Version,
        offered: Version,
    },
    /// The bridge would need the client's own API, which this host cannot reach.
    Refused(String),
}

impl fmt::Display for BridgeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownBridge(bridge) => write!(formatter, "no bridge is registered as {bridge}"),
            Self::UnknownMethod { bridge, method } => {
                write!(formatter, "{bridge} maps no method {method}")
            }
            Self::WrongArity {
                bridge,
                method,
                expected,
                got,
            } => write!(
                formatter,
                "{bridge}::{method} wants {expected} arguments, the call passed {got}"
            ),
            Self::TypeMismatch {
                method,
                argument,
                wanted,
                found,
            } => write!(
                formatter,
                "{method}'s {argument} wants {wanted}, the value is {found}"
            ),
            Self::NoImplementation { bridge, local } => {
                write!(formatter, "{bridge} needs {local}, which nothing implements")
            }
            Self::Implementation { method, why } => {
                write!(formatter, "the implementation of {method} refused the call: {why}")
            }
            Self::VersionMismatch {
                bridge,
                requested,
                offered,
            } => write!(
                formatter,
                "{bridge} was asked for {requested}, but offers {offered}"
            ),
            Self::Refused(why) => write!(formatter, "refused: {why}"),
        }
    }
}

impl std::error::Error for BridgeError {}

/// What one call mapped in and out, so a mismatch can be explained.
#[derive(Debug, Clone, PartialEq)]
pub struct Translation {
    pub bridge: String,
    pub method: String,
    pub local: String,
    /// The declared kind of each argument, in order.
    pub in_kinds: Vec<String>,
    /// The declared return kind.
    pub out_kind: String,
    pub ok: bool,
    pub note: String,
}

/// Two bridges mapping one foreign method to different local functions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub foreign: String,
    pub bridges: Vec<String>,
    pub reason: String,
}

/// What the bridges here add up to.
#[derive(Debug, Clone, PartialEq)]
pub struct BridgeStatus {
    pub registered: usize,
    pub available: usize,
    pub methods: usize,
    pub adapters: usize,
    pub conflicts: usize,
    pub bridges: Vec<(String, Version)>,
}

#[derive(Default)]
struct Registry {
    bridges: BTreeMap<String, Bridge>,
    adapters: BTreeMap<(String, String), Handler>,
    translations: Vec<Translation>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

fn held() -> MutexGuard<'static, Registry> {
    registry().lock().unwrap_or_else(|error| error.into_inner())
}

/// Publishes a bridge declaration. A second bridge of the same name is refused and answers
/// `false`.
pub fn register(bridge: Bridge) -> bool {
    let name = bridge.name.clone();
    held().bridges.insert(name, bridge).is_none()
}

/// Takes a bridge and its adapters back out, and answers whether it was there.
pub fn unregister(name: &str) -> bool {
    let mut held = held();
    let removed = held.bridges.remove(name).is_some();
    if removed {
        held.adapters.retain(|(bridge, _), _| bridge != name);
    }
    removed
}

/// A registered bridge by foreign API name.
#[must_use]
pub fn get(name: &str) -> Option<Bridge> {
    held().bridges.get(name).cloned()
}

/// Every registered bridge, in name order.
#[must_use]
pub fn get_all() -> Vec<Bridge> {
    held().bridges.values().cloned().collect()
}

/// Whether a bridge is registered and every method it declares has an implementation.
#[must_use]
pub fn is_available(name: &str) -> bool {
    available_in(&held(), name)
}

fn available_in(held: &Registry, name: &str) -> bool {
    match held.bridges.get(name) {
        Some(bridge) => {
            !bridge.methods.is_empty()
                && bridge.methods.iter().all(|method| {
                    held.adapters
                        .contains_key(&(name.to_owned(), method.local.clone()))
                })
        }
        None => false,
    }
}

/// The foreign version of a registered bridge.
#[must_use]
pub fn get_version(name: &str) -> Option<Version> {
    held().bridges.get(name).map(|bridge| bridge.version)
}

/// The capabilities a registered bridge offers.
#[must_use]
pub fn get_capabilities(name: &str) -> Vec<Capability> {
    held()
        .bridges
        .get(name)
        .map(|bridge| bridge.capabilities.clone())
        .unwrap_or_default()
}

/// Resolves a bridge for a caller that requires a version, using the interop rule: the major
/// matches and the bridge's minor is at least the requested one.
pub fn require(name: &str, version: Version) -> Result<Bridge, BridgeError> {
    let held = held();
    let bridge = held
        .bridges
        .get(name)
        .ok_or_else(|| BridgeError::UnknownBridge(name.to_owned()))?;
    if bridge.version.satisfies(version) {
        Ok(bridge.clone())
    } else {
        Err(BridgeError::VersionMismatch {
            bridge: name.to_owned(),
            requested: version,
            offered: bridge.version,
        })
    }
}

/// Binds a local implementation to a bridge method's local function name. A second adapter for
/// the same bridge and local name is refused.
pub fn register_adapter(adapter: Adapter) -> bool {
    let key = (adapter.bridge.clone(), adapter.local.clone());
    held().adapters.insert(key, adapter.handler).is_none()
}

/// Removes an adapter, and answers whether it was there.
pub fn unregister_adapter(bridge: &str, local: &str) -> bool {
    held()
        .adapters
        .remove(&(bridge.to_owned(), local.to_owned()))
        .is_some()
}

/// Calls a foreign method: maps the arguments in, invokes the local implementation, maps the
/// result back, and records the translation.
pub fn call(bridge: &str, method: &str, args: &[Foreign]) -> Result<Foreign, BridgeError> {
    let (result, local, in_kinds, out_kind) = dispatch(bridge, method, args);
    record(Translation {
        bridge: bridge.to_owned(),
        method: method.to_owned(),
        local,
        in_kinds,
        out_kind,
        ok: result.is_ok(),
        note: match &result {
            Ok(_) => "mapped the arguments in and a value out".to_owned(),
            Err(error) => error.to_string(),
        },
    });
    result
}

fn dispatch(
    bridge: &str,
    method: &str,
    args: &[Foreign],
) -> (Result<Foreign, BridgeError>, String, Vec<String>, String) {
    let found = {
        let held = held();
        match held.bridges.get(bridge) {
            None => None,
            Some(declared) => declared.get_method(method).cloned().map(|method| {
                let handler = held
                    .adapters
                    .get(&(bridge.to_owned(), method.local.clone()))
                    .map(Arc::clone);
                (method, handler)
            }),
        }
    };
    let Some((methodmap, handler)) = found else {
        let error = if get(bridge).is_none() {
            BridgeError::UnknownBridge(bridge.to_owned())
        } else {
            BridgeError::UnknownMethod {
                bridge: bridge.to_owned(),
                method: method.to_owned(),
            }
        };
        return (Err(error), String::new(), Vec::new(), String::new());
    };
    let local = methodmap.local.clone();
    let in_kinds: Vec<String> = methodmap
        .args
        .iter()
        .map(|arg| arg.kind.as_str().to_owned())
        .collect();
    let out_kind = methodmap.returns.as_str().to_owned();
    if methodmap.args.len() != args.len() {
        let error = BridgeError::WrongArity {
            bridge: bridge.to_owned(),
            method: method.to_owned(),
            expected: methodmap.args.len(),
            got: args.len(),
        };
        return (Err(error), local, in_kinds, out_kind);
    }
    let mut mapped = Vec::with_capacity(args.len());
    for (arg, value) in methodmap.args.iter().zip(args) {
        match map_in(method, arg, value) {
            Ok(value) => mapped.push(value),
            Err(error) => return (Err(error), local, in_kinds, out_kind),
        }
    }
    let Some(handler) = handler else {
        let error = BridgeError::NoImplementation {
            bridge: bridge.to_owned(),
            local: local.clone(),
        };
        return (Err(error), local, in_kinds, out_kind);
    };
    let produced = match handler(&mapped) {
        Ok(value) => value,
        Err(why) => {
            let error = BridgeError::Implementation {
                method: method.to_owned(),
                why,
            };
            return (Err(error), local, in_kinds, out_kind);
        }
    };
    match map_out(method, methodmap.returns, &produced) {
        Ok(value) => (Ok(value), local, in_kinds, out_kind),
        Err(error) => (Err(error), local, in_kinds, out_kind),
    }
}

/// The mapping table, inbound: a foreign value becomes the [`ConfigValue`] its declared kind
/// names, or a mismatch naming both kinds.
fn map_in(method: &str, arg: &ArgMap, value: &Foreign) -> Result<ConfigValue, BridgeError> {
    match (arg.kind, value) {
        (ArgKind::String, Foreign::Text(text)) => Ok(ConfigValue::Text(text.clone())),
        (ArgKind::Integer, Foreign::Integer(number)) => Ok(ConfigValue::Integer(*number)),
        (ArgKind::Float, Foreign::Float(number)) => Ok(ConfigValue::Float(*number)),
        (ArgKind::Boolean, Foreign::Boolean(flag)) => Ok(ConfigValue::Boolean(*flag)),
        (ArgKind::List, Foreign::List(items)) => {
            Ok(ConfigValue::List(items.iter().map(Foreign::to_config).collect()))
        }
        (ArgKind::Map, Foreign::Map(fields)) => Ok(ConfigValue::Map(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), value.to_config()))
                .collect(),
        )),
        _ => Err(BridgeError::TypeMismatch {
            method: method.to_owned(),
            argument: arg.name.clone(),
            wanted: arg.kind.as_str().to_owned(),
            found: value.kind().as_str().to_owned(),
        }),
    }
}

/// The mapping table, outbound: the local return becomes the foreign value its declared kind
/// names, or a mismatch naming both kinds.
fn map_out(method: &str, wanted: ArgKind, value: &ConfigValue) -> Result<Foreign, BridgeError> {
    let mapped = match wanted {
        ArgKind::String => value.as_text().map(|text| Foreign::Text(text.to_owned())),
        ArgKind::Integer => value.as_integer().map(Foreign::Integer),
        ArgKind::Float => value.as_float().map(Foreign::Float),
        ArgKind::Boolean => value.as_bool().map(Foreign::Boolean),
        ArgKind::List => value.as_list().map(|items| {
            Foreign::List(items.iter().map(to_foreign).collect())
        }),
        ArgKind::Map => value.as_map().map(|fields| {
            Foreign::Map(
                fields
                    .iter()
                    .map(|(key, value)| (key.clone(), to_foreign(value)))
                    .collect(),
            )
        }),
    };
    mapped.ok_or_else(|| BridgeError::TypeMismatch {
        method: method.to_owned(),
        argument: "return".to_owned(),
        wanted: wanted.as_str().to_owned(),
        found: value.kind().as_str().to_owned(),
    })
}

fn to_foreign(value: &ConfigValue) -> Foreign {
    match value {
        ConfigValue::Boolean(flag) => Foreign::Boolean(*flag),
        ConfigValue::Integer(number) => Foreign::Integer(*number),
        ConfigValue::Float(number) | ConfigValue::Percentage(number) => Foreign::Float(*number),
        ConfigValue::Duration(millis) => Foreign::Integer(*millis as i64),
        ConfigValue::List(items) | ConfigValue::Set(items) => {
            Foreign::List(items.iter().map(to_foreign).collect())
        }
        ConfigValue::Map(fields) | ConfigValue::Object(fields) | ConfigValue::Compound(fields) => {
            Foreign::Map(
                fields
                    .iter()
                    .map(|(key, value)| (key.clone(), to_foreign(value)))
                    .collect(),
            )
        }
        other => Foreign::Text(other.render()),
    }
}

fn record(translation: Translation) {
    let mut held = held();
    if held.translations.len() == 64 {
        held.translations.remove(0);
    }
    held.translations.push(translation);
}

/// Every call's translation report, oldest first.
#[must_use]
pub fn get_diagnostics() -> Vec<Translation> {
    held().translations.clone()
}

/// Every foreign method name two bridges map to different local functions.
#[must_use]
pub fn get_conflicts() -> Vec<Conflict> {
    conflicts_in(&held())
}

fn conflicts_in(held: &Registry) -> Vec<Conflict> {
    let mut claims: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (name, bridge) in &held.bridges {
        for method in &bridge.methods {
            claims
                .entry(method.foreign.clone())
                .or_default()
                .push((name.clone(), method.local.clone()));
        }
    }
    claims
        .into_iter()
        .filter_map(|(foreign, claims)| {
            let mut locals: Vec<&str> = claims.iter().map(|(_, local)| local.as_str()).collect();
            locals.sort_unstable();
            locals.dedup();
            if locals.len() < 2 {
                return None;
            }
            let reason = format!("{foreign} maps to more than one local function");
            Some(Conflict {
                foreign,
                bridges: claims.iter().map(|(bridge, _)| bridge.clone()).collect(),
                reason,
            })
        })
        .collect()
}

/// What the bridges here add up to.
#[must_use]
pub fn get_status() -> BridgeStatus {
    let held = held();
    BridgeStatus {
        registered: held.bridges.len(),
        available: held
            .bridges
            .keys()
            .filter(|name| available_in(&held, name))
            .count(),
        methods: held
            .bridges
            .values()
            .map(|bridge| bridge.methods.len())
            .sum(),
        adapters: held.adapters.len(),
        conflicts: conflicts_in(&held).len(),
        bridges: held
            .bridges
            .values()
            .map(|bridge| (bridge.name.clone(), bridge.version))
            .collect(),
    }
}

fn translate(
    domain: &str,
    key: &str,
    wanted: ArgKind,
    payload: &Foreign,
) -> Result<ConfigValue, BridgeError> {
    let arg = ArgMap::new(key, wanted);
    let result = map_in(domain, &arg, payload);
    record(Translation {
        bridge: "(foreign)".to_owned(),
        method: domain.to_owned(),
        local: String::new(),
        in_kinds: vec![wanted.as_str().to_owned()],
        out_kind: wanted.as_str().to_owned(),
        ok: result.is_ok(),
        note: match &result {
            Ok(_) => format!("mapped {key} in as {}", wanted.as_str()),
            Err(error) => error.to_string(),
        },
    });
    result
}

/// Maps a foreign event, which arrives as a map, into a local value.
pub fn translate_event(key: &str, payload: &Foreign) -> Result<ConfigValue, BridgeError> {
    translate("event", key, ArgKind::Map, payload)
}

/// Maps foreign registry entries, which arrive as a map, into a local value.
pub fn translate_registry(key: &str, payload: &Foreign) -> Result<ConfigValue, BridgeError> {
    translate("registry", key, ArgKind::Map, payload)
}

/// Maps a foreign resource reference, which arrives as a string, into a local value.
pub fn translate_resource(key: &str, payload: &Foreign) -> Result<ConfigValue, BridgeError> {
    translate("resource", key, ArgKind::String, payload)
}

/// Maps foreign configuration, which arrives as a map, into a local value.
pub fn translate_config(key: &str, payload: &Foreign) -> Result<ConfigValue, BridgeError> {
    translate("config", key, ArgKind::Map, payload)
}

/// Maps a foreign command invocation, whose arguments arrive as a list, into a local value.
pub fn translate_command(key: &str, payload: &Foreign) -> Result<ConfigValue, BridgeError> {
    translate("command", key, ArgKind::List, payload)
}

/// Maps a foreign network message, which arrives as a map, into a local value.
pub fn translate_network_message(key: &str, payload: &Foreign) -> Result<ConfigValue, BridgeError> {
    translate("network_message", key, ArgKind::Map, payload)
}

/// The client's own classes. There are none to read here: they are compiled into the client,
/// so this answers empty and warns.
#[must_use]
pub fn get_client_classes() -> Vec<String> {
    crate::warn!(
        "a mod asked for the client's own classes: {}",
        why_unknown("classes")
    );
    Vec::new()
}

/// The client's native bindings. This host cannot resolve a native symbol in the client, so
/// this answers empty and warns.
#[must_use]
pub fn get_client_native_bindings() -> Vec<String> {
    crate::warn!(
        "a mod asked for the client's native bindings: {}",
        why_unknown("native")
    );
    Vec::new()
}

/// Bridges into the client's own API. This host cannot call a symbol in the client's address
/// space, so this is refused.
pub fn bridge_into_client(_name: &str) -> Result<Bridge, BridgeError> {
    let why = why_unknown("client");
    crate::warn!("a mod asked this host to bridge into the client: {why}");
    Err(BridgeError::Refused(why))
}

/// Why a question about the client's own API has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "classes" => "the client's classes are compiled into the client, and this host cannot \
                      read them"
            .to_owned(),
        "native" => {
            "the client's native bindings are symbols in the client's image, which this host \
             cannot resolve"
                .to_owned()
        }
        "client" => {
            "bridging into the client means calling a symbol in the client's address space, \
             which this host cannot do safely"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["classes", "native", "client"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        *held() = Registry::default();
    }

    fn bridge() -> Bridge {
        Bridge::new("legacy:math", Version::new(1, 2, 0))
            .kind(AdapterKind::Shim)
            .capability(Capability::FilesystemRead)
            .method(
                MethodMap::new("add", "math_add", ArgKind::Integer)
                    .arg("a", ArgKind::Integer)
                    .arg("b", ArgKind::Integer),
            )
    }

    fn adder() -> Adapter {
        Adapter::new("legacy:math", "math_add", |args| match args {
            [ConfigValue::Integer(a), ConfigValue::Integer(b)] => Ok(ConfigValue::Integer(a + b)),
            _ => Err("math_add wants two integers".to_owned()),
        })
    }

    #[test]
    fn a_bridge_registers_reads_back_and_a_duplicate_is_refused() {
        let _serial = serial();
        fresh();
        assert!(register(bridge()));
        assert!(!register(bridge()), "a second bridge of the same name is refused");
        assert!(get("legacy:math").is_some());
        assert_eq!(get_all().len(), 1);
        assert_eq!(get_version("legacy:math"), Some(Version::new(1, 2, 0)));
        assert_eq!(get_version("legacy:ghost"), None);
        assert_eq!(get_capabilities("legacy:math"), vec![Capability::FilesystemRead]);
        assert!(!is_available("legacy:math"), "nothing implements it yet");

        assert!(register_adapter(adder()));
        assert!(is_available("legacy:math"));
        assert!(!register_adapter(adder()), "a second adapter for the same local is refused");
        assert!(unregister_adapter("legacy:math", "math_add"));
        assert!(!unregister_adapter("legacy:math", "math_add"));
        assert!(!is_available("legacy:math"), "its adapter is gone");

        assert!(unregister("legacy:math"));
        assert!(!unregister("legacy:math"));
        assert!(get("legacy:math").is_none());
        assert!(get_all().is_empty());
    }

    #[test]
    fn a_required_version_is_checked_with_the_interop_rule() {
        let _serial = serial();
        fresh();
        register(bridge());

        let resolved = require("legacy:math", Version::new(1, 0, 0)).expect("1.2.0 serves 1.0.0");
        assert_eq!(resolved.name, "legacy:math");
        assert_eq!(resolved.version, Version::new(1, 2, 0));

        match require("legacy:math", Version::new(1, 5, 0)) {
            Err(BridgeError::VersionMismatch {
                bridge,
                requested,
                offered,
            }) => {
                assert_eq!(bridge, "legacy:math");
                assert_eq!(requested, Version::new(1, 5, 0));
                assert_eq!(offered, Version::new(1, 2, 0));
            }
            other => panic!("expected a version mismatch, got {other:?}"),
        }
        assert!(matches!(
            require("legacy:math", Version::new(2, 0, 0)),
            Err(BridgeError::VersionMismatch { .. })
        ));
        assert!(matches!(
            require("legacy:ghost", Version::new(1, 0, 0)),
            Err(BridgeError::UnknownBridge(_))
        ));
    }

    #[test]
    fn a_call_maps_arguments_in_invokes_and_maps_the_result_out() {
        let _serial = serial();
        fresh();
        register(bridge());
        register_adapter(adder());

        assert_eq!(
            call(
                "legacy:math",
                "add",
                &[Foreign::Integer(20), Foreign::Integer(22)]
            ),
            Ok(Foreign::Integer(42))
        );
        let report = get_diagnostics().last().cloned().expect("a report");
        assert!(report.ok);
        assert_eq!(report.local, "math_add");
        assert_eq!(report.in_kinds, vec!["integer", "integer"]);
        assert_eq!(report.out_kind, "integer");
    }

    #[test]
    fn a_foreign_value_of_the_wrong_kind_is_refused_naming_both_kinds() {
        let _serial = serial();
        fresh();
        register(bridge());
        register_adapter(adder());

        assert_eq!(
            call(
                "legacy:math",
                "add",
                &[Foreign::Text("twenty".to_owned()), Foreign::Integer(22)]
            ),
            Err(BridgeError::TypeMismatch {
                method: "add".to_owned(),
                argument: "a".to_owned(),
                wanted: "integer".to_owned(),
                found: "string".to_owned(),
            })
        );
        let report = get_diagnostics().last().cloned().expect("a report");
        assert!(!report.ok);
        assert!(report.note.contains("integer"));
        assert!(report.note.contains("string"));
    }

    #[test]
    fn unknown_bridge_method_and_wrong_arity_are_each_refused() {
        let _serial = serial();
        fresh();
        register(bridge());
        register_adapter(adder());

        assert_eq!(
            call("legacy:math", "none", &[]),
            Err(BridgeError::UnknownMethod {
                bridge: "legacy:math".to_owned(),
                method: "none".to_owned(),
            })
        );
        assert_eq!(
            call("legacy:ghost", "add", &[]),
            Err(BridgeError::UnknownBridge("legacy:ghost".to_owned()))
        );
        assert_eq!(
            call("legacy:math", "add", &[Foreign::Integer(1)]),
            Err(BridgeError::WrongArity {
                bridge: "legacy:math".to_owned(),
                method: "add".to_owned(),
                expected: 2,
                got: 1,
            })
        );
    }

    #[test]
    fn a_missing_implementation_and_a_failing_one_are_reported() {
        let _serial = serial();
        fresh();
        register(bridge());
        assert_eq!(
            call("legacy:math", "add", &[Foreign::Integer(1), Foreign::Integer(2)]),
            Err(BridgeError::NoImplementation {
                bridge: "legacy:math".to_owned(),
                local: "math_add".to_owned(),
            })
        );

        register_adapter(Adapter::new("legacy:math", "math_add", |_| {
            Err("math_add is closed".to_owned())
        }));
        assert_eq!(
            call("legacy:math", "add", &[Foreign::Integer(1), Foreign::Integer(2)]),
            Err(BridgeError::Implementation {
                method: "add".to_owned(),
                why: "math_add is closed".to_owned(),
            })
        );
    }

    #[test]
    fn a_local_return_that_does_not_fit_its_declared_kind_is_refused() {
        let _serial = serial();
        fresh();
        register(
            Bridge::new("legacy:text", Version::new(1, 0, 0))
                .method(MethodMap::new("name", "text_name", ArgKind::String)),
        );
        register_adapter(Adapter::new("legacy:text", "text_name", |_| {
            Ok(ConfigValue::Integer(7))
        }));

        assert_eq!(
            call("legacy:text", "name", &[]),
            Err(BridgeError::TypeMismatch {
                method: "name".to_owned(),
                argument: "return".to_owned(),
                wanted: "string".to_owned(),
                found: "integer".to_owned(),
            })
        );
    }

    #[test]
    fn the_domain_translations_use_the_same_mapping_table() {
        let _serial = serial();
        fresh();
        let mut event = BTreeMap::new();
        event.insert("id".to_owned(), Foreign::Integer(7));
        assert_eq!(
            translate_event("tick", &Foreign::Map(event.clone())),
            Ok(ConfigValue::Map(BTreeMap::from([(
                "id".to_owned(),
                ConfigValue::Integer(7),
            )])))
        );
        assert_eq!(
            translate_command("give", &Foreign::List(vec![Foreign::Text("sword".to_owned())])),
            Ok(ConfigValue::List(vec![ConfigValue::Text("sword".to_owned())]))
        );
        assert_eq!(
            translate_resource("icon", &Foreign::Integer(3)),
            Err(BridgeError::TypeMismatch {
                method: "resource".to_owned(),
                argument: "icon".to_owned(),
                wanted: "string".to_owned(),
                found: "integer".to_owned(),
            })
        );
        assert_eq!(
            translate_config("keys", &Foreign::Map(event)),
            Ok(ConfigValue::Map(BTreeMap::from([(
                "id".to_owned(),
                ConfigValue::Integer(7),
            )])))
        );
        assert!(get_diagnostics().len() >= 3);
    }

    #[test]
    fn conflicts_status_and_the_client_refusals_are_reported() {
        let _serial = serial();
        fresh();
        register(
            Bridge::new("legacy:a", Version::new(1, 0, 0))
                .method(MethodMap::new("greet", "a_greet", ArgKind::String)),
        );
        register(
            Bridge::new("legacy:b", Version::new(2, 0, 0))
                .method(MethodMap::new("greet", "b_greet", ArgKind::String)),
        );

        let conflicts = get_conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].foreign, "greet");
        assert_eq!(conflicts[0].bridges, vec!["legacy:a", "legacy:b"]);

        let status = get_status();
        assert_eq!(status.registered, 2);
        assert_eq!(status.methods, 2);
        assert_eq!(status.conflicts, 1);
        assert_eq!(status.available, 0, "neither is implemented");
        assert_eq!(
            status.bridges,
            vec![
                ("legacy:a".to_owned(), Version::new(1, 0, 0)),
                ("legacy:b".to_owned(), Version::new(2, 0, 0)),
            ]
        );

        assert!(get_client_classes().is_empty());
        assert!(get_client_native_bindings().is_empty());
        assert!(matches!(
            bridge_into_client("client:tick"),
            Err(BridgeError::Refused(_))
        ));
        assert_eq!(refusals().len(), 3);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert_ne!(why_unknown("classes"), why_unknown("native"));
        assert!(why_unknown("something else").contains("not something"));
    }
}
