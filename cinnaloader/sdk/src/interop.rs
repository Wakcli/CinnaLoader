//! Inter-mod communication: real API contracts between the mods in this process, and the
//! client's own API surface refused.
//!
//! Where section 65's channels carry *data* between mods, this carries *contracts*: one mod
//! declares an API and publishes an implementation, another resolves it and calls it.
//!
//! * **A declaration.** [`ModApi`] is a name, a [`Version`] of `major.minor.patch`, a list of
//!   [`Capability`]s and a list of [`MethodSpec`]s — each method's name, arity and
//!   [`ReturnKind`], as real data.
//! * **A provider registry.** [`register_service`] publishes a [`Service`] and its handler;
//!   publishing the same API name twice is refused, and [`unregister_service`] takes it back.
//! * **A consumer lookup.** [`require`] resolves a request against a documented rule: a
//!   provider at the same major with a minor at least the request's satisfies it, a different
//!   major does not, and an API whose provider is absent, a request that no provider's version
//!   fits, and a capability the provider does not offer are each their own [`ApiError`], so a
//!   caller can tell them apart.
//! * **A real call.** [`call_service`] finds the method, checks the argument count against its
//!   declared arity and calls the registered handler, answering its value or a real error.
//! * **Negotiation.** [`get_compatibility_status`] reports what was asked for, what the
//!   provider offers and what was resolved, so a mismatch is a sentence rather than a `false`.
//! * **An event bus.** [`subscribe`] follows a topic and [`send_message`] publishes to it,
//!   answering how many subscribers it reached; [`unsubscribe`] leaves.
//!
//! What this host cannot do is reach the client's own API. The client's public API surface,
//! its registered capabilities and calling into it all live inside the client's address space;
//! each is refused with its own sentence in [`why_unknown`], listed by [`refusals`], and never
//! answers a fake success.
//!
//! ```
//! use cinnaloader::capability::Capability;
//! use cinnaloader::config::ConfigValue;
//! use cinnaloader::interop::{self, ApiError, MethodSpec, ModApi, ReturnKind, Service, Version};
//!
//! let api = ModApi::new("hello:math", Version::new(1, 2, 0))
//!     .capability(Capability::FilesystemRead)
//!     .method(MethodSpec::new("add", 2, ReturnKind::Int));
//! assert!(interop::register_service(
//!     Service::new(api, "hello:provider", Version::new(1, 0, 0)),
//!     |args| match args {
//!         [ConfigValue::Integer(a), ConfigValue::Integer(b)] => Ok(ConfigValue::Integer(a + b)),
//!         _ => Err("add wants two integers".to_owned()),
//!     },
//! ));
//! assert!(interop::has_mod("hello:math"));
//! assert_eq!(interop::get_api_version("hello:math"), Some(Version::new(1, 2, 0)));
//! let service = interop::require("hello:math", Version::new(1, 0, 0)).expect("compatible");
//! assert_eq!(service.provider, "hello:provider");
//! assert!(matches!(
//!     interop::require("hello:math", Version::new(2, 0, 0)),
//!     Err(ApiError::VersionMismatch { .. })
//! ));
//! assert_eq!(
//!     interop::call_service("hello:math", "add", &[ConfigValue::Integer(2), ConfigValue::Integer(3)]),
//!     Ok(ConfigValue::Integer(5))
//! );
//! assert!(interop::get_public_api().is_empty());
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use crate::capability::Capability;
pub use crate::config::ConfigValue as Value;

/// A version a mod names its API with, `major.minor.patch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    #[must_use]
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self { major, minor, patch }
    }

    /// Whether a provider at `self` satisfies a consumer asking for `requested`: the major
    /// matches and the provider's minor is at least the consumer's. A different major is not
    /// compatible, and the patch is not part of the rule — a patched provider still serves an
    /// request that asked for its minor.
    #[must_use]
    pub fn satisfies(self, requested: Self) -> bool {
        self.major == requested.major && self.minor >= requested.minor
    }
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl FromStr for Version {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = text.trim().split('.').collect();
        if parts.len() != 3 {
            return Err(format!("{text} is not major.minor.patch"));
        }
        let number = |part: &str| {
            part.parse::<u64>()
                .map_err(|_| format!("{part} is not a number"))
        };
        Ok(Self::new(
            number(parts[0])?,
            number(parts[1])?,
            number(parts[2])?,
        ))
    }
}

/// What a method answers with. The wishlist's return kinds, spelled as variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReturnKind {
    Unit,
    Bool,
    Int,
    Float,
    Text,
    Value,
}

impl ReturnKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unit => "unit",
            Self::Bool => "bool",
            Self::Int => "int",
            Self::Float => "float",
            Self::Text => "text",
            Self::Value => "value",
        }
    }
}

/// One method an API declares: its name, how many arguments it takes and what it answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodSpec {
    pub name: String,
    pub arity: usize,
    pub returns: ReturnKind,
}

impl MethodSpec {
    #[must_use]
    pub fn new(name: &str, arity: usize, returns: ReturnKind) -> Self {
        Self {
            name: name.to_owned(),
            arity,
            returns,
        }
    }
}

/// An API a mod declares and another mod may publish, resolve and call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModApi {
    pub name: String,
    pub version: Version,
    pub capabilities: Vec<Capability>,
    pub methods: Vec<MethodSpec>,
}

impl ModApi {
    #[must_use]
    pub fn new(name: &str, version: Version) -> Self {
        Self {
            name: name.to_owned(),
            version,
            capabilities: Vec::new(),
            methods: Vec::new(),
        }
    }

    #[must_use]
    pub fn capability(mut self, capability: Capability) -> Self {
        if !self.capabilities.contains(&capability) {
            self.capabilities.push(capability);
        }
        self
    }

    #[must_use]
    pub fn method(mut self, spec: MethodSpec) -> Self {
        self.methods.retain(|held| held.name != spec.name);
        self.methods.push(spec);
        self
    }

    #[must_use]
    pub fn with_method(self, name: &str, arity: usize, returns: ReturnKind) -> Self {
        self.method(MethodSpec::new(name, arity, returns))
    }

    /// The declaration of one method, if the API has it.
    #[must_use]
    pub fn get_method(&self, name: &str) -> Option<&MethodSpec> {
        self.methods.iter().find(|spec| spec.name == name)
    }
}

/// A published implementation of a declared API: the declaration, the mod that offers it and
/// that mod's own version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub api: ModApi,
    pub provider: String,
    pub mod_version: Version,
}

impl Service {
    #[must_use]
    pub fn new(api: ModApi, provider: &str, mod_version: Version) -> Self {
        Self {
            api,
            provider: provider.to_owned(),
            mod_version,
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.api.name
    }

    #[must_use]
    pub fn as_provider(&self) -> ApiProvider {
        ApiProvider {
            mod_id: self.provider.clone(),
            mod_version: self.mod_version,
        }
    }
}

/// The mod behind a service, as it identifies itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiProvider {
    pub mod_id: String,
    pub mod_version: Version,
}

/// A mod that wants an API: the name and version it asks for and the capabilities it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiConsumer {
    pub api: String,
    pub version: Version,
    pub capabilities: Vec<Capability>,
}

impl ApiConsumer {
    #[must_use]
    pub fn new(api: &str, version: Version) -> Self {
        Self {
            api: api.to_owned(),
            version,
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

    /// Resolves the request, refusing it when no provider fits or one the consumer needs is
    /// missing.
    pub fn require(&self) -> Result<Service, ApiError> {
        let service = resolve_compatible(&self.api, self.version)?;
        for capability in &self.capabilities {
            if !service.api.capabilities.contains(capability) {
                return Err(ApiError::MissingCapability {
                    api: self.api.clone(),
                    capability: *capability,
                });
            }
        }
        Ok(service)
    }
}

/// Why an interop call did not do what it was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// No provider publishes an API by that name.
    UnknownApi(String),
    /// The API has no method by that name.
    UnknownMethod { api: String, method: String },
    /// The call's argument count does not match the method's declared arity.
    WrongArity {
        api: String,
        method: String,
        expected: usize,
        got: usize,
    },
    /// The provider's version does not satisfy the requested one.
    VersionMismatch {
        api: String,
        requested: Version,
        offered: Version,
        provider: String,
    },
    /// The provider does not offer a capability the consumer needs.
    MissingCapability {
        api: String,
        capability: Capability,
    },
    /// The provider's own handler refused the call.
    ProviderError(String),
    /// The call would need the client's own API, which this host cannot reach.
    Refused(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownApi(api) => write!(formatter, "no provider publishes {api}"),
            Self::UnknownMethod { api, method } => {
                write!(formatter, "{api} declares no method {method}")
            }
            Self::WrongArity {
                api,
                method,
                expected,
                got,
            } => write!(
                formatter,
                "{api}::{method} wants {expected} arguments, the call passed {got}"
            ),
            Self::VersionMismatch {
                api,
                requested,
                offered,
                provider,
            } => write!(
                formatter,
                "{api} was asked for {requested}, but {provider} offers {offered}"
            ),
            Self::MissingCapability { api, capability } => {
                write!(formatter, "{api} does not offer the {capability} capability")
            }
            Self::ProviderError(why) => write!(formatter, "the provider refused the call: {why}"),
            Self::Refused(why) => write!(formatter, "refused: {why}"),
        }
    }
}

impl std::error::Error for ApiError {}

/// A named hook an API opens for other mods to extend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionPoint {
    pub name: String,
    pub owner: String,
    pub api: String,
}

impl ExtensionPoint {
    #[must_use]
    pub fn new(name: &str, api: &str) -> Self {
        Self {
            name: name.to_owned(),
            owner: crate::metadata::id(),
            api: api.to_owned(),
        }
    }

    #[must_use]
    pub fn owner(mut self, owner: &str) -> Self {
        self.owner = owner.to_owned();
        self
    }
}

/// One contribution a mod made to an extension point.
#[derive(Debug, Clone, PartialEq)]
pub struct Extension {
    pub id: u64,
    pub point: String,
    pub provider: String,
    pub value: Value,
}

/// A bridge from an API name a consumer asks for to the service that really serves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModBridge {
    pub name: String,
    pub target: String,
    pub owner: String,
}

impl ModBridge {
    #[must_use]
    pub fn new(name: &str, target: &str) -> Self {
        Self {
            name: name.to_owned(),
            target: target.to_owned(),
            owner: crate::metadata::id(),
        }
    }
}

/// An explicit mapping from a version a consumer asks for to the version a provider offers,
/// for the case where a deliberate shim makes a major difference usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityAdapter {
    pub api: String,
    pub from: Version,
    pub to: Version,
}

impl CompatibilityAdapter {
    #[must_use]
    pub fn new(api: &str, from: Version, to: Version) -> Self {
        Self {
            api: api.to_owned(),
            from,
            to,
        }
    }
}

/// What a consumer asked for, what a provider offers and what was resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct CompatibilityStatus {
    pub api: String,
    pub requested: Version,
    pub provider: Option<String>,
    pub offered: Option<Version>,
    /// The version the consumer got, or `None` when the request was refused.
    pub resolved: Option<Version>,
    pub capabilities: Vec<Capability>,
    pub missing: Vec<Capability>,
    pub bridged: Option<String>,
    pub compatible: bool,
    pub reason: String,
}

/// Every shared key/value the mods in this process publish.
pub type SharedDataStore = BTreeMap<String, Value>;

type Handler = Arc<dyn Fn(&[Value]) -> Result<Value, String> + Send + Sync>;
type MessageHandler = Arc<dyn Fn(&Value) + Send + Sync>;
type Subscription = (String, String, MessageHandler);

struct Published {
    service: Service,
    handler: Handler,
}

#[derive(Default)]
struct Registry {
    providers: BTreeMap<String, Published>,
    points: BTreeMap<String, ExtensionPoint>,
    extensions: BTreeMap<u64, Extension>,
    bridges: BTreeMap<String, ModBridge>,
    adapters: BTreeMap<(String, Version), CompatibilityAdapter>,
    subs: BTreeMap<u64, Subscription>,
    shared: BTreeMap<String, Value>,
    next_extension: u64,
    next_sub: u64,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

fn held() -> MutexGuard<'static, Registry> {
    registry().lock().unwrap_or_else(|error| error.into_inner())
}

/// The declaration a provider published under this API name.
#[must_use]
pub fn get_mod_api(name: &str) -> Option<ModApi> {
    held().providers.get(name).map(|published| published.service.api.clone())
}

/// Whether any provider publishes this API name.
#[must_use]
pub fn has_mod(name: &str) -> bool {
    held().providers.contains_key(name)
}

/// The version of the mod that provides an API, by the provider's own id.
#[must_use]
pub fn get_mod_version(mod_id: &str) -> Option<Version> {
    held()
        .providers
        .values()
        .find(|published| published.service.provider == mod_id)
        .map(|published| published.service.mod_version)
}

/// The version of a declared API.
#[must_use]
pub fn get_api_version(name: &str) -> Option<Version> {
    held().providers.get(name).map(|published| published.service.api.version)
}

/// Publishes an implementation of a declared API. A second publish of the same API name is
/// refused and answers `false`.
pub fn register_service<F>(service: Service, handler: F) -> bool
where
    F: Fn(&[Value]) -> Result<Value, String> + Send + Sync + 'static,
{
    let name = service.api.name.clone();
    let mut held = held();
    if held.providers.contains_key(&name) {
        return false;
    }
    held.providers.insert(
        name,
        Published {
            service,
            handler: Arc::new(handler),
        },
    );
    true
}

/// Takes a published service back out, and answers whether it was there.
pub fn unregister_service(name: &str) -> bool {
    held().providers.remove(name).is_some()
}

/// A published service by API name.
#[must_use]
pub fn get_service(name: &str) -> Option<Service> {
    held().providers.get(name).map(|published| published.service.clone())
}

/// Every published service whose API offers this capability.
#[must_use]
pub fn find_services(capability: Capability) -> Vec<Service> {
    held()
        .providers
        .values()
        .filter(|published| published.service.api.capabilities.contains(&capability))
        .map(|published| published.service.clone())
        .collect()
}

/// Opens an extension point under this mod's id. A second point of the same name is refused.
pub fn register_extension_point(point: ExtensionPoint) -> bool {
    let name = point.name.clone();
    let mut held = held();
    if held.points.contains_key(&name) {
        return false;
    }
    held.points.insert(name, point);
    true
}

/// Closes an extension point and drops its extensions, answering whether it was there.
pub fn unregister_extension_point(name: &str) -> bool {
    let mut held = held();
    if held.points.remove(name).is_none() {
        return false;
    }
    held.extensions.retain(|_, extension| extension.point != name);
    true
}

/// An open extension point by name.
#[must_use]
pub fn get_extension_point(name: &str) -> Option<ExtensionPoint> {
    held().points.get(name).cloned()
}

/// Contributes to an extension point, answering the extension's handle, or `None` when the
/// point is not open.
pub fn register_extension(point: &str, value: Value) -> Option<u64> {
    let mut held = held();
    if !held.points.contains_key(point) {
        return None;
    }
    held.next_extension += 1;
    let id = held.next_extension;
    held.extensions.insert(
        id,
        Extension {
            id,
            point: point.to_owned(),
            provider: crate::metadata::id(),
            value,
        },
    );
    Some(id)
}

/// Removes an extension, and answers whether it was there.
pub fn unregister_extension(id: u64) -> bool {
    held().extensions.remove(&id).is_some()
}

/// Every contribution to an extension point, in registration order.
#[must_use]
pub fn get_extensions(point: &str) -> Vec<Extension> {
    held()
        .extensions
        .values()
        .filter(|extension| extension.point == point)
        .cloned()
        .collect()
}

/// Resolves a consumer's request against the providers here, and answers the service that
/// satisfies it. See [`Version::satisfies`] for the rule; a registered
/// [`CompatibilityAdapter`] can make a different major usable on purpose.
pub fn require(name: &str, version: Version) -> Result<Service, ApiError> {
    resolve_compatible(name, version)
}

/// The same as [`require`], plus a capability the consumer needs.
pub fn require_capability(
    name: &str,
    version: Version,
    capability: Capability,
) -> Result<Service, ApiError> {
    ApiConsumer::new(name, version).capability(capability).require()
}

fn resolve_compatible(name: &str, requested: Version) -> Result<Service, ApiError> {
    let held = held();
    let service = resolve_in(&held, name).ok_or_else(|| ApiError::UnknownApi(name.to_owned()))?;
    if service.api.version.satisfies(requested) {
        return Ok(service);
    }
    if let Some(adapter) = held.adapters.get(&(name.to_owned(), requested)) {
        if service.api.version.satisfies(adapter.to) {
            return Ok(service);
        }
    }
    Err(ApiError::VersionMismatch {
        api: name.to_owned(),
        requested,
        offered: service.api.version,
        provider: service.provider.clone(),
    })
}

fn resolve_in(held: &Registry, name: &str) -> Option<Service> {
    if let Some(published) = held.providers.get(name) {
        return Some(published.service.clone());
    }
    let bridge = held.bridges.get(name)?;
    held.providers.get(&bridge.target).map(|published| published.service.clone())
}

/// Calls a method on a published service: finds the API and method, checks the argument count
/// against the declared arity and calls the registered handler.
pub fn call_service(name: &str, method: &str, args: &[Value]) -> Result<Value, ApiError> {
    let (handler, spec) = {
        let held = held();
        let published = held
            .providers
            .get(name)
            .ok_or_else(|| ApiError::UnknownApi(name.to_owned()))?;
        let spec = published
            .service
            .api
            .get_method(method)
            .cloned()
            .ok_or_else(|| ApiError::UnknownMethod {
                api: name.to_owned(),
                method: method.to_owned(),
            })?;
        (Arc::clone(&published.handler), spec)
    };
    if args.len() != spec.arity {
        return Err(ApiError::WrongArity {
            api: name.to_owned(),
            method: method.to_owned(),
            expected: spec.arity,
            got: args.len(),
        });
    }
    handler(args).map_err(ApiError::ProviderError)
}

/// The wishlist's spelling of [`call_service`].
pub fn call(name: &str, method: &str, args: &[Value]) -> Result<Value, ApiError> {
    call_service(name, method, args)
}

/// Follows a topic under this mod's own id, and answers the subscription's handle.
pub fn subscribe<F>(topic: &str, handler: F) -> u64
where
    F: Fn(&Value) + Send + Sync + 'static,
{
    subscribe_for(&crate::metadata::id(), topic, handler)
}

/// The same, attributed to an explicit subscriber — how a peer in this process is modelled.
pub fn subscribe_for<F>(owner: &str, topic: &str, handler: F) -> u64
where
    F: Fn(&Value) + Send + Sync + 'static,
{
    let mut held = held();
    held.next_sub += 1;
    let id = held.next_sub;
    held.subs
        .insert(id, (owner.to_owned(), topic.to_owned(), Arc::new(handler)));
    id
}

/// Leaves a topic, and answers whether the subscription was there.
pub fn unsubscribe(id: u64) -> bool {
    held().subs.remove(&id).is_some()
}

/// Publishes a value on a topic and answers how many subscribers it reached.
pub fn send_message(topic: &str, payload: Value) -> usize {
    let handlers: Vec<MessageHandler> = held()
        .subs
        .values()
        .filter(|(_, on, _)| on == topic)
        .map(|(_, _, handler)| Arc::clone(handler))
        .collect();
    let mut delivered = 0;
    for handler in handlers {
        handler(&payload);
        delivered += 1;
    }
    delivered
}

/// A value another mod published under this key.
#[must_use]
pub fn get_shared_data(key: &str) -> Option<Value> {
    held().shared.get(key).cloned()
}

/// Publishes a value under a key, answering whether the key was new.
pub fn set_shared_data(key: &str, value: Value) -> bool {
    held().shared.insert(key.to_owned(), value).is_none()
}

/// Removes a key, and answers whether it was there.
pub fn remove_shared_data(key: &str) -> bool {
    held().shared.remove(key).is_some()
}

/// Every shared key/value the mods here publish.
#[must_use]
pub fn shared_data() -> SharedDataStore {
    held().shared.clone()
}

/// Registers a bridge from an API name to the service that serves it, so a consumer that
/// knows the old name still resolves. A second bridge of the same name is refused.
pub fn register_bridge(bridge: ModBridge) -> bool {
    let name = bridge.name.clone();
    held().bridges.insert(name, bridge).is_none()
}

/// Removes a bridge, and answers whether it was there.
pub fn unregister_bridge(name: &str) -> bool {
    held().bridges.remove(name).is_some()
}

/// A registered bridge by name.
#[must_use]
pub fn get_bridge(name: &str) -> Option<ModBridge> {
    held().bridges.get(name).cloned()
}

/// Registers an explicit compatibility adapter that maps a requested version to a version a
/// provider offers, for a deliberate cross-major shim. A second adapter for the same API and
/// requested version is refused.
pub fn register_compatibility_adapter(adapter: CompatibilityAdapter) -> bool {
    let key = (adapter.api.clone(), adapter.from);
    held().adapters.insert(key, adapter).is_none()
}

/// What a consumer asked for, what the provider offers and what was resolved, so a mismatch
/// can be explained rather than only refused.
#[must_use]
pub fn get_compatibility_status(name: &str, version: Version) -> CompatibilityStatus {
    let held = held();
    let adapter = held.adapters.get(&(name.to_owned(), version)).cloned();
    let mut status = CompatibilityStatus {
        api: name.to_owned(),
        requested: version,
        provider: None,
        offered: None,
        resolved: None,
        capabilities: Vec::new(),
        missing: Vec::new(),
        bridged: None,
        compatible: false,
        reason: format!("no provider publishes {name}"),
    };
    let Some(service) = resolve_in(&held, name) else {
        return status;
    };
    status.provider = Some(service.provider.clone());
    status.offered = Some(service.api.version);
    status.capabilities = service.api.capabilities.clone();
    status.bridged = held
        .bridges
        .get(name)
        .filter(|bridge| bridge.target == service.api.name)
        .map(|bridge| bridge.target.clone());
    let target = adapter.as_ref().map_or(version, |adapter| adapter.to);
    if service.api.version.satisfies(target) {
        status.compatible = true;
        status.resolved = Some(service.api.version);
        status.reason = match &adapter {
            Some(adapter) => format!(
                "adapter maps {name} {} to {}, satisfied by {}",
                adapter.from, adapter.to, service.api.version
            ),
            None => format!(
                "{} offers {}, which satisfies a request for {version}",
                service.provider, service.api.version
            ),
        };
    } else {
        status.reason = format!(
            "{} offers {}, which does not satisfy a request for {version}",
            service.provider, service.api.version
        );
    }
    status
}

/// The client's own public API surface. There is none to read here: it is compiled into the
/// client, so this answers empty and warns.
#[must_use]
pub fn get_public_api() -> Vec<String> {
    refuse_names("public_api")
}

/// The capabilities the client registers for itself. This host cannot see the client's
/// registry, so this answers empty and warns.
#[must_use]
pub fn client_capabilities() -> Vec<String> {
    refuse_names("capabilities")
}

/// Calls into the client's own API. This host cannot invoke a symbol in the client safely, so
/// this is refused.
pub fn call_client(_method: &str, _args: &[Value]) -> Result<Value, ApiError> {
    crate::warn!(
        "a mod asked this host to call into the client's API: {}",
        why_unknown("client")
    );
    Err(ApiError::Refused(why_unknown("client")))
}

/// Why a question about the client's own API has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "public_api" => {
            "the client's public API surface is compiled into the client, and this host cannot \
             read it"
                .to_owned()
        }
        "capabilities" => {
            "the client's registered capabilities live in the client's own registry, which this \
             host cannot see"
                .to_owned()
        }
        "client" => {
            "calling into the client means invoking a symbol in the client's address space, \
             which this host cannot do safely"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["public_api", "capabilities", "client"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refuse_names(what: &str) -> Vec<String> {
    crate::warn!(
        "a mod asked this host for the client's own API: {}",
        why_unknown(what)
    );
    Vec::new()
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

    fn service(version: Version) -> Service {
        let api = ModApi::new("hello:math", version)
            .capability(Capability::FilesystemRead)
            .with_method("add", 2, ReturnKind::Int);
        Service::new(api, "hello:provider", Version::new(1, 0, 0))
    }

    fn publish(version: Version) {
        assert!(register_service(service(version), |args| match args {
            [Value::Integer(a), Value::Integer(b)] => Ok(Value::Integer(a + b)),
            _ => Err("add wants two integers".to_owned()),
        }));
    }

    #[test]
    fn a_compatible_provider_resolves_and_a_major_mismatch_is_refused() {
        let _serial = serial();
        fresh();
        publish(Version::new(1, 2, 0));

        assert_eq!(get_api_version("hello:math"), Some(Version::new(1, 2, 0)));
        assert_eq!(get_mod_version("hello:provider"), Some(Version::new(1, 0, 0)));
        assert_eq!(get_mod_version("hello:ghost"), None);

        let resolved = require("hello:math", Version::new(1, 0, 0)).expect("1.2.0 serves 1.0.0");
        assert_eq!(resolved.provider, "hello:provider");

        match require("hello:math", Version::new(1, 5, 0)) {
            Err(ApiError::VersionMismatch {
                api,
                requested,
                offered,
                provider,
            }) => {
                assert_eq!(api, "hello:math");
                assert_eq!(requested, Version::new(1, 5, 0));
                assert_eq!(offered, Version::new(1, 2, 0));
                assert_eq!(provider, "hello:provider");
            }
            other => panic!("expected a version mismatch, got {other:?}"),
        }

        match require("hello:math", Version::new(2, 0, 0)) {
            Err(ApiError::VersionMismatch { offered, .. }) => {
                assert_eq!(offered, Version::new(1, 2, 0));
            }
            other => panic!("a different major must not satisfy, got {other:?}"),
        }
    }

    #[test]
    fn publishing_twice_is_refused_and_unpublishing_works() {
        let _serial = serial();
        fresh();
        assert!(register_service(service(Version::new(1, 0, 0)), |_| {
            Ok(Value::Boolean(true))
        }));
        assert!(
            !register_service(service(Version::new(1, 0, 0)), |_| {
                Ok(Value::Boolean(true))
            }),
            "a second publish of the same name is refused"
        );
        assert!(has_mod("hello:math"));
        assert_eq!(get_service("hello:math").map(|held| held.provider), Some("hello:provider".to_owned()));
        assert!(unregister_service("hello:math"));
        assert!(!unregister_service("hello:math"));
        assert!(!has_mod("hello:math"));
        assert_eq!(get_mod_api("hello:math"), None);
    }

    #[test]
    fn a_missing_capability_is_refused_naming_it_and_differs_from_a_version_mismatch() {
        let _serial = serial();
        fresh();
        publish(Version::new(1, 2, 0));

        let held = require_capability("hello:math", Version::new(1, 0, 0), Capability::FilesystemRead)
            .expect("the provider offers it");
        assert!(held.api.capabilities.contains(&Capability::FilesystemRead));

        match require_capability("hello:math", Version::new(1, 0, 0), Capability::PacketSend) {
            Err(ApiError::MissingCapability { api, capability }) => {
                assert_eq!(api, "hello:math");
                assert_eq!(capability, Capability::PacketSend);
                assert!(ApiError::MissingCapability {
                    api: api.clone(),
                    capability,
                }
                .to_string()
                    .contains("packet_send"));
            }
            other => panic!("expected a missing capability, got {other:?}"),
        }

        let wrong_version = require_capability(
            "hello:math",
            Version::new(2, 0, 0),
            Capability::FilesystemRead,
        )
        .expect_err("a major mismatch is refused");
        assert!(matches!(wrong_version, ApiError::VersionMismatch { .. }));
        assert_ne!(
            wrong_version,
            ApiError::MissingCapability {
                api: "hello:math".to_owned(),
                capability: Capability::FilesystemRead,
            },
            "the two refusals are distinct"
        );
    }

    #[test]
    fn a_call_dispatches_checks_arity_and_reports_provider_errors() {
        let _serial = serial();
        fresh();
        publish(Version::new(1, 2, 0));

        assert_eq!(
            call_service("hello:math", "add", &[Value::Integer(20), Value::Integer(22)]),
            Ok(Value::Integer(42))
        );
        assert_eq!(call("hello:math", "add", &[Value::Integer(1), Value::Integer(2)]), Ok(Value::Integer(3)));

        assert_eq!(
            call_service("hello:math", "add", &[Value::Integer(1)]),
            Err(ApiError::WrongArity {
                api: "hello:math".to_owned(),
                method: "add".to_owned(),
                expected: 2,
                got: 1,
            })
        );
        assert_eq!(
            call_service("hello:math", "none", &[]),
            Err(ApiError::UnknownMethod {
                api: "hello:math".to_owned(),
                method: "none".to_owned(),
            })
        );
        assert_eq!(
            call_service("hello:missing", "add", &[]),
            Err(ApiError::UnknownApi("hello:missing".to_owned()))
        );
        assert!(matches!(
            call_service("hello:math", "add", &[Value::Text("x".to_owned()), Value::Text("y".to_owned())]),
            Err(ApiError::ProviderError(_))
        ));
    }

    #[test]
    fn negotiation_reports_what_was_asked_got_and_offered() {
        let _serial = serial();
        fresh();
        publish(Version::new(1, 2, 0));

        let ok = get_compatibility_status("hello:math", Version::new(1, 0, 0));
        assert!(ok.compatible);
        assert_eq!(ok.requested, Version::new(1, 0, 0));
        assert_eq!(ok.offered, Some(Version::new(1, 2, 0)));
        assert_eq!(ok.resolved, Some(Version::new(1, 2, 0)));
        assert_eq!(ok.provider.as_deref(), Some("hello:provider"));
        assert!(ok.capabilities.contains(&Capability::FilesystemRead));
        assert!(ok.reason.contains("1.2.0"));

        let bad = get_compatibility_status("hello:math", Version::new(2, 0, 0));
        assert!(!bad.compatible);
        assert_eq!(bad.resolved, None);
        assert_eq!(bad.offered, Some(Version::new(1, 2, 0)));
        assert!(bad.reason.contains("2.0.0"));

        let ghost = get_compatibility_status("hello:ghost", Version::new(1, 0, 0));
        assert!(!ghost.compatible);
        assert!(ghost.provider.is_none());
        assert!(ghost.reason.contains("no provider"));
    }

    #[test]
    fn the_event_bus_delivers_to_distinct_subscribers() {
        let _serial = serial();
        fresh();
        let first = subscribe_for("hello:a", "hello:tick", |_value| {});
        let second = subscribe_for("hello:b", "hello:tick", |_value| {});
        let _wrong_topic = subscribe_for("hello:c", "hello:other", |_value| {});

        assert_eq!(send_message("hello:tick", Value::Integer(1)), 2);
        assert_eq!(send_message("hello:other", Value::Integer(1)), 1);
        assert_eq!(send_message("hello:none", Value::Integer(1)), 0);

        assert!(unsubscribe(first));
        assert!(!unsubscribe(first));
        assert_eq!(send_message("hello:tick", Value::Integer(2)), 1);
        assert!(unsubscribe(second));
        assert_eq!(send_message("hello:tick", Value::Integer(3)), 0);
    }

    #[test]
    fn extension_points_and_extensions_are_registered_and_read_back() {
        let _serial = serial();
        fresh();
        publish(Version::new(1, 2, 0));
        let point = ExtensionPoint::new("hello:menu", "hello:math").owner("hello:host");
        assert!(register_extension_point(point.clone()));
        assert!(!register_extension_point(point.clone()));
        assert_eq!(get_extension_point("hello:menu"), Some(point));

        let first = register_extension("hello:menu", Value::Text("entry-a".to_owned()))
            .expect("the point is open");
        register_extension("hello:menu", Value::Text("entry-b".to_owned())).expect("the point is open");
        assert_eq!(get_extensions("hello:menu").len(), 2);
        assert_eq!(register_extension("hello:closed", Value::Boolean(true)), None);

        assert_eq!(
            find_services(Capability::FilesystemRead).len(),
            1,
            "the published service offers it"
        );

        assert!(unregister_extension(first));
        assert!(!unregister_extension(first));
        assert_eq!(get_extensions("hello:menu").len(), 1);
        assert!(unregister_extension_point("hello:menu"));
        assert!(get_extensions("hello:menu").is_empty(), "closing drops its extensions");
        assert!(!unregister_extension_point("hello:menu"));
    }

    #[test]
    fn shared_data_bridges_adapters_and_refusals_answer_honestly() {
        let _serial = serial();
        fresh();
        assert!(set_shared_data("hello:seed", Value::Integer(7)));
        assert!(!set_shared_data("hello:seed", Value::Integer(8)));
        assert_eq!(get_shared_data("hello:seed"), Some(Value::Integer(8)));
        assert!(remove_shared_data("hello:seed"));
        assert!(!remove_shared_data("hello:seed"));

        publish(Version::new(2, 1, 0));
        assert_eq!(
            require("hello:math", Version::new(2, 0, 0)).map(|held| held.api.version).ok(),
            Some(Version::new(2, 1, 0))
        );
        assert!(matches!(
            require("hello:old", Version::new(1, 0, 0)),
            Err(ApiError::UnknownApi(_))
        ));
        assert!(register_bridge(ModBridge::new("hello:old", "hello:math")));
        assert!(!register_bridge(ModBridge::new("hello:old", "hello:math")));
        assert!(require("hello:old", Version::new(2, 0, 0)).is_ok(), "the bridge resolves it");
        assert!(get_bridge("hello:old").is_some());
        assert!(unregister_bridge("hello:old"));

        assert!(register_compatibility_adapter(CompatibilityAdapter::new(
            "hello:math",
            Version::new(3, 0, 0),
            Version::new(2, 1, 0),
        )));
        assert!(require("hello:math", Version::new(3, 0, 0)).is_ok(), "the adapter makes it usable");

        assert!(get_public_api().is_empty());
        assert!(client_capabilities().is_empty());
        assert!(matches!(
            call_client("tick", &[]),
            Err(ApiError::Refused(_))
        ));
        assert_eq!(refusals().len(), 3);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert_ne!(why_unknown("public_api"), why_unknown("client"));
        assert!(why_unknown("something else").contains("not something"));
    }
}
