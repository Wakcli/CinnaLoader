//! Typed accessors over a store a mod owns, in the forms the wishlist names.
//!
//! An [`Accessor`] is a name, a target, a declared [`ConfigType`] and an [`AccessForm`] — by
//! index, by key, by dotted path, first, last or optional. It reads a [`ValueStore`], a value
//! tree a mod owns, and the read comes in two spellings. [`get`] is the *checked* read: it
//! validates what it found against the accessor's kind and answers a real [`AccessError`].
//! [`get_unchecked`] is the caller's promise — it resolves the form but does not validate the
//! kind, so a value the checked read refuses is returned as it stands. Which one to use is the
//! mod's decision and is written down here rather than guessed.
//!
//! [`get_field`] and [`set_field`] read and write a named entry of the store directly, and
//! [`invoke`] calls a method registered against the store, after an arity check. The client's
//! own field and method tables and its obfuscation mappings are refused with a sentence per
//! kind, reachable through [`refusals`].
//!
//! ```
//! use cinnaloader::accessors::{self, AccessForm, Accessor, ValueStore};
//! use cinnaloader::config::{ConfigType, ConfigValue};
//!
//! accessors::register(Accessor::new("first", "hello:list", AccessForm::first(), ConfigType::Integer));
//! let store = ValueStore::new(ConfigValue::List(vec![ConfigValue::Integer(7)]));
//! assert_eq!(accessors::get(&store, "first"), Ok(ConfigValue::Integer(7)));
//! accessors::unregister("first");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::config::{ConfigType, ConfigValue};

/// How an accessor picks the value it reads.
#[derive(Debug, Clone, PartialEq)]
pub enum AccessForm {
    /// The item at an index of a list.
    Index(usize),
    /// The value under a key of a map.
    Key(String),
    /// A `.`-separated path through nested maps and list indices.
    Path(String),
    /// The first item of a list.
    First,
    /// The last item of a list.
    Last,
    /// A key that may be absent; a fallback stands in for it.
    Optional(String, ConfigValue),
}

impl AccessForm {
    #[must_use]
    pub fn index(index: usize) -> Self {
        Self::Index(index)
    }

    #[must_use]
    pub fn key(key: &str) -> Self {
        Self::Key(key.to_owned())
    }

    #[must_use]
    pub fn path(path: &str) -> Self {
        Self::Path(path.to_owned())
    }

    #[must_use]
    pub fn first() -> Self {
        Self::First
    }

    #[must_use]
    pub fn last() -> Self {
        Self::Last
    }

    #[must_use]
    pub fn optional(key: &str, fallback: ConfigValue) -> Self {
        Self::Optional(key.to_owned(), fallback)
    }

    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Index(index) => format!("index {index}"),
            Self::Key(key) => format!("key {key}"),
            Self::Path(path) => format!("path {path}"),
            Self::First => "first".to_owned(),
            Self::Last => "last".to_owned(),
            Self::Optional(key, _) => format!("optional key {key}"),
        }
    }
}

/// A named, typed read from a store a mod owns.
#[derive(Debug, Clone, PartialEq)]
pub struct Accessor {
    pub name: String,
    pub target: String,
    pub form: AccessForm,
    pub kind: ConfigType,
    pub version: String,
}

impl Accessor {
    #[must_use]
    pub fn new(name: &str, target: &str, form: AccessForm, kind: ConfigType) -> Self {
        Self {
            name: name.to_owned(),
            target: target.to_owned(),
            form,
            kind,
            version: String::new(),
        }
    }

    #[must_use]
    pub fn version(mut self, version: &str) -> Self {
        self.version = version.to_owned();
        self
    }

    /// Resolves the form against a store's root, without checking the declared kind.
    fn resolve(&self, root: &ConfigValue) -> Result<ConfigValue, AccessError> {
        match &self.form {
            AccessForm::Index(index) => match root {
                ConfigValue::List(items) | ConfigValue::Set(items) => {
                    items.get(*index).cloned().ok_or_else(|| AccessError::OutOfRange {
                        accessor: self.name.clone(),
                        index: *index,
                        len: items.len(),
                    })
                }
                other => Err(AccessError::NotAList {
                    accessor: self.name.clone(),
                    found: other.kind().as_str(),
                }),
            },
            AccessForm::First => match root {
                ConfigValue::List(items) | ConfigValue::Set(items) => {
                    items.first().cloned().ok_or_else(|| AccessError::Empty {
                        accessor: self.name.clone(),
                    })
                }
                other => Err(AccessError::NotAList {
                    accessor: self.name.clone(),
                    found: other.kind().as_str(),
                }),
            },
            AccessForm::Last => match root {
                ConfigValue::List(items) | ConfigValue::Set(items) => {
                    items.last().cloned().ok_or_else(|| AccessError::Empty {
                        accessor: self.name.clone(),
                    })
                }
                other => Err(AccessError::NotAList {
                    accessor: self.name.clone(),
                    found: other.kind().as_str(),
                }),
            },
            AccessForm::Key(key) => match root.as_map() {
                Some(map) => map.get(key).cloned().ok_or_else(|| AccessError::Missing {
                    accessor: self.name.clone(),
                    key: key.clone(),
                }),
                None => Err(AccessError::NotAMap {
                    accessor: self.name.clone(),
                    found: root.kind().as_str(),
                }),
            },
            AccessForm::Optional(key, fallback) => match root.as_map() {
                Some(map) => Ok(map.get(key).cloned().unwrap_or_else(|| fallback.clone())),
                None => Err(AccessError::NotAMap {
                    accessor: self.name.clone(),
                    found: root.kind().as_str(),
                }),
            },
            AccessForm::Path(path) => {
                let mut current = root;
                for part in path.split('.') {
                    current = match current {
                        ConfigValue::Map(fields)
                        | ConfigValue::Object(fields)
                        | ConfigValue::Compound(fields) => {
                            fields.get(part).ok_or_else(|| AccessError::Missing {
                                accessor: self.name.clone(),
                                key: path.clone(),
                            })?
                        }
                        ConfigValue::List(items) | ConfigValue::Set(items) => {
                            let index: usize =
                                part.parse().map_err(|_| AccessError::Missing {
                                    accessor: self.name.clone(),
                                    key: path.clone(),
                                })?;
                            items.get(index).ok_or_else(|| AccessError::OutOfRange {
                                accessor: self.name.clone(),
                                index,
                                len: items.len(),
                            })?
                        }
                        other => {
                            return Err(AccessError::NotAMap {
                                accessor: self.name.clone(),
                                found: other.kind().as_str(),
                            })
                        }
                    };
                }
                Ok(current.clone())
            }
        }
    }
}

/// A value tree a mod owns, which accessors read.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueStore {
    root: ConfigValue,
}

impl ValueStore {
    #[must_use]
    pub fn new(root: ConfigValue) -> Self {
        Self { root }
    }

    #[must_use]
    pub fn root(&self) -> &ConfigValue {
        &self.root
    }
}

/// A method registered against a store's root, for [`invoke`].
#[derive(Debug, Clone)]
pub struct Method {
    pub name: String,
    pub arity: usize,
    call: fn(&ConfigValue, &[ConfigValue]) -> ConfigValue,
}

impl Method {
    #[must_use]
    pub fn new(
        name: &str,
        arity: usize,
        call: fn(&ConfigValue, &[ConfigValue]) -> ConfigValue,
    ) -> Self {
        Self {
            name: name.to_owned(),
            arity,
            call,
        }
    }

    #[must_use]
    pub fn call(&self, store: &ConfigValue, args: &[ConfigValue]) -> ConfigValue {
        (self.call)(store, args)
    }
}

/// What can go wrong reading a store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessError {
    /// No accessor with that name is registered.
    UnknownAccessor { accessor: String },
    /// The form wanted a list and found something else.
    NotAList {
        accessor: String,
        found: &'static str,
    },
    /// The form wanted a map and found something else.
    NotAMap {
        accessor: String,
        found: &'static str,
    },
    /// The index is past the end of the list.
    OutOfRange {
        accessor: String,
        index: usize,
        len: usize,
    },
    /// The list is empty, so first or last has nothing to give.
    Empty { accessor: String },
    /// The form wanted a key that is not there.
    Missing { accessor: String, key: String },
    /// A checked read found a value of the wrong kind.
    WrongKind {
        accessor: String,
        wanted: &'static str,
        found: &'static str,
    },
    /// A store entry is not set.
    Absent { key: String },
    /// No method with that name is registered.
    UnknownMethod { method: String },
    /// A method was called with the wrong number of arguments.
    WrongArity {
        method: String,
        wanted: usize,
        found: usize,
    },
}

impl fmt::Display for AccessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownAccessor { accessor } => {
                write!(formatter, "{accessor} is not a registered accessor")
            }
            Self::NotAList { accessor, found } => {
                write!(formatter, "{accessor} wanted a list, found {found}")
            }
            Self::NotAMap { accessor, found } => {
                write!(formatter, "{accessor} wanted a map, found {found}")
            }
            Self::OutOfRange {
                accessor,
                index,
                len,
            } => write!(
                formatter,
                "{accessor} wanted index {index} of {len} item(s)"
            ),
            Self::Empty { accessor } => write!(formatter, "{accessor} wanted an item of an empty list"),
            Self::Missing { accessor, key } => write!(formatter, "{accessor} found no {key}"),
            Self::WrongKind {
                accessor,
                wanted,
                found,
            } => write!(formatter, "{accessor} holds {found}, not {wanted}"),
            Self::Absent { key } => write!(formatter, "{key} is not set in the store"),
            Self::UnknownMethod { method } => {
                write!(formatter, "{method} is not a registered method")
            }
            Self::WrongArity {
                method,
                wanted,
                found,
            } => write!(formatter, "{method} takes {wanted} argument(s), got {found}"),
        }
    }
}

impl std::error::Error for AccessError {}

#[derive(Default)]
struct Registries {
    accessors: BTreeMap<String, Accessor>,
    methods: BTreeMap<String, Method>,
    errors: Vec<String>,
}

fn registries() -> &'static Mutex<Registries> {
    static REGISTRIES: OnceLock<Mutex<Registries>> = OnceLock::new();
    REGISTRIES.get_or_init(|| Mutex::new(Registries::default()))
}

fn with<T>(body: impl FnOnce(&mut Registries) -> T) -> T {
    let mut guard = registries()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn lookup(name: &str) -> Result<Accessor, AccessError> {
    with(|registries| registries.accessors.get(name).cloned()).ok_or_else(|| {
        AccessError::UnknownAccessor {
            accessor: name.to_owned(),
        }
    })
}

fn record(error: &AccessError) {
    with(|registries| registries.errors.push(error.to_string()));
}

/// Registers an accessor, or replaces the one with that name. Answers whether it was new.
pub fn register(accessor: Accessor) -> bool {
    with(|registries| {
        registries
            .accessors
            .insert(accessor.name.clone(), accessor)
            .is_none()
    })
}

/// Forgets an accessor.
pub fn unregister(name: &str) -> bool {
    with(|registries| registries.accessors.remove(name).is_some())
}

/// Whether an accessor is registered.
#[must_use]
pub fn is_available(name: &str) -> bool {
    with(|registries| registries.accessors.contains_key(name))
}

/// Every registered accessor, which is the mod's mapping table.
#[must_use]
pub fn get_mappings() -> Vec<Accessor> {
    with(|registries| registries.accessors.values().cloned().collect())
}

/// The registered accessors that apply to a version: those tagged with it, and the untagged.
#[must_use]
pub fn get_version_mapping(version: &str) -> Vec<Accessor> {
    with(|registries| {
        registries
            .accessors
            .values()
            .filter(|accessor| accessor.version.is_empty() || accessor.version == version)
            .cloned()
            .collect()
    })
}

/// The checked read: resolves the form, then validates the found value against the accessor's
/// declared kind, recording a real error when it does not.
pub fn get(store: &ValueStore, name: &str) -> Result<ConfigValue, AccessError> {
    let accessor = lookup(name)?;
    let value = accessor.resolve(store.root())?;
    if !accessor.kind.accepts(value.kind()) {
        let error = AccessError::WrongKind {
            accessor: name.to_owned(),
            wanted: accessor.kind.as_str(),
            found: value.kind().as_str(),
        };
        record(&error);
        return Err(error);
    }
    Ok(value)
}

/// The unchecked read: resolves the form and returns what it found, leaving the kind to the
/// caller's promise.
pub fn get_unchecked(store: &ValueStore, name: &str) -> Result<ConfigValue, AccessError> {
    let accessor = lookup(name)?;
    accessor.resolve(store.root())
}

/// Reads a named entry of the store directly.
pub fn get_field(store: &ValueStore, key: &str) -> Result<ConfigValue, AccessError> {
    match store.root.as_map() {
        Some(map) => map
            .get(key)
            .cloned()
            .ok_or_else(|| AccessError::Absent { key: key.to_owned() }),
        None => Err(AccessError::NotAMap {
            accessor: key.to_owned(),
            found: store.root.kind().as_str(),
        }),
    }
}

/// Writes a named entry of the store directly.
pub fn set_field(
    store: &mut ValueStore,
    key: &str,
    value: ConfigValue,
) -> Result<(), AccessError> {
    match &mut store.root {
        ConfigValue::Map(fields)
        | ConfigValue::Object(fields)
        | ConfigValue::Compound(fields) => {
            fields.insert(key.to_owned(), value);
            Ok(())
        }
        other => Err(AccessError::NotAMap {
            accessor: key.to_owned(),
            found: other.kind().as_str(),
        }),
    }
}

/// Registers a method a store can be invoked with.
pub fn register_method(method: Method) -> bool {
    with(|registries| {
        registries
            .methods
            .insert(method.name.clone(), method)
            .is_none()
    })
}

/// Calls a registered method against the store, checking the arity first.
pub fn invoke(
    store: &ValueStore,
    method: &str,
    args: &[ConfigValue],
) -> Result<ConfigValue, AccessError> {
    let held = with(|registries| registries.methods.get(method).cloned()).ok_or_else(|| {
        AccessError::UnknownMethod {
            method: method.to_owned(),
        }
    })?;
    if held.arity != args.len() {
        return Err(AccessError::WrongArity {
            method: method.to_owned(),
            wanted: held.arity,
            found: args.len(),
        });
    }
    Ok(held.call(store.root(), args))
}

/// Every error a checked read recorded.
#[must_use]
pub fn get_errors() -> Vec<String> {
    with(|registries| registries.errors.clone())
}

/// Forgets the recorded errors, and answers how many there were.
pub fn clear_errors() -> usize {
    with(|registries| {
        let count = registries.errors.len();
        registries.errors.clear();
        count
    })
}

/// Why an accessor question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "mapping" => {
            "the client's obfuscation mappings are its build's own, behind a hook the project \
             records as never"
                .to_owned()
        }
        "field" => {
            "a private client field is reached through a mapping this host does not hold"
                .to_owned()
        }
        "method" => {
            "a client method is reached through the same mapping, which this host never sees"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// The client's obfuscation mappings. Empty: see [`why_unknown`].
#[must_use]
pub fn get_client_mappings() -> Vec<Accessor> {
    crate::warn!(
        "a mod asked for the client's mappings: {}",
        why_unknown("mapping")
    );
    Vec::new()
}

/// A name from the client's mappings. `None`.
#[must_use]
pub fn resolve_client_mapping(_name: &str) -> Option<String> {
    crate::warn!(
        "a mod asked for a client mapping: {}",
        why_unknown("mapping")
    );
    None
}

/// A private field of the client. `None`.
#[must_use]
pub fn get_client_field(_type_name: &str, _field: &str) -> Option<ConfigValue> {
    crate::warn!(
        "a mod asked for a private client field: {}",
        why_unknown("field")
    );
    None
}

/// A method of the client. `None`.
#[must_use]
pub fn invoke_client_method(
    _type_name: &str,
    _method: &str,
    _args: &[ConfigValue],
) -> Option<ConfigValue> {
    crate::warn!(
        "a mod asked to invoke a client method: {}",
        why_unknown("method")
    );
    None
}

/// What this host cannot reach about the client's internals, for a panel or a report.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["mapping", "field", "method"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|registries| {
            registries.accessors.clear();
            registries.methods.clear();
            registries.errors.clear();
        });
    }

    fn list() -> ValueStore {
        ValueStore::new(ConfigValue::List(vec![
            ConfigValue::Integer(10),
            ConfigValue::Integer(20),
            ConfigValue::Integer(30),
        ]))
    }

    fn map() -> ValueStore {
        let mut fields = BTreeMap::new();
        fields.insert("name".to_owned(), ConfigValue::Text("wisp".to_owned()));
        fields.insert(
            "pos".to_owned(),
            ConfigValue::List(vec![ConfigValue::Integer(1), ConfigValue::Integer(2)]),
        );
        ValueStore::new(ConfigValue::Map(fields))
    }

    #[test]
    fn an_accessor_is_registered_and_read_back_by_name() {
        let _serial = serial();
        fresh();
        assert!(register(Accessor::new(
            "hello:first",
            "hello:list",
            AccessForm::first(),
            ConfigType::Integer
        )));
        assert!(is_available("hello:first"));
        assert!(!is_available("nothing"));
        assert_eq!(get_mappings().len(), 1);
        assert!(!register(Accessor::new(
            "hello:first",
            "hello:list",
            AccessForm::last(),
            ConfigType::Integer
        )));
        assert_eq!(get_mappings().len(), 1, "the second replaces the first");
        assert!(unregister("hello:first"));
        assert!(!unregister("hello:first"));
    }

    #[test]
    fn an_accessor_reads_by_index_key_and_path() {
        let _serial = serial();
        fresh();
        register(Accessor::new(
            "hello:second",
            "hello:list",
            AccessForm::index(1),
            ConfigType::Integer,
        ));
        assert_eq!(get(&list(), "hello:second"), Ok(ConfigValue::Integer(20)));
        register(Accessor::new(
            "hello:name",
            "hello:map",
            AccessForm::key("name"),
            ConfigType::Text,
        ));
        assert_eq!(
            get(&map(), "hello:name"),
            Ok(ConfigValue::Text("wisp".to_owned()))
        );
        register(Accessor::new(
            "hello:y",
            "hello:map",
            AccessForm::path("pos.1"),
            ConfigType::Integer,
        ));
        assert_eq!(get(&map(), "hello:y"), Ok(ConfigValue::Integer(2)));
        assert_eq!(
            AccessForm::path("pos.1").describe(),
            "path pos.1".to_owned()
        );
    }

    #[test]
    fn first_last_and_optional_answer_exact_results() {
        let _serial = serial();
        fresh();
        register(Accessor::new(
            "hello:first",
            "hello:list",
            AccessForm::first(),
            ConfigType::Integer,
        ));
        register(Accessor::new(
            "hello:last",
            "hello:list",
            AccessForm::last(),
            ConfigType::Integer,
        ));
        register(Accessor::new(
            "hello:missing",
            "hello:map",
            AccessForm::optional("absent", ConfigValue::Text("none".to_owned())),
            ConfigType::Text,
        ));
        assert_eq!(get(&list(), "hello:first"), Ok(ConfigValue::Integer(10)));
        assert_eq!(get(&list(), "hello:last"), Ok(ConfigValue::Integer(30)));
        assert_eq!(
            get(&map(), "hello:missing"),
            Ok(ConfigValue::Text("none".to_owned()))
        );
    }

    #[test]
    fn a_checked_read_refuses_what_an_unchecked_read_accepts() {
        let _serial = serial();
        fresh();
        register(Accessor::new(
            "hello:name",
            "hello:map",
            AccessForm::key("name"),
            ConfigType::Integer,
        ));
        assert_eq!(
            get(&map(), "hello:name"),
            Err(AccessError::WrongKind {
                accessor: "hello:name".to_owned(),
                wanted: "integer",
                found: "text",
            })
        );
        assert_eq!(
            get_unchecked(&map(), "hello:name"),
            Ok(ConfigValue::Text("wisp".to_owned())),
            "the caller promised the kind, so nothing is validated"
        );
        assert_eq!(
            get_errors(),
            vec!["hello:name holds text, not integer".to_owned()]
        );
        assert_eq!(clear_errors(), 1);
        assert!(get_errors().is_empty());
    }

    #[test]
    fn out_of_range_missing_and_not_a_list_are_real_errors() {
        let _serial = serial();
        fresh();
        register(Accessor::new(
            "hello:beyond",
            "hello:list",
            AccessForm::index(9),
            ConfigType::Integer,
        ));
        register(Accessor::new(
            "hello:none",
            "hello:map",
            AccessForm::key("absent"),
            ConfigType::Text,
        ));
        register(Accessor::new(
            "hello:empty",
            "hello:list",
            AccessForm::first(),
            ConfigType::Integer,
        ));
        register(Accessor::new(
            "hello:wrong",
            "hello:map",
            AccessForm::index(0),
            ConfigType::Integer,
        ));
        assert_eq!(
            get(&list(), "hello:beyond"),
            Err(AccessError::OutOfRange {
                accessor: "hello:beyond".to_owned(),
                index: 9,
                len: 3,
            })
        );
        assert_eq!(
            get(&map(), "hello:none"),
            Err(AccessError::Missing {
                accessor: "hello:none".to_owned(),
                key: "absent".to_owned(),
            })
        );
        assert_eq!(
            get(&ValueStore::new(ConfigValue::List(Vec::new())), "hello:empty"),
            Err(AccessError::Empty {
                accessor: "hello:empty".to_owned(),
            })
        );
        assert_eq!(
            get(&map(), "hello:wrong"),
            Err(AccessError::NotAList {
                accessor: "hello:wrong".to_owned(),
                found: "map",
            })
        );
        assert_eq!(
            get(&list(), "nothing"),
            Err(AccessError::UnknownAccessor {
                accessor: "nothing".to_owned(),
            })
        );
    }

    #[test]
    fn a_field_reads_and_writes_the_store_directly() {
        let _serial = serial();
        fresh();
        assert_eq!(get_field(&map(), "name"), Ok(ConfigValue::Text("wisp".to_owned())));
        assert_eq!(
            get_field(&map(), "absent"),
            Err(AccessError::Absent {
                key: "absent".to_owned(),
            })
        );
        assert_eq!(
            get_field(&list(), "name"),
            Err(AccessError::NotAMap {
                accessor: "name".to_owned(),
                found: "list",
            })
        );
        let mut store = map();
        assert!(set_field(&mut store, "name", ConfigValue::Text("ember".to_owned())).is_ok());
        assert_eq!(get_field(&store, "name"), Ok(ConfigValue::Text("ember".to_owned())));
        let mut flat = list();
        assert!(set_field(&mut flat, "name", ConfigValue::Text("ember".to_owned())).is_err());
    }

    #[test]
    fn a_method_is_invoked_with_an_arity_check() {
        let _serial = serial();
        fresh();
        fn sum(_store: &ConfigValue, args: &[ConfigValue]) -> ConfigValue {
            ConfigValue::Integer(args.iter().filter_map(ConfigValue::as_integer).sum::<i64>())
        }
        assert!(register_method(Method::new("hello:sum", 2, sum)));
        assert_eq!(
            invoke(
                &list(),
                "hello:sum",
                &[ConfigValue::Integer(3), ConfigValue::Integer(4)]
            ),
            Ok(ConfigValue::Integer(7))
        );
        assert_eq!(
            invoke(&list(), "hello:sum", &[ConfigValue::Integer(3)]),
            Err(AccessError::WrongArity {
                method: "hello:sum".to_owned(),
                wanted: 2,
                found: 1,
            })
        );
        assert_eq!(
            invoke(&list(), "nope", &[]),
            Err(AccessError::UnknownMethod {
                method: "nope".to_owned(),
            })
        );
    }

    #[test]
    fn a_version_mapping_filters_the_table_and_the_client_is_refused() {
        let _serial = serial();
        fresh();
        register(
            Accessor::new("hello:a", "hello:map", AccessForm::first(), ConfigType::Integer)
                .version("1.0.0"),
        );
        register(Accessor::new(
            "hello:b",
            "hello:map",
            AccessForm::last(),
            ConfigType::Integer,
        ));
        assert_eq!(get_version_mapping("1.0.0").len(), 2, "an untagged accessor applies to all");
        assert_eq!(get_version_mapping("2.0.0").len(), 1);
        assert!(get_client_mappings().is_empty());
        assert!(resolve_client_mapping("aa").is_none());
        assert!(get_client_field("hello:point", "x").is_none());
        assert!(invoke_client_method("hello:point", "run", &[]).is_none());
        assert_eq!(refusals().len(), 3);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }
}
