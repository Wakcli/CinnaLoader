//! Errors with a stable code, a chain of causes, a recoverability verdict and a retry policy.
//!
//! An [`Error`] is more than a message: [`create`] gives it a code, and the code's entry in
//! the registry answers its default severity, where it came from, whether it can be recovered
//! from and whether it is worth retrying. [`context`] and [`wrap`] push causes onto it, and
//! [`Error::render`] prints the whole chain.
//!
//! A [`RetryPolicy`] owns the attempt limit and the backoff: [`Backoff::Fixed`] waits the same
//! time every attempt, [`Backoff::Exponential`] doubles from a base and stops at a cap.
//! [`RetryPolicy::should_retry`] refuses an attempt at or past `max_attempts` and never retries
//! an error the registry marked not recoverable.
//!
//! [`report`] groups the errors collected so far by code, and renders them as text or JSON.
//! What this host cannot reach is the **client's** error state and its crash handling; those are
//! refused with a sentence in [`why_unknown`] and listed by [`refusals`].
//!
//! ```
//! use cinnaloader::errors::{self, Origin, RetryPolicy};
//!
//! let error = errors::create("network", "the socket dropped");
//! assert_eq!(errors::classify(&error), Origin::Loader);
//!
//! let error = errors::context(error, "while sending a chat line");
//! assert_eq!(errors::get_root_cause(&error).message, "while sending a chat line");
//! assert!(error.render().contains("caused by context: while sending a chat line"));
//!
//! let policy = RetryPolicy::exponential(3, 50, 200);
//! assert_eq!(policy.delay(1), 50);
//! assert_eq!(policy.delay(3), 200);
//! assert!(policy.should_retry(1, &error));
//! assert!(!policy.should_retry(3, &error));
//!
//! let report = errors::report();
//! assert_eq!(report.total, errors::get_recent(8).len());
//! assert!(report.json().contains("\"code\":\"network\""));
//! ```

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::serialization::{self, Map, Value};

/// How loud an error is, and whether it stops anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Info,
    Warning,
    Error,
    Fatal,
}

impl Severity {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
            Self::Fatal => "fatal",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Where an error came from: a mod, the loader, the player's input, or the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    Mod,
    Loader,
    UserInput,
    WorldState,
}

impl Origin {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mod => "mod",
            Self::Loader => "loader",
            Self::UserInput => "user_input",
            Self::WorldState => "world_state",
        }
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What to do about an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recovery {
    Retry,
    Skip,
    Abort,
    /// Carry on with a named stand-in.
    UseDefault(String),
}

impl Recovery {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Retry => "retry",
            Self::Skip => "skip",
            Self::Abort => "abort",
            Self::UseDefault(_) => "use_default",
        }
    }
}

impl fmt::Display for Recovery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// How long to wait before the next attempt.
///
/// `Fixed` waits `millis` every time. `Exponential` waits `base_millis * 2^(attempt - 1)`,
/// with `attempt` counting from 1, and never waits longer than `cap_millis`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backoff {
    Fixed(u64),
    Exponential { base_millis: u64, cap_millis: u64 },
}

impl Backoff {
    /// The wait before attempt `attempt`, 1-based. A past-the-cap attempt still answers the cap.
    #[must_use]
    pub fn delay(&self, attempt: u32) -> u64 {
        match *self {
            Self::Fixed(millis) => millis,
            Self::Exponential {
                base_millis,
                cap_millis,
            } => {
                let shift = attempt.saturating_sub(1).min(63);
                base_millis
                    .saturating_mul(1u64 << shift)
                    .min(cap_millis)
            }
        }
    }
}

/// An attempt limit and a backoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub backoff: Backoff,
}

impl RetryPolicy {
    #[must_use]
    pub fn fixed(max_attempts: u32, millis: u64) -> Self {
        Self {
            max_attempts,
            backoff: Backoff::Fixed(millis),
        }
    }

    #[must_use]
    pub fn exponential(max_attempts: u32, base_millis: u64, cap_millis: u64) -> Self {
        Self {
            max_attempts,
            backoff: Backoff::Exponential {
                base_millis,
                cap_millis,
            },
        }
    }

    #[must_use]
    pub fn delay(&self, attempt: u32) -> u64 {
        self.backoff.delay(attempt)
    }

    /// Whether attempt `attempt` (1-based) may be followed by another one. The counter stops at
    /// `max_attempts`, and an error the registry marked not recoverable is never retried.
    #[must_use]
    pub fn should_retry(&self, attempt: u32, error: &Error) -> bool {
        error.is_retryable() && attempt >= 1 && attempt < self.max_attempts
    }
}

/// A registered error code and everything its entry decides for an error that carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Code {
    pub id: String,
    pub message: String,
    pub severity: Severity,
    pub origin: Origin,
    pub recoverable: bool,
    pub retryable: bool,
    pub recovery: Recovery,
}

impl Code {
    #[must_use]
    pub fn new(id: &str, message: &str) -> Self {
        Self {
            id: id.to_owned(),
            message: message.to_owned(),
            severity: Severity::Error,
            origin: Origin::Loader,
            recoverable: true,
            retryable: false,
            recovery: Recovery::Skip,
        }
    }

    #[must_use]
    pub fn severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    #[must_use]
    pub fn origin(mut self, origin: Origin) -> Self {
        self.origin = origin;
        self
    }

    #[must_use]
    pub fn recoverable(mut self, recoverable: bool) -> Self {
        self.recoverable = recoverable;
        if !recoverable {
            self.retryable = false;
        }
        self
    }

    #[must_use]
    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable && self.recoverable;
        self
    }

    #[must_use]
    pub fn recovery(mut self, recovery: Recovery) -> Self {
        self.recovery = recovery;
        self
    }
}

/// One reason an error happened, below the error itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cause {
    pub code: String,
    pub message: String,
}

/// A code, a message, a severity, where it came from, whether it can be recovered from, a
/// recovery action, and the chain of causes that led to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    id: u64,
    code: String,
    message: String,
    severity: Severity,
    origin: Origin,
    recoverable: bool,
    retryable: bool,
    recovery: Box<Recovery>,
    causes: Vec<Cause>,
    mod_id: Box<str>,
    at_millis: u64,
}

impl Error {
    fn from_code(code: &Code, message: &str) -> Self {
        Self {
            id: 0,
            code: code.id.clone(),
            message: message.to_owned(),
            severity: code.severity,
            origin: code.origin,
            recoverable: code.recoverable,
            retryable: code.retryable,
            recovery: Box::new(code.recovery.clone()),
            causes: Vec::new(),
            mod_id: crate::metadata::id().into_boxed_str(),
            at_millis: 0,
        }
    }

    /// The stand-in for a code nobody registered.
    fn generic(code: &str, message: &str) -> Self {
        Self {
            id: 0,
            code: code.to_owned(),
            message: message.to_owned(),
            severity: Severity::Warning,
            origin: Origin::Loader,
            recoverable: true,
            retryable: false,
            recovery: Box::new(Recovery::Skip),
            causes: Vec::new(),
            mod_id: crate::metadata::id().into_boxed_str(),
            at_millis: 0,
        }
    }

    fn refused(what: &str) -> Self {
        Self {
            id: 0,
            code: "refused".to_owned(),
            message: why_unknown(what),
            severity: Severity::Fatal,
            origin: Origin::Loader,
            recoverable: false,
            retryable: false,
            recovery: Box::new(Recovery::Abort),
            causes: Vec::new(),
            mod_id: crate::metadata::id().into_boxed_str(),
            at_millis: 0,
        }
    }

    #[must_use]
    pub fn id(&self) -> u64 {
        self.id
    }

    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    #[must_use]
    pub fn severity(&self) -> Severity {
        self.severity
    }

    #[must_use]
    pub fn origin(&self) -> Origin {
        self.origin
    }

    #[must_use]
    pub fn is_recoverable(&self) -> bool {
        self.recoverable
    }

    #[must_use]
    pub fn is_retryable(&self) -> bool {
        self.recoverable && self.retryable
    }

    #[must_use]
    pub fn recovery(&self) -> &Recovery {
        &self.recovery
    }

    #[must_use]
    pub fn causes(&self) -> &[Cause] {
        &self.causes
    }

    #[must_use]
    pub fn mod_id(&self) -> &str {
        &self.mod_id
    }

    #[must_use]
    pub fn at_millis(&self) -> u64 {
        self.at_millis
    }

    /// The error and every cause under it, one line each, in order.
    #[must_use]
    pub fn render(&self) -> String {
        let mut text = format!(
            "[{}] {} ({}, {}): {}",
            self.severity, self.code, self.origin, self.mod_id, self.message
        );
        for cause in &self.causes {
            text.push_str(&format!("\n  caused by {}: {}", cause.code, cause.message));
        }
        text
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.render())
    }
}

impl std::error::Error for Error {}

/// A recovery action together with the attempt budget for its code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryStrategy {
    pub code: String,
    pub action: Recovery,
    pub policy: RetryPolicy,
}

impl RecoveryStrategy {
    #[must_use]
    pub fn new(code: &str, action: Recovery, policy: RetryPolicy) -> Self {
        Self {
            code: code.to_owned(),
            action,
            policy,
        }
    }
}

/// How a recovery is going for one code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryStatus {
    pub code: String,
    pub attempts: u32,
    pub action: Recovery,
    pub policy: RetryPolicy,
    pub exhausted: bool,
}

/// The errors so far, grouped by code with a count each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorReport {
    pub total: usize,
    pub groups: Vec<Group>,
}

/// One code's share of the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub code: String,
    pub count: usize,
    pub severity: Severity,
    pub origin: Origin,
}

impl ErrorReport {
    fn build(errors: &[Error]) -> Self {
        let mut groups: Vec<Group> = Vec::new();
        for error in errors {
            match groups.iter_mut().find(|group| group.code == error.code) {
                Some(group) => group.count += 1,
                None => groups.push(Group {
                    code: error.code.clone(),
                    count: 1,
                    severity: error.severity,
                    origin: error.origin,
                }),
            }
        }
        Self {
            total: errors.len(),
            groups,
        }
    }

    #[must_use]
    pub fn text(&self) -> String {
        let mut text = format!(
            "{} error(s) across {} code(s)\n",
            self.total,
            self.groups.len()
        );
        for group in &self.groups {
            text.push_str(&format!(
                "  {}: {} ({}, {})\n",
                group.code, group.count, group.severity, group.origin
            ));
        }
        text
    }

    /// The report as JSON, through [`crate::serialization`].
    #[must_use]
    pub fn json(&self) -> String {
        let mut root = Map::new();
        root.insert("total", Value::Int(self.total as i64));
        let groups = self
            .groups
            .iter()
            .map(|group| {
                let mut entry = Map::new();
                entry.insert("code", Value::str(&group.code));
                entry.insert("count", Value::Int(group.count as i64));
                entry.insert("severity", Value::str(group.severity.as_str()));
                entry.insert("origin", Value::str(group.origin.as_str()));
                Value::Map(entry)
            })
            .collect();
        root.insert("groups", Value::List(groups));
        serialization::to_json(&Value::Map(root))
            .expect("a report holds no non-finite float")
    }
}

impl fmt::Display for ErrorReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.text())
    }
}

struct Handler {
    name: String,
    run: Box<dyn Fn(&Error) + Send>,
}

struct Registry {
    codes: Vec<Code>,
    errors: Vec<Error>,
    handlers: Vec<Handler>,
    strategies: Vec<RecoveryStrategy>,
    recoveries: HashMap<String, (u32, Recovery)>,
    failures: u64,
    next_id: u64,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        Mutex::new(Registry {
            codes: builtin_codes(),
            errors: Vec::new(),
            handlers: Vec::new(),
            strategies: Vec::new(),
            recoveries: HashMap::new(),
            failures: 0,
            next_id: 1,
        })
    })
}

fn with<T>(body: impl FnOnce(&mut Registry) -> T) -> T {
    let mut held = registry().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut held)
}

fn started() -> &'static std::time::Instant {
    static STARTED: OnceLock<std::time::Instant> = OnceLock::new();
    STARTED.get_or_init(std::time::Instant::now)
}

fn find_code(id: &str) -> Option<Code> {
    with(|registry| registry.codes.iter().find(|code| code.id == id).cloned())
}

fn store(registry: &mut Registry, mut error: Error) -> Error {
    error.id = registry.next_id;
    registry.next_id += 1;
    error.at_millis = started().elapsed().as_millis() as u64;
    registry.failures += 1;
    registry.errors.push(error.clone());
    for handler in &registry.handlers {
        (handler.run)(&error);
    }
    error
}

/// Builds an error for a code and records it. An unregistered code keeps its name but is
/// given the generic defaults, with a warning that no code was registered for it.
pub fn create(code: &str, message: &str) -> Error {
    let error = match find_code(code) {
        Some(descriptor) => Error::from_code(&descriptor, message),
        None => {
            crate::warn!("error code {code} is not registered; using a generic warning");
            Error::generic(code, message)
        }
    };
    with(|registry| store(registry, error))
}

/// Wraps `cause` as the first link in a new error's chain.
pub fn wrap(code: &str, message: &str, cause: Error) -> Error {
    let mut error = match find_code(code) {
        Some(descriptor) => Error::from_code(&descriptor, message),
        None => {
            crate::warn!("error code {code} is not registered; using a generic warning");
            Error::generic(code, message)
        }
    };
    error.causes.push(Cause {
        code: cause.code.clone(),
        message: cause.message.clone(),
    });
    error.causes.extend(cause.causes);
    with(|registry| store(registry, error))
}

/// Pushes a context cause onto an error, updating the recorded copy in place so the count does
/// not grow.
pub fn context(mut error: Error, message: &str) -> Error {
    error.causes.push(Cause {
        code: "context".to_owned(),
        message: message.to_owned(),
    });
    with(|registry| match registry.errors.iter_mut().find(|held| held.id == error.id) {
        Some(stored) => {
            *stored = error.clone();
            error
        }
        None => store(registry, error),
    })
}

/// The code's registered entry, or a refusal that names it.
pub fn describe_code(code: &str) -> Result<Code, Error> {
    find_code(code).ok_or_else(|| {
        crate::warn!("no error code named {code} is registered");
        Error::refused_code(code)
    })
}

/// Registers a code, refusing a name already taken.
pub fn register_code(code: Code) -> bool {
    with(|registry| {
        if registry.codes.iter().any(|held| held.id == code.id) {
            crate::warn!("the error code {} is already registered", code.id);
            return false;
        }
        registry.codes.push(code);
        true
    })
}

/// Removes a code by name, and answers whether it was there.
pub fn unregister_code(id: &str) -> bool {
    with(|registry| {
        let before = registry.codes.len();
        registry.codes.retain(|code| code.id != id);
        registry.codes.len() != before
    })
}

/// Every registered code.
#[must_use]
pub fn codes() -> Vec<Code> {
    with(|registry| registry.codes.clone())
}

/// The origin an error is classified under.
#[must_use]
pub fn classify(error: &Error) -> Origin {
    error.origin
}

/// The recorded error with that id.
#[must_use]
pub fn get(id: u64) -> Option<Error> {
    with(|registry| registry.errors.iter().find(|error| error.id == id).cloned())
}

/// The last `count` errors, oldest first.
#[must_use]
pub fn get_recent(count: usize) -> Vec<Error> {
    with(|registry| {
        let start = registry.errors.len().saturating_sub(count);
        registry.errors[start..].to_vec()
    })
}

/// Every error whose recording mod is `id`.
#[must_use]
pub fn get_by_mod(id: &str) -> Vec<Error> {
    with(|registry| {
        registry
            .errors
            .iter()
            .filter(|error| error.mod_id.as_ref() == id)
            .cloned()
            .collect()
    })
}

/// Every error classified under `origin`.
#[must_use]
pub fn get_by_category(origin: Origin) -> Vec<Error> {
    with(|registry| {
        registry
            .errors
            .iter()
            .filter(|error| error.origin == origin)
            .cloned()
            .collect()
    })
}

/// The error's whole chain as text. A mod cannot walk the client's call stack, so this renders
/// the causes the mod itself attached, in order.
#[must_use]
pub fn get_stack_trace(error: &Error) -> String {
    let mut text = error.render();
    text.push_str("\n  (a mod cannot walk the client's call stack; this is the error's cause chain)");
    text
}

/// The deepest cause under an error, or the error itself when it has none.
#[must_use]
pub fn get_root_cause(error: &Error) -> Cause {
    error.causes.last().cloned().unwrap_or_else(|| Cause {
        code: error.code.clone(),
        message: error.message.clone(),
    })
}

/// Registers a handler that sees every recorded error. Names are unique.
pub fn register_handler(name: &str, handler: impl Fn(&Error) + Send + 'static) -> bool {
    with(|registry| {
        if registry.handlers.iter().any(|held| held.name == name) {
            crate::warn!("an error handler named {name} is already registered");
            return false;
        }
        registry.handlers.push(Handler {
            name: name.to_owned(),
            run: Box::new(handler),
        });
        true
    })
}

/// Removes a handler by name, and answers whether it was there.
pub fn unregister_handler(name: &str) -> bool {
    with(|registry| {
        let before = registry.handlers.len();
        registry.handlers.retain(|handler| handler.name != name);
        registry.handlers.len() != before
    })
}

/// Registers the action and attempt budget for a code, replacing any earlier one.
pub fn register_recovery_strategy(strategy: RecoveryStrategy) -> bool {
    with(|registry| {
        registry
            .strategies
            .retain(|held| held.code != strategy.code);
        registry.strategies.push(strategy);
        true
    })
}

/// The action for an error: a registered strategy for its code wins, otherwise the action the
/// code gave it. Records one recovery attempt against the code.
pub fn attempt_recovery(error: &Error) -> Recovery {
    let action = with(|registry| {
        registry
            .strategies
            .iter()
            .find(|strategy| strategy.code == error.code)
            .map(|strategy| strategy.action.clone())
            .unwrap_or_else(|| (*error.recovery).clone())
    });
    with(|registry| {
        let entry = registry
            .recoveries
            .entry(error.code.clone())
            .or_insert_with(|| (0, action.clone()));
        entry.0 += 1;
        entry.1 = action.clone();
    });
    action
}

/// How many recovery attempts a code has taken, and whether its budget is spent. `None` until
/// [`attempt_recovery`] has been called for it.
#[must_use]
pub fn get_recovery_status(code: &str) -> Option<RecoveryStatus> {
    with(|registry| {
        let (attempts, action) = registry.recoveries.get(code).cloned()?;
        let policy = registry
            .strategies
            .iter()
            .find(|strategy| strategy.code == code)
            .map(|strategy| strategy.policy)
            .unwrap_or(DEFAULT_POLICY);
        Some(RecoveryStatus {
            code: code.to_owned(),
            attempts,
            action,
            policy,
            exhausted: attempts >= policy.max_attempts,
        })
    })
}

/// The default budget when a code has no registered strategy.
pub const DEFAULT_POLICY: RetryPolicy = RetryPolicy {
    max_attempts: 3,
    backoff: Backoff::Fixed(100),
};

/// How many errors have been recorded.
#[must_use]
pub fn get_failure_count() -> u64 {
    with(|registry| registry.failures)
}

/// Sets the failure counter back to zero and answers its previous value.
pub fn reset_failure_count() -> u64 {
    with(|registry| std::mem::replace(&mut registry.failures, 0))
}

/// The errors collected so far, grouped by code.
#[must_use]
pub fn report() -> ErrorReport {
    with(|registry| ErrorReport::build(&registry.errors))
}

/// Writes [`report`]'s text to a file, and answers where it went. With no path it goes into the
/// mod's data directory as `errors.txt`.
pub fn export_report(path: Option<&Path>) -> std::io::Result<PathBuf> {
    let path = match path {
        Some(path) => path.to_owned(),
        None => {
            let Some(dir) = crate::metadata::data_directory() else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no directory is known for this mod",
                ));
            };
            std::fs::create_dir_all(&dir)?;
            dir.join("errors.txt")
        }
    };
    std::fs::write(&path, report().text())?;
    Ok(path)
}

/// Forgets the collected errors and counters, leaving the codes, handlers and strategies in
/// place. The tests use it so one case does not read another's errors.
pub fn clear() {
    with(|registry| {
        registry.errors.clear();
        registry.failures = 0;
        registry.recoveries.clear();
    });
}

fn builtin_codes() -> Vec<Code> {
    use Origin::{Loader, Mod, UserInput, WorldState};
    use Recovery::{Abort, Skip};
    use Severity::{Error as E, Fatal, Warning as W};
    vec![
        Code::new("mod_load", "a mod did not load")
            .severity(E)
            .origin(Mod)
            .retryable(false)
            .recovery(Skip),
        Code::new("dependency", "a required dependency is missing").severity(E).origin(Loader).recoverable(false).recovery(Abort),
        Code::new("version_mismatch", "a version does not match").severity(E).origin(Loader).recoverable(false).recovery(Abort),
        Code::new("api_unavailable", "an API is not available here").severity(W).origin(Loader).recovery(Recovery::UseDefault("omit".to_owned())),
        Code::new("missing_capability", "a required capability is not held").severity(W).origin(Loader).recoverable(false).recovery(Abort),
        Code::new("permission_denied", "a permission was denied").severity(E).origin(UserInput).recoverable(false).recovery(Abort),
        Code::new("registry_conflict", "two mods claimed the same name").severity(E).origin(Mod).recoverable(false).recovery(Abort),
        Code::new("resource_not_found", "an asset is missing").severity(W).origin(Loader).retryable(true).recovery(Recovery::UseDefault("default asset".to_owned())),
        Code::new("resource_reload", "a resource pack failed to reload").severity(W).origin(Loader).retryable(true).recovery(Recovery::Retry),
        Code::new("serialization", "a value did not serialize").severity(E).origin(Mod).recovery(Skip),
        Code::new("config_validation", "a config value is invalid").severity(E).origin(UserInput).recoverable(false).recovery(Abort),
        Code::new("network", "the network dropped").severity(W).origin(Loader).retryable(true).recovery(Recovery::Retry),
        Code::new("packet_decode", "a packet did not decode").severity(W).origin(Loader).retryable(true).recovery(Skip),
        Code::new("render", "a frame did not render").severity(E).origin(Loader).retryable(true).recovery(Recovery::Retry),
        Code::new("shader_compile", "a shader did not compile").severity(E).origin(Mod).recovery(Recovery::UseDefault("fallback shader".to_owned())),
        Code::new("thread_violation", "called off the thread that owns it").severity(Fatal).origin(Mod).recoverable(false).recovery(Abort),
        Code::new("invalid_game_state", "the game is in a state that forbids this").severity(E).origin(WorldState).recoverable(false).recovery(Abort),
        Code::new("world_unavailable", "the world is not loaded").severity(W).origin(WorldState).retryable(true).recovery(Recovery::Retry),
        Code::new("entity_unavailable", "the entity is gone").severity(W).origin(WorldState).recovery(Skip),
        Code::new("patch_conflict", "two patches claimed the same bytes").severity(E).origin(Mod).recoverable(false).recovery(Abort),
        Code::new("script_execution", "a script threw").severity(E).origin(Mod).recovery(Skip),
        Code::new("migration", "a data migration failed").severity(E).origin(Loader).recoverable(false).recovery(Abort),
    ]
}

impl Error {
    fn refused_code(code: &str) -> Self {
        Self {
            id: 0,
            code: "unknown_code".to_owned(),
            message: format!("no error code named {code} is registered"),
            severity: Severity::Warning,
            origin: Origin::Loader,
            recoverable: true,
            retryable: false,
            recovery: Box::new(Recovery::Skip),
            causes: Vec::new(),
            mod_id: crate::metadata::id().into_boxed_str(),
            at_millis: 0,
        }
    }
}

/// Why a question about the client's error state or crash handling has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "set_error_state" => {
            "the client's error state is the client's to set, and this host has no handle on \
             the object that owns it"
                .to_owned()
        }
        "clear_error_state" => {
            "clearing the client's error state would reach into a structure only the client \
             owns, which this host cannot see"
                .to_owned()
        }
        "get_crash_handler" => {
            "the crash handler belongs to the client's crash layer, and this host cannot read \
             a pointer the client never published"
                .to_owned()
        }
        "register_crash_handler" => {
            "the client installs its own crash handler, and a mod cannot replace it from here; \
             this host only reports errors the loader already caught"
                .to_owned()
        }
        "install_crash_recovery" => {
            "crash recovery is the loader's and the client's business, not a mod's; a mod gets \
             the error it caused and nothing more"
                .to_owned()
        }
        _ => format!("{what} is not something this module refuses"),
    }
}

/// What this host cannot reach about the client's error state and crash handling.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    [
        "set_error_state",
        "clear_error_state",
        "get_crash_handler",
        "register_crash_handler",
        "install_crash_recovery",
    ]
    .iter()
    .map(|what| ((*what).to_owned(), why_unknown(what)))
    .collect()
}

fn refused(what: &str) -> Error {
    let why = why_unknown(what);
    crate::warn!("a mod asked this host for the client's error state: {why}");
    Error::refused(what)
}

/// Setting the client's error state. [`Error::refused`] with [`why_unknown`].
pub fn set_error_state(_state: &str) -> Result<(), Error> {
    Err(refused("set_error_state"))
}

/// Clearing the client's error state. [`Error::refused`] with [`why_unknown`].
pub fn clear_error_state() -> Result<(), Error> {
    Err(refused("clear_error_state"))
}

/// Reading the client's crash handler. [`Error::refused`] with [`why_unknown`].
pub fn get_crash_handler() -> Result<String, Error> {
    Err(refused("get_crash_handler"))
}

/// Replacing the client's crash handler. [`Error::refused`] with [`why_unknown`].
pub fn register_crash_handler() -> Result<bool, Error> {
    Err(refused("register_crash_handler"))
}

/// Installing a crash recovery action of the client's. [`Error::refused`] with [`why_unknown`].
pub fn install_crash_recovery() -> Result<bool, Error> {
    Err(refused("install_crash_recovery"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    #[test]
    fn a_code_lookup_answers_the_defaults_and_refuses_unknown() {
        let _serial = serial();
        let network = describe_code("network").expect("network is registered");
        assert_eq!(network.severity, Severity::Warning);
        assert_eq!(network.origin, Origin::Loader);
        assert!(network.recoverable && network.retryable);
        assert_eq!(network.recovery, Recovery::Retry);

        let unknown = describe_code("no_such_code").expect_err("unknown code is refused");
        assert_eq!(unknown.code(), "unknown_code");
        assert!(unknown.message().contains("no_such_code"));
    }

    #[test]
    fn a_chain_renders_in_order_and_names_the_root_cause() {
        let _serial = serial();
        clear();
        let inner = create("network", "the socket dropped");
        let middle = context(inner, "while sending a chat line");
        let outer = wrap("render", "the frame failed", middle);
        let text = outer.render();
        let network = text.find("caused by network: the socket dropped").expect("the network cause");
        let line = text.find("caused by context: while sending a chat line").expect("the context cause");
        assert!(network < line, "the causes render in order: {text}");
        assert_eq!(get_root_cause(&outer).message, "while sending a chat line");
        assert!(get_stack_trace(&outer).contains("cause chain"));
        assert_eq!(get(outer.id()).map(|held| held.code), Some("render".to_owned()));
    }

    #[test]
    fn a_fixed_policy_stops_at_the_attempt_boundary() {
        let _serial = serial();
        let policy = RetryPolicy::fixed(3, 100);
        let error = create("network", "worth another try");
        assert_eq!(policy.delay(1), 100);
        assert_eq!(policy.delay(2), 100);
        assert_eq!(policy.delay(3), 100);
        assert!(policy.should_retry(1, &error));
        assert!(policy.should_retry(2, &error));
        assert!(!policy.should_retry(3, &error), "the counter stops at max_attempts");
        assert!(!policy.should_retry(4, &error));
    }

    #[test]
    fn an_exponential_policy_backs_off_to_a_cap() {
        let _serial = serial();
        let policy = RetryPolicy::exponential(5, 50, 200);
        assert_eq!(policy.delay(1), 50);
        assert_eq!(policy.delay(2), 100);
        assert_eq!(policy.delay(3), 200);
        assert_eq!(policy.delay(4), 200, "the cap holds");
        assert_eq!(policy.delay(9), 200);
        let error = create("network", "still retryable");
        assert!(policy.should_retry(1, &error));
        assert!(policy.should_retry(4, &error));
        assert!(!policy.should_retry(5, &error));
    }

    #[test]
    fn a_non_recoverable_error_is_never_retried() {
        let _serial = serial();
        clear();
        let denied = create("permission_denied", "the player said no");
        assert!(!denied.is_recoverable());
        assert!(!denied.is_retryable());
        let policy = RetryPolicy::exponential(8, 10, 1000);
        for attempt in 1..=8 {
            assert!(!policy.should_retry(attempt, &denied), "attempt {attempt} is refused");
        }
        assert_eq!(policy.delay(4), 80, "the delay is still defined");
    }

    #[test]
    fn the_report_groups_by_code_with_counts() {
        let _serial = serial();
        clear();
        create("network", "one");
        create("network", "two");
        create("mod_load", "three");
        let report = report();
        assert_eq!(report.total, 3);
        assert_eq!(report.groups.len(), 2);
        let network = report.groups.iter().find(|group| group.code == "network").expect("network");
        assert_eq!(network.count, 2);
        assert_eq!(network.severity, Severity::Warning);
        let mod_load = report.groups.iter().find(|group| group.code == "mod_load").expect("mod_load");
        assert_eq!(mod_load.count, 1);
        assert_eq!(mod_load.origin, Origin::Mod);
        assert!(report.text().contains("network: 2 (warning, loader)"), "{}", report.text());
        assert!(report.json().contains("\"code\":\"network\""), "{}", report.json());
    }

    #[test]
    fn a_recovery_strategy_decides_the_action() {
        let _serial = serial();
        clear();
        let error = create("network", "the socket dropped");
        assert_eq!(error.recovery(), &Recovery::Retry, "the code's default");
        assert!(register_recovery_strategy(RecoveryStrategy::new(
            "network",
            Recovery::UseDefault("cached response".to_owned()),
            RetryPolicy::fixed(2, 25),
        )));
        assert_eq!(
            attempt_recovery(&error),
            Recovery::UseDefault("cached response".to_owned()),
            "the registered strategy wins"
        );
        let status = get_recovery_status("network").expect("a recovery was attempted");
        assert_eq!(status.attempts, 1);
        assert!(!status.exhausted);
        assert_eq!(status.policy.max_attempts, 2);
        assert_eq!(attempt_recovery(&error), Recovery::UseDefault("cached response".to_owned()));
        let status = get_recovery_status("network").expect("still tracked");
        assert_eq!(status.attempts, 2);
        assert!(status.exhausted, "the budget is spent at max_attempts");
        unregister_code("network");
        assert!(describe_code("network").is_err());
        assert!(register_code(Code::new("network", "the network dropped").severity(Severity::Warning).origin(Origin::Loader).retryable(true).recovery(Recovery::Retry)));
    }

    #[test]
    fn a_handler_sees_a_recorded_error_and_the_count_resets() {
        let _serial = serial();
        clear();
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let into = std::sync::Arc::clone(&seen);
        assert!(register_handler("probe", move |error: &Error| {
            into.lock().unwrap().push(error.code().to_owned());
        }));
        assert!(!register_handler("probe", |_| {}), "handler names are unique");
        create("network", "one");
        create("render", "two");
        assert_eq!(*seen.lock().unwrap(), vec!["network".to_owned(), "render".to_owned()]);
        assert!(unregister_handler("probe"));
        assert!(!unregister_handler("probe"));
        assert_eq!(get_failure_count(), 2);
        assert_eq!(reset_failure_count(), 2);
        assert_eq!(get_failure_count(), 0);
        assert_eq!(get_by_category(Origin::Loader).len(), 2, "network and render are both loader");
        assert_eq!(get_by_mod(&crate::metadata::id()).len(), 2);
    }

    #[test]
    fn the_client_error_state_and_crash_handling_are_refused() {
        assert_eq!(refusals().len(), 5);
        assert!(why_unknown("set_error_state").contains("client"));
        assert!(set_error_state("broken").is_err());
        assert!(clear_error_state().is_err());
        assert!(get_crash_handler().is_err());
        assert!(register_crash_handler().is_err());
        assert!(install_crash_recovery().is_err());
        assert!(why_unknown("nonsense").contains("not something this module refuses"));
    }
}
