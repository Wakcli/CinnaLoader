//! Server-side permissions: nodes, roles and users, resolved to a decision.
//!
//! A **node** is a dotted permission string, `hello.commands.give`. A rule is a pattern:
//! `*` matches every node, `hello.*` matches `hello.commands.give` and any deeper node under
//! `hello.` but not `other.thing`, and a `*` may only be the last segment. A **role** has a
//! priority, a list of grants and denials, an optional prefix and suffix, and an optional
//! parent it inherits from. A **user** holds roles.
//!
//! Resolution, for one node: the matching rule with the highest priority decides; at equal
//! priority an explicit deny beats an allow; a remaining tie goes to the more specific
//! pattern, so a specific grant overrides a broader one. No match is a deny. The deciding
//! role is reported, so a surprise can be explained instead of guessed at.
//!
//! The whole set is a [`crate::config::ConfigValue`] that round trips through
//! [`crate::serialization`], and [`export_json`]/[`import_json`] are the same value in JSON.
//!
//! ```
//! use cinnaloader::server_permissions::{self, Role};
//!
//! server_permissions::create_group(Role::new("member", 0).grant("hello.*")).expect("a new role");
//! server_permissions::add_to_group("alice", "member").expect("the role exists");
//! assert!(server_permissions::check("alice", "hello.commands.give"));
//! assert!(!server_permissions::check("alice", "other.thing"));
//! assert_eq!(
//!     server_permissions::resolve("alice", "other.thing").to_string(),
//!     "deny (no role matched)"
//! );
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::config::ConfigValue;

/// One permission node: a dotted string, and the pattern it is matched as.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Node {
    path: String,
}

impl Node {
    /// Builds a node, refusing an empty path or a `*` anywhere but the last segment.
    pub fn new(path: &str) -> Result<Self, Error> {
        let path = path.trim();
        if path.is_empty() {
            return Err(Error::BadNode(path.to_owned()));
        }
        let segments: Vec<&str> = path.split('.').collect();
        let last = segments.len() - 1;
        for (at, segment) in segments.iter().enumerate() {
            if segment.is_empty() || (segment.contains('*') && (*segment != "*" || at != last)) {
                return Err(Error::BadNode(path.to_owned()));
            }
        }
        Ok(Self {
            path: path.to_owned(),
        })
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub fn is_wildcard(&self) -> bool {
        self.path.contains('*')
    }

    /// How many literal segments it has, which is what makes one pattern more specific than
    /// another.
    #[must_use]
    pub fn specificity(&self) -> usize {
        self.path.split('.').filter(|segment| *segment != "*").count()
    }

    /// Whether this pattern covers `node`. `*` covers everything, `hello.*` covers any node
    /// under `hello.` and nothing else.
    #[must_use]
    pub fn matches(&self, node: &str) -> bool {
        if let Some(prefix) = self.path.strip_suffix(".*") {
            return node.len() > prefix.len() + 1
                && node.starts_with(prefix)
                && node.as_bytes()[prefix.len()] == b'.';
        }
        if self.path == "*" {
            return !node.is_empty();
        }
        self.path == node
    }
}

impl fmt::Display for Node {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.path)
    }
}

/// A role: a name, a priority, what it grants and denies, and an optional parent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Role {
    pub name: String,
    pub priority: i32,
    pub grants: Vec<String>,
    pub denials: Vec<String>,
    pub parent: Option<String>,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
}

impl Role {
    #[must_use]
    pub fn new(name: &str, priority: i32) -> Self {
        Self {
            name: name.to_owned(),
            priority,
            grants: Vec::new(),
            denials: Vec::new(),
            parent: None,
            prefix: None,
            suffix: None,
        }
    }

    #[must_use]
    pub fn grant(mut self, node: &str) -> Self {
        self.grants.push(node.to_owned());
        self
    }

    #[must_use]
    pub fn deny(mut self, node: &str) -> Self {
        self.denials.push(node.to_owned());
        self
    }

    #[must_use]
    pub fn parent(mut self, parent: &str) -> Self {
        self.parent = Some(parent.to_owned());
        self
    }

    #[must_use]
    pub fn prefix(mut self, prefix: &str) -> Self {
        self.prefix = Some(prefix.to_owned());
        self
    }

    #[must_use]
    pub fn suffix(mut self, suffix: &str) -> Self {
        self.suffix = Some(suffix.to_owned());
        self
    }
}

/// A user: a name and the roles it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub name: String,
    pub roles: Vec<String>,
}

/// What the set decided about one node, and who decided it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub allowed: bool,
    pub role: Option<String>,
    pub pattern: Option<String>,
}

impl fmt::Display for Decision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.allowed, &self.role, &self.pattern) {
            (true, Some(role), Some(pattern)) => write!(formatter, "allow by {role} via {pattern}"),
            (false, Some(role), Some(pattern)) => write!(formatter, "deny by {role} via {pattern}"),
            _ => formatter.write_str("deny (no role matched)"),
        }
    }
}

/// What went wrong inside the set, each naming the thing at fault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    DuplicateRole(String),
    UnknownRole(String),
    BadNode(String),
    Cycle(String),
    RoleInUse { role: String, by: String },
    BadValue(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateRole(name) => write!(formatter, "a role named {name} is already defined"),
            Self::UnknownRole(name) => write!(formatter, "there is no role named {name}"),
            Self::BadNode(node) => write!(
                formatter,
                "{node} is not a permission node: a '*' may only be the last segment"
            ),
            Self::Cycle(path) => write!(formatter, "inheritance would cycle: {path}"),
            Self::RoleInUse { role, by } => {
                write!(formatter, "{by} inherits from {role}, so it cannot be deleted")
            }
            Self::BadValue(what) => formatter.write_str(what),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Default)]
struct State {
    nodes: BTreeMap<String, Node>,
    roles: BTreeMap<String, Role>,
    users: BTreeMap<String, User>,
    saved: Option<ConfigValue>,
}

fn held() -> &'static Mutex<State> {
    static HELD: OnceLock<Mutex<State>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(State::default()))
}

fn with<T>(body: impl FnOnce(&mut State) -> T) -> T {
    let mut guard = held().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Remembers a permission node, so [`get_effective_permissions`] can name it. Answers
/// whether it was new.
pub fn register(node: &str) -> bool {
    let Ok(node) = Node::new(node) else {
        return false;
    };
    with(|state| state.nodes.insert(node.path.clone(), node).is_none())
}

/// Forgets a registered node, and answers whether it was there.
pub fn unregister(node: &str) -> bool {
    with(|state| state.nodes.remove(node.trim()).is_some())
}

/// A registered node by path.
#[must_use]
pub fn get(node: &str) -> Option<Node> {
    with(|state| state.nodes.get(node.trim()).cloned())
}

/// Adds a role. A name already taken is refused.
pub fn create_group(role: Role) -> Result<(), Error> {
    if role.name.trim().is_empty() {
        return Err(Error::BadValue("a role needs a name".to_owned()));
    }
    for pattern in role.grants.iter().chain(role.denials.iter()) {
        Node::new(pattern)?;
    }
    with(|state| {
        if state.roles.contains_key(&role.name) {
            return Err(Error::DuplicateRole(role.name.clone()));
        }
        if let Some(parent) = &role.parent {
            if !state.roles.contains_key(parent) {
                return Err(Error::UnknownRole(parent.clone()));
            }
        }
        let mut probe = state.roles.clone();
        probe.insert(role.name.clone(), role.clone());
        if let Some(path) = cycle_path(&probe, &role.name) {
            return Err(Error::Cycle(path));
        }
        state.roles.insert(role.name.clone(), role);
        Ok(())
    })
}

/// Removes a role. One another role inherits from is refused, naming the dependant.
pub fn delete_group(name: &str) -> Result<(), Error> {
    with(|state| {
        if !state.roles.contains_key(name) {
            return Err(Error::UnknownRole(name.to_owned()));
        }
        if let Some(dependant) = state
            .roles
            .values()
            .find(|role| role.parent.as_deref() == Some(name))
        {
            return Err(Error::RoleInUse {
                role: name.to_owned(),
                by: dependant.name.clone(),
            });
        }
        state.roles.remove(name);
        for user in state.users.values_mut() {
            user.roles.retain(|role| role != name);
        }
        Ok(())
    })
}

/// Every role, by name.
#[must_use]
pub fn get_groups() -> Vec<Role> {
    with(|state| state.roles.values().cloned().collect())
}

/// One role by name.
#[must_use]
pub fn get_group(name: &str) -> Option<Role> {
    with(|state| state.roles.get(name).cloned())
}

/// Gives a role a node, and takes it off the role's denials.
pub fn grant(role: &str, node: &str) -> Result<(), Error> {
    let node = Node::new(node)?;
    with(|state| {
        let Some(held) = state.roles.get_mut(role) else {
            return Err(Error::UnknownRole(role.to_owned()));
        };
        held.denials.retain(|denial| denial != &node.path);
        if !held.grants.contains(&node.path) {
            held.grants.push(node.path.clone());
        }
        Ok(())
    })
}

/// Denies a role a node, and takes it off the role's grants.
pub fn revoke(role: &str, node: &str) -> Result<(), Error> {
    let node = Node::new(node)?;
    with(|state| {
        let Some(held) = state.roles.get_mut(role) else {
            return Err(Error::UnknownRole(role.to_owned()));
        };
        held.grants.retain(|grant| grant != &node.path);
        if !held.denials.contains(&node.path) {
            held.denials.push(node.path.clone());
        }
        Ok(())
    })
}

/// Puts a user in a role.
pub fn add_to_group(user: &str, role: &str) -> Result<(), Error> {
    with(|state| {
        if !state.roles.contains_key(role) {
            return Err(Error::UnknownRole(role.to_owned()));
        }
        let entry = state.users.entry(user.to_owned()).or_insert_with(|| User {
            name: user.to_owned(),
            roles: Vec::new(),
        });
        if !entry.roles.contains(&role.to_owned()) {
            entry.roles.push(role.to_owned());
        }
        Ok(())
    })
}

/// Takes a user out of a role, and answers whether it was in it.
pub fn remove_from_group(user: &str, role: &str) -> Result<bool, Error> {
    with(|state| {
        if !state.roles.contains_key(role) {
            return Err(Error::UnknownRole(role.to_owned()));
        }
        match state.users.get_mut(user) {
            Some(entry) => {
                let before = entry.roles.len();
                entry.roles.retain(|held| held != role);
                Ok(entry.roles.len() != before)
            }
            None => Ok(false),
        }
    })
}

/// Sets a role's parent, refusing one that would make the chain a cycle and naming the cycle.
pub fn set_inheritance(role: &str, parent: Option<&str>) -> Result<(), Error> {
    with(|state| {
        if !state.roles.contains_key(role) {
            return Err(Error::UnknownRole(role.to_owned()));
        }
        let Some(parent) = parent else {
            if let Some(held) = state.roles.get_mut(role) {
                held.parent = None;
            }
            return Ok(());
        };
        if !state.roles.contains_key(parent) {
            return Err(Error::UnknownRole(parent.to_owned()));
        }
        let mut probe = state.roles.clone();
        if let Some(held) = probe.get_mut(role) {
            held.parent = Some(parent.to_owned());
        }
        if let Some(path) = cycle_path(&probe, role) {
            return Err(Error::Cycle(path));
        }
        if let Some(held) = state.roles.get_mut(role) {
            held.parent = Some(parent.to_owned());
        }
        Ok(())
    })
}

/// The role a role inherits from, if any.
#[must_use]
pub fn get_inheritance(role: &str) -> Option<String> {
    with(|state| state.roles.get(role).and_then(|held| held.parent.clone()))
}

/// A role's chat prefix.
#[must_use]
pub fn get_prefix(role: &str) -> Option<String> {
    with(|state| state.roles.get(role).and_then(|held| held.prefix.clone()))
}

/// Sets a role's chat prefix; an empty one clears it.
pub fn set_prefix(role: &str, prefix: &str) -> Result<(), Error> {
    set_text(role, prefix, |held, value| held.prefix = value)
}

/// A role's chat suffix.
#[must_use]
pub fn get_suffix(role: &str) -> Option<String> {
    with(|state| state.roles.get(role).and_then(|held| held.suffix.clone()))
}

/// Sets a role's chat suffix; an empty one clears it.
pub fn set_suffix(role: &str, suffix: &str) -> Result<(), Error> {
    set_text(role, suffix, |held, value| held.suffix = value)
}

fn set_text(
    role: &str,
    text: &str,
    set: fn(&mut Role, Option<String>),
) -> Result<(), Error> {
    let value = (!text.is_empty()).then(|| text.to_owned());
    with(|state| match state.roles.get_mut(role) {
        Some(held) => {
            set(held, value);
            Ok(())
        }
        None => Err(Error::UnknownRole(role.to_owned())),
    })
}

/// Every role a user holds, directly or through inheritance.
#[must_use]
pub fn roles_of(user: &str) -> Vec<String> {
    with(|state| roles_for(state, user))
}

/// Resolves `node` for `user`, reporting the role and pattern that decided.
#[must_use]
pub fn resolve(user: &str, node: &str) -> Decision {
    with(|state| resolve_in(state, user, node))
}

/// Whether `user` may use `node`. With no matching rule the answer is no.
#[must_use]
pub fn has_permission(user: &str, node: &str) -> bool {
    resolve(user, node).allowed
}

/// The same as [`has_permission`], under the name the wishlist uses.
#[must_use]
pub fn check(user: &str, node: &str) -> bool {
    has_permission(user, node)
}

/// The concrete nodes a user may use, in sorted order: every registered node that resolves
/// to an allow, plus the literal grants of the user's roles that resolve to one.
#[must_use]
pub fn get_effective_permissions(user: &str) -> Vec<String> {
    with(|state| {
        let mut candidates: BTreeSet<String> = state.nodes.keys().cloned().collect();
        for role in roles_for(state, user) {
            if let Some(held) = state.roles.get(&role) {
                for grant in &held.grants {
                    if !grant.contains('*') {
                        candidates.insert(grant.clone());
                    }
                }
            }
        }
        candidates
            .into_iter()
            .filter(|node| resolve_in(state, user, node).allowed)
            .collect()
    })
}

/// The grants a role has through itself and its parents, minus the denials. Sorted.
#[must_use]
pub fn effective_grants(role: &str) -> Vec<String> {
    with(|state| {
        let mut grants: BTreeSet<String> = BTreeSet::new();
        let mut denials: BTreeSet<String> = BTreeSet::new();
        for name in lineage(state, role) {
            if let Some(held) = state.roles.get(&name) {
                grants.extend(held.grants.iter().cloned());
                denials.extend(held.denials.iter().cloned());
            }
        }
        grants.difference(&denials).cloned().collect()
    })
}

/// The whole set as a config value, sorted so the same set exports the same bytes.
#[must_use]
pub fn export() -> ConfigValue {
    with(|state| {
        let mut root = BTreeMap::new();
        root.insert(
            "nodes".to_owned(),
            ConfigValue::List(
                state
                    .nodes
                    .keys()
                    .map(|path| ConfigValue::String(path.clone()))
                    .collect(),
            ),
        );
        root.insert(
            "roles".to_owned(),
            ConfigValue::List(state.roles.values().map(role_value).collect()),
        );
        root.insert(
            "users".to_owned(),
            ConfigValue::List(state.users.values().map(user_value).collect()),
        );
        ConfigValue::Map(root)
    })
}

/// Replaces the whole set from [`export`]'s value, refusing one that names a missing parent,
/// cycles, or is not a map at all.
pub fn import(value: &ConfigValue) -> Result<(), Error> {
    let mut fresh = state_from(value)?;
    fresh.saved = Some(value.clone());
    with(|state| {
        *state = fresh;
    });
    Ok(())
}

/// The whole set as JSON, through [`crate::serialization`].
#[must_use]
pub fn export_json() -> String {
    crate::serialization::to_json(&to_serialized(&export())).unwrap_or_default()
}

/// Reads a set back from [`export_json`]'s text.
pub fn import_json(text: &str) -> Result<(), Error> {
    let value = crate::serialization::from_json(text)
        .map_err(|error| Error::BadValue(error.to_string()))?;
    import(&from_serialized(value))
}

/// Puts the set back to the last [`import`], discarding changes made since, and answers how
/// many roles are in force.
pub fn reload() -> usize {
    with(|state| {
        let Some(saved) = state.saved.clone() else {
            *state = State::default();
            return 0;
        };
        match state_from(&saved) {
            Ok(mut fresh) => {
                fresh.saved = Some(saved);
                let count = fresh.roles.len();
                *state = fresh;
                count
            }
            Err(_) => 0,
        }
    })
}

/// Why a question about the client's or the real server's own permission state has no answer
/// here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "client" => {
            "the client's own permission state is the account's privileges, which this host \
             is not given and will not guess at"
                .to_owned()
        }
        "server" => {
            "the real server's permission state lives on that server, which this client is \
             not connected to and cannot read or edit"
                .to_owned()
        }
        _ => format!("{what} is not permission state this host can see"),
    }
}

/// The client's and the real server's own permission state, with the reason each.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["client", "server"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// Reads the real server's permission state. `false`, with the reason.
pub fn sync_from_server() -> bool {
    crate::warn!(
        "a mod asked this host for the server's permissions: {}",
        why_unknown("server")
    );
    false
}

/// Writes this set into the real server's permission state. `false`, with the reason.
pub fn sync_to_server() -> bool {
    crate::warn!(
        "a mod asked this host to write the server's permissions: {}",
        why_unknown("client")
    );
    false
}

fn roles_for(state: &State, user: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Some(entry) = state.users.get(user) else {
        return out;
    };
    for role in &entry.roles {
        for name in lineage(state, role) {
            if !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

fn lineage(state: &State, role: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = Some(role.to_owned());
    let mut guard = state.roles.len() + 1;
    while let Some(name) = current {
        if !out.contains(&name) {
            out.push(name.clone());
        }
        current = state.roles.get(&name).and_then(|held| held.parent.clone());
        guard -= 1;
        if guard == 0 {
            break;
        }
    }
    out
}

fn resolve_in(state: &State, user: &str, node: &str) -> Decision {
    let Ok(query) = Node::new(node) else {
        return Decision {
            allowed: false,
            role: None,
            pattern: None,
        };
    };
    let mut best: Option<(i32, u8, usize, String, String)> = None;
    for name in roles_for(state, user) {
        let Some(held) = state.roles.get(&name) else {
            continue;
        };
        let rules = held
            .grants
            .iter()
            .map(|pattern| (pattern, false))
            .chain(held.denials.iter().map(|pattern| (pattern, true)));
        for (pattern, deny) in rules {
            let Ok(rule) = Node::new(pattern) else {
                continue;
            };
            if !rule.matches(query.path()) {
                continue;
            }
            let key = (
                held.priority,
                u8::from(deny),
                rule.specificity(),
                held.name.clone(),
                pattern.clone(),
            );
            if best
                .as_ref()
                .is_none_or(|held| (held.0, held.1, held.2) < (key.0, key.1, key.2))
            {
                best = Some(key);
            }
        }
    }
    match best {
        Some((_, deny, _, role, pattern)) => Decision {
            allowed: deny == 0,
            role: Some(role),
            pattern: Some(pattern),
        },
        None => Decision {
            allowed: false,
            role: None,
            pattern: None,
        },
    }
}

/// The cycle a role's parent chain closes, as `a -> b -> a`, or `None`.
fn cycle_path(roles: &BTreeMap<String, Role>, start: &str) -> Option<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut current = start.to_owned();
    loop {
        if let Some(at) = seen.iter().position(|name| *name == current) {
            let mut path = seen[at..].to_vec();
            path.push(current);
            return Some(path.join(" -> "));
        }
        seen.push(current.clone());
        current = roles.get(&current).and_then(|held| held.parent.clone())?;
    }
}

fn role_value(role: &Role) -> ConfigValue {
    let mut fields = BTreeMap::new();
    fields.insert("name".to_owned(), ConfigValue::String(role.name.clone()));
    fields.insert(
        "priority".to_owned(),
        ConfigValue::Integer(i64::from(role.priority)),
    );
    if let Some(parent) = &role.parent {
        fields.insert("parent".to_owned(), ConfigValue::String(parent.clone()));
    }
    if let Some(prefix) = &role.prefix {
        fields.insert("prefix".to_owned(), ConfigValue::String(prefix.clone()));
    }
    if let Some(suffix) = &role.suffix {
        fields.insert("suffix".to_owned(), ConfigValue::String(suffix.clone()));
    }
    fields.insert("grants".to_owned(), text_list(&role.grants));
    fields.insert("denials".to_owned(), text_list(&role.denials));
    ConfigValue::Map(fields)
}

fn user_value(user: &User) -> ConfigValue {
    let mut fields = BTreeMap::new();
    fields.insert("name".to_owned(), ConfigValue::String(user.name.clone()));
    let mut roles = user.roles.clone();
    roles.sort();
    fields.insert("roles".to_owned(), text_list(&roles));
    ConfigValue::Map(fields)
}

fn text_list(values: &[String]) -> ConfigValue {
    let mut sorted = values.to_vec();
    sorted.sort();
    ConfigValue::List(
        sorted
            .into_iter()
            .map(ConfigValue::String)
            .collect(),
    )
}

fn state_from(value: &ConfigValue) -> Result<State, Error> {
    let map = value
        .as_map()
        .ok_or_else(|| Error::BadValue("the permission set is not a map".to_owned()))?;
    let mut state = State::default();
    if let Some(nodes) = map.get("nodes").and_then(ConfigValue::as_list) {
        for item in nodes {
            let path = item
                .as_text()
                .ok_or_else(|| Error::BadValue("a node is not text".to_owned()))?;
            let node = Node::new(path)?;
            state.nodes.insert(path.to_owned(), node);
        }
    }
    if let Some(roles) = map.get("roles").and_then(ConfigValue::as_list) {
        for item in roles {
            let role = role_from(item)?;
            state.roles.insert(role.name.clone(), role);
        }
    }
    if let Some(users) = map.get("users").and_then(ConfigValue::as_list) {
        for item in users {
            let user = user_from(item)?;
            state.users.insert(user.name.clone(), user);
        }
    }
    for role in state.roles.values() {
        if let Some(parent) = &role.parent {
            if !state.roles.contains_key(parent) {
                return Err(Error::UnknownRole(parent.clone()));
            }
        }
    }
    for name in state.roles.keys() {
        if let Some(path) = cycle_path(&state.roles, name) {
            return Err(Error::Cycle(path));
        }
    }
    Ok(state)
}

fn role_from(value: &ConfigValue) -> Result<Role, Error> {
    let map = value
        .as_map()
        .ok_or_else(|| Error::BadValue("a role is not a map".to_owned()))?;
    let name = map
        .get("name")
        .and_then(ConfigValue::as_text)
        .ok_or_else(|| Error::BadValue("a role has no name".to_owned()))?
        .to_owned();
    let priority = map
        .get("priority")
        .and_then(ConfigValue::as_integer)
        .and_then(|number| i32::try_from(number).ok())
        .unwrap_or(0);
    let text = |key: &str| {
        map.get(key)
            .and_then(ConfigValue::as_text)
            .map(str::to_owned)
    };
    let list = |key: &str| {
        map.get(key)
            .and_then(ConfigValue::as_list)
            .map(|items| {
                items
                    .iter()
                    .filter_map(ConfigValue::as_text)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    Ok(Role {
        name,
        priority,
        grants: list("grants"),
        denials: list("denials"),
        parent: text("parent"),
        prefix: text("prefix"),
        suffix: text("suffix"),
    })
}

fn user_from(value: &ConfigValue) -> Result<User, Error> {
    let map = value
        .as_map()
        .ok_or_else(|| Error::BadValue("a user is not a map".to_owned()))?;
    let name = map
        .get("name")
        .and_then(ConfigValue::as_text)
        .ok_or_else(|| Error::BadValue("a user has no name".to_owned()))?
        .to_owned();
    let roles = map
        .get("roles")
        .and_then(ConfigValue::as_list)
        .map(|items| {
            items
                .iter()
                .filter_map(ConfigValue::as_text)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    Ok(User { name, roles })
}

fn to_serialized(value: &ConfigValue) -> crate::serialization::Value {
    use crate::serialization::Value;
    match value {
        ConfigValue::Map(fields)
        | ConfigValue::Object(fields)
        | ConfigValue::Compound(fields) => Value::Map(
            fields
                .iter()
                .map(|(key, held)| (key.clone(), to_serialized(held)))
                .collect(),
        ),
        ConfigValue::List(items) | ConfigValue::Set(items) => {
            Value::List(items.iter().map(to_serialized).collect())
        }
        ConfigValue::Boolean(flag) => Value::Bool(*flag),
        ConfigValue::Integer(number) => Value::Int(*number),
        ConfigValue::Float(number) | ConfigValue::Percentage(number) => Value::Float(*number),
        other => Value::Str(other.render()),
    }
}

fn from_serialized(value: crate::serialization::Value) -> ConfigValue {
    use crate::serialization::Value;
    match value {
        Value::Null => ConfigValue::String(String::new()),
        Value::Bool(flag) => ConfigValue::Boolean(flag),
        Value::Int(number) => ConfigValue::Integer(number),
        Value::Float(number) => ConfigValue::Float(number),
        Value::Str(text) => ConfigValue::String(text),
        Value::List(items) => ConfigValue::List(items.into_iter().map(from_serialized).collect()),
        Value::Map(map) => ConfigValue::Map(
            map.iter()
                .map(|(key, held)| (key.to_owned(), from_serialized(held.clone())))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|state| {
            state.nodes.clear();
            state.roles.clear();
            state.users.clear();
            state.saved = None;
        });
    }

    #[test]
    fn a_wildcard_matches_a_deep_node_and_not_a_sibling() {
        let _serial = serial();
        fresh();
        create_group(Role::new("member", 0).grant("hello.*")).unwrap();
        add_to_group("alice", "member").unwrap();
        assert!(check("alice", "hello.commands.give"));
        assert!(check("alice", "hello.anything.deeper.still"));
        assert!(!check("alice", "other.thing"));
        assert_eq!(resolve("alice", "hello.commands.give").pattern.as_deref(), Some("hello.*"));

        create_group(Role::new("star", 0).grant("*")).unwrap();
        add_to_group("bob", "star").unwrap();
        assert!(check("bob", "other.thing"), "a bare * covers everything");
    }

    #[test]
    fn a_more_specific_match_beats_a_wildcard() {
        let _serial = serial();
        fresh();
        create_group(Role::new("lead", 0).grant("hello.*").grant("hello.commands.give")).unwrap();
        add_to_group("bob", "lead").unwrap();
        assert_eq!(
            resolve("bob", "hello.commands.give").pattern.as_deref(),
            Some("hello.commands.give")
        );
        assert_eq!(
            resolve("bob", "hello.commands.list").pattern.as_deref(),
            Some("hello.*")
        );
    }

    #[test]
    fn an_explicit_deny_beats_an_allow_at_the_same_priority() {
        let _serial = serial();
        fresh();
        create_group(Role::new("guest", 0).grant("hello.commands.give")).unwrap();
        create_group(Role::new("banned", 0).deny("hello.commands.give")).unwrap();
        add_to_group("carol", "guest").unwrap();
        add_to_group("carol", "banned").unwrap();
        let decision = resolve("carol", "hello.commands.give");
        assert!(!decision.allowed);
        assert_eq!(decision.role.as_deref(), Some("banned"));
    }

    #[test]
    fn a_higher_priority_role_wins_over_a_lower_one() {
        let _serial = serial();
        fresh();
        create_group(Role::new("member", 0).deny("hello.commands.give")).unwrap();
        create_group(Role::new("admin", 10).grant("hello.commands.give")).unwrap();
        add_to_group("dave", "member").unwrap();
        add_to_group("dave", "admin").unwrap();
        let decision = resolve("dave", "hello.commands.give");
        assert!(decision.allowed);
        assert_eq!(decision.role.as_deref(), Some("admin"));
    }

    #[test]
    fn an_unknown_node_is_denied_with_no_role_and_the_client_state_is_refused() {
        let _serial = serial();
        fresh();
        create_group(Role::new("member", 0).grant("hello.*")).unwrap();
        add_to_group("alice", "member").unwrap();
        let decision = resolve("alice", "other.thing");
        assert!(!decision.allowed);
        assert_eq!(decision.role, None);
        assert_eq!(decision.to_string(), "deny (no role matched)");

        assert!(!sync_from_server());
        assert!(!sync_to_server());
        assert!(refusals().iter().any(|(what, _)| what == "server"));
        assert!(refusals().iter().any(|(what, _)| what == "client"));
    }

    #[test]
    fn an_inheritance_cycle_is_refused_naming_it() {
        let _serial = serial();
        fresh();
        create_group(Role::new("a", 0)).unwrap();
        create_group(Role::new("b", 0)).unwrap();
        set_inheritance("a", Some("b")).unwrap();
        let error = set_inheritance("b", Some("a")).unwrap_err();
        assert_eq!(error, Error::Cycle("b -> a -> b".to_owned()));
        assert_eq!(get_inheritance("b"), None, "the refused link was not set");
    }

    #[test]
    fn a_duplicate_role_and_a_role_in_use_are_refused() {
        let _serial = serial();
        fresh();
        create_group(Role::new("base", 0)).unwrap();
        assert_eq!(create_group(Role::new("base", 1)).unwrap_err(), Error::DuplicateRole("base".to_owned()));
        create_group(Role::new("child", 0).parent("base")).unwrap();
        assert_eq!(
            delete_group("base").unwrap_err(),
            Error::RoleInUse { role: "base".to_owned(), by: "child".to_owned() }
        );
        assert!(delete_group("child").is_ok());
        assert!(delete_group("base").is_ok());
        assert_eq!(delete_group("base").unwrap_err(), Error::UnknownRole("base".to_owned()));
    }

    #[test]
    fn a_role_inherits_its_parent_and_the_decider_is_reported() {
        let _serial = serial();
        fresh();
        create_group(Role::new("base", 0).grant("hello.*").deny("hello.secret")).unwrap();
        create_group(Role::new("child", 5).parent("base").grant("hello.commands.list")).unwrap();
        add_to_group("eve", "child").unwrap();
        assert_eq!(
            resolve("eve", "hello.commands.give").role.as_deref(),
            Some("base"),
            "the parent's grant decided"
        );
        assert!(!check("eve", "hello.secret"), "the parent's denial came along");
        assert_eq!(effective_grants("child"), vec!["hello.*".to_owned(), "hello.commands.list".to_owned()]);
    }

    #[test]
    fn the_whole_set_round_trips_through_a_config_value() {
        let _serial = serial();
        fresh();
        register("hello.commands.give");
        register("hello.commands.list");
        create_group(Role::new("base", 0).grant("hello.*").prefix("[base]").suffix("!")).unwrap();
        create_group(Role::new("vip", 5).parent("base").deny("hello.commands.give")).unwrap();
        add_to_group("alice", "vip").unwrap();

        let exported = export();
        import(&exported).expect("what export wrote reads back");
        assert_eq!(export(), exported, "the value round trips exactly");

        let json = export_json();
        import_json(&json).expect("what export_json wrote reads back");
        assert_eq!(export_json(), json, "the JSON round trips exactly");
        assert_eq!(get_effective_permissions("alice"), vec!["hello.commands.list".to_owned()]);

        grant("vip", "hello.commands.give").unwrap();
        assert!(check("alice", "hello.commands.give"));
        assert_eq!(reload(), 2, "the last imported snapshot is back");
        assert!(!check("alice", "hello.commands.give"), "the grant made since is gone");
    }
}
